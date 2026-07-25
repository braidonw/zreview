#![allow(clippy::unreadable_literal)]

use std::sync::Arc;

use domain::{
    CommentThread, DiffFile, DiffLine, DiffLineKind, FileStatus, PlacedComments, ReviewSession,
    SessionSource,
};
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
        CloseComment,
        CopySelectedLine,
    ]
);
actions!(
    review_session,
    [SelectNextFile, SelectPreviousFile, ToggleViewed]
);

const ROW_HEIGHT: f32 = 24.0;
const COMMENT_HEIGHT: f32 = 104.0;
const GUTTER_WIDTH: f32 = 58.0;

fn short_sha(value: &str) -> &str {
    value.get(..7).unwrap_or(value)
}

/// `CommentEditor` renders inside `DiffView`, so a bare `DiffView` predicate also
/// matches while the composer is focused — which would make `j`, `k`, and `c`
/// impossible to type into a review comment. Negating the composer's context
/// excludes it, because GPUI evaluates `!` against the whole focus path.
const DIFF_CONTEXT: &str = "DiffView && !CommentEditor";
const SESSION_CONTEXT: &str = "ReviewSession && !CommentEditor";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("j", SelectNextLine, Some(DIFF_CONTEXT)),
        KeyBinding::new("down", SelectNextLine, Some(DIFF_CONTEXT)),
        KeyBinding::new("k", SelectPreviousLine, Some(DIFF_CONTEXT)),
        KeyBinding::new("up", SelectPreviousLine, Some(DIFF_CONTEXT)),
        KeyBinding::new("c", ToggleComment, Some(DIFF_CONTEXT)),
        KeyBinding::new("cmd-c", CopySelectedLine, Some(DIFF_CONTEXT)),
        KeyBinding::new("cmd-shift-j", SelectNextFile, Some(SESSION_CONTEXT)),
        KeyBinding::new("cmd-shift-k", SelectPreviousFile, Some(SESSION_CONTEXT)),
        KeyBinding::new("cmd-shift-v", ToggleViewed, Some(SESSION_CONTEXT)),
        KeyBinding::new("escape", CloseComment, Some("CommentEditor")),
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

/// Everything one virtualized diff row needs to draw itself.
struct DiffRow<'a> {
    line: &'a DiffLine,
    index: usize,
    selected: bool,
    show_comment: bool,
    threads: &'a [CommentThread],
}

pub struct DiffView {
    file: Arc<DiffFile>,
    /// Index of `file` in the session, which is how threads are keyed.
    file_index: usize,
    comments: Arc<PlacedComments>,
    list_state: ListState,
    selected_line: usize,
    comment_line: Option<usize>,
    comment_editor: Entity<CommentEditor>,
    focus_handle: FocusHandle,
}

