use std::{env, process::ExitCode, sync::Arc};

use domain::DiffFile;
use git::{ComparisonMode, load_comparison};
use gpui::{
    App, AppContext, Application, Bounds, Focusable, WindowBounds, WindowOptions, px, size,
};
use ui::DiffView;

fn main() -> ExitCode {
    let file = match load_requested_file() {
        Ok(file) => file,
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
                    window.set_window_title("ZReview — GPUI diff spike");
                    cx.new(|cx| DiffView::new(file, window, cx))
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

fn load_requested_file() -> Result<Arc<DiffFile>, String> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments.is_empty() {
        return Ok(Arc::new(DiffFile::demo(100_000)));
    }
    if !(2..=3).contains(&arguments.len()) {
        return Err("expected a repository, base revision, and optional head revision".to_owned());
    }

    let repository = &arguments[0];
    let base = &arguments[1];
    let head = arguments.get(2).map_or("HEAD", String::as_str);
    let comparison = load_comparison(repository, base, head, ComparisonMode::MergeBase)
        .map_err(|error| error.to_string())?;
    let file = comparison
        .files
        .iter()
        .find(|file| !file.is_binary && !file.lines.is_empty())
        .or_else(|| comparison.files.first())
        .ok_or_else(|| format!("{base}...{head} contains no changed files"))?;

    Ok(Arc::new(file.clone()))
}
