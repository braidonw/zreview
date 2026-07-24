use std::{env, process::ExitCode};

use domain::{DiffFile, FileStatus, ReviewSession, SessionSource};
use git::{ComparisonMode, load_comparison};
use gpui::{
    App, AppContext, Application, Bounds, Focusable, WindowBounds, WindowOptions, px, size,
};
use ui::ReviewView;

fn main() -> ExitCode {
    let session = match load_requested_session() {
        Ok(session) => session,
        Err(message) => {
            eprintln!("zreview: {message}");
            eprintln!("usage: zreview [<repository> <base> [<head>]]");
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
                    window.set_window_title("ZReview — local comparison");
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
    if !(2..=3).contains(&arguments.len()) {
        return Err("expected a repository, base revision, and optional head revision".to_owned());
    }

    let repository = &arguments[0];
    let base = &arguments[1];
    let head = arguments.get(2).map_or("HEAD", String::as_str);
    let comparison = load_comparison(repository, base, head, ComparisonMode::MergeBase)
        .map_err(|error| error.to_string())?;
    let source = SessionSource::LocalComparison {
        repository_root: comparison.repository_root,
        base_sha: comparison.base_sha,
        head_sha: comparison.head_sha,
    };

    ReviewSession::new(source, comparison.files)
        .map_err(|_| format!("{base}...{head} contains no changed files"))
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
