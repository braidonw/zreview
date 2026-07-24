#![allow(clippy::unreadable_literal)]

use std::sync::Arc;

use domain::{DiffFile, DiffLine, DiffLineKind};
use gpui::{
    App, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyBinding, KeyDownEvent,
    ListAlignment, ListState, MouseButton, Render, SharedString, Window, actions, div, list,
    prelude::*, px, rgb,
};

actions!(
    diff_view,
    [
        SelectNextLine,
        SelectPreviousLine,
        ToggleComment,
        CopySelectedLine,
    ]
);

const ROW_HEIGHT: f32 = 24.0;
const COMMENT_HEIGHT: f32 = 104.0;
const GUTTER_WIDTH: f32 = 58.0;

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("j", SelectNextLine, Some("DiffView")),
        KeyBinding::new("down", SelectNextLine, Some("DiffView")),
        KeyBinding::new("k", SelectPreviousLine, Some("DiffView")),
        KeyBinding::new("up", SelectPreviousLine, Some("DiffView")),
        KeyBinding::new("c", ToggleComment, Some("DiffView")),
        KeyBinding::new("cmd-c", CopySelectedLine, Some("DiffView")),
    ]);
}

/// A deliberately small text-entry control for the virtualization spike.
///
/// It proves that a focused, variable-height child can live inside the list.
/// Production review comments will replace this with a full IME-aware editor.
pub struct CommentEditor {
    content: String,
    focus_handle: FocusHandle,
}

impl CommentEditor {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            content: String::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.content.clear();
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let handled = match keystroke.key.as_str() {
            "backspace" => {
                self.content.pop();
                true
            }
            "enter" => {
                self.content.push('\n');
                true
            }
            "v" if keystroke.modifiers.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.content.push_str(&text);
                }
                true
            }
            _ if !keystroke.modifiers.control
                && !keystroke.modifiers.platform
                && !keystroke.modifiers.function =>
            {
                if let Some(text) = keystroke.key_char.as_deref() {
                    self.content.push_str(text);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };

        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }
}

impl Focusable for CommentEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CommentEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let display: SharedString = if self.content.is_empty() {
            "Write a review comment…".into()
        } else {
            format!("{}│", self.content.replace('\n', " ↵ ")).into()
        };
        let is_empty = self.content.is_empty();
        let focus_handle = self.focus_handle.clone();

        div()
            .id("comment-editor")
            .key_context("CommentEditor")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&focus_handle);
                cx.stop_propagation();
            })
            .h(px(58.0))
            .w_full()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x3b82f6))
            .bg(rgb(0x111827))
            .text_color(if is_empty {
                rgb(0x6b7280)
            } else {
                rgb(0xe5e7eb)
            })
            .text_sm()
            .cursor_text()
            .child(display)
    }
}

pub struct DiffView {
    file: Arc<DiffFile>,
    list_state: ListState,
    selected_line: usize,
    comment_line: Option<usize>,
    comment_editor: Entity<CommentEditor>,
    focus_handle: FocusHandle,
}

impl DiffView {
    #[must_use]
    pub fn new(file: Arc<DiffFile>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let item_count = file.line_count();
        Self {
            file,
            list_state: ListState::new(item_count, ListAlignment::Top, px(ROW_HEIGHT)),
            selected_line: 0,
            comment_line: None,
            comment_editor: cx.new(CommentEditor::new),
            focus_handle: cx.focus_handle(),
        }
    }

    #[must_use]
    pub const fn selected_line(&self) -> usize {
        self.selected_line
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_line = index.min(self.file.line_count().saturating_sub(1));
        self.list_state.scroll_to_reveal_item(self.selected_line);
        cx.notify();
    }

    fn select_next_line(&mut self, _: &SelectNextLine, _: &mut Window, cx: &mut Context<Self>) {
        self.select(self.selected_line.saturating_add(1), cx);
    }

