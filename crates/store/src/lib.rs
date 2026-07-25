//! Local persistence for review state.
//!
//! Only drafts are stored so far. They are the state whose loss actually costs
//! the reviewer something: a snapshot can be rebuilt from Git and comments
//! refetched from GitHub, but words that were typed and not saved are gone.
//!
//! Writes go through [`DraftWriter`], which owns a thread, because PLAN's
//! performance budget rules out database work on the UI thread and a draft is
//! written on every keystroke.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, Sender},
    },
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};

use domain::{DiffAnchor, DiffSide, DraftSink};
use rusqlite::Connection;
use thiserror::Error;

/// The schema version this build expects.
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("could not create the review data directory {directory}: {source}")]
    CreateDirectory {
        directory: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not open the review database: {0}")]
    Open(#[source] rusqlite::Error),

    #[error("could not migrate the review database: {0}")]
    Migrate(#[source] rusqlite::Error),

    #[error("could not read stored drafts: {0}")]
    Read(#[source] rusqlite::Error),

    #[error("could not write a draft: {0}")]
    Write(#[source] rusqlite::Error),

    #[error("a stored draft has an unrecognized diff side {0:?}")]
    UnknownSide(String),

    #[error("no home directory to store review data in")]
    NoHomeDirectory,
}

/// The database drafts and other review state live in.
pub struct DraftStore {
    connection: Connection,
}

impl DraftStore {
    /// Opens, creating and migrating as needed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the directory or database cannot be created,
    /// or the schema cannot be brought up to date.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory).map_err(|source| StoreError::CreateDirectory {
                directory: directory.to_path_buf(),
                source,
            })?;
        }
        let connection = Connection::open(path).map_err(StoreError::Open)?;
        Self::prepare(connection)
    }

    /// Opens a throwaway database, for tests.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the schema cannot be created.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory().map_err(StoreError::Open)?;
        Self::prepare(connection)
    }

    fn prepare(connection: Connection) -> Result<Self, StoreError> {
        // WAL with NORMAL synchronous makes a per-keystroke write cheap: it does
        // not fsync on every commit, and a crash can lose at most the last
        // fraction of a second rather than corrupting the file.
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(StoreError::Open)?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(StoreError::Open)?;

        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    /// Brings the schema up to [`SCHEMA_VERSION`].
    ///
    /// `PRAGMA user_version` is the database's own place for this, so no
    /// bookkeeping table is needed. Each step is guarded by version so an older
    /// database upgrades in order rather than all at once.
    fn migrate(&self) -> Result<(), StoreError> {
        let version: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(StoreError::Migrate)?;

        if version < 1 {
            self.connection
                .execute_batch(
                    "CREATE TABLE drafts (
                        scope      TEXT    NOT NULL,
                        head_sha   TEXT    NOT NULL,
                        path       TEXT    NOT NULL,
                        side       TEXT    NOT NULL,
                        line       INTEGER NOT NULL,
                        body       TEXT    NOT NULL,
                        updated_at INTEGER NOT NULL,
                        PRIMARY KEY (scope, head_sha, path, side, line)
                    ) STRICT;",
                )
                .map_err(StoreError::Migrate)?;
        }

        if version < SCHEMA_VERSION {
            self.connection
                .pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(StoreError::Migrate)?;
        }
        Ok(())
    }

    /// Every draft stored under a scope, ready to hand to
    /// `ReviewSession::restore_drafts`.
    ///
    /// Drafts written against any head are returned, including heads that are no
    /// longer current: deciding which of them still anchor is the session's job,
    /// and hiding the rest here would lose them.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the rows cannot be read or hold an
    /// unrecognized diff side.
    pub fn load(&self, scope: &str) -> Result<Vec<(DiffAnchor, String)>, StoreError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT head_sha, path, side, line, body FROM drafts
                 WHERE scope = ?1
                 ORDER BY path, line, side",
            )
            .map_err(StoreError::Read)?;

        let rows = statement
            .query_map([scope], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(StoreError::Read)?;

        let mut drafts = Vec::new();
        for row in rows {
            let (head_sha, path, side, line, body) = row.map_err(StoreError::Read)?;
            let side = DiffSide::from_github(&side).ok_or(StoreError::UnknownSide(side))?;
            drafts.push((
                DiffAnchor {
                    path: path.into(),
                    side,
                    line: line.try_into().unwrap_or(u32::MAX),
                    head_sha: head_sha.into(),
                },
                body,
            ));
        }
        Ok(drafts)
    }

    /// Writes the current text at an anchor, replacing anything already there.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Write`] when the row cannot be written.
    pub fn upsert(&self, scope: &str, anchor: &DiffAnchor, body: &str) -> Result<(), StoreError> {
        self.connection
            .execute(
                "INSERT INTO drafts (scope, head_sha, path, side, line, body, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (scope, head_sha, path, side, line)
                 DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
                rusqlite::params![
                    scope,
                    anchor.head_sha.as_ref(),
                    anchor.path.as_ref(),
                    anchor.side.github_value(),
                    i64::from(anchor.line),
                    body,
                    epoch_seconds(),
                ],
            )
            .map_err(StoreError::Write)?;
        Ok(())
    }

    /// Removes the draft at an anchor.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Write`] when the row cannot be removed.
    pub fn delete(&self, scope: &str, anchor: &DiffAnchor) -> Result<(), StoreError> {
        self.connection
            .execute(
                "DELETE FROM drafts
                 WHERE scope = ?1 AND head_sha = ?2 AND path = ?3 AND side = ?4 AND line = ?5",
                rusqlite::params![
                    scope,
                    anchor.head_sha.as_ref(),
                    anchor.path.as_ref(),
                    anchor.side.github_value(),
                    i64::from(anchor.line),
                ],
            )
            .map_err(StoreError::Write)?;
        Ok(())
    }
}