impl DiffView {
    #[must_use]
    pub fn new(
        file: Arc<DiffFile>,
        file_index: usize,
        comments: Arc<PlacedComments>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let item_count = file.line_count();
        Self {
            file,
            file_index,
            comments,
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

    fn set_file(&mut self, file: Arc<DiffFile>, file_index: usize, cx: &mut Context<Self>) {
        self.list_state = ListState::new(file.line_count(), ListAlignment::Top, px(ROW_HEIGHT));
        self.file = file;
        self.file_index = file_index;
        self.selected_line = 0;
        self.comment_line = None;
        self.comment_editor.update(cx, CommentEditor::clear);
        cx.notify();
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

    /// Dismisses the composer. Bound to `escape` in the composer's own context,
    /// since `c` is now reserved for typing while the composer has focus.
    fn close_comment(&mut self, _: &CloseComment, window: &mut Window, cx: &mut Context<Self>) {
        if self.comment_line.take().is_some() {
            window.focus(&self.focus_handle);
            cx.notify();
        }
    }

    fn copy_selected_line(&mut self, _: &CopySelectedLine, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(line) = self.file.line(self.selected_line) {
            cx.write_to_clipboard(ClipboardItem::new_string(line.text.to_string()));
        }
    }

    /// Renders one published thread read-only beneath the row it is anchored to.
    ///
    /// Replying to and resolving threads are deliberately out of the MVP, so this
    /// never offers an action that would post to GitHub.
    fn render_thread(thread: &CommentThread) -> gpui::AnyElement {
        let replies = thread.reply_count();

        div()
            // Comment ids are unique across the forge, so they key the element
            // without colliding between rows.
            .id(("thread", thread.id()))
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_l_2()
            .border_color(rgb(0x818cf8))
            .bg(rgb(0x131c31))
            .children(thread.comments().iter().map(|comment| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .text_xs()
                            .child(
                                div()
                                    .text_color(rgb(0xc7d2fe))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(SharedString::from(comment.author.to_string())),
                            )
                            .child(
                                div()
                                    .text_color(rgb(0x64748b))
                                    .child(SharedString::from(comment.created_at.to_string())),
                            )
                            .when(comment.is_multiline(), |header| {
                                header.child(div().text_color(rgb(0x64748b)).child(format!(
                                    "lines {}–{}",
                                    comment.start_line.unwrap_or_default(),
                                    comment.line.unwrap_or_default(),
                                )))
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xcbd5e1))
                            .child(SharedString::from(comment.body.to_string())),
                    )
            }))
            .when(replies > 0, |thread| {
                thread.child(div().text_xs().text_color(rgb(0x64748b)).child(format!(
                    "{replies} repl{}",
                    if replies == 1 { "y" } else { "ies" }
                )))
            })
            .into_any()
    }

    #[allow(clippy::too_many_lines)]
    fn render_diff_line(
        row: &DiffRow<'_>,
        view: &Entity<Self>,
        comment_editor: &Entity<CommentEditor>,
    ) -> gpui::AnyElement {
        let &DiffRow {
            line,
            index,
            selected,
            show_comment,
            threads,
        } = row;
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
                    .when(!threads.is_empty(), |row| {
                        row.child(
                            div()
                                .mr_2()
                                .px_1()
                                .rounded_sm()
                                .bg(rgb(0x312e81))
                                .text_xs()
                                .text_color(rgb(0xc7d2fe))
                                .child(format!("{}", threads.len())),
                        )
                    })
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
            .when(!threads.is_empty(), |row| {
                row.child(
                    div()
                        .ml(px(GUTTER_WIDTH * 2.0 + 20.0))
                        .mr_3()
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(threads.iter().map(Self::render_thread)),
                )
            })
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
        let file_index = self.file_index;
        let comments = Arc::clone(&self.comments);
        let selected_line = self.selected_line;
        let comment_line = self.comment_line;
        let view = cx.entity();
        let comment_editor = self.comment_editor.clone();
        let path = SharedString::from(file.path.to_string());
        let line_count = file.line_count();
        let thread_count = comments.thread_count_for_file(file_index);
        let unplaced = comments
            .unplaced_for_file(file_index)
            .cloned()
            .collect::<Vec<_>>();
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
            .on_action(cx.listener(Self::close_comment))
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
                            Self::render_diff_line(
                                &DiffRow {
                                    line: &file.lines[index],
                                    index,
                                    selected: selected_line == index,
                                    show_comment: comment_line == Some(index),
                                    threads: comments.threads_at(file_index, index),
                                },
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
                                    .child("Conversations"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .child(format!("{thread_count} on a diff line")),
                            )
                            // Threads GitHub reports without a usable position
                            // would otherwise be invisible, so they are listed
                            // against the file with the reason they cannot be
                            // shown inline.
                            .when(!unplaced.is_empty(), |panel| {
                                panel
                                    .child(
                                        div()
                                            .mt_2()
                                            .text_xs()
                                            .text_color(rgb(0xfbbf24))
                                            .child(format!("{} not on a line", unplaced.len())),
                                    )
                                    .children(unplaced.iter().map(|unplaced| {
                                        let root = unplaced.thread.root();
                                        div()
                                            .p_2()
                                            .rounded_md()
                                            .bg(rgb(0x131c31))
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0xfbbf24))
                                                    .child(unplaced.reason.to_string()),
                                            )
                                            .child(
                                                div().text_xs().text_color(rgb(0xc7d2fe)).child(
                                                    SharedString::from(root.author.to_string()),
                                                ),
                                            )
                                            .child(
                                                div().text_xs().text_color(rgb(0xcbd5e1)).child(
                                                    SharedString::from(root.body.to_string()),
                                                ),
                                            )
                                    }))
                            })
                            .child(div().mt_4().text_xs().text_color(rgb(0x64748b)).child(
                                format!(
                                    "Row {} · j/k move · c comment · esc close",
                                    selected_line + 1
                                ),
                            )),
                    ),
            )
    }
}