    fn select_previous_line(
        &mut self,
        _: &SelectPreviousLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select(self.selected_line.saturating_sub(1), cx);
    }

    fn toggle_comment(&mut self, _: &ToggleComment, window: &mut Window, cx: &mut Context<Self>) {
        if self.comment_line == Some(self.selected_line) {
            self.comment_line = None;
            window.focus(&self.focus_handle);
        } else {
            self.comment_line = Some(self.selected_line);
            self.comment_editor.update(cx, CommentEditor::clear);
            let editor_focus = self.comment_editor.read(cx).focus_handle.clone();
            window.focus(&editor_focus);
        }
        self.list_state.scroll_to_reveal_item(self.selected_line);
        cx.notify();
    }

    fn copy_selected_line(&mut self, _: &CopySelectedLine, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(line) = self.file.line(self.selected_line) {
            cx.write_to_clipboard(ClipboardItem::new_string(line.text.to_string()));
        }
    }

    #[allow(clippy::too_many_lines)]
    fn render_diff_line(
        line: &DiffLine,
        index: usize,
        selected: bool,
        show_comment: bool,
        view: &Entity<Self>,
        comment_editor: &Entity<CommentEditor>,
    ) -> gpui::AnyElement {
        let (row_bg, marker_color) = match line.kind {
            DiffLineKind::Context => (rgb(0x0f172a), rgb(0x64748b)),
            DiffLineKind::Addition => (rgb(0x10281d), rgb(0x4ade80)),
            DiffLineKind::Deletion => (rgb(0x30191d), rgb(0xf87171)),
            DiffLineKind::NoNewlineMarker => (rgb(0x0f172a), rgb(0xfbbf24)),
        };
        let old_number = line
            .old_line
            .map_or_else(String::new, |value| value.to_string());
        let new_number = line
            .new_line
            .map_or_else(String::new, |value| value.to_string());
        let marker = line.kind.marker().to_string();
        let text = SharedString::from(line.text.to_string());

        let select_view = view.clone();
        let comment_view = view.clone();
        let close_view = view.clone();

        div()
            .id(("diff-row", index))
            .w_full()
            .flex()
            .flex_col()
            .bg(if selected { rgb(0x1e3a5f) } else { row_bg })
            .child(
                div()
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .flex()
                    .items_center()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        select_view.update(cx, |this, cx| {
                            this.selected_line = index;
                            window.focus(&this.focus_handle);
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .w(px(GUTTER_WIDTH))
                            .pr_2()
                            .text_right()
                            .text_color(rgb(0x64748b))
                            .child(old_number),
                    )
                    .child(
                        div()
                            .w(px(GUTTER_WIDTH))
                            .pr_2()
                            .text_right()
                            .text_color(rgb(0x64748b))
                            .child(new_number),
                    )
                    .child(div().w(px(20.0)).text_color(marker_color).child(marker))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(rgb(0xdbeafe))
                            .child(text),
                    )
                    .when(selected && !show_comment, |row| {
                        row.child(
                            div()
                                .id(("add-comment", index))
                                .mr_2()
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .bg(rgb(0x2563eb))
                                .text_xs()
                                .text_color(rgb(0xffffff))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    cx.stop_propagation();
                                    comment_view.update(cx, |this, cx| {
                                        this.selected_line = index;
                                        this.comment_line = Some(index);
                                        this.comment_editor.update(cx, CommentEditor::clear);
                                        let handle =
                                            this.comment_editor.read(cx).focus_handle.clone();
                                        window.focus(&handle);
                                        cx.notify();
                                    });
                                })
                                .child("Comment"),
                        )
                    }),
            )
            .when(show_comment, |row| {
                row.child(
                    div()
                        .h(px(COMMENT_HEIGHT))
                        .ml(px(GUTTER_WIDTH * 2.0 + 20.0))
                        .mr_3()
                        .pt_2()
                        .flex()
                        .gap_2()
                        .child(div().flex_1().child(comment_editor.clone()))
                        .child(
                            div()
                                .id(("close-comment", index))
                                .px_3()
                                .h(px(32.0))
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(0x475569))
                                .text_color(rgb(0xcbd5e1))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    cx.stop_propagation();
                                    close_view.update(cx, |this, cx| {
                                        this.comment_line = None;
                                        window.focus(&this.focus_handle);
                                        cx.notify();
                                    });
                                })
                                .child("Close"),
                        ),
                )
            })
            .into_any()
    }
}