/// Where review data lives on this machine.
///
/// # Errors
///
/// Returns [`StoreError::NoHomeDirectory`] when `HOME` is not set.
pub fn default_database_path() -> Result<PathBuf, StoreError> {
    let home = std::env::var_os("HOME").ok_or(StoreError::NoHomeDirectory)?;
    Ok(PathBuf::from(home)
        .join("Library/Application Support/ZReview")
        .join("review-data.sqlite3"))
}

enum Command {
    Upsert { anchor: DiffAnchor, body: String },
    Delete { anchor: DiffAnchor },
}

/// Writes draft changes on its own thread.
///
/// A draft is saved on every keystroke, and PLAN's performance budget rules out
/// database work on the UI thread, so the caller hands over a change and returns
/// immediately. Failures are recorded rather than returned, because there is no
/// useful answer to give a keystroke — they surface the next time the view asks.
pub struct DraftWriter {
    sender: Option<Sender<Command>>,
    failure: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl DraftWriter {
    /// Starts the writer thread for one scope.
    #[must_use]
    pub fn spawn(store: DraftStore, scope: String) -> Self {
        let (sender, receiver) = mpsc::channel();
        let failure = Arc::new(Mutex::new(None));
        let thread_failure = Arc::clone(&failure);

        let thread = std::thread::Builder::new()
            .name("zreview-draft-writer".to_owned())
            .spawn(move || {
                // Ends when the sender is dropped, which is how the writer stops.
                for command in receiver {
                    let result = match &command {
                        Command::Upsert { anchor, body } => store.upsert(&scope, anchor, body),
                        Command::Delete { anchor } => store.delete(&scope, anchor),
                    };
                    let mut recorded = lock(&thread_failure);
                    match result {
                        // Clearing on success means a transient failure stops
                        // being reported once writes work again.
                        Ok(()) => *recorded = None,
                        Err(error) => *recorded = Some(error.to_string()),
                    }
                }
            })
            .ok();

        Self {
            sender: Some(sender),
            failure,
            thread,
        }
    }

    fn send(&self, command: Command) {
        if let Some(sender) = &self.sender
            && sender.send(command).is_err()
        {
            // The writer thread is gone, so nothing more will be persisted and
            // the reviewer needs to know.
            *lock(&self.failure) = Some("the draft writer stopped unexpectedly".to_owned());
        }
    }
}

impl DraftSink for DraftWriter {
    fn save(&self, anchor: &DiffAnchor, body: &str) {
        self.send(Command::Upsert {
            anchor: anchor.clone(),
            body: body.to_owned(),
        });
    }

