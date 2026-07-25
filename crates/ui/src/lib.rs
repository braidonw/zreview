#![allow(clippy::unreadable_literal)]

use std::sync::Arc;

use domain::{
    ChangeCounts, CommentThread, DiffAnchor, DiffFile, DiffLine, DiffLineKind, DraftComment,
    DraftSink, Drafts, FileStatus, LoadStage, LoadedSession, PlacedComments, ReviewSession,
    SessionFailure, SessionSource,
};
use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding,
    KeyDownEvent, ListAlignment, ListState, MouseButton, Render, SharedString, Subscription,
    Window, actions, div, list, prelude::*, px, rgb,
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

/// Emitted when the reviewer changes the composer's text.
///
/// Carried as an event rather than polled so a draft is stored as it is typed,
/// which is what makes the text survive a crash.
pub struct CommentEdited;

/// A deliberately small text-entry control for the virtualization spike.
///
/// It proves that a focused, variable-height child can live inside the list.
/// Production review comments will replace this with a full IME-aware editor.
pub struct CommentEditor {
    content: String,
    focus_handle: FocusHandle,
}

impl EventEmitter<CommentEdited> for CommentEditor {}

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

    /// Loads existing text without reporting it as an edit.
    ///
    /// Opening the composer on a line that already has a draft has to show that
    /// draft, and echoing it back as a change would be a pointless write.
    fn load(&mut self, content: String, cx: &mut Context<Self>) {
        self.content = content;
        cx.notify();
    }

    #[must_use]
    fn content(&self) -> &str {
        &self.content
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
            cx.emit(CommentEdited);
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
    draft: Option<&'a DraftComment>,
    /// Set when a hunk begins at this row, so the header is drawn above it.
    hunk_header: Option<&'a Arc<str>>,
}

/// What the diff view asks the session to do about drafts.
///
/// The diff view renders a read-only snapshot and never mutates the session
/// directly, so the session stays the single owner of draft state.
pub enum DiffViewEvent {
    /// The composer's text for a row changed.
    DraftEdited { row: usize, body: String },
    /// The reviewer discarded a row's draft.
    DraftDiscarded { row: usize },
    /// A stale draft should move onto a row in the current diff.
    DraftReanchored { stale: DiffAnchor, row: usize },
}

pub struct DiffView {
    file: Arc<DiffFile>,
    /// Index of `file` in the session, which is how threads and drafts are keyed.
    file_index: usize,
    comments: Arc<PlacedComments>,
    drafts: Arc<Drafts>,
    list_state: ListState,
    selected_line: usize,
    comment_line: Option<usize>,
    comment_editor: Entity<CommentEditor>,
    /// Held so the composer's edits keep reaching this view.
    _editor_subscription: Subscription,
    focus_handle: FocusHandle,
}

impl EventEmitter<DiffViewEvent> for DiffView {}

impl DiffView {
    #[must_use]
    pub fn new(
        file: Arc<DiffFile>,
        file_index: usize,
        comments: Arc<PlacedComments>,
        drafts: Arc<Drafts>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let item_count = file.line_count();
        let comment_editor = cx.new(CommentEditor::new);
        let subscription = cx.subscribe(&comment_editor, |this, editor, _: &CommentEdited, cx| {
            let Some(row) = this.comment_line else {
                return;
            };
            let body = editor.read(cx).content().to_owned();
            cx.emit(DiffViewEvent::DraftEdited { row, body });
        });

        Self {
            file,
            file_index,
            comments,
            drafts,
            list_state: ListState::new(item_count, ListAlignment::Top, px(ROW_HEIGHT)),
            selected_line: 0,
            comment_line: None,
            comment_editor,
            _editor_subscription: subscription,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Replaces the draft snapshot this view renders.
    pub fn set_drafts(&mut self, drafts: Arc<Drafts>, cx: &mut Context<Self>) {
        self.drafts = drafts;
        cx.notify();
    }

    #[must_use]
    pub const fn selected_line(&self) -> usize {
        self.selected_line
    }

    /// Switches to another file.
    ///
    /// The composer closes, which no longer loses anything: whatever was typed is
    /// already stored as a draft on the session.
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
            self.open_composer(self.selected_line, window, cx);
        }
        self.list_state.scroll_to_reveal_item(self.selected_line);
        cx.notify();
    }

    /// Opens the composer on a row, showing that row's draft if it has one.
    fn open_composer(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.comment_line = Some(row);
        let existing = self
            .drafts
            .at(self.file_index, row)
            .map(|draft| draft.body.clone())
            .unwrap_or_default();
        self.comment_editor
            .update(cx, |editor, cx| editor.load(existing, cx));
        let editor_focus = self.comment_editor.read(cx).focus_handle.clone();
        window.focus(&editor_focus);
    }