pub struct ReviewView {
    session: ReviewSession,
    diff_view: Entity<DiffView>,
    file_list_state: ListState,
}

impl ReviewView {
    #[must_use]
    pub fn new(session: ReviewSession, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selected_file = Arc::new(session.selected_file().clone());
        let selected_index = session.selected_file_index();
        let comments = session.shared_comments();
        let file_count = session.files().len();
        Self {
            session,
            diff_view: cx
                .new(|cx| DiffView::new(selected_file, selected_index, comments, window, cx)),
            file_list_state: ListState::new(file_count, ListAlignment::Top, px(36.0)),
        }
    }

    #[must_use]
    pub const fn selected_file_index(&self) -> usize {
        self.session.selected_file_index()
    }

    fn select_file(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.select_file(index) {
            let file = Arc::new(self.session.selected_file().clone());
            self.diff_view
                .update(cx, |view, cx| view.set_file(file, index, cx));
            self.file_list_state.scroll_to_reveal_item(index);
        }
        let focus_handle = self.diff_view.read(cx).focus_handle.clone();
        window.focus(&focus_handle);
        cx.notify();
    }

    fn select_next_file(
        &mut self,
        _: &SelectNextFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = self
            .session
            .selected_file_index()
            .saturating_add(1)
            .min(self.session.files().len().saturating_sub(1));
        self.select_file(next, window, cx);
    }

    fn select_previous_file(
        &mut self,
        _: &SelectPreviousFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_file(
            self.session.selected_file_index().saturating_sub(1),
            window,
            cx,
        );
    }

    fn toggle_viewed(&mut self, _: &ToggleViewed, _: &mut Window, cx: &mut Context<Self>) {
        self.session.toggle_selected_viewed();
        cx.notify();
    }

    fn render_file_row(
        file: &DiffFile,
        index: usize,
        selected: bool,
        viewed: bool,
        threads: usize,
        review_view: &Entity<Self>,
    ) -> gpui::AnyElement {
        let (status, status_color) = match file.status {
            FileStatus::Added => ("A", rgb(0x4ade80)),
            FileStatus::Deleted => ("D", rgb(0xf87171)),
            FileStatus::Modified => ("M", rgb(0xfbbf24)),
            FileStatus::Renamed => ("R", rgb(0x60a5fa)),
            FileStatus::Copied => ("C", rgb(0xa78bfa)),
            FileStatus::TypeChanged => ("T", rgb(0xf59e0b)),
            FileStatus::Unmerged => ("U", rgb(0xfb7185)),
        };
        let (additions, deletions) =
            file.lines
                .iter()
                .fold((0_usize, 0_usize), |counts, line| match line.kind {
                    DiffLineKind::Addition => (counts.0 + 1, counts.1),
                    DiffLineKind::Deletion => (counts.0, counts.1 + 1),
                    DiffLineKind::Context | DiffLineKind::NoNewlineMarker => counts,
                });
        let path = SharedString::from(file.path.to_string());
        let view = review_view.clone();

        div()
            .id(("changed-file", index))
            .h(px(36.0))
            .w_full()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .bg(if selected {
                rgb(0x1e3a5f)
            } else {
                rgb(0x0f172a)
            })
            .when(viewed, |row| row.opacity(0.55))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                view.update(cx, |this, cx| this.select_file(index, window, cx));
            })
            .child(
                div()
                    .w(px(16.0))
                    .text_color(status_color)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(status),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(0xcbd5e1))
                    .child(path),
            )
            .when(threads > 0, |row| {
                row.child(
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(rgb(0x312e81))
                        .text_xs()
                        .text_color(rgb(0xc7d2fe))
                        .child(format!("{threads}")),
                )
            })
            .when(file.is_binary, |row| {
                row.child(div().text_xs().text_color(rgb(0x64748b)).child("binary"))
            })
            .when(!file.is_binary, |row| {
                row.child(
                    div()
                        .flex()
                        .gap_1()
                        .text_xs()
                        .child(
                            div()
                                .text_color(rgb(0x4ade80))
                                .child(format!("+{additions}")),
                        )
                        .child(
                            div()
                                .text_color(rgb(0xf87171))
                                .child(format!("-{deletions}")),
                        ),
                )
            })
            .when(viewed, |row| {
                row.child(div().text_color(rgb(0x4ade80)).child("✓"))
            })
            .into_any()
    }
}

impl Focusable for ReviewView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.diff_view.read(cx).focus_handle.clone()
    }
}