    fn discard(&self, anchor: &DiffAnchor) {
        self.send(Command::Delete {
            anchor: anchor.clone(),
        });
    }

    fn failure(&self) -> Option<String> {
        lock(&self.failure).clone()
    }
}

impl Drop for DraftWriter {
    /// Finishes queued writes before going away, so quitting does not drop the
    /// last keystrokes.
    fn drop(&mut self) {
        self.sender = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER_HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SCOPE: &str = "github:acme/widgets#42";

    fn anchor(path: &str, line: u32, side: DiffSide, head_sha: &str) -> DiffAnchor {
        DiffAnchor {
            path: path.into(),
            side,
            line,
            head_sha: head_sha.into(),
        }
    }

    #[test]
    fn a_draft_round_trips() {
        let store = DraftStore::open_in_memory().unwrap();
        let anchor = anchor("src/review.rs", 11, DiffSide::Right, HEAD);

        store.upsert(SCOPE, &anchor, "needs a test").unwrap();

        let loaded = store.load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, anchor);
        assert_eq!(loaded[0].1, "needs a test");
    }

    #[test]
    fn writing_the_same_anchor_replaces_the_body() {
        let store = DraftStore::open_in_memory().unwrap();
        let anchor = anchor("src/review.rs", 11, DiffSide::Right, HEAD);

        store.upsert(SCOPE, &anchor, "first").unwrap();
        store.upsert(SCOPE, &anchor, "second").unwrap();

        let loaded = store.load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1, "one anchor holds one draft");
        assert_eq!(loaded[0].1, "second");
    }

    #[test]
    fn the_two_sides_of_a_line_are_separate_rows() {
        let store = DraftStore::open_in_memory().unwrap();
        store
            .upsert(SCOPE, &anchor("src/a.rs", 10, DiffSide::Right, HEAD), "new")
            .unwrap();
        store
            .upsert(SCOPE, &anchor("src/a.rs", 10, DiffSide::Left, HEAD), "old")
            .unwrap();

        assert_eq!(store.load(SCOPE).unwrap().len(), 2);
    }

