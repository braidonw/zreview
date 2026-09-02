use std::{env, path::Path, process::ExitCode};

use github::PullRequestSelector;
use session::{ReviewStorage, SessionRequest};

const USAGE: &str = "usage:
  desktop
  desktop <repository> <base> [<head>]
  desktop pr [<repository>] <number-or-url>";

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
        [first, rest @ ..] if first == "pr" => {
            Ok((parse_pull_request(rest)?, ReviewStorage::Default))
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

fn parse_pull_request(arguments: &[String]) -> Result<SessionRequest, String> {
    let (repository, selector) = match arguments {
        [selector] => (
            env::current_dir().map_err(|error| error.to_string())?,
            selector,
        ),
        [repository, selector] => (Path::new(repository).to_path_buf(), selector),
        _ => return Err("expected a pull request number or URL".to_owned()),
    };

    Ok(SessionRequest::PullRequest {
        repository,
        selector: parse_pull_request_selector(selector)?,
    })
}

fn parse_pull_request_selector(value: &str) -> Result<PullRequestSelector, String> {
    if value.starts_with("https://") {
        Ok(PullRequestSelector::Url(value.to_owned()))
    } else {
        value
            .parse::<u64>()
            .ok()
            .filter(|number| *number > 0)
            .map(PullRequestSelector::Number)
            .ok_or_else(|| format!("invalid pull request number or URL {value:?}"))
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
        assert!(parse_arguments(&arguments(&["pr"])).is_err());
        assert!(parse_arguments(&arguments(&["pr", "a", "b", "c"])).is_err());
    }

    #[test]
    fn parses_pull_request_numbers_and_urls() {
        assert_eq!(
            parse_pull_request_selector("42").unwrap(),
            PullRequestSelector::Number(42),
        );
        assert_eq!(
            parse_pull_request_selector("https://github.com/acme/widgets/pull/42").unwrap(),
            PullRequestSelector::Url("https://github.com/acme/widgets/pull/42".to_owned()),
        );
        assert!(parse_pull_request_selector("0").is_err());
        assert!(parse_pull_request_selector("not-a-pr").is_err());
    }

    #[test]
    fn a_pull_request_takes_an_optional_repository_and_uses_default_storage() {
        assert_eq!(
            parse_arguments(&arguments(&["pr", "/tmp/repository", "42"])).unwrap(),
            (
                SessionRequest::PullRequest {
                    repository: Path::new("/tmp/repository").to_path_buf(),
                    selector: PullRequestSelector::Number(42),
                },
                ReviewStorage::Default,
            ),
        );
        // Without a repository the current directory is used, so this only
        // asserts that the selector and storage survived.
        let (request, storage) = parse_arguments(&arguments(&["pr", "42"])).unwrap();
        assert!(matches!(
            request,
            SessionRequest::PullRequest {
                selector: PullRequestSelector::Number(42),
                ..
            },
        ));
        assert_eq!(storage, ReviewStorage::Default);
    }
}
