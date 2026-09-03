//! Reading Home's Drafts counts from the local store.
//!
//! Runs read-only, after a refresh has fetched its rows, so a broken database
//! never blocks the pull requests Home did manage to list.

use std::{collections::HashMap, path::Path};

use domain::SessionFailure;
use store::{ReviewStore, StoreError};

/// Every stored Drafts count, by scope.
///
/// The table is the reviewer's own unsent comments, never large enough to ask
/// for less than all of it, so the model joins rows onto this rather than the
/// other way around. A database that does not exist yet means no Drafts
/// anywhere, which is not a failure.
///
/// # Errors
///
/// Returns what Home shows above the list when the database cannot be opened
/// or read.
pub(crate) fn read(database_path: &Path) -> Result<HashMap<String, usize>, SessionFailure> {
    let store = match ReviewStore::open_read_only(database_path) {
        Ok(store) => store,
        Err(StoreError::Missing) => return Ok(HashMap::new()),
        Err(
            error @ (StoreError::CreateDirectory { .. }
            | StoreError::Open(_)
            | StoreError::Migrate(_)
            | StoreError::Read(_)
            | StoreError::Write(_)
            | StoreError::UnknownSide(_)
            | StoreError::UnknownOrigin(_)
            | StoreError::NoHomeDirectory
            | StoreError::UnsupportedSchemaVersion(_)),
        ) => return Err(drafts_failure(&error)),
    };
    store
        .count_by_scope()
        .map_err(|error| drafts_failure(&error))
}

fn drafts_failure(error: &StoreError) -> SessionFailure {
    SessionFailure::from_error("Drafts could not be read", error)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn a_missing_database_reads_as_no_drafts() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");

        let counts = read(&path).unwrap();

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

        let counts = read(&path).unwrap();

        assert_eq!(counts["github:acme/widgets#412"], 1);
    }

    #[test]
    fn a_database_that_is_not_a_valid_sqlite_file_is_a_failure() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("review-data.sqlite3");
        std::fs::write(&path, b"not a database").unwrap();

        let failure = read(&path).unwrap_err();

        assert_eq!(failure.summary, "Drafts could not be read");
    }
}
