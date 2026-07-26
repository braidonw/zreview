//! Local persistence for review state.
//!
//! Drafts, review summaries, and the review-engine state that belongs to them:
//! where an accepted finding came from, and which findings the reviewer
//! dismissed. This is the state whose loss actually costs the reviewer
//! something — a snapshot can be rebuilt from Git and comments refetched from
//! GitHub, but words that were typed and not saved are gone, and so is the
//! triage work of rejecting twenty findings one by one.
//!
//! Writes go through [`ReviewStateWriter`], which owns a thread, because PLAN's
//! performance budget rules out database work on the UI thread and a draft is
//! written on every keystroke.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, Sender},
    },
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};

use domain::{
    DiffAnchor, DiffSide, FindingOrigin, FindingProvenance, GuidanceCitation, ReviewStateSink,
};
use rusqlite::Connection;
use thiserror::Error;

/// How provenance rows are matched to the drafts they belong to.
type ProvenanceKey = (String, String, String, i64);

/// The schema version this build expects.
const SCHEMA_VERSION: i64 = 4;

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

    #[error("could not read stored review state: {0}")]
    Read(#[source] rusqlite::Error),

    #[error("could not write a draft: {0}")]
    Write(#[source] rusqlite::Error),

    #[error("a stored draft has an unrecognized diff side {0:?}")]
    UnknownSide(String),

    #[error("a stored draft has an unrecognized finding origin {0:?}")]
    UnknownOrigin(String),

    #[error("no home directory to store review data in")]
    NoHomeDirectory,
}

/// The database a review's local state lives in.
pub struct ReviewStore {
    connection: Connection,
}

impl ReviewStore {
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

        if version < 2 {
            self.connection
                .execute_batch(
                    "CREATE TABLE review_summaries (
                        scope      TEXT    NOT NULL,
                        head_sha   TEXT    NOT NULL,
                        body       TEXT    NOT NULL,
                        updated_at INTEGER NOT NULL,
                        PRIMARY KEY (scope, head_sha)
                    ) STRICT;",
                )
                .map_err(StoreError::Migrate)?;
        }

        if version < 3 {
            // Nullable: a single-line draft has no range, and every draft written
            // before this column existed is one.
            self.connection
                .execute_batch("ALTER TABLE drafts ADD COLUMN start_line INTEGER;")
                .map_err(StoreError::Migrate)?;
        }

        if version < 4 {
            // Provenance for a draft that began as a backend's finding. Kept in
            // its own tables rather than as a JSON column so the functional core
            // stays dependency-free: no serde derive is needed on domain types.
            //
            // Citations are a child table because a finding can rest on several
            // guidance files, and a delimited column would break on a path
            // containing the delimiter.
            self.connection
                .execute_batch(
                    "CREATE TABLE draft_provenance (
                        scope       TEXT    NOT NULL,
                        head_sha    TEXT    NOT NULL,
                        path        TEXT    NOT NULL,
                        side        TEXT    NOT NULL,
                        line        INTEGER NOT NULL,
                        origin_kind TEXT    NOT NULL,
                        origin_name TEXT    NOT NULL,
                        confidence  REAL    NOT NULL,
                        fingerprint TEXT    NOT NULL,
                        PRIMARY KEY (scope, head_sha, path, side, line)
                    ) STRICT;
                     CREATE TABLE draft_guidance (
                        scope         TEXT    NOT NULL,
                        head_sha      TEXT    NOT NULL,
                        path          TEXT    NOT NULL,
                        side          TEXT    NOT NULL,
                        line          INTEGER NOT NULL,
                        guidance_path TEXT    NOT NULL,
                        content_hash  TEXT    NOT NULL,
                        PRIMARY KEY (scope, head_sha, path, side, line, guidance_path)
                    ) STRICT;
                     CREATE TABLE dismissed_findings (
                        scope        TEXT    NOT NULL,
                        head_sha     TEXT    NOT NULL,
                        fingerprint  TEXT    NOT NULL,
                        dismissed_at INTEGER NOT NULL,
                        PRIMARY KEY (scope, head_sha, fingerprint)
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
                "SELECT head_sha, path, side, line, body, start_line FROM drafts
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
                    row.get::<_, Option<i64>>(5)?,
                ))
            })
            .map_err(StoreError::Read)?;

        let mut drafts = Vec::new();
        for row in rows {
            let (head_sha, path, side, line, body, start_line) = row.map_err(StoreError::Read)?;
            let side = DiffSide::from_github(&side).ok_or(StoreError::UnknownSide(side))?;
            drafts.push((
                DiffAnchor {
                    path: path.into(),
                    side,
                    line: line.try_into().unwrap_or(u32::MAX),
                    start_line: start_line.and_then(|start| u32::try_from(start).ok()),
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
                "INSERT INTO drafts
                     (scope, head_sha, path, side, line, body, updated_at, start_line)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (scope, head_sha, path, side, line)
                 DO UPDATE SET
                     body = excluded.body,
                     updated_at = excluded.updated_at,
                     start_line = excluded.start_line",
                rusqlite::params![
                    scope,
                    anchor.head_sha.as_ref(),
                    anchor.path.as_ref(),
                    anchor.side.github_value(),
                    i64::from(anchor.line),
                    body,
                    epoch_seconds(),
                    anchor.start_line.map(i64::from),
                ],
            )
            .map_err(StoreError::Write)?;
        Ok(())
    }

    /// Records where a draft came from, when a backend proposed it.
    ///
    /// Replaces any provenance already at the anchor, citations included, so
    /// re-accepting a finding does not accumulate stale citations beside the
    /// current ones.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Write`] when the rows cannot be written.
    pub fn upsert_provenance(
        &mut self,
        scope: &str,
        anchor: &DiffAnchor,
        provenance: &FindingProvenance,
    ) -> Result<(), StoreError> {
        let (kind, name) = match &provenance.origin {
            FindingOrigin::Check(name) => ("check", name),
            FindingOrigin::Ai(name) => ("ai", name),
        };
        let key = rusqlite::params![
            scope,
            anchor.head_sha.as_ref(),
            anchor.path.as_ref(),
            anchor.side.github_value(),
            i64::from(anchor.line),
        ];

        let transaction = self.connection.transaction().map_err(StoreError::Write)?;
        transaction
            .execute(
                "INSERT INTO draft_provenance
                     (scope, head_sha, path, side, line,
                      origin_kind, origin_name, confidence, fingerprint)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (scope, head_sha, path, side, line)
                 DO UPDATE SET
                     origin_kind = excluded.origin_kind,
                     origin_name = excluded.origin_name,
                     confidence = excluded.confidence,
                     fingerprint = excluded.fingerprint",
                rusqlite::params![
                    scope,
                    anchor.head_sha.as_ref(),
                    anchor.path.as_ref(),
                    anchor.side.github_value(),
                    i64::from(anchor.line),
                    kind,
                    name.as_ref(),
                    f64::from(provenance.confidence),
                    provenance.fingerprint,
                ],
            )
            .map_err(StoreError::Write)?;
        transaction
            .execute(
                "DELETE FROM draft_guidance
                 WHERE scope = ?1 AND head_sha = ?2 AND path = ?3 AND side = ?4 AND line = ?5",
                key,
            )
            .map_err(StoreError::Write)?;
        for citation in &provenance.guidance_sources {
            transaction
                .execute(
                    "INSERT INTO draft_guidance
                         (scope, head_sha, path, side, line, guidance_path, content_hash)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        scope,
                        anchor.head_sha.as_ref(),
                        anchor.path.as_ref(),
                        anchor.side.github_value(),
                        i64::from(anchor.line),
                        citation.path.as_ref(),
                        citation.content_hash.as_ref(),
                    ],
                )
                .map_err(StoreError::Write)?;
        }
        transaction.commit().map_err(StoreError::Write)
    }

    /// Provenance for every draft under a scope, keyed the same way drafts are.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the rows cannot be read or hold an unrecognized
    /// diff side or origin.
    pub fn load_provenance(
        &self,
        scope: &str,
    ) -> Result<Vec<(DiffAnchor, FindingProvenance)>, StoreError> {
        let mut citations: HashMap<ProvenanceKey, Vec<GuidanceCitation>> = HashMap::new();
        let mut statement = self
            .connection
            .prepare(
                "SELECT head_sha, path, side, line, guidance_path, content_hash
                 FROM draft_guidance WHERE scope = ?1
                 ORDER BY guidance_path",
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
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(StoreError::Read)?;
        for row in rows {
            let (head_sha, path, side, line, guidance_path, content_hash) =
                row.map_err(StoreError::Read)?;
            citations
                .entry((head_sha, path, side, line))
                .or_default()
                .push(GuidanceCitation {
                    path: guidance_path.into(),
                    content_hash: content_hash.into(),
                });
        }

        let mut statement = self
            .connection
            .prepare(
                "SELECT head_sha, path, side, line, origin_kind, origin_name, confidence,
                        fingerprint
                 FROM draft_provenance WHERE scope = ?1",
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
                    row.get::<_, String>(5)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })
            .map_err(StoreError::Read)?;

        let mut provenance = Vec::new();
        for row in rows {
            let (head_sha, path, side, line, kind, name, confidence, fingerprint) =
                row.map_err(StoreError::Read)?;
            let parsed_side = DiffSide::from_github(&side)
                .ok_or_else(|| StoreError::UnknownSide(side.clone()))?;
            let origin = match kind.as_str() {
                "check" => FindingOrigin::Check(name.into()),
                "ai" => FindingOrigin::Ai(name.into()),
                other => return Err(StoreError::UnknownOrigin(other.to_owned())),
            };
            let guidance_sources = citations
                .remove(&(head_sha.clone(), path.clone(), side, line))
                .unwrap_or_default();
            provenance.push((
                DiffAnchor {
                    path: path.into(),
                    side: parsed_side,
                    line: line.try_into().unwrap_or(u32::MAX),
                    // Provenance is looked up by the draft's key, which does not
                    // include the range, so this is filled from the draft itself.
                    start_line: None,
                    head_sha: head_sha.into(),
                },
                FindingProvenance {
                    origin,
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "confidence is a 0..=1 ratio; f32 is the domain's own width"
                    )]
                    confidence: confidence as f32,
                    guidance_sources,
                    fingerprint,
                },
            ));
        }
        Ok(provenance)
    }

    /// Records that the reviewer rejected a claim, so a re-run does not offer it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Write`] when the row cannot be written.
    pub fn insert_dismissal(
        &self,
        scope: &str,
        head_sha: &str,
        fingerprint: &str,
    ) -> Result<(), StoreError> {
        self.connection
            .execute(
                "INSERT INTO dismissed_findings (scope, head_sha, fingerprint, dismissed_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (scope, head_sha, fingerprint) DO NOTHING",
                rusqlite::params![scope, head_sha, fingerprint, epoch_seconds()],
            )
            .map_err(StoreError::Write)?;
        Ok(())
    }

    /// Claims dismissed against a snapshot.
    ///
    /// Keyed by head as well as scope, because a dismissal is a judgement about
    /// code at a particular revision. A force-push that rewrites the line should
    /// let the finding be raised again rather than silently keeping it suppressed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Read`] when the rows cannot be read.
    pub fn load_dismissals(&self, scope: &str, head_sha: &str) -> Result<Vec<String>, StoreError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT fingerprint FROM dismissed_findings
                 WHERE scope = ?1 AND head_sha = ?2",
            )
            .map_err(StoreError::Read)?;
        let rows = statement
            .query_map(rusqlite::params![scope, head_sha], |row| {
                row.get::<_, String>(0)
            })
            .map_err(StoreError::Read)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Read)
    }

    /// The saved review summary for a snapshot, if one was written.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Read`] when the row cannot be read.
    pub fn load_summary(&self, scope: &str, head_sha: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT body FROM review_summaries WHERE scope = ?1 AND head_sha = ?2",
                rusqlite::params![scope, head_sha],
                |row| row.get(0),
            )
            .map_or_else(
                |error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Read(other)),
                },
                |body: String| Ok(Some(body)),
            )
    }

    /// Writes the review summary for a snapshot, replacing any earlier one.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Write`] when the row cannot be written.
    pub fn upsert_summary(
        &self,
        scope: &str,
        head_sha: &str,
        body: &str,
    ) -> Result<(), StoreError> {
        self.connection
            .execute(
                "INSERT INTO review_summaries (scope, head_sha, body, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (scope, head_sha)
                 DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
                rusqlite::params![scope, head_sha, body, epoch_seconds()],
            )
            .map_err(StoreError::Write)?;
        Ok(())
    }

    /// Removes everything a submitted review consumed: its drafts and its summary.
    ///
    /// One transaction, so a crash midway cannot leave the summary behind without
    /// the comments it belonged to.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Write`] when the rows cannot be removed.
    pub fn clear_submitted(
        &mut self,
        scope: &str,
        head_sha: &str,
        anchors: &[DiffAnchor],
    ) -> Result<(), StoreError> {
        let transaction = self.connection.transaction().map_err(StoreError::Write)?;
        for anchor in anchors {
            transaction
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
        }
        transaction
            .execute(
                "DELETE FROM review_summaries WHERE scope = ?1 AND head_sha = ?2",
                rusqlite::params![scope, head_sha],
            )
            .map_err(StoreError::Write)?;
        transaction.commit().map_err(StoreError::Write)
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
    Upsert {
        anchor: DiffAnchor,
        body: String,
    },
    Delete {
        anchor: DiffAnchor,
    },
    Summary {
        head_sha: String,
        body: String,
    },
    ClearSubmitted {
        head_sha: String,
        anchors: Vec<DiffAnchor>,
    },
    Provenance {
        anchor: DiffAnchor,
        provenance: FindingProvenance,
    },
    Dismiss {
        head_sha: String,
        fingerprint: String,
    },
}

