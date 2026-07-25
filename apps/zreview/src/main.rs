use std::{env, path::Path, process::ExitCode};

use domain::{DiffFile, FileStatus, ReviewSession, SessionSource};
use git::{ComparisonMode, load_comparison};
use github::{GithubClient, PullRequestLocator, PullRequestSelector};
use gpui::{
    App, AppContext, Application, Bounds, Focusable, WindowBounds, WindowOptions, px, size,
};
use ui::ReviewView;

fn main() -> ExitCode {
    let session = match load_requested_session() {
        Ok(session) => session,
        Err(message) => {
            eprintln!("zreview: {message}");
            eprintln!("usage:");
            eprintln!("  zreview");
            eprintln!("  zreview <repository> <base> [<head>]");
            eprintln!("  zreview pr [<repository>] <number-or-url>");
            return ExitCode::FAILURE;
        }
    };

    Application::new().run(move |cx: &mut App| {
        ui::init(cx);

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
                    cx.new(|cx| ReviewView::new(session, window, cx))
                },
            )
            .expect("failed to open ZReview window");

        window
            .update(cx, |view, window, cx| {
                window.focus(&view.focus_handle(cx));
            })
            .expect("failed to focus ZReview diff");
        cx.activate(true);
    });

    ExitCode::SUCCESS
}

fn load_requested_session() -> Result<ReviewSession, String> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments.is_empty() {
        return demo_session();
    }
    if arguments.first().is_some_and(|argument| argument == "pr") {
        return load_pull_request_session(&arguments[1..]);
    }
    if !(2..=3).contains(&arguments.len()) {
        return Err("expected a repository, base revision, and optional head revision".to_owned());
    }

    load_local_session(&arguments)
}

fn load_local_session(arguments: &[String]) -> Result<ReviewSession, String> {
    let repository = &arguments[0];
    let base = &arguments[1];
    let head = arguments.get(2).map_or("HEAD", String::as_str);
    let comparison = load_comparison(repository, base, head, ComparisonMode::MergeBase)
        .map_err(|error| error.to_string())?;
    let source = SessionSource::LocalComparison {
        repository_root: comparison.repository_root,
        base_sha: comparison.base_sha,
        diff_base_sha: comparison.diff_base_sha,
        head_sha: comparison.head_sha,
    };

    ReviewSession::new(source, comparison.files)
        .map_err(|_| format!("{base}...{head} contains no changed files"))
}

fn load_pull_request_session(arguments: &[String]) -> Result<ReviewSession, String> {
    let (repository, selector) = match arguments {
        [selector] => (
            env::current_dir().map_err(|error| error.to_string())?,
            selector,
        ),
        [repository, selector] => (Path::new(repository).to_path_buf(), selector),
        _ => return Err("expected a PR number/URL and an optional repository path".to_owned()),
    };
    let selector = parse_pull_request_selector(selector)?;
    let client = GithubClient::default();
    let pull_request = client
        .load_pull_request(&repository, &selector)
        .map_err(|error| error.to_string())?;
    let metadata = pull_request.metadata;
    let comparison = pull_request.comparison;
    let number = metadata.number;
    let locator = PullRequestLocator {
        repository: metadata.repository.clone(),
        number,
    };
    let source = SessionSource::GitHubPullRequest {
        repository_root: comparison.repository_root,
        owner: metadata.repository.owner.into(),
        repository: metadata.repository.name.into(),
        number: metadata.number,
        title: metadata.title.into(),
        url: metadata.url.into(),
        base_ref: metadata.base_ref.into(),
        head_ref: metadata.head_ref.into(),
        base_sha: comparison.base_sha,
        recorded_base_sha: metadata.base_sha,
        diff_base_sha: comparison.diff_base_sha,
        head_sha: metadata.head_sha,
    };

    let mut session = ReviewSession::new(source, comparison.files)
        .map_err(|_| format!("pull request #{number} contains no changed files"))?;

    // Existing conversations are part of the review, but a PR is still worth
    // reading without them, so a failure here is reported rather than fatal.
    match client.fetch_review_comments(&repository, &locator) {
        Ok(comments) => {
            session.set_review_comments(comments);
        }
        Err(error) => eprintln!("zreview: could not load existing review comments: {error}"),
    }

    Ok(session)
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

fn demo_session() -> Result<ReviewSession, String> {
    let files = (0..12)
        .map(|index| {
            let mut file = DiffFile::demo(if index == 0 {
                100_000
            } else {
                200 + index * 25
            });
            file.path = format!("src/review_fixture_{index:02}.rs").into();
            file.status = match index % 4 {
                0 => FileStatus::Modified,
                1 => FileStatus::Added,
                2 => FileStatus::Deleted,
                _ => FileStatus::Renamed,
            };
            file
        })
        .collect::<Vec<_>>();

    ReviewSession::new(SessionSource::Demo, files.into()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
