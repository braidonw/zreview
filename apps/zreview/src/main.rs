use std::{env, path::Path, process::ExitCode};

use github::PullRequestSelector;
use gpui::{App, AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};
use session::SessionRequest;
use ui::SessionView;

mod loading;

const USAGE: &str = "usage:
  zreview
  zreview <repository> <base> [<head>]
  zreview pr [<repository>] <number-or-url>";

fn main() -> ExitCode {
    // Only argument parsing happens before the window opens. Everything that can
    // be slow or fail — Git, gh, the network — is reported inside the app.
    let request = match parse_arguments(&env::args().skip(1).collect::<Vec<_>>()) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("zreview: {message}");
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    Application::new().run(move |cx: &mut App| {
        ui::init(cx);

        let description = request.description();
        let bounds = Bounds::centered(None, size(px(1_280.0), px(800.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    focus: true,
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |window, cx| {
                    window.set_window_title("ZReview");
                    cx.new(|cx| SessionView::loading(description, cx))
                },
            )
            .expect("failed to open ZReview window");

        loading::spawn(window, request, cx);
        cx.activate(true);
    });

    ExitCode::SUCCESS
}

fn parse_arguments(arguments: &[String]) -> Result<SessionRequest, String> {
    match arguments {
        [] => Ok(SessionRequest::Demo),
        [first, rest @ ..] if first == "pr" => parse_pull_request(rest),
        [repository, base] => Ok(SessionRequest::LocalComparison {
            repository: Path::new(repository).to_path_buf(),
            base: base.clone(),
            head: "HEAD".to_owned(),
        }),
        [repository, base, head] => Ok(SessionRequest::LocalComparison {
            repository: Path::new(repository).to_path_buf(),
            base: base.clone(),
            head: head.clone(),
        }),
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
    fn no_arguments_opens_the_generated_fixture() {
        assert_eq!(parse_arguments(&[]).unwrap(), SessionRequest::Demo);
    }

    #[test]
    fn a_local_comparison_defaults_its_head_to_head() {
        assert_eq!(
            parse_arguments(&arguments(&["/tmp/repository", "main"])).unwrap(),
            SessionRequest::LocalComparison {
                repository: Path::new("/tmp/repository").to_path_buf(),
                base: "main".to_owned(),
                head: "HEAD".to_owned(),
            },
        );
        assert_eq!(
            parse_arguments(&arguments(&["/tmp/repository", "main", "feature"])).unwrap(),
            SessionRequest::LocalComparison {
                repository: Path::new("/tmp/repository").to_path_buf(),
                base: "main".to_owned(),
                head: "feature".to_owned(),
            },
        );
    }

    #[test]
    fn a_pull_request_takes_an_optional_repository() {
        assert_eq!(
            parse_arguments(&arguments(&["pr", "/tmp/repository", "42"])).unwrap(),
            SessionRequest::PullRequest {
                repository: Path::new("/tmp/repository").to_path_buf(),
                selector: PullRequestSelector::Number(42),
            },
        );
        // Without a repository the current directory is used, so this only
        // asserts that the selector survived.
        let request = parse_arguments(&arguments(&["pr", "42"])).unwrap();
        assert!(matches!(
            request,
            SessionRequest::PullRequest {
                selector: PullRequestSelector::Number(42),
                ..
            },
        ));
    }

    #[test]
    fn unusable_argument_counts_are_rejected() {
        assert!(parse_arguments(&arguments(&["only-a-repository"])).is_err());
        assert!(parse_arguments(&arguments(&["a", "b", "c", "d"])).is_err());
        assert!(parse_arguments(&arguments(&["pr"])).is_err());
        assert!(parse_arguments(&arguments(&["pr", "a", "b", "c"])).is_err());
    }
}