impl Focusable for DiffView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DiffView {
    #[allow(clippy::too_many_lines)]
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let file = self.file.clone();
        let selected_line = self.selected_line;
        let comment_line = self.comment_line;
        let view = cx.entity();
        let comment_editor = self.comment_editor.clone();
        let path = SharedString::from(file.path.to_string());
        let line_count = file.line_count();
        let hunk_header = file.hunks.first().map_or_else(
            || SharedString::from("No hunks"),
            |hunk| SharedString::from(hunk.header.to_string()),
        );

        div()
            .id("diff-view")
            .key_context("DiffView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next_line))
            .on_action(cx.listener(Self::select_previous_line))
            .on_action(cx.listener(Self::toggle_comment))
            .on_action(cx.listener(Self::copy_selected_line))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x020617))
            .font_family("SF Mono")
            .text_size(px(13.0))
            .child(
                div()
                    .h(px(48.0))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(0x1e293b))
                    .bg(rgb(0x0f172a))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_color(rgb(0xf8fafc))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("ZReview"),
                            )
                            .child(div().text_color(rgb(0x94a3b8)).child(path)),
                    )
                    .child(
                        div()
                            .text_color(rgb(0x94a3b8))
                            .child(format!("{line_count} lines · GPUI virtualization spike")),
                    ),
            )
            .child(
                div()
                    .h(px(32.0))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .bg(rgb(0x172554))
                    .text_color(rgb(0x93c5fd))
                    .child(hunk_header),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        list(self.list_state.clone(), move |index, _, _| {
                            let line = &file.lines[index];
                            Self::render_diff_line(
                                line,
                                index,
                                selected_line == index,
                                comment_line == Some(index),
                                &view,
                                &comment_editor,
                            )
                        })
                        .flex_1(),
                    )
                    .child(
                        div()
                            .w(px(230.0))
                            .flex_shrink_0()
                            .p_4()
                            .border_l_1()
                            .border_color(rgb(0x1e293b))
                            .bg(rgb(0x0f172a))
                            .text_color(rgb(0x94a3b8))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .text_color(rgb(0xf8fafc))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Prototype controls"),
                            )
                            .child("j / ↓  Next line")
                            .child("k / ↑  Previous line")
                            .child("c      Toggle comment")
                            .child("⌘C     Copy selected line")
                            .child(format!("Selected row: {}", selected_line + 1))
                            .child(
                                div()
                                    .mt_4()
                                    .text_xs()
                                    .text_color(rgb(0x64748b))
                                    .child(
                                        "The comment field is intentionally minimal; this spike validates focus and variable-height rows.",
                                    ),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn renders_and_navigates_a_large_diff(cx: &mut TestAppContext) {
        cx.update(init);
        let file = Arc::new(DiffFile::demo(100_000));
        let (view, cx) = cx.add_window_view(|window, cx| DiffView::new(file, window, cx));

        cx.update(|window, app| {
            window.focus(&view.read(app).focus_handle(app));
        });
        cx.dispatch_action(SelectNextLine);
        assert_eq!(cx.read(|app| view.read(app).selected_line()), 1);

        cx.dispatch_action(ToggleComment);
        cx.simulate_input("hello");

        assert_eq!(cx.read(|app| view.read(app).comment_line), Some(1));
        assert_eq!(
            cx.read(|app| view.read(app).comment_editor.read(app).content.clone()),
            "hello",
        );
    }
}
