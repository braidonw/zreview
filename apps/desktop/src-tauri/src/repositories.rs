//! Reading the configured repositories, and resolving each one to a clone.
//!
//! The file and Git live here rather than in the Home model, which takes what
//! this finds as data.

use std::path::{Path, PathBuf};

use app::{RepositoryEntry, RepositoryOutcome};
use domain::SessionFailure;
use settings::{Settings, SettingsError};

/// Reads the settings file and resolves every clone it lists.
///
/// Runs on every refresh, so a hand edit takes effect without a restart. One
/// clone that cannot be resolved is an entry with a reason, not a failure. Only
/// the file itself failing stops Home listing anything at all.
///
/// # Errors
///
/// Returns the failure to show in place of the list when the file cannot be
/// read or parsed.
pub fn read(settings_path: &Path) -> Result<Vec<RepositoryEntry>, SessionFailure> {
    let settings = settings::load(settings_path).map_err(|error| settings_failure(&error))?;
    Ok(resolve_picked(&settings.repositories))
}

/// Resolves folders, whether picked or read from the file, to their clones.
#[must_use]
pub fn resolve_picked(folders: &[PathBuf]) -> Vec<RepositoryEntry> {
    folders
        .iter()
        .map(|folder| RepositoryEntry {
            path: folder.clone(),
            outcome: resolve(folder),
        })
        .collect()
}

/// Writes the repository list, replacing whatever the file held.
///
/// # Errors
///
/// Returns the failure to show above the list when the file cannot be written.
pub fn write(settings_path: &Path, repositories: Vec<PathBuf>) -> Result<(), SessionFailure> {
    settings::save(settings_path, &Settings { repositories })
        .map_err(|error| settings_failure(&error))
}

/// Where the settings file lives.
///
/// # Errors
///
/// Returns the failure to show when there is no home directory to look in.
pub fn settings_path() -> Result<PathBuf, SessionFailure> {
    settings::default_settings_path().map_err(|error| settings_failure(&error))
}

fn resolve(folder: &Path) -> RepositoryOutcome {
    match github::resolve_clone(folder) {
        Ok(resolved) => RepositoryOutcome::Valid {
            root: resolved.root,
            slug: resolved.slug.full_name(),
        },
        Err(error) => RepositoryOutcome::Failed {
            reason: error.to_string(),
        },
    }
}

/// What Home shows in place of its list when the settings file itself fails.
///
/// Every case names the file, because the reviewer's next move is to open it.
fn settings_failure(error: &SettingsError) -> SessionFailure {
    match error {
        SettingsError::NoHomeDirectory => {
            SessionFailure::from_error("Home has nowhere to keep your settings", error)
                .with_remediation("Set HOME, then reopen ZReview.")
        }
        SettingsError::Read { path, .. } | SettingsError::Parse { path, .. } => {
            SessionFailure::from_error("Home could not read your settings", error)
                .with_remediation(format!("Fix {}, then press r to refresh.", path.display()))
        }
        SettingsError::CreateDirectory { directory, .. } => {
            SessionFailure::from_error("Home could not save your settings", error).with_remediation(
                format!("Check that {} can be created.", directory.display()),
            )
        }
        SettingsError::Write { path, .. } => {
            SessionFailure::from_error("Home could not save your settings", error)
                .with_remediation(format!("Check that {} is writable.", path.display()))
        }
        SettingsError::NonUtf8Path { .. } => {
            SessionFailure::from_error("Home could not save your settings", error)
                .with_remediation("Move that clone to a path made only of text.")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, path::Path, process::Command};

    use app::RepositoryOutcome;
    use tempfile::TempDir;

    use super::*;

    fn git<I, S>(repository: &Path, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let status = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// A clone whose `origin` points at GitHub, which is what Home configures.
    fn clone_of(slug: &str) -> TempDir {
        let directory = TempDir::new().unwrap();
        git(directory.path(), ["init", "--quiet"]);
        git(
            directory.path(),
            [
                "remote",
                "add",
                "origin",
                &format!("https://github.com/{slug}.git"),
            ],
        );
        directory
    }

    fn settings_file(directory: &TempDir, contents: &str) -> PathBuf {
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn a_settings_file_that_was_never_written_reads_as_no_repositories() {
        let directory = TempDir::new().unwrap();

        let entries = read(&directory.path().join("settings.toml")).unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn every_listed_clone_comes_back_with_its_slug_and_its_root() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let path = settings_file(
            &directory,
            &format!(
                "repositories = [{:?}]\n",
                clone.path().display().to_string()
            ),
        );

        let entries = read(&path).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, clone.path());
        assert_eq!(entries[0].slug(), Some("acme/widgets"));
    }

    #[test]
    fn a_listed_clone_that_is_gone_carries_its_reason_and_the_rest_still_resolve() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let path = settings_file(
            &directory,
            &format!(
                "repositories = [{:?}, \"/nowhere/at/all\"]\n",
                clone.path().display().to_string()
            ),
        );

        let entries = read(&path).unwrap();

        assert_eq!(entries[0].slug(), Some("acme/widgets"));
        assert_eq!(entries[1].reason(), Some("the folder no longer exists"));
    }

    #[test]
    fn a_malformed_settings_file_becomes_a_failure_naming_the_path_and_the_problem() {
        let directory = TempDir::new().unwrap();
        let path = settings_file(&directory, "repositories = [this is not valid toml");

        let failure = read(&path).unwrap_err();

        assert_eq!(failure.summary, "Home could not read your settings");
        let detail = failure.detail.expect("the parser's message should survive");
        assert!(
            detail.contains(&path.display().to_string()),
            "the detail should name the file: {detail}",
        );
        assert!(
            failure
                .remediation
                .expect("a malformed file is something a reviewer can fix")
                .contains(&path.display().to_string()),
        );
    }

    #[test]
    fn a_picked_folder_resolves_to_the_worktree_root_rather_than_itself() {
        let clone = clone_of("acme/widgets");
        let nested = clone.path().join("crates/review");
        std::fs::create_dir_all(&nested).unwrap();

        let entries = resolve_picked(std::slice::from_ref(&nested));

        assert_eq!(entries[0].path, nested);
        match &entries[0].outcome {
            RepositoryOutcome::Valid { root, slug } => {
                assert_eq!(*root, clone.path().canonicalize().unwrap());
                assert_eq!(slug, "acme/widgets");
            }
            RepositoryOutcome::Failed { reason } => panic!("unexpectedly refused: {reason}"),
        }
    }

    #[test]
    fn a_picked_folder_that_is_not_a_clone_carries_its_reason() {
        let directory = TempDir::new().unwrap();

        let entries = resolve_picked(&[directory.path().to_path_buf()]);

        assert_eq!(entries[0].reason(), Some("not a Git repository"));
    }
}
