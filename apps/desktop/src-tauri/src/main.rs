use std::{env, path::Path, process::ExitCode};

use desktop_lib::Launch;
use github::PullRequestSelector;
use session::{ReviewStorage, SessionRequest};

const USAGE: &str = "usage:
  desktop
  desktop demo
  desktop <repository> <base> [<head>]
  desktop pr [<repository>] <number-or-url>";

fn main() -> ExitCode {
    // Only argument parsing happens before the window opens. Everything that can be slow or fail, such as Git, gh, or the network, is reported inside the app.
    let launch = match parse_arguments(&env::args().skip(1).collect::<Vec<_>>()) {
        Ok(launch) => launch,
        Err(message) => {
            eprintln!("desktop: {message}");
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    desktop_lib::run(launch);

    ExitCode::SUCCESS
}

fn parse_arguments(arguments: &[String]) -> Result<Launch, String> {
    match arguments {
        [] => Ok(Launch::Home),
        [only] if only == "demo" => Ok(session(SessionRequest::Demo, ReviewStorage::Disabled)),
        [first, rest @ ..] if first == "pr" => {
            Ok(session(parse_pull_request(rest)?, ReviewStorage::Default))
        }
        [repository, base] => Ok(session(
            SessionRequest::LocalComparison {
                repository: Path::new(repository).to_path_buf(),
                base: base.clone(),
                head: "HEAD".to_owned(),
            },
            ReviewStorage::Default,
        )),
        [repository, base, head] => Ok(session(
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

fn session(request: SessionRequest, storage: ReviewStorage) -> Launch {
    Launch::Session { request, storage }
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

    /// The Session a launch opens, for the arguments that open one directly.
    fn opened_session(launch: Launch) -> (SessionRequest, ReviewStorage) {
        match launch {
            Launch::Session { request, storage } => (request, storage),
            Launch::Home => panic!("expected a session launch, got Home"),
        }
    }

    #[test]
    fn no_arguments_opens_home() {
        assert_eq!(parse_arguments(&[]).unwrap(), Launch::Home);
    }

    #[test]
    fn demo_opens_the_generated_fixture_with_storage_disabled() {
        assert_eq!(
            opened_session(parse_arguments(&arguments(&["demo"])).unwrap()),
            (SessionRequest::Demo, ReviewStorage::Disabled),
        );
    }

    #[test]
    fn a_local_comparison_defaults_its_head_to_head_and_uses_default_storage() {
        assert_eq!(
            opened_session(parse_arguments(&arguments(&["/tmp/repository", "main"])).unwrap()),
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
            opened_session(
                parse_arguments(&arguments(&["/tmp/repository", "main", "feature"])).unwrap()
            ),
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
            opened_session(parse_arguments(&arguments(&["pr", "/tmp/repository", "42"])).unwrap()),
            (
                SessionRequest::PullRequest {
                    repository: Path::new("/tmp/repository").to_path_buf(),
                    selector: PullRequestSelector::Number(42),
                },
                ReviewStorage::Default,
            ),
        );
        let (request, storage) =
            opened_session(parse_arguments(&arguments(&["pr", "42"])).unwrap());
        assert!(matches!(
            request,
            SessionRequest::PullRequest {
                selector: PullRequestSelector::Number(42),
                ..
            },
        ));
        assert_eq!(storage, ReviewStorage::Default);
    }

    #[test]
    fn a_pull_request_repository_accepts_a_url_selector() {
        assert_eq!(
            opened_session(
                parse_arguments(&arguments(&[
                    "pr",
                    "/tmp/repository",
                    "https://github.com/acme/widgets/pull/42",
                ]))
                .unwrap()
            ),
            (
                SessionRequest::PullRequest {
                    repository: Path::new("/tmp/repository").to_path_buf(),
                    selector: PullRequestSelector::Url(
                        "https://github.com/acme/widgets/pull/42".to_owned()
                    ),
                },
                ReviewStorage::Default,
            ),
        );
    }
}