impl Render for ReviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.session.shared_files();
        let selected = self.session.selected_file_index();
        let viewed = (0..files.len())
            .map(|index| self.session.is_viewed(index))
            .collect::<Vec<_>>();
        let viewed_count = self.session.viewed_count();
        let file_count = files.len();
        let comments = self.session.shared_comments();
        let total_threads = comments.thread_count();
        let (source_label, source_title) = match self.session.source() {
            SessionSource::Demo => (
                SharedString::from("Generated fixture"),
                SharedString::from("Diff virtualization demo"),
            ),
            SessionSource::LocalComparison {
                base_sha, head_sha, ..
            } => (
                SharedString::from("Local comparison"),
                SharedString::from(format!("{}…{}", short_sha(base_sha), short_sha(head_sha))),
            ),
            SessionSource::GitHubPullRequest {
                owner,
                repository,
                number,
                title,
                ..
            } => (
                SharedString::from(format!("{owner}/{repository} · PR #{number}")),
                SharedString::from(title.to_string()),
            ),
        };
        let review_view = cx.entity();

        div()
            .id("review-session")
            .key_context("ReviewSession")
            .on_action(cx.listener(Self::select_next_file))
            .on_action(cx.listener(Self::select_previous_file))
            .on_action(cx.listener(Self::toggle_viewed))
            .size_full()
            .flex()
            .bg(rgb(0x020617))
            .child(
                div()
                    .w(px(290.0))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(rgb(0x1e293b))
                    .bg(rgb(0x0f172a))
                    .child(
                        div()
                            .h(px(112.0))
                            .flex_shrink_0()
                            .px_3()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap_1()
                            .border_b_1()
                            .border_color(rgb(0x1e293b))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x60a5fa))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(source_label),
                            )
                            .child(
                                div()
                                    .text_color(rgb(0xf8fafc))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(source_title),
                            )
                            .child(div().text_xs().text_color(rgb(0x64748b)).child(format!(
                                "{file_count} files · {viewed_count} viewed · {total_threads} conversations"
                            ))),
                    )
                    .child(
                        list(self.file_list_state.clone(), move |index, _, _| {
                            Self::render_file_row(
                                &files[index],
                                index,
                                selected == index,
                                viewed[index],
                                comments.thread_count_for_file(index),
                                &review_view,
                            )
                        })
                        .flex_1(),
                    ),
            )
            .child(div().flex_1().min_w_0().child(self.diff_view.clone()))
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
        let (view, cx) = cx.add_window_view(|window, cx| {
            DiffView::new(file, 0, Arc::new(PlacedComments::default()), window, cx)
        });

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

    /// The composer renders inside `DiffView`, so navigation bindings used to
    /// swallow every `j`, `k`, and `c` typed into a review comment — and `c`
    /// closed the composer mid-sentence.
    #[gpui::test]
    fn comment_editor_receives_keys_that_the_diff_binds(cx: &mut TestAppContext) {
        cx.update(init);
        let file = Arc::new(DiffFile::demo(100));
        let (view, cx) = cx.add_window_view(|window, cx| {
            DiffView::new(file, 0, Arc::new(PlacedComments::default()), window, cx)
        });

        cx.update(|window, app| {
            window.focus(&view.read(app).focus_handle(app));
        });
        cx.dispatch_action(ToggleComment);
        cx.simulate_input("jerky code");

        assert_eq!(
            cx.read(|app| view.read(app).comment_editor.read(app).content.clone()),
            "jerky code",
        );
        // Typing must not have navigated the diff or dismissed the composer.
        assert_eq!(cx.read(|app| view.read(app).selected_line()), 0);
        assert_eq!(cx.read(|app| view.read(app).comment_line), Some(0));
    }

    #[gpui::test]
    fn escape_dismisses_the_comment_editor_and_restores_navigation(cx: &mut TestAppContext) {
        cx.update(init);
        let file = Arc::new(DiffFile::demo(100));
        let (view, cx) = cx.add_window_view(|window, cx| {
            DiffView::new(file, 0, Arc::new(PlacedComments::default()), window, cx)
        });

        cx.update(|window, app| {
            window.focus(&view.read(app).focus_handle(app));
        });
        cx.dispatch_action(ToggleComment);
        cx.simulate_input("k");
        assert_eq!(cx.read(|app| view.read(app).selected_line()), 0);

        cx.simulate_keystrokes("escape");
        assert_eq!(cx.read(|app| view.read(app).comment_line), None);

        // With the composer gone, the same key navigates again.
        cx.simulate_input("j");
        assert_eq!(cx.read(|app| view.read(app).selected_line()), 1);
    }

    #[gpui::test]
    fn switches_files_and_tracks_viewed_state(cx: &mut TestAppContext) {
        cx.update(init);
        let mut first = DiffFile::demo(20);
        first.path = "src/first.rs".into();
        let mut second = DiffFile::demo(30);
        second.path = "src/second.rs".into();
        let session =
            ReviewSession::new(domain::SessionSource::Demo, vec![first, second].into()).unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));

        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });
        cx.dispatch_action(SelectNextFile);
        cx.dispatch_action(ToggleViewed);

        assert_eq!(cx.read(|app| view.read(app).selected_file_index()), 1);
        assert!(cx.read(|app| view.read(app).session.is_viewed(1)));
        assert_eq!(
            cx.read(|app| { view.read(app).diff_view.read(app).file.path.to_string() }),
            "src/second.rs",
        );
    }

    /// Threads add height to a row inside the virtualized list, so rendering a
    /// session that has them must not disturb navigation or the composer.
    #[gpui::test]
    fn renders_existing_threads_inside_the_virtualized_diff(cx: &mut TestAppContext) {
        cx.update(init);
        let mut file = DiffFile::demo(200);
        file.path = "src/review.rs".into();
        let head_sha: Arc<str> = "a".repeat(40).into();
        let source = domain::SessionSource::LocalComparison {
            repository_root: std::path::PathBuf::from("/tmp/repository"),
            base_sha: Arc::clone(&head_sha),
            diff_base_sha: Arc::clone(&head_sha),
            head_sha: Arc::clone(&head_sha),
        };
        let mut session = ReviewSession::new(source, vec![file].into()).unwrap();

        // Row 6 of the demo fixture is an addition on new line 6, plus a reply.
        let anchored = |id: u64, in_reply_to_id: Option<u64>| domain::ReviewComment {
            id,
            author: "reviewer".into(),
            body: "needs a test".into(),
            path: "src/review.rs".into(),
            side: domain::DiffSide::Right,
            line: Some(6),
            start_line: None,
            in_reply_to_id,
            is_file_level: false,
            created_at: "2026-07-25T00:00:00Z".into(),
            url: "https://github.com/acme/widgets/pull/1".into(),
        };
        let mut outdated = anchored(3, None);
        outdated.line = None;
        let placed =
            session.set_review_comments(vec![anchored(1, None), anchored(2, Some(1)), outdated]);
        assert_eq!(placed, 2, "one anchored thread and one outdated");
        assert_eq!(session.comments().threads_at(0, 6).len(), 1);
        assert_eq!(session.comments().unplaced().len(), 1);

        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));
        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });

        // Scroll onto the commented row and open a composer beside the thread.
        for _ in 0..6 {
            cx.dispatch_action(SelectNextLine);
        }
        cx.dispatch_action(ToggleComment);
        cx.simulate_input("also check the edge case");

        assert_eq!(
            cx.read(|app| view.read(app).diff_view.read(app).selected_line()),
            6,
        );
        assert_eq!(
            cx.read(|app| view
                .read(app)
                .diff_view
                .read(app)
                .comment_editor
                .read(app)
                .content
                .clone()),
            "also check the edge case",
        );
    }

    /// Switching files resets the composer, so file navigation must not fire
    /// while a comment is being written either.
    #[gpui::test]
    fn file_navigation_does_not_fire_while_composing(cx: &mut TestAppContext) {
        cx.update(init);
        let mut first = DiffFile::demo(20);
        first.path = "src/first.rs".into();
        let mut second = DiffFile::demo(30);
        second.path = "src/second.rs".into();
        let session =
            ReviewSession::new(domain::SessionSource::Demo, vec![first, second].into()).unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));

        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });
        cx.dispatch_action(ToggleComment);
        cx.simulate_keystrokes("cmd-shift-j");

        assert_eq!(cx.read(|app| view.read(app).selected_file_index()), 0);
        assert_eq!(
            cx.read(|app| view.read(app).diff_view.read(app).comment_line),
            Some(0),
        );

        // Dismissing the composer hands file navigation back.
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("cmd-shift-j");
        assert_eq!(cx.read(|app| view.read(app).selected_file_index()), 1);
    }
}