/// Writes draft changes on its own thread.
///
/// A draft is saved on every keystroke, and PLAN's performance budget rules out
/// database work on the UI thread, so the caller hands over a change and returns
/// immediately. Failures are recorded rather than returned, because there is no
/// useful answer to give a keystroke — they surface the next time the view asks.
pub struct ReviewStateWriter {
    sender: Option<Sender<Command>>,
    failure: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl ReviewStateWriter {
    /// Starts the writer thread for one scope.
    #[must_use]
    pub fn spawn(store: ReviewStore, scope: String) -> Self {
        let (sender, receiver) = mpsc::channel();
        let failure = Arc::new(Mutex::new(None));
        let thread_failure = Arc::clone(&failure);

        let thread = std::thread::Builder::new()
            .name("zreview-draft-writer".to_owned())
            .spawn(move || {
                let mut store = store;
                // Ends when the sender is dropped, which is how the writer stops.
                for command in receiver {
                    let result = match &command {
                        Command::Upsert { anchor, body } => store.upsert(&scope, anchor, body),
                        Command::Delete { anchor } => store.delete(&scope, anchor),
                        Command::Summary { head_sha, body } => {
                            store.upsert_summary(&scope, head_sha, body)
                        }
                        Command::ClearSubmitted { head_sha, anchors } => {
                            store.clear_submitted(&scope, head_sha, anchors)
                        }
                        Command::Provenance { anchor, provenance } => {
                            store.upsert_provenance(&scope, anchor, provenance)
                        }
                        Command::Dismiss {
                            head_sha,
                            fingerprint,
                        } => store.insert_dismissal(&scope, head_sha, fingerprint),
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

impl ReviewStateSink for ReviewStateWriter {
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

    fn save_summary(&self, head_sha: &str, body: &str) {
        self.send(Command::Summary {
            head_sha: head_sha.to_owned(),
            body: body.to_owned(),
        });
    }

    fn clear_submitted(&self, head_sha: &str, anchors: &[DiffAnchor]) {
        self.send(Command::ClearSubmitted {
            head_sha: head_sha.to_owned(),
            anchors: anchors.to_vec(),
        });
    }

    fn save_provenance(&self, anchor: &DiffAnchor, provenance: &FindingProvenance) {
        self.send(Command::Provenance {
            anchor: anchor.clone(),
            provenance: provenance.clone(),
        });
    }

    fn dismiss_finding(&self, head_sha: &str, fingerprint: &str) {
        self.send(Command::Dismiss {
            head_sha: head_sha.to_owned(),
            fingerprint: fingerprint.to_owned(),
        });
    }

    fn failure(&self) -> Option<String> {
        lock(&self.failure).clone()
    }
}

impl Drop for ReviewStateWriter {
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
            start_line: None,
            head_sha: head_sha.into(),
        }
    }

    fn provenance() -> FindingProvenance {
        FindingProvenance {
            origin: FindingOrigin::Ai("claude-code".into()),
            confidence: 0.75,
            guidance_sources: vec![
                GuidanceCitation {
                    path: "AGENTS.md".into(),
                    content_hash: "hash-a".into(),
                },
                GuidanceCitation {
                    path: "crates/ui/AGENTS.md".into(),
                    content_hash: "hash-b".into(),
                },
            ],
            fingerprint: "abc123def4567890".to_owned(),
        }
    }

    #[test]
    fn provenance_survives_a_round_trip_with_every_citation() {
        let mut store = ReviewStore::open_in_memory().unwrap();
        let anchor = anchor("src/queue.rs", 12, DiffSide::Right, HEAD);
        store.upsert(SCOPE, &anchor, "Handle the failure.").unwrap();

        store
            .upsert_provenance(SCOPE, &anchor, &provenance())
            .unwrap();
        let loaded = store.load_provenance(SCOPE).unwrap();

        assert_eq!(loaded.len(), 1);
        let (loaded_anchor, loaded_provenance) = &loaded[0];
        assert_eq!(loaded_anchor.path.as_ref(), "src/queue.rs");
        assert_eq!(loaded_anchor.line, 12);
        assert_eq!(loaded_anchor.side, DiffSide::Right);
        assert_eq!(
            loaded_provenance.origin,
            FindingOrigin::Ai("claude-code".into())
        );
        assert!((loaded_provenance.confidence - 0.75).abs() < f32::EPSILON);
        assert_eq!(loaded_provenance.fingerprint, "abc123def4567890");
        assert_eq!(loaded_provenance.guidance_sources.len(), 2);
        assert_eq!(
            loaded_provenance.guidance_sources[0].path.as_ref(),
            "AGENTS.md"
        );
        assert_eq!(
            loaded_provenance.guidance_sources[1].content_hash.as_ref(),
            "hash-b"
        );
    }

    #[test]
    fn rewriting_provenance_replaces_its_citations_rather_than_adding_to_them() {
        let mut store = ReviewStore::open_in_memory().unwrap();
        let anchor = anchor("src/queue.rs", 12, DiffSide::Right, HEAD);
        store
            .upsert_provenance(SCOPE, &anchor, &provenance())
            .unwrap();

        let mut second = provenance();
        second.guidance_sources = vec![GuidanceCitation {
            path: "CONTRIBUTING.md".into(),
            content_hash: "hash-c".into(),
        }];
        store.upsert_provenance(SCOPE, &anchor, &second).unwrap();

        let loaded = store.load_provenance(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1.guidance_sources.len(), 1);
        assert_eq!(
            loaded[0].1.guidance_sources[0].path.as_ref(),
            "CONTRIBUTING.md"
        );
    }

    /// The two sides of a line are separate drafts, so separate provenance.
    #[test]
    fn provenance_is_keyed_by_side_as_well_as_line() {
        let mut store = ReviewStore::open_in_memory().unwrap();
        let right = anchor("src/queue.rs", 12, DiffSide::Right, HEAD);
        let left = anchor("src/queue.rs", 12, DiffSide::Left, HEAD);
        let mut from_check = provenance();
        from_check.origin = FindingOrigin::Check("clippy".into());

        store
            .upsert_provenance(SCOPE, &right, &provenance())
            .unwrap();
        store.upsert_provenance(SCOPE, &left, &from_check).unwrap();

        let loaded = store.load_provenance(SCOPE).unwrap();
        assert_eq!(loaded.len(), 2);
        let by_side: Vec<_> = loaded
            .iter()
            .map(|(anchor, provenance)| (anchor.side, provenance.origin.clone()))
            .collect();
        assert!(by_side.contains(&(DiffSide::Right, FindingOrigin::Ai("claude-code".into()))));
        assert!(by_side.contains(&(DiffSide::Left, FindingOrigin::Check("clippy".into()))));
    }

    #[test]
    fn a_draft_the_reviewer_wrote_has_no_provenance_row() {
        let store = ReviewStore::open_in_memory().unwrap();
        let anchor = anchor("src/queue.rs", 12, DiffSide::Right, HEAD);
        store.upsert(SCOPE, &anchor, "my own words").unwrap();

        assert!(store.load_provenance(SCOPE).unwrap().is_empty());
    }

    #[test]
    fn a_dismissal_survives_and_is_scoped_to_its_head() {
        let store = ReviewStore::open_in_memory().unwrap();

        store
            .insert_dismissal(SCOPE, HEAD, "fingerprint-one")
            .unwrap();
        store
            .insert_dismissal(SCOPE, HEAD, "fingerprint-two")
            .unwrap();

        let mut dismissed = store.load_dismissals(SCOPE, HEAD).unwrap();
        dismissed.sort();
        assert_eq!(dismissed, vec!["fingerprint-one", "fingerprint-two"]);

        // A different head is a different judgement: the code may have changed.
        assert!(store.load_dismissals(SCOPE, OTHER_HEAD).unwrap().is_empty());
        // As is a different review.
        assert!(
            store
                .load_dismissals("local:/tmp/other", HEAD)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn dismissing_the_same_claim_twice_is_not_an_error() {
        let store = ReviewStore::open_in_memory().unwrap();

        store.insert_dismissal(SCOPE, HEAD, "fingerprint").unwrap();
        store.insert_dismissal(SCOPE, HEAD, "fingerprint").unwrap();

        assert_eq!(store.load_dismissals(SCOPE, HEAD).unwrap().len(), 1);
    }

    /// A database written by the previous build must gain the new tables without
    /// losing what is already in it.
    #[test]
    fn a_version_three_database_upgrades_and_keeps_its_drafts() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review.db");

        // Exactly what version 3 looked like.
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE drafts (
                        scope TEXT NOT NULL, head_sha TEXT NOT NULL, path TEXT NOT NULL,
                        side TEXT NOT NULL, line INTEGER NOT NULL, body TEXT NOT NULL,
                        updated_at INTEGER NOT NULL, start_line INTEGER,
                        PRIMARY KEY (scope, head_sha, path, side, line)
                     ) STRICT;
                     CREATE TABLE review_summaries (
                        scope TEXT NOT NULL, head_sha TEXT NOT NULL, body TEXT NOT NULL,
                        updated_at INTEGER NOT NULL, PRIMARY KEY (scope, head_sha)
                     ) STRICT;",
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO drafts
                        (scope, head_sha, path, side, line, body, updated_at, start_line)
                     VALUES (?1, ?2, 'src/old.rs', 'RIGHT', 3, 'written before the upgrade', 0, NULL)",
                    rusqlite::params![SCOPE, HEAD],
                )
                .unwrap();
            connection.pragma_update(None, "user_version", 3).unwrap();
        }

        let mut store = ReviewStore::open(&path).unwrap();

        let drafts = store.load(SCOPE).unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].1, "written before the upgrade");
        // And the new tables work.
        let anchor = anchor("src/old.rs", 3, DiffSide::Right, HEAD);
        store
            .upsert_provenance(SCOPE, &anchor, &provenance())
            .unwrap();
        assert_eq!(store.load_provenance(SCOPE).unwrap().len(), 1);
        store.insert_dismissal(SCOPE, HEAD, "fingerprint").unwrap();
        assert_eq!(store.load_dismissals(SCOPE, HEAD).unwrap().len(), 1);
    }

    #[test]
    fn a_draft_round_trips() {
        let store = ReviewStore::open_in_memory().unwrap();
        let anchor = anchor("src/review.rs", 11, DiffSide::Right, HEAD);

        store.upsert(SCOPE, &anchor, "needs a test").unwrap();

        let loaded = store.load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, anchor);
        assert_eq!(loaded[0].1, "needs a test");
    }

    #[test]
    fn writing_the_same_anchor_replaces_the_body() {
        let store = ReviewStore::open_in_memory().unwrap();
        let anchor = anchor("src/review.rs", 11, DiffSide::Right, HEAD);

        store.upsert(SCOPE, &anchor, "first").unwrap();
        store.upsert(SCOPE, &anchor, "second").unwrap();

        let loaded = store.load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1, "one anchor holds one draft");
        assert_eq!(loaded[0].1, "second");
    }