    /// The whole reason the head is not part of the scope: a pull request that
    /// was pushed to must still hand back what was written against the old head.
    #[test]
    fn drafts_from_an_earlier_head_are_still_returned() {
        let store = DraftStore::open_in_memory().unwrap();
        store
            .upsert(
                SCOPE,
                &anchor("src/review.rs", 11, DiffSide::Right, OTHER_HEAD),
                "written before the push",
            )
            .unwrap();
        store
            .upsert(
                SCOPE,
                &anchor("src/review.rs", 11, DiffSide::Right, HEAD),
                "written after",
            )
            .unwrap();

        let loaded = store.load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 2, "both heads are kept");
        let bodies: Vec<&str> = loaded.iter().map(|(_, body)| body.as_str()).collect();
        assert!(bodies.contains(&"written before the push"));
    }

    #[test]
    fn scopes_do_not_see_each_other() {
        let store = DraftStore::open_in_memory().unwrap();
        let anchor = anchor("src/review.rs", 11, DiffSide::Right, HEAD);
        store.upsert(SCOPE, &anchor, "for this review").unwrap();
        store
            .upsert("github:acme/widgets#43", &anchor, "for another")
            .unwrap();

        assert_eq!(store.load(SCOPE).unwrap().len(), 1);
        assert_eq!(store.load("github:acme/widgets#43").unwrap().len(), 1);
        assert!(store.load("github:acme/widgets#99").unwrap().is_empty());
    }

    #[test]
    fn deleting_removes_only_that_anchor() {
        let store = DraftStore::open_in_memory().unwrap();
        let kept = anchor("src/review.rs", 10, DiffSide::Right, HEAD);
        let removed = anchor("src/review.rs", 11, DiffSide::Right, HEAD);
        store.upsert(SCOPE, &kept, "keep me").unwrap();
        store.upsert(SCOPE, &removed, "remove me").unwrap();

        store.delete(SCOPE, &removed).unwrap();

        let loaded = store.load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1, "keep me");
        // Deleting something already gone is not an error.
        store.delete(SCOPE, &removed).unwrap();
    }

    #[test]
    fn drafts_load_in_reading_order() {
        let store = DraftStore::open_in_memory().unwrap();
        for (path, line) in [("src/b.rs", 5), ("src/a.rs", 40), ("src/a.rs", 4)] {
            store
                .upsert(
                    SCOPE,
                    &anchor(path, line, DiffSide::Right, HEAD),
                    &format!("{path}:{line}"),
                )
                .unwrap();
        }

        let bodies: Vec<String> = store
            .load(SCOPE)
            .unwrap()
            .into_iter()
            .map(|(_, body)| body)
            .collect();
        assert_eq!(bodies, ["src/a.rs:4", "src/a.rs:40", "src/b.rs:5"]);
    }

    #[test]
    fn a_draft_outlives_the_process_that_wrote_it() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("nested/review-data.sqlite3");
        let anchor = anchor("src/review.rs", 11, DiffSide::Right, HEAD);

        {
            let store = DraftStore::open(&path).unwrap();
            store.upsert(SCOPE, &anchor, "survives").unwrap();
        }

        let reopened = DraftStore::open(&path).unwrap();
        assert_eq!(reopened.load(SCOPE).unwrap()[0].1, "survives");
    }

    #[test]
    fn opening_an_existing_database_does_not_re_migrate() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");

        let first = DraftStore::open(&path).unwrap();
        let version: i64 = first
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        drop(first);

        // Would fail on a duplicate CREATE TABLE if migration re-ran.
        assert!(DraftStore::open(&path).is_ok());
    }

    #[test]
    fn the_writer_persists_and_removes_through_the_sink() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");
        let kept = anchor("src/review.rs", 10, DiffSide::Right, HEAD);
        let discarded = anchor("src/review.rs", 11, DiffSide::Right, HEAD);

        {
            let writer = DraftWriter::spawn(DraftStore::open(&path).unwrap(), SCOPE.to_owned());
            writer.save(&kept, "keep me");
            writer.save(&discarded, "not this");
            writer.discard(&discarded);
            assert_eq!(writer.failure(), None);
            // Dropping joins the thread, so every queued write has landed.
        }

        let loaded = DraftStore::open(&path).unwrap().load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1, "keep me");
    }

    #[test]
    fn the_writer_keeps_the_last_write_that_wins() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");
        let anchor = anchor("src/review.rs", 11, DiffSide::Right, HEAD);

        {
            let writer = DraftWriter::spawn(DraftStore::open(&path).unwrap(), SCOPE.to_owned());
            // Mirrors typing: one write per keystroke.
            for body in ["n", "ne", "nee", "need"] {
                writer.save(&anchor, body);
            }
        }

        let loaded = DraftStore::open(&path).unwrap().load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1, "need");
    }

    #[test]
    fn a_stored_side_that_cannot_be_read_is_reported() {
        let store = DraftStore::open_in_memory().unwrap();
        store
            .connection
            .execute(
                "INSERT INTO drafts (scope, head_sha, path, side, line, body, updated_at)
                 VALUES (?1, ?2, 'src/a.rs', 'SIDEWAYS', 1, 'body', 0)",
                rusqlite::params![SCOPE, HEAD],
            )
            .unwrap();

        let error = store.load(SCOPE).unwrap_err();
        assert!(
            matches!(&error, StoreError::UnknownSide(side) if side == "SIDEWAYS"),
            "unexpected error: {error}",
        );
    }

    #[test]
    fn the_default_path_is_under_application_support() {
        let path = default_database_path().unwrap();
        assert!(path.ends_with("Library/Application Support/ZReview/review-data.sqlite3"));
    }
}