    /// Discards the row's draft and closes the composer.
    fn discard_draft(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.comment_line = None;
        self.comment_editor.update(cx, CommentEditor::clear);
        window.focus(&self.focus_handle);
        cx.emit(DiffViewEvent::DraftDiscarded { row });
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
            draft,
            hunk_header,
        } = row;
        // While the composer is open the draft is being edited in it, so showing
        // it read-only underneath as well would duplicate the same text.
        let resting_draft = draft.filter(|_| !show_comment);
        let draft_exists = draft.is_some();
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
        let discard_view = view.clone();

        div()
            .id(("diff-row", index))
            .w_full()
            .flex()
            .flex_col()
            .bg(if selected { rgb(0x1e3a5f) } else { row_bg })
            // Drawn above the first row of its hunk, so every hunk in a file is
            // labelled rather than only the first.
            .children(hunk_header.map(|header| {
                div()
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px_3()
                    .flex()
                    .items_center()
                    .bg(rgb(0x172554))
                    .text_color(rgb(0x93c5fd))
                    .child(SharedString::from(header.to_string()))
            }))
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
                    .when(draft.is_some(), |row| {
                        row.child(
                            div()
                                .mr_2()
                                .px_1()
                                .rounded_sm()
                                .bg(rgb(0x78350f))
                                .text_xs()
                                .text_color(rgb(0xfde68a))
                                .child("draft"),
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
                                        this.open_composer(index, window, cx);
                                        cx.notify();
                                    });
                                })
                                .child(if draft_exists { "Edit" } else { "Comment" }),
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
            // A draft the reviewer is not currently editing, shown so the diff
            // reflects everything they have written.
            .children(resting_draft.map(|draft| {
                div().ml(px(GUTTER_WIDTH * 2.0 + 20.0)).mr_3().py_2().child(
                    div()
                        .p_2()
                        .rounded_md()
                        .border_l_2()
                        .border_color(rgb(0xfbbf24))
                        .bg(rgb(0x1c1917))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .text_xs()
                                .child(
                                    div()
                                        .text_color(rgb(0xfde68a))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("Your draft"),
                                )
                                .when(draft.is_stale, |header| {
                                    header.child(
                                        div().text_color(rgb(0xf87171)).child("needs re-anchoring"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xe5e7eb))
                                .child(SharedString::from(draft.body.clone())),
                        ),
                )
            }))
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
                                .child("Done"),
                        )
                        .child(
                            div()
                                .id(("discard-draft", index))
                                .px_3()
                                .h(px(32.0))
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(0x7f1d1d))
                                .text_color(rgb(0xfca5a5))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    cx.stop_propagation();
                                    discard_view.update(cx, |this, cx| {
                                        this.discard_draft(index, window, cx);
                                    });
                                })
                                .child("Discard"),
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
        let drafts = Arc::clone(&self.drafts);
        let draft_count = self.drafts.for_path(&self.file.path).count();
        // Stale drafts have no row, so the panel is the only place their text can
        // appear — and the only place the reviewer can act on it.
        let stale_drafts = self
            .drafts
            .stale()
            .filter(|draft| draft.anchor.path == self.file.path)
            .cloned()
            .collect::<Vec<_>>();
        let selected_line = self.selected_line;
        let comment_line = self.comment_line;
        let view = cx.entity();
        // The list closure takes `view`; the side panel needs its own handle.
        let panel_view = view.clone();
        let comment_editor = self.comment_editor.clone();
        let path = SharedString::from(file.path.to_string());
        let line_count = file.line_count();
        let thread_count = comments.thread_count_for_file(file_index);
        let unplaced = comments
            .unplaced_for_file(file_index)
            .cloned()
            .collect::<Vec<_>>();
        // Tracks the selection rather than being pinned to the first hunk, so it
        // says where the reviewer actually is.
        let current_hunk = file.hunk_at(selected_line).map_or_else(
            || SharedString::from("—"),
            |hunk| SharedString::from(hunk.header.to_string()),
        );
        let hunk_count = file.hunks.len();
        let empty_reason = file.empty_reason();

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
                    .child(div().text_color(rgb(0x94a3b8)).child(format!(
                        "{line_count} lines · {hunk_count} hunk{}",
                        if hunk_count == 1 { "" } else { "s" }
                    ))),
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
                    .child(current_hunk),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    // A file with no rows explains itself instead of rendering an
                    // empty pane that is indistinguishable from a bug.
                    .children(empty_reason.map(|reason| {
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(rgb(0xf8fafc))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(reason.label()),
                            )
                            .child(
                                div()
                                    .max_w(px(420.0))
                                    .text_xs()
                                    .text_color(rgb(0x64748b))
                                    .child(reason.detail()),
                            )
                    }))
                    .when(empty_reason.is_none(), |pane| {
                        pane.child(
                            list(self.list_state.clone(), move |index, _, _| {
                                Self::render_diff_line(
                                    &DiffRow {
                                        line: &file.lines[index],
                                        index,
                                        selected: selected_line == index,
                                        show_comment: comment_line == Some(index),
                                        threads: comments.threads_at(file_index, index),
                                        draft: drafts.at(file_index, index),
                                        hunk_header: file.hunk_header_at(index),
                                    },
                                    &view,
                                    &comment_editor,
                                )
                            })
                            .flex_1(),
                        )
                    })
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
                            .when(draft_count > 0, |panel| {
                                panel.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0xfde68a))
                                        .child(format!("{draft_count} of your drafts")),
                                )
                            })
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
                            // A stale draft is text the reviewer wrote that
                            // currently cannot be submitted. It is shown here with
                            // the one action that fixes that.
                            .when(!stale_drafts.is_empty(), |panel| {
                                panel
                                    .child(div().mt_2().text_xs().text_color(rgb(0xf87171)).child(
                                        format!(
                                            "{} draft{} need re-anchoring",
                                            stale_drafts.len(),
                                            if stale_drafts.len() == 1 { "" } else { "s" },
                                        ),
                                    ))
                                    .children(stale_drafts.iter().map(|draft| {
                                        let move_view = panel_view.clone();
                                        let stale = draft.anchor.clone();
                                        div()
                                            .p_2()
                                            .rounded_md()
                                            .border_l_2()
                                            .border_color(rgb(0xf87171))
                                            .bg(rgb(0x1c1917))
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(div().text_xs().text_color(rgb(0x94a3b8)).child(
                                                format!(
                                                    "was {} line {}",
                                                    draft.anchor.side, draft.anchor.line,
                                                ),
                                            ))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0xe5e7eb))
                                                    .child(SharedString::from(draft.body.clone())),
                                            )
                                            .child(
                                                div()
                                                    .id(("reanchor", draft.anchor.line))
                                                    .mt_1()
                                                    .px_2()
                                                    .py_1()
                                                    .rounded_sm()
                                                    .bg(rgb(0x2563eb))
                                                    .text_xs()
                                                    .text_color(rgb(0xffffff))
                                                    .cursor_pointer()
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_, _window, cx| {
                                                            cx.stop_propagation();
                                                            let stale = stale.clone();
                                                            move_view.update(cx, |this, cx| {
                                                                cx.emit(
                                                                    DiffViewEvent::DraftReanchored {
                                                                        stale,
                                                                        row: this.selected_line,
                                                                    },
                                                                );
                                                            });
                                                        },
                                                    )
                                                    .child(format!(
                                                        "Move to row {}",
                                                        selected_line + 1
                                                    )),
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

/// Emitted when the session's drafts change, so whoever owns persistence can
/// write them without this view knowing about a database.
pub struct DraftsChanged {
    /// The draft that changed, or `None` when one was discarded.
    pub draft: Option<DraftComment>,
    /// The anchor that was affected, whether written or discarded.
    pub anchor: DiffAnchor,
}

pub struct ReviewView {
    session: ReviewSession,
    diff_view: Entity<DiffView>,
    file_list_state: ListState,
    /// Held so draft edits keep reaching the session.
    _diff_subscription: Subscription,
}

impl EventEmitter<DraftsChanged> for ReviewView {}

impl ReviewView {
    #[must_use]
    pub fn new(session: ReviewSession, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selected_file = Arc::new(session.selected_file().clone());
        let selected_index = session.selected_file_index();
        let comments = session.shared_comments();
        let drafts = session.shared_drafts();
        let file_count = session.files().len();
        let diff_view =
            cx.new(|cx| DiffView::new(selected_file, selected_index, comments, drafts, window, cx));
        let subscription = cx.subscribe(&diff_view, Self::on_diff_event);

        Self {
            session,
            diff_view,
            file_list_state: ListState::new(file_count, ListAlignment::Top, px(36.0)),
            _diff_subscription: subscription,
        }
    }

    /// Applies a draft change to the session, then republishes it for rendering
    /// and for persistence.
    fn on_diff_event(
        &mut self,
        _diff_view: Entity<DiffView>,
        event: &DiffViewEvent,
        cx: &mut Context<Self>,
    ) {
        let file = self.session.selected_file_index();

        // Moving a draft touches two positions, so it reports both: the old one is
        // now empty and the new one holds the text. Persistence needs both or the
        // draft would come back twice.
        if let DiffViewEvent::DraftReanchored { stale, row } = event {
            if let Some(moved) = self.session.reanchor_draft(stale, file, *row) {
                let drafts = self.session.shared_drafts();
                let draft = drafts.get(&moved.anchored).cloned();
                self.diff_view
                    .update(cx, |view, cx| view.set_drafts(drafts, cx));
                cx.emit(DraftsChanged {
                    draft: None,
                    anchor: moved.vacated,
                });
                cx.emit(DraftsChanged {
                    draft,
                    anchor: moved.anchored,
                });
                cx.notify();
            }
            return;
        }

        let row = match event {
            DiffViewEvent::DraftEdited { row, .. } | DiffViewEvent::DraftDiscarded { row } => *row,
            DiffViewEvent::DraftReanchored { .. } => return,
        };
        // The anchor is read before the change so a discarded draft still reports
        // which position it was removed from.
        let Some(anchor) = self.session.anchor_for(file, row) else {
            return;
        };

        match event {
            DiffViewEvent::DraftEdited { body, .. } => {
                self.session.set_draft(file, row, body.clone());
            }
            DiffViewEvent::DraftDiscarded { .. } | DiffViewEvent::DraftReanchored { .. } => {
                self.session.clear_draft(file, row);
            }
        }

        let drafts = self.session.shared_drafts();
        let draft = drafts.get(&anchor).cloned();
        self.diff_view
            .update(cx, |view, cx| view.set_drafts(drafts, cx));
        cx.emit(DraftsChanged { draft, anchor });
        cx.notify();
    }

    #[must_use]
    pub const fn session(&self) -> &ReviewSession {
        &self.session
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

    /// The sidebar header: what is under review, and its overall counts.
    fn render_source_header(&self) -> gpui::Div {
        let (label, title) = match self.session.source() {
            SessionSource::Demo => (
                SharedString::from("Generated fixture"),
                SharedString::from("Diff virtualization demo"),
            ),
            SessionSource::LocalComparison {
                base_sha, head_sha, ..
            } => (
                SharedString::from("Local comparison"),
                // `…` is the merge-base notation this comparison actually uses.
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
        let file_count = self.session.files().len();
        let viewed_count = self.session.viewed_count();
        let thread_count = self.session.comments().thread_count();

        div()
            .flex_shrink_0()
            .px_3()
            .py_3()
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
                    .child(label),
            )
            .child(
                div()
                    .text_color(rgb(0xf8fafc))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(title),
            )
            .child(div().text_xs().text_color(rgb(0x64748b)).child(format!(
                "{file_count} files · {viewed_count} viewed · {thread_count} conversations"
            )))
            // Conversations that would not load, or drafts that are not being
            // saved, must be visible: a session that quietly lacks either looks
            // exactly like one that has nothing to show.
            .children(self.session.warnings().iter().map(|warning| {
                div()
                    .text_xs()
                    .text_color(rgb(0xfbbf24))
                    .child(SharedString::from(warning.summary.clone()))
            }))
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
        // Counted when the file was built, not per frame.
        let ChangeCounts {
            additions,
            deletions,
        } = file.counts;
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
        let comments = self.session.shared_comments();
        let header = self.render_source_header();
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
                    .child(header)
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

/// The root view, and the session state machine PLAN section 9 calls for.
///
/// The window opens on this in [`SessionState::Loading`] before any Git or
/// GitHub work starts, so a slow or failing load is something the reviewer
/// watches rather than a terminal they may not be looking at.
enum SessionState {
    Loading {
        /// What is being opened, known from the request alone.
        description: SharedString,
        /// The stage the loader last reported.
        stage: SharedString,
    },
    Ready {
        review: Entity<ReviewView>,
        /// Held so draft changes keep reaching storage.
        _drafts_subscription: Subscription,
    },
    Failed(SessionFailure),
}

pub struct SessionView {
    state: SessionState,
    /// Where draft changes are written, once the session is ready.
    draft_sink: Option<Box<dyn DraftSink>>,
    focus_handle: FocusHandle,
}

impl SessionView {
    /// Opens on the loading screen for a request that has not started yet.
    #[must_use]
    pub fn loading(description: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            state: SessionState::Loading {
                description: description.into(),
                stage: SharedString::from(LoadStage::default().label()),
            },
            draft_sink: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Records the stage the loader has reached.
    ///
    /// Ignored once the session has finished, so a late report cannot move a
    /// ready or failed view back to loading.
    pub fn set_stage(&mut self, label: impl Into<SharedString>, cx: &mut Context<Self>) {
        if let SessionState::Loading { stage, .. } = &mut self.state {
            let label = label.into();
            if *stage != label {
                *stage = label;
                cx.notify();
            }
        }
    }

    /// Moves to the loaded session, or to the failure that stopped it.
    pub fn finish(
        &mut self,
        result: Result<LoadedSession, SessionFailure>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state = match result {
            Ok(loaded) => {
                self.draft_sink = loaded.draft_sink;
                let review = cx.new(|cx| ReviewView::new(loaded.session, window, cx));
                let subscription = cx.subscribe(&review, Self::on_drafts_changed);
                let focus_handle = review.read(cx).diff_view.read(cx).focus_handle.clone();
                window.focus(&focus_handle);
                SessionState::Ready {
                    review,
                    _drafts_subscription: subscription,
                }
            }
            Err(failure) => {
                window.focus(&self.focus_handle);
                SessionState::Failed(failure)
            }
        };
        cx.notify();
    }

    /// Writes a draft change through to storage.
    ///
    /// The sink is asked to save on every keystroke, which is what makes the text
    /// survive a crash; it is required not to block, so this stays cheap.
    fn on_drafts_changed(
        &mut self,
        _review: Entity<ReviewView>,
        event: &DraftsChanged,
        cx: &mut Context<Self>,
    ) {
        if let Some(sink) = &self.draft_sink {
            match &event.draft {
                Some(draft) => sink.save(&event.anchor, &draft.body),
                None => sink.discard(&event.anchor),
            }
        }
        // Renders the alarm if writing has started failing.
        cx.notify();
    }

    /// The loaded review, once there is one.
    #[cfg(test)]
    fn review(&self) -> Option<&Entity<ReviewView>> {
        match &self.state {
            SessionState::Ready { review, .. } => Some(review),
            _ => None,
        }
    }

    /// The reason drafts are not reaching storage, if they are not.
    #[must_use]
    pub fn draft_write_failure(&self) -> Option<String> {
        self.draft_sink.as_ref().and_then(|sink| sink.failure())
    }

    #[must_use]
    pub fn is_loading(&self) -> bool {
        matches!(self.state, SessionState::Loading { .. })
    }

    #[must_use]
    pub fn failure(&self) -> Option<&SessionFailure> {
        match &self.state {
            SessionState::Failed(failure) => Some(failure),
            _ => None,
        }
    }

    fn render_centered(children: Vec<gpui::AnyElement>) -> gpui::Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(rgb(0x020617))
            .font_family("SF Mono")
            .text_size(px(13.0))
            .children(children)
    }
}

impl Focusable for SessionView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.state {
            // Hand focus to the diff so its keybindings work immediately.
            SessionState::Ready { review, .. } => {
                review.read(cx).diff_view.read(cx).focus_handle.clone()
            }
            _ => self.focus_handle.clone(),
        }
    }
}

impl Render for SessionView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match &self.state {
            SessionState::Loading { description, stage } => Self::render_centered(vec![
                div()
                    .text_color(rgb(0xf8fafc))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("Opening {description}"))
                    .into_any(),
                div()
                    .text_color(rgb(0x93c5fd))
                    .child(format!("{stage}…"))
                    .into_any(),
            ])
            .into_any(),

            SessionState::Failed(failure) => Self::render_centered(vec![
                div()
                    .max_w(px(680.0))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_color(rgb(0xf87171))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(SharedString::from(failure.summary.clone())),
                    )
                    // Remediation before detail: the reviewer wants the next
                    // action more than the underlying message.
                    .children(failure.remediation.as_ref().map(|remediation| {
                        div()
                            .p_3()
                            .rounded_md()
                            .border_l_2()
                            .border_color(rgb(0xfbbf24))
                            .bg(rgb(0x131c31))
                            .text_color(rgb(0xfde68a))
                            .child(SharedString::from(remediation.clone()))
                    }))
                    .children(failure.detail.as_ref().map(|detail| {
                        div()
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .child(SharedString::from(detail.clone()))
                    }))
                    .into_any(),
            ])
            .into_any(),

            // A banner rather than a line in the sidebar: writes failing means
            // the reviewer's work is being lost as they type, which outranks
            // anything else on screen.
            SessionState::Ready { review, .. } => div()
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(0x020617))
                .children(self.draft_write_failure().map(|failure| {
                    div()
                        .flex_shrink_0()
                        .px_4()
                        .py_2()
                        .bg(rgb(0x7f1d1d))
                        .text_color(rgb(0xfee2e2))
                        .font_family("SF Mono")
                        .text_size(px(13.0))
                        .child(format!("Drafts are not being saved: {failure}"))
                }))
                .child(div().flex_1().min_h_0().child(review.clone()))
                .into_any(),
        }
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
            DiffView::new(
                file,
                0,
                Arc::new(PlacedComments::default()),
                Arc::new(Drafts::default()),
                window,
                cx,
            )
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
            DiffView::new(
                file,
                0,
                Arc::new(PlacedComments::default()),
                Arc::new(Drafts::default()),
                window,
                cx,
            )
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
            DiffView::new(
                file,
                0,
                Arc::new(PlacedComments::default()),
                Arc::new(Drafts::default()),
                window,
                cx,
            )
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

    fn repository_backed_session(paths: &[&str]) -> ReviewSession {
        let head_sha: Arc<str> = "a".repeat(40).into();
        let files = paths
            .iter()
            .map(|path| {
                let mut file = DiffFile::demo(40);
                file.path = (*path).into();
                file
            })
            .collect::<Vec<_>>();
        ReviewSession::new(
            domain::SessionSource::LocalComparison {
                repository_root: std::path::PathBuf::from("/tmp/repository"),
                base_sha: Arc::clone(&head_sha),
                diff_base_sha: Arc::clone(&head_sha),
                head_sha,
            },
            files.into(),
        )
        .unwrap()
    }

    /// The window opens before loading starts, so the view has to begin in a
    /// state that has no session at all.
    #[gpui::test]
    fn a_session_view_starts_loading_then_becomes_ready(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) =
            cx.add_window_view(|_window, cx| SessionView::loading("pull request #42", cx));

        assert!(cx.read(|app| view.read(app).is_loading()));
        assert!(cx.read(|app| view.read(app).failure().is_none()));

        cx.update(|_window, app| {
            view.update(app, |view, cx| {
                view.set_stage(LoadStage::BuildingDiff.label(), cx);
            });
        });
        assert!(cx.read(|app| view.read(app).is_loading()));

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.finish(
                    Ok(LoadedSession::unsaved(repository_backed_session(&[
                        "src/review.rs",
                    ]))),
                    window,
                    cx,
                );
            });
        });

        assert!(!cx.read(|app| view.read(app).is_loading()));
        assert!(cx.read(|app| view.read(app).failure().is_none()));
        // The diff takes focus, so its keybindings work without a click.
        cx.dispatch_action(SelectNextLine);
    }

    /// Records what a session view asks storage to do.
    #[derive(Clone, Default)]
    struct RecordingSink {
        calls: Arc<std::sync::Mutex<Vec<String>>>,
        failure: Option<String>,
    }

    impl RecordingSink {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl domain::DraftSink for RecordingSink {
        fn save(&self, anchor: &DiffAnchor, body: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("save {} {} {body}", anchor.path, anchor.line));
        }

        fn discard(&self, anchor: &DiffAnchor) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("discard {} {}", anchor.path, anchor.line));
        }

        fn failure(&self) -> Option<String> {
            self.failure.clone()
        }
    }

    fn review_of(view: &Entity<SessionView>, app: &mut App) -> Entity<ReviewView> {
        view.read(app)
            .review()
            .expect("the session should be ready")
            .clone()
    }

    fn ready_session_view(
        cx: &mut TestAppContext,
        sink: Option<Box<dyn domain::DraftSink>>,
    ) -> (Entity<SessionView>, &mut gpui::VisualTestContext) {
        let (view, cx) = cx.add_window_view(|_window, cx| SessionView::loading("a review", cx));
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.finish(
                    Ok(LoadedSession {
                        session: repository_backed_session(&["src/review.rs"]),
                        draft_sink: sink,
                    }),
                    window,
                    cx,
                );
            });
        });
        (view, cx)
    }

    /// The last link in the chain: a keystroke reaching storage.
    #[gpui::test]
    fn draft_changes_reach_the_sink(cx: &mut TestAppContext) {
        cx.update(init);
        let sink = RecordingSink::default();
        let (_view, cx) = ready_session_view(cx, Some(Box::new(sink.clone())));

        cx.dispatch_action(ToggleComment);
        cx.simulate_input("hi");

        // One write per keystroke, each with the text so far.
        assert_eq!(
            sink.calls(),
            [
                "save src/review.rs 1 h".to_owned(),
                "save src/review.rs 1 hi".to_owned(),
            ],
        );
    }

    #[gpui::test]
    fn discarding_a_draft_reaches_the_sink(cx: &mut TestAppContext) {
        cx.update(init);
        let sink = RecordingSink::default();
        let (_view, cx) = ready_session_view(cx, Some(Box::new(sink.clone())));

        cx.dispatch_action(ToggleComment);
        cx.simulate_input("x");
        cx.simulate_keystrokes("backspace");

        // An emptied draft is a removal, not a blank comment.
        assert_eq!(
            sink.calls().last().map(String::as_str),
            Some("discard src/review.rs 1"),
        );
    }

    /// Writes failing means work is being lost as it is typed, so the view has to
    /// be able to say so.
    #[gpui::test]
    fn a_failing_sink_is_reported(cx: &mut TestAppContext) {
        cx.update(init);
        let sink = RecordingSink {
            failure: Some("disk is full".to_owned()),
            ..RecordingSink::default()
        };
        let (view, cx) = ready_session_view(cx, Some(Box::new(sink)));

        assert_eq!(
            cx.read(|app| view.read(app).draft_write_failure()),
            Some("disk is full".to_owned()),
        );
    }

    #[gpui::test]
    fn a_session_without_a_sink_still_accepts_drafts(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = ready_session_view(cx, None);

        cx.dispatch_action(ToggleComment);
        cx.simulate_input("in memory only");

        assert_eq!(cx.read(|app| view.read(app).draft_write_failure()), None);
    }

    #[gpui::test]
    fn a_failed_load_shows_its_remediation_instead_of_a_session(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) =
            cx.add_window_view(|_window, cx| SessionView::loading("pull request #42", cx));

        let failure = SessionFailure::from_error(
            "GitHub is not authenticated",
            &std::io::Error::other("The token in GH_TOKEN is invalid."),
        )
        .with_remediation("Run `gh auth login`, then reopen the pull request.");
        cx.update(|window, app| {
            view.update(app, |view, cx| view.finish(Err(failure), window, cx));
        });

        assert!(!cx.read(|app| view.read(app).is_loading()));
        let shown = cx.read(|app| view.read(app).failure().cloned()).unwrap();
        assert_eq!(shown.summary, "GitHub is not authenticated");
        assert!(shown.remediation.unwrap().contains("gh auth login"));
        assert!(shown.detail.unwrap().contains("GH_TOKEN"));
    }

    /// A stage report can arrive after the load finished; it must not drag a
    /// ready or failed view back to loading.
    #[gpui::test]
    fn a_late_stage_report_cannot_reopen_a_finished_session(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) =
            cx.add_window_view(|_window, cx| SessionView::loading("pull request #42", cx));

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.finish(Err(SessionFailure::new("Could not load")), window, cx);
            });
        });
        cx.update(|_window, app| {
            view.update(app, |view, cx| {
                view.set_stage(LoadStage::BuildingDiff.label(), cx);
            });
        });

        assert!(!cx.read(|app| view.read(app).is_loading()));
        assert!(cx.read(|app| view.read(app).failure().is_some()));
    }

    #[gpui::test]
    fn a_conversation_load_failure_is_visible_on_a_ready_session(cx: &mut TestAppContext) {
        cx.update(init);
        let mut session = repository_backed_session(&["src/review.rs"]);
        session.push_warning(SessionFailure::new("GitHub's rate limit is exhausted"));

        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));

        // Rendered from the session, so the sidebar cannot silently show zero
        // conversations as though there were none.
        assert_eq!(
            cx.read(|app| view
                .read(app)
                .session
                .warnings()
                .first()
                .map(|warning| warning.summary.clone())),
            Some("GitHub's rate limit is exhausted".to_owned()),
        );
    }

    /// The point of the whole path: what is typed becomes a draft anchored to the
    /// row, without the reviewer doing anything to save it.
    #[gpui::test]
    fn typing_in_the_composer_stores_an_anchored_draft(cx: &mut TestAppContext) {
        cx.update(init);
        let session = repository_backed_session(&["src/review.rs"]);
        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));
        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });

        // Row 6 of the fixture is an addition, so it anchors on the right.
        for _ in 0..6 {
            cx.dispatch_action(SelectNextLine);
        }
        cx.dispatch_action(ToggleComment);
        cx.simulate_input("needs a test");

        let draft = cx
            .read(|app| view.read(app).session().draft_at(0, 6).cloned())
            .expect("typing should have stored a draft");
        assert_eq!(draft.body, "needs a test");
        assert_eq!(draft.anchor.side, domain::DiffSide::Right);
        assert_eq!(draft.anchor.path.as_ref(), "src/review.rs");
        assert!(!draft.is_stale);
        assert_eq!(cx.read(|app| view.read(app).session().drafts().len()), 1);
    }

    /// Closing the composer and switching files used to lose the text. It is now
    /// stored on the session, so it survives both.
    #[gpui::test]
    fn a_draft_survives_closing_the_composer_and_changing_files(cx: &mut TestAppContext) {
        cx.update(init);
        let session = repository_backed_session(&["src/first.rs", "src/second.rs"]);
        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));
        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });

        cx.dispatch_action(ToggleComment);
        cx.simulate_input("a thought");
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("cmd-shift-j");
        cx.simulate_keystrokes("cmd-shift-k");

        assert_eq!(
            cx.read(|app| view
                .read(app)
                .session()
                .draft_at(0, 0)
                .map(|d| d.body.clone())),
            Some("a thought".to_owned()),
        );
    }

    /// Reopening a line has to show what is already there, or the reviewer would
    /// silently start a second comment over the top of the first.
    #[gpui::test]
    fn reopening_the_composer_loads_the_existing_draft(cx: &mut TestAppContext) {
        cx.update(init);
        let session = repository_backed_session(&["src/review.rs"]);
        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));
        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });

        cx.dispatch_action(ToggleComment);
        cx.simulate_input("first half");
        cx.simulate_keystrokes("escape");
        cx.dispatch_action(ToggleComment);

        assert_eq!(
            cx.read(|app| view
                .read(app)
                .diff_view
                .read(app)
                .comment_editor
                .read(app)
                .content()
                .to_owned()),
            "first half",
        );

        // Appending continues the same draft rather than starting another.
        cx.simulate_input(" and second");
        assert_eq!(
            cx.read(|app| view
                .read(app)
                .session()
                .draft_at(0, 0)
                .map(|d| d.body.clone())),
            Some("first half and second".to_owned()),
        );
        assert_eq!(cx.read(|app| view.read(app).session().drafts().len()), 1);
    }

    #[gpui::test]
    fn emptying_the_composer_removes_the_draft(cx: &mut TestAppContext) {
        cx.update(init);
        let session = repository_backed_session(&["src/review.rs"]);
        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));
        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });

        cx.dispatch_action(ToggleComment);
        cx.simulate_input("oops");
        assert_eq!(cx.read(|app| view.read(app).session().drafts().len()), 1);

        for _ in 0..4 {
            cx.simulate_keystrokes("backspace");
        }

        assert!(cx.read(|app| view.read(app).session().drafts().is_empty()));
    }

    /// Moving a stale draft has to reach storage as a removal *and* a write, or
    /// reopening would show the draft in both places.
    #[gpui::test]
    fn re_anchoring_a_stale_draft_reaches_the_sink_from_both_sides(cx: &mut TestAppContext) {
        cx.update(init);
        let sink = RecordingSink::default();
        let mut session = repository_backed_session(&["src/review.rs"]);
        let stale = DiffAnchor {
            path: "src/review.rs".into(),
            side: domain::DiffSide::Right,
            line: 9_999,
            head_sha: "a".repeat(40).into(),
        };
        session.restore_drafts([(stale.clone(), "written last week".to_owned())]);
        assert_eq!(session.drafts().stale_count(), 1);

        let (view, cx) = cx.add_window_view(|_window, cx| SessionView::loading("a review", cx));
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.finish(
                    Ok(LoadedSession {
                        session,
                        draft_sink: Some(Box::new(sink.clone())),
                    }),
                    window,
                    cx,
                );
            });
        });

        // Select a commentable row, then move the stale draft onto it.
        for _ in 0..6 {
            cx.dispatch_action(SelectNextLine);
        }
        cx.update(|_window, app| {
            let review = review_of(&view, app);
            review.update(app, |review, cx| {
                let diff = review.diff_view.clone();
                diff.update(cx, |_diff, cx| {
                    cx.emit(DiffViewEvent::DraftReanchored {
                        stale: stale.clone(),
                        row: 6,
                    });
                });
            });
        });

        cx.update(|_window, app| {
            let session = review_of(&view, app).read(app).session();
            assert_eq!(session.drafts().stale_count(), 0, "no longer stale");
            assert_eq!(
                session.draft_at(0, 6).map(|draft| draft.body.clone()),
                Some("written last week".to_owned()),
            );
            assert_eq!(session.drafts().len(), 1, "moved, not duplicated");
        });

        assert_eq!(
            sink.calls(),
            [
                // The position it left, then the one it now occupies.
                "discard src/review.rs 9999".to_owned(),
                "save src/review.rs 6 written last week".to_owned(),
            ],
        );
    }

    /// A file with nothing to render must not put the view in a state where
    /// navigation misbehaves.
    #[gpui::test]
    fn a_file_with_no_rows_renders_without_breaking_navigation(cx: &mut TestAppContext) {
        cx.update(init);
        let binary = Arc::new(DiffFile {
            path: "image.bin".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: true,
            hunks: Arc::from([]),
            counts: ChangeCounts::default(),
            lines: Arc::from([]),
        });
        assert_eq!(binary.empty_reason(), Some(domain::EmptyDiffReason::Binary));

        let (view, cx) = cx.add_window_view(|window, cx| {
            DiffView::new(
                binary,
                0,
                Arc::new(PlacedComments::default()),
                Arc::new(Drafts::default()),
                window,
                cx,
            )
        });
        cx.update(|window, app| {
            window.focus(&view.read(app).focus_handle(app));
        });

        // Navigating an empty file stays put rather than panicking on an index.
        cx.dispatch_action(SelectNextLine);
        cx.dispatch_action(SelectPreviousLine);
        assert_eq!(cx.read(|app| view.read(app).selected_line()), 0);

        // And there is no row to comment on.
        cx.dispatch_action(ToggleComment);
        assert_eq!(cx.read(|app| view.read(app).comment_line), Some(0));
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