    #[test]
    fn the_two_sides_of_a_line_are_separate_rows() {
        let store = ReviewStore::open_in_memory().unwrap();
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
        let store = ReviewStore::open_in_memory().unwrap();
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
        let store = ReviewStore::open_in_memory().unwrap();
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
        let store = ReviewStore::open_in_memory().unwrap();
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
        let store = ReviewStore::open_in_memory().unwrap();
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
            let store = ReviewStore::open(&path).unwrap();
            store.upsert(SCOPE, &anchor, "survives").unwrap();
        }

        let reopened = ReviewStore::open(&path).unwrap();
        assert_eq!(reopened.load(SCOPE).unwrap()[0].1, "survives");
    }

    #[test]
    fn opening_an_existing_database_does_not_re_migrate() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");

        let first = ReviewStore::open(&path).unwrap();
        let version: i64 = first
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        drop(first);

        // Would fail on a duplicate CREATE TABLE if migration re-ran.
        assert!(ReviewStore::open(&path).is_ok());
    }

    #[test]
    fn the_writer_persists_and_removes_through_the_sink() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");
        let kept = anchor("src/review.rs", 10, DiffSide::Right, HEAD);
        let discarded = anchor("src/review.rs", 11, DiffSide::Right, HEAD);

        {
            let writer =
                ReviewStateWriter::spawn(ReviewStore::open(&path).unwrap(), SCOPE.to_owned());
            writer.save(&kept, "keep me");
            writer.save(&discarded, "not this");
            writer.discard(&discarded);
            assert_eq!(writer.failure(), None);
            // Dropping joins the thread, so every queued write has landed.
        }

        let loaded = ReviewStore::open(&path).unwrap().load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1, "keep me");
    }

    #[test]
    fn the_writer_keeps_the_last_write_that_wins() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");
        let anchor = anchor("src/review.rs", 11, DiffSide::Right, HEAD);

        {
            let writer =
                ReviewStateWriter::spawn(ReviewStore::open(&path).unwrap(), SCOPE.to_owned());
            // Mirrors typing: one write per keystroke.
            for body in ["n", "ne", "nee", "need"] {
                writer.save(&anchor, body);
            }
        }

        let loaded = ReviewStore::open(&path).unwrap().load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1, "need");
    }

    #[test]
    fn a_range_survives_storage() {
        let store = ReviewStore::open_in_memory().unwrap();
        let mut ranged = anchor("src/review.rs", 12, DiffSide::Right, HEAD);
        ranged.start_line = Some(10);

        store.upsert(SCOPE, &ranged, "this block").unwrap();

        let loaded = store.load(SCOPE).unwrap();
        assert_eq!(loaded[0].0.start_line, Some(10));
        assert_eq!(loaded[0].0.line, 12);
        assert!(loaded[0].0.is_multiline());
    }

    /// Narrowing a range back to one line must clear the stored start, not leave a
    /// range behind on the same key.
    #[test]
    fn rewriting_a_range_as_a_single_line_clears_the_start() {
        let store = ReviewStore::open_in_memory().unwrap();
        let mut ranged = anchor("src/review.rs", 12, DiffSide::Right, HEAD);
        ranged.start_line = Some(10);
        store.upsert(SCOPE, &ranged, "this block").unwrap();

        let single = anchor("src/review.rs", 12, DiffSide::Right, HEAD);
        store.upsert(SCOPE, &single, "just this line").unwrap();

        let loaded = store.load(SCOPE).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0.start_line, None);
        assert_eq!(loaded[0].1, "just this line");
    }

    #[test]
    fn a_stored_side_that_cannot_be_read_is_reported() {
        let store = ReviewStore::open_in_memory().unwrap();
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
