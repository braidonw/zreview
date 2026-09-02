use std::{env, path::Path, process::ExitCode};

use session::{ReviewStorage, SessionRequest};

const USAGE: &str = "usage:
  desktop
  desktop <repository> <base> [<head>]";

fn main() -> ExitCode {
    // Only argument parsing happens before the window opens. Everything that can
    // be slow or fail, such as Git, is reported inside the app.
    let (request, storage) = match parse_arguments(&env::args().skip(1).collect::<Vec<_>>()) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("desktop: {message}");
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    desktop_lib::run(request, storage);

    ExitCode::SUCCESS
}

fn parse_arguments(arguments: &[String]) -> Result<(SessionRequest, ReviewStorage), String> {
    match arguments {
        [] => Ok((SessionRequest::Demo, ReviewStorage::Disabled)),
        [first, ..] if first == "pr" => {
            Err("pull requests are not supported by this build".to_owned())
        }
        [repository, base] => Ok((
            SessionRequest::LocalComparison {
                repository: Path::new(repository).to_path_buf(),
                base: base.clone(),
                head: "HEAD".to_owned(),
            },
            ReviewStorage::Default,
        )),
        [repository, base, head] => Ok((
            SessionRequest::LocalComparison {
                repository: Path::new(repository).to_path_buf(),
                base: base.clone(),
                head: head.clone(),
            },
            ReviewStorage::Default,
        )),
        _ => Err("expected a repository, base revision, and optional head revision".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn no_arguments_opens_the_generated_fixture_with_storage_disabled() {
        assert_eq!(
            parse_arguments(&[]).unwrap(),
            (SessionRequest::Demo, ReviewStorage::Disabled),
        );
    }

    #[test]
    fn a_local_comparison_defaults_its_head_to_head_and_uses_default_storage() {
        assert_eq!(
            parse_arguments(&arguments(&["/tmp/repository", "main"])).unwrap(),
            (
                SessionRequest::LocalComparison {
                    repository: Path::new("/tmp/repository").to_path_buf(),
                    base: "main".to_owned(),
                    head: "HEAD".to_owned(),
                },
                ReviewStorage::Default,
            ),
        );
        assert_eq!(
            parse_arguments(&arguments(&["/tmp/repository", "main", "feature"])).unwrap(),
            (
                SessionRequest::LocalComparison {
                    repository: Path::new("/tmp/repository").to_path_buf(),
                    base: "main".to_owned(),
                    head: "feature".to_owned(),
                },
                ReviewStorage::Default,
            ),
        );
    }

    #[test]
    fn unusable_argument_counts_are_rejected() {
        assert!(parse_arguments(&arguments(&["only-a-repository"])).is_err());
        assert!(parse_arguments(&arguments(&["a", "b", "c", "d"])).is_err());
    }

    /// Pull requests are out of scope for this CLI, so `pr` is intercepted
    /// before arity matching rather than parsed as a repository name.
    #[test]
    fn a_pr_argument_is_rejected() {
        assert!(parse_arguments(&arguments(&["pr"])).is_err());
        assert!(parse_arguments(&arguments(&["pr", "123"])).is_err());
    }
}
