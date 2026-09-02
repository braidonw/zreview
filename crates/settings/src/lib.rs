//! The reviewer's repository list, stored at `~/.config/zreview/settings.toml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The reviewer's configured repositories.
///
/// Unknown keys are rejected rather than ignored, so a typo never silently
/// empties Home. A future settings key can relax this deliberately.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default)]
    pub repositories: Vec<PathBuf>,
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("no home directory to store settings in")]
    NoHomeDirectory,

    #[error("could not read the settings file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse the settings file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("could not create the settings directory {directory}: {source}")]
    CreateDirectory {
        directory: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not write the settings file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("repository path {path:?} is not valid UTF-8 and cannot be saved")]
    NonUtf8Path { path: PathBuf },
}

/// Where the settings file lives on this machine.
///
/// # Errors
///
/// Returns [`SettingsError::NoHomeDirectory`] when `HOME` is not set.
pub fn default_settings_path() -> Result<PathBuf, SettingsError> {
    let home = std::env::var_os("HOME").ok_or(SettingsError::NoHomeDirectory)?;
    Ok(PathBuf::from(home).join(".config/zreview/settings.toml"))
}

/// Loads the settings at `path`.
///
/// A missing file loads as an empty repository list, since nothing has been
/// configured yet.
///
/// # Errors
///
/// Returns [`SettingsError`] when the file exists but cannot be read or
/// parsed.
pub fn load(path: &Path) -> Result<Settings, SettingsError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => {
            return Err(SettingsError::Read {
                path: path.to_path_buf(),
                source: error,
            });
        }
    };
    toml::from_str(&contents).map_err(|source| SettingsError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// Writes `settings` to `path` as pretty TOML, creating the parent directory
/// if needed.
///
/// Comments and hand formatting in an existing file are not preserved.
///
/// # Errors
///
/// Returns [`SettingsError`] when the parent directory or file cannot be
/// written.
///
/// # Panics
///
/// Never in practice. TOML serialization can only fail on a non-UTF-8 path,
/// and every path is checked before this point.
pub fn save(path: &Path, settings: &Settings) -> Result<(), SettingsError> {
    for repository in &settings.repositories {
        if repository.to_str().is_none() {
            return Err(SettingsError::NonUtf8Path {
                path: repository.clone(),
            });
        }
    }
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|source| SettingsError::CreateDirectory {
            directory: directory.to_path_buf(),
            source,
        })?;
    }
    let serialized =
        toml::to_string_pretty(settings).expect("every repository path is valid UTF-8");
    std::fs::write(path, serialized).map_err(|source| SettingsError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    /// `remove_var`/`set_var` are unsafe under edition 2024 and this workspace
    /// forbids unsafe code, so `HOME` is exercised in a re-exec'd child instead
    /// of by mutating this process's environment.
    ///
    /// Asserts the child ran exactly one test and it passed, since a filter
    /// that matches nothing exits success with zero tests run.
    fn assert_single_test_passed(output: &std::process::Output) {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "child did not pass: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "expected exactly one test to run, got: {stdout}"
        );
    }

    const MISSING_HOME_CHILD: &str = "ZREVIEW_SETTINGS_MISSING_HOME_CHILD";

    #[test]
    fn an_unset_home_produces_the_missing_home_error() {
        if std::env::var_os(MISSING_HOME_CHILD).is_none() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::an_unset_home_produces_the_missing_home_error",
                ])
                .env(MISSING_HOME_CHILD, "1")
                .env_remove("HOME")
                .output()
                .unwrap();
            assert_single_test_passed(&output);
            return;
        }

        let error = default_settings_path().unwrap_err();
        assert!(matches!(error, SettingsError::NoHomeDirectory));
    }

    const DEFAULT_PATH_CHILD: &str = "ZREVIEW_SETTINGS_DEFAULT_PATH_CHILD";

    #[test]
    fn the_default_path_is_under_a_temporary_homes_dot_config() {
        if std::env::var_os(DEFAULT_PATH_CHILD).is_none() {
            let home = TempDir::new().unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::the_default_path_is_under_a_temporary_homes_dot_config",
                ])
                .env(DEFAULT_PATH_CHILD, "1")
                .env("HOME", home.path())
                .output()
                .unwrap();
            assert_single_test_passed(&output);
            return;
        }

        let home = std::env::var_os("HOME").unwrap();
        let expected = PathBuf::from(&home).join(".config/zreview/settings.toml");
        assert_eq!(default_settings_path().unwrap(), expected);
    }

    #[test]
    fn a_missing_file_loads_as_an_empty_repository_list() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.toml");

        let settings = load(&path).unwrap();

        assert!(settings.repositories.is_empty());
    }

    #[test]
    fn an_empty_file_loads_as_an_empty_repository_list() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, "").unwrap();

        let settings = load(&path).unwrap();

        assert!(settings.repositories.is_empty());
    }

    #[test]
    fn a_round_trip_preserves_the_order_and_content_of_several_paths() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.toml");
        let settings = Settings {
            repositories: vec![
                PathBuf::from("/Users/braidon/Developer/zreview"),
                PathBuf::from("/Users/braidon/Developer/widgets"),
                PathBuf::from("/Users/braidon/Developer/acme"),
            ],
        };

        save(&path, &settings).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded, settings);
    }

    #[test]
    fn a_malformed_file_produces_an_error_naming_the_path_and_the_parse_problem() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, "repositories = [this is not valid toml").unwrap();

        let error = load(&path).unwrap_err();
        let rendered = error.to_string();

        match &error {
            SettingsError::Parse {
                path: error_path, ..
            } => assert_eq!(*error_path, path),
            other => panic!("expected a parse error, got {other}"),
        }
        assert!(
            rendered.contains(&path.display().to_string()),
            "error does not name the path: {rendered}"
        );
        assert!(
            rendered.contains("unclosed array"),
            "error does not carry the parser's message: {rendered}"
        );
    }

    #[test]
    fn a_misspelled_key_produces_a_parse_error_naming_the_path() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, r#"repositorys = ["/a"]"#).unwrap();

        let error = load(&path).unwrap_err();

        match error {
            SettingsError::Parse {
                path: error_path, ..
            } => assert_eq!(error_path, path),
            other => panic!("expected a parse error, got {other}"),
        }
    }

    #[test]
    fn a_read_error_other_than_not_found_is_surfaced() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.toml");
        std::fs::create_dir(&path).unwrap();

        let error = load(&path).unwrap_err();

        assert!(
            matches!(&error, SettingsError::Read { path: error_path, .. } if *error_path == path)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_path_is_refused_and_the_file_is_left_as_it_was() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.toml");
        let original = Settings {
            repositories: vec![PathBuf::from("/Users/braidon/Developer/zreview")],
        };
        save(&path, &original).unwrap();
        let original_contents = std::fs::read_to_string(&path).unwrap();

        let non_utf8 = PathBuf::from(OsStr::from_bytes(&[0xFF, 0xFE]));
        let attempted = Settings {
            repositories: vec![non_utf8.clone()],
        };
        let error = save(&path, &attempted).unwrap_err();

        match error {
            SettingsError::NonUtf8Path { path: bad_path } => assert_eq!(bad_path, non_utf8),
            other => panic!("expected a non-UTF-8 path error, got {other}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original_contents);
    }

    /// The earlier test's directory already exists from a prior save, so it
    /// cannot tell whether the UTF-8 guard runs before `create_dir_all`. This
    /// one starts with no parent directory at all.
    #[cfg(unix)]
    #[test]
    fn a_non_utf8_path_is_refused_before_the_parent_directory_is_created() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let directory = TempDir::new().unwrap();
        let path = directory.path().join("nested/settings.toml");
        let non_utf8 = PathBuf::from(OsStr::from_bytes(&[0xFF, 0xFE]));
        let attempted = Settings {
            repositories: vec![non_utf8.clone()],
        };

        let error = save(&path, &attempted).unwrap_err();

        match error {
            SettingsError::NonUtf8Path { path: bad_path } => assert_eq!(bad_path, non_utf8),
            other => panic!("expected a non-UTF-8 path error, got {other}"),
        }
        assert!(!path.parent().unwrap().exists());
    }
}
