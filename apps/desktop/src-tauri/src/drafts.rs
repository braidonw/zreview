//! Reading Home's Drafts counts from the local store.
//!
//! Runs read-only, after a refresh has fetched its rows, so a broken database
//! never blocks the pull requests Home did manage to list.

use std::{collections::HashMap, path::Path};

use domain::SessionFailure;
use store::{ReviewStore, StoreError};

/// Counts of stored Drafts per row, keyed by [`app::HomeRow::draft_scope`].
///
/// A database that does not exist yet means no Drafts anywhere, which is not a
/// failure.
///
/// # Errors
///
/// Returns what Home shows above the list when the database cannot be opened
/// or read.
pub(crate) fn read(
    database_path: &Path,
    rows: &[app::HomeRow],
) -> Result<HashMap<String, usize>, SessionFailure> {
    let store = match ReviewStore::open_read_only(database_path) {
        Ok(store) => store,
        Err(StoreError::Missing) => return Ok(HashMap::new()),
        Err(error) => return Err(drafts_failure(&error)),
    };
    let scopes = rows
        .iter()
        .map(app::HomeRow::draft_scope)
        .collect::<Vec<_>>();
    store
        .count_by_scopes(&scopes)
        .map_err(|error| drafts_failure(&error))
}

fn drafts_failure(error: &StoreError) -> SessionFailure {
    SessionFailure::from_error("Drafts could not be read", error)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn row(repository: &str, number: u64) -> app::HomeRow {
        app::HomeRow {
            group: app::HomeGroup::ToReview,
            repository: repository.to_owned(),
            number,
            title: "a pull request".to_owned(),
            url: format!("https://github.com/{repository}/pull/{number}"),
            author_login: None,
            updated_at_ms: 0,
            review_status: None,
            check_status: None,
            drafts: None,
        }
    }

    #[test]
    fn a_missing_database_reads_as_no_drafts() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");

        let counts = read(&path, &[row("acme/widgets", 412)]).unwrap();

        assert!(counts.is_empty());
    }

    #[test]
    fn an_existing_database_answers_with_its_counts() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");
        {
            let store = ReviewStore::open(&path).unwrap();
            store
                .upsert(
                    "github:acme/widgets#412",
                    &domain::DiffAnchor {
                        path: "src/a.rs".into(),
                        side: domain::DiffSide::Right,
                        line: 1,
                        start_line: None,
                        head_sha: "a".repeat(40).into(),
                    },
                    "a draft",
                )
                .unwrap();
        }

        let counts = read(&path, &[row("acme/widgets", 412)]).unwrap();

        assert_eq!(counts["github:acme/widgets#412"], 1);
    }

    #[test]
    fn a_database_that_is_not_a_valid_sqlite_file_is_a_failure() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");
        std::fs::write(&path, b"not a database").unwrap();

        let failure = read(&path, &[row("acme/widgets", 412)]).unwrap_err();

        assert_eq!(failure.summary, "Drafts could not be read");
    }
}
