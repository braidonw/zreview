use std::sync::Arc;

use domain::DiffFile;
use gpui::{
    App, AppContext, Application, Bounds, Focusable, WindowBounds, WindowOptions, px, size,
};
use ui::DiffView;

fn main() {
    Application::new().run(|cx: &mut App| {
        ui::init(cx);

        let bounds = Bounds::centered(None, size(px(1_280.0), px(800.0)), cx);
        let file = Arc::new(DiffFile::demo(100_000));
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
}
