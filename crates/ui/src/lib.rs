#![allow(clippy::unreadable_literal)]

mod findings;
mod text;
pub mod theme;

pub use text::TextBuffer;

use std::sync::{Arc, Mutex};

use app::{
    FindingDisposition, PendingSend, ReviewModel, SessionModel, SessionPhase, SubmissionState, lock,
};
use domain::{
    ChangeCounts, CommentThread, DiffAnchor, DiffFile, DiffLine, DiffLineKind, DraftComment,
    Drafts, ExcludedDraft, FileStatus, FindingId, LoadedSession, PlacedComments, ReviewEvent,
    ReviewSession, ReviewSubmission, SessionFailure, SessionSource,
};
use gpui::{
    App, ClipboardItem, Context, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, KeyBinding, ListAlignment, ListState, MouseButton, Render,
    SharedString, Subscription, Window, actions, div, list, prelude::*, px, rgb,
};

actions!(
    diff_view,
    [
        SelectNextLine,
        SelectPreviousLine,
        ExtendSelectionDown,
        ExtendSelectionUp,
        ToggleComment,
        CloseComment,
        CopySelectedLine,
    ]
);
actions!(
    review_session,
    [SelectNextFile, SelectPreviousFile, ToggleViewed]
);
actions!(
    review_findings,
    [RunReview, AcceptFinding, DismissFinding, SelectNextFinding]
);
actions!(
    comment_editor,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveLineStart,
        MoveLineEnd,
        SelectCharLeft,
        SelectCharRight,
        SelectLineUp,
        SelectLineDown,
        SelectAllText,
        DeleteBackward,
        DeleteForward,
        InsertNewline,
        CopySelection,
        CutSelection,
        PasteText,
    ]
);

use theme::ROW_HEIGHT;
const COMMENT_HEIGHT: f32 = 104.0;
use theme::GUTTER_WIDTH;

fn short_sha(value: &str) -> &str {
    value.get(..7).unwrap_or(value)
}

/// The review a loaded session is showing.
///
/// A [`ReviewView`] is built when the session becomes ready and dropped if it ever
/// stops being ready, so the phase cannot have moved on underneath one.
fn loaded_review(model: &SessionModel) -> &ReviewModel {
    model
        .review()
        .expect("a review view outlives its session's ready phase")
}

/// `CommentEditor` renders inside `DiffView`, so a bare `DiffView` predicate also
/// matches while the composer is focused. That would make `j`, `k`, and `c`
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
        KeyBinding::new("shift-j", ExtendSelectionDown, Some(DIFF_CONTEXT)),
        KeyBinding::new("shift-down", ExtendSelectionDown, Some(DIFF_CONTEXT)),
        KeyBinding::new("shift-k", ExtendSelectionUp, Some(DIFF_CONTEXT)),
        KeyBinding::new("shift-up", ExtendSelectionUp, Some(DIFF_CONTEXT)),
        KeyBinding::new("c", ToggleComment, Some(DIFF_CONTEXT)),
        KeyBinding::new("cmd-c", CopySelectedLine, Some(DIFF_CONTEXT)),
        KeyBinding::new("cmd-shift-j", SelectNextFile, Some(SESSION_CONTEXT)),
        KeyBinding::new("cmd-shift-k", SelectPreviousFile, Some(SESSION_CONTEXT)),
        KeyBinding::new("cmd-shift-v", ToggleViewed, Some(SESSION_CONTEXT)),
        KeyBinding::new("cmd-shift-r", RunReview, Some(SESSION_CONTEXT)),
        KeyBinding::new("cmd-shift-f", SelectNextFinding, Some(SESSION_CONTEXT)),
        KeyBinding::new("cmd-shift-y", AcceptFinding, Some(SESSION_CONTEXT)),
        KeyBinding::new("cmd-shift-d", DismissFinding, Some(SESSION_CONTEXT)),
        KeyBinding::new("escape", CloseComment, Some("CommentEditor")),
        // The composer is a real text field, so it needs the bindings a reviewer
        // will reach for without thinking.
        KeyBinding::new("left", MoveLeft, Some("CommentEditor")),
        KeyBinding::new("right", MoveRight, Some("CommentEditor")),
        KeyBinding::new("up", MoveUp, Some("CommentEditor")),
        KeyBinding::new("down", MoveDown, Some("CommentEditor")),
        KeyBinding::new("home", MoveLineStart, Some("CommentEditor")),
        KeyBinding::new("end", MoveLineEnd, Some("CommentEditor")),
        KeyBinding::new("cmd-left", MoveLineStart, Some("CommentEditor")),
        KeyBinding::new("cmd-right", MoveLineEnd, Some("CommentEditor")),
        KeyBinding::new("shift-left", SelectCharLeft, Some("CommentEditor")),
        KeyBinding::new("shift-right", SelectCharRight, Some("CommentEditor")),
        KeyBinding::new("shift-up", SelectLineUp, Some("CommentEditor")),
        KeyBinding::new("shift-down", SelectLineDown, Some("CommentEditor")),
        KeyBinding::new("cmd-a", SelectAllText, Some("CommentEditor")),
        KeyBinding::new("backspace", DeleteBackward, Some("CommentEditor")),
        KeyBinding::new("delete", DeleteForward, Some("CommentEditor")),
        KeyBinding::new("enter", InsertNewline, Some("CommentEditor")),
        KeyBinding::new("cmd-c", CopySelection, Some("CommentEditor")),
        KeyBinding::new("cmd-x", CutSelection, Some("CommentEditor")),
        KeyBinding::new("cmd-v", PasteText, Some("CommentEditor")),
    ]);
}

/// Emitted when the reviewer changes the composer's text.
///
/// Carried as an event rather than polled so a draft is stored as it is typed,
/// which is what makes the text survive a crash.
pub(crate) struct CommentEdited;

/// A multi-line text field for writing a review comment.
///
/// Editing rules live in [`TextBuffer`] so they can be tested without a window;
/// this type is key dispatch, drawing, and the platform input-method bridge.
pub struct CommentEditor {
    text: TextBuffer,
    focus_handle: FocusHandle,
}

impl EventEmitter<CommentEdited> for CommentEditor {}

impl CommentEditor {
    fn new(cx: &mut Context<Self>) -> Self {
        Self::with_content(String::new(), cx)
    }

    fn with_content(content: String, cx: &mut Context<Self>) -> Self {
        Self {
            text: TextBuffer::new(content),
            focus_handle: cx.focus_handle(),
        }
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.load(String::new(), cx);
    }

    /// Loads existing text without reporting it as an edit.
    ///
    /// Opening the composer on a line that already has a draft has to show that
    /// draft, and echoing it back as a change would be a pointless write.
    fn load(&mut self, content: String, cx: &mut Context<Self>) {
        self.text.set_content(content);
        cx.notify();
    }

    #[must_use]
    fn content(&self) -> &str {
        self.text.content()
    }

    /// Reports an edit and redraws. Moving the caret changes no text, so it only
    /// redraws and does not come through here.
    fn edited(cx: &mut Context<Self>) {
        cx.emit(CommentEdited);
        cx.notify();
    }

    fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.text.move_left();
        cx.notify();
    }

    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.text.move_right();
        cx.notify();
    }

    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.text.move_up();
        cx.notify();
    }

    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.text.move_down();
        cx.notify();
    }

    fn move_line_start(&mut self, _: &MoveLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.text.move_line_start();
        cx.notify();
    }

    fn move_line_end(&mut self, _: &MoveLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.text.move_line_end();
        cx.notify();
    }

    fn select_char_left(&mut self, _: &SelectCharLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.text.select_left();
        cx.notify();
    }

    fn select_char_right(&mut self, _: &SelectCharRight, _: &mut Window, cx: &mut Context<Self>) {
        self.text.select_right();
        cx.notify();
    }

    fn select_line_up(&mut self, _: &SelectLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.text.select_up();
        cx.notify();
    }

    fn select_line_down(&mut self, _: &SelectLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.text.select_down();
        cx.notify();
    }

    fn select_all_text(&mut self, _: &SelectAllText, _: &mut Window, cx: &mut Context<Self>) {
        self.text.select_all();
        cx.notify();
    }

    fn delete_backward(&mut self, _: &DeleteBackward, _: &mut Window, cx: &mut Context<Self>) {
        self.text.backspace();
        Self::edited(cx);
    }

    fn delete_forward(&mut self, _: &DeleteForward, _: &mut Window, cx: &mut Context<Self>) {
        self.text.delete_forward();
        Self::edited(cx);
    }

    fn insert_newline(&mut self, _: &InsertNewline, _: &mut Window, cx: &mut Context<Self>) {
        self.text.insert("\n");
        Self::edited(cx);
    }

    fn copy_selection(&mut self, _: &CopySelection, _: &mut Window, cx: &mut Context<Self>) {
        if !self.text.selected_text().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text.selected_text().to_owned(),
            ));
        }
    }

    fn cut_selection(&mut self, _: &CutSelection, _: &mut Window, cx: &mut Context<Self>) {
        if self.text.selected_text().is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.text.selected_text().to_owned(),
        ));
        self.text.insert("");
        Self::edited(cx);
    }

    fn paste_text(&mut self, _: &PasteText, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.text.insert(&pasted);
            Self::edited(cx);
        }
    }

    /// Draws one line, splitting it so the caret and any selection are visible.
    ///
    /// Positions come from the buffer rather than from measured glyphs, so the
    /// caret sits between spans instead of at an exact pixel. That is enough to
    /// edit by; precise mouse positioning needs shaped-line hit testing.
    fn render_line(&self, line: &str, line_start: usize, focused: bool) -> gpui::Div {
        let line_range = line_start..line_start + line.len();
        let selection = self.text.selection();
        let selected = selection.start.max(line_range.start)..selection.end.min(line_range.end);
        let cursor = self.text.cursor();

        // Split the line at the caret and at each end of the selection, so each
        // piece is uniformly plain or highlighted.
        let mut boundaries = vec![selected.start, selected.end, cursor, line_range.end];
        boundaries.retain(|boundary| {
            (line_range.start..=line_range.end).contains(boundary) && *boundary > line_range.start
        });
        boundaries.sort_unstable();
        boundaries.dedup();

        let mut row = div().flex().items_center().min_h(px(18.0));
        let mut at = line_range.start;
        for boundary in boundaries {
            let highlighted = !selected.is_empty() && at >= selected.start && at < selected.end;
            let span = &line[at - line_range.start..boundary - line_range.start];
            if !span.is_empty() {
                row = row.child(
                    div()
                        .when(highlighted, |piece| piece.bg(rgb(theme::accent::DIM)))
                        .child(SharedString::from(span.to_owned())),
                );
            }
            at = boundary;
            if boundary == cursor && focused {
                row = row.child(Self::caret());
            }
        }
        // A caret at the very start of the line has no span before it.
        if cursor == line_range.start && focused {
            row = div()
                .flex()
                .items_center()
                .min_h(px(18.0))
                .child(Self::caret())
                .child(div().flex().items_center().children(vec![row]));
        }
        row
    }

    fn caret() -> gpui::Div {
        div()
            .w(px(1.5))
            .h(px(16.0))
            .bg(rgb(theme::text::PRIMARY))
            .flex_shrink_0()
    }
}

impl EntityInputHandler for CommentEditor {
    fn text_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        adjusted: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.text.range_from_utf16(&range_utf16);
        *adjusted = Some(self.text.range_to_utf16(&range));
        Some(self.text.content()[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        Some(gpui::UTF16Selection {
            range: self.text.range_to_utf16(&self.text.selection()),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.text
            .marked()
            .map(|marked| self.text.range_to_utf16(&marked))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.text.unmark();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match range_utf16 {
            Some(range) => {
                let range = self.text.range_from_utf16(&range);
                self.text.replace(range, text);
            }
            None => self.text.insert(text),
        }
        Self::edited(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<std::ops::Range<usize>>,
        text: &str,
        new_selection_utf16: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|range| self.text.range_from_utf16(&range));
        self.text.replace_and_mark(range, text, new_selection_utf16);
        // Composing counts as editing: an in-progress composition is text the
        // reviewer would not want to lose to a crash.
        Self::edited(cx);
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        element_bounds: gpui::Bounds<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        // The field's own bounds, so a candidate window appears beside the
        // composer. Placing it exactly at the caret needs shaped-line metrics.
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        // Mouse positioning is not supported yet; the caret is moved with the
        // keyboard.
        None
    }
}

impl Focusable for CommentEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CommentEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let focus_handle = self.focus_handle.clone();
        let entity = cx.entity();
        let handler_focus = self.focus_handle.clone();

        let mut line_start = 0;
        let lines = self
            .text
            .lines()
            .map(|line| {
                let rendered = self.render_line(line, line_start, focused);
                line_start += line.len() + 1;
                rendered
            })
            .collect::<Vec<_>>();

        div()
            .id("comment-editor")
            .key_context("CommentEditor")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::move_line_start))
            .on_action(cx.listener(Self::move_line_end))
            .on_action(cx.listener(Self::select_char_left))
            .on_action(cx.listener(Self::select_char_right))
            .on_action(cx.listener(Self::select_line_up))
            .on_action(cx.listener(Self::select_line_down))
            .on_action(cx.listener(Self::select_all_text))
            .on_action(cx.listener(Self::delete_backward))
            .on_action(cx.listener(Self::delete_forward))
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::cut_selection))
            .on_action(cx.listener(Self::paste_text))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&focus_handle);
                cx.stop_propagation();
            })
            .min_h(px(58.0))
            .w_full()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(if focused {
                rgb(theme::accent::BASE)
            } else {
                rgb(theme::surface::OVERLAY)
            })
            .bg(rgb(theme::surface::RAISED))
            .text_color(rgb(theme::text::PRIMARY))
            .text_sm()
            .cursor_text()
            .flex()
            .flex_col()
            // Installs the platform input handler during paint, which is what makes
            // dead keys, accents, and input-method composition work at all.
            .child(
                gpui::canvas(
                    |_bounds, _window, _cx| {},
                    // Installed during paint, which is where GPUI requires it.
                    move |bounds, (), window, cx| {
                        window.handle_input(
                            &handler_focus,
                            ElementInputHandler::new(bounds, entity),
                            cx,
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(if self.text.is_empty() && !focused {
                vec![
                    div()
                        .text_color(rgb(theme::text::TERTIARY))
                        .child("Write a review comment..."),
                ]
            } else {
                lines
            })
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
    /// Whether this row is inside the span a comment would cover.
    in_selection: bool,
}

/// What the diff view asks the model to do about drafts.
///
/// The diff view renders a read-only snapshot and never mutates the session
/// directly, so the model stays the single owner of draft state.
pub(crate) enum DiffViewEvent {
    /// The composer's text changed. `rows` is the span it covers, which is one row
    /// for an ordinary comment.
    Edited {
        rows: std::ops::RangeInclusive<usize>,
        body: String,
    },
    /// The reviewer discarded a row's draft.
    Discarded { row: usize },
    /// A stale draft should move onto a row in the current diff.
    Reanchored { stale: DiffAnchor, row: usize },
}

pub(crate) struct DiffView {
    file: Arc<DiffFile>,
    /// Index of `file` in the session, which is how threads and drafts are keyed.
    file_index: usize,
    comments: Arc<PlacedComments>,
    drafts: Arc<Drafts>,
    list_state: ListState,
    selected_line: usize,
    /// Where a multi-line selection began, when one is being extended.
    ///
    /// The selection runs between this and `selected_line` in either direction, so
    /// a reviewer can grow it upwards or downwards from where they started.
    selection_anchor: Option<usize>,
    /// The span the open composer covers, frozen when it opened so moving the
    /// cursor afterwards does not silently re-target the draft.
    comment_rows: Option<std::ops::RangeInclusive<usize>>,
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
            let Some(rows) = this.comment_rows.clone() else {
                return;
            };
            let body = editor.read(cx).content().to_owned();
            cx.emit(DiffViewEvent::Edited { rows, body });
        });

        Self {
            file,
            file_index,
            comments,
            drafts,
            list_state: ListState::new(item_count, ListAlignment::Top, px(ROW_HEIGHT)),
            selected_line: 0,
            selection_anchor: None,
            comment_rows: None,
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

    /// Switches to another file.
    ///
    /// The composer closes, which no longer loses anything: whatever was typed is
    /// already stored as a draft on the session.
    fn set_file(&mut self, file: Arc<DiffFile>, file_index: usize, cx: &mut Context<Self>) {
        self.list_state = ListState::new(file.line_count(), ListAlignment::Top, px(ROW_HEIGHT));
        self.file = file;
        self.file_index = file_index;
        self.selected_line = 0;
        self.selection_anchor = None;
        self.comment_rows = None;
        self.comment_editor.update(cx, CommentEditor::clear);
        cx.notify();
    }

    /// Moves the cursor, abandoning any range being built.
    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selection_anchor = None;
        self.move_cursor(index, cx);
    }

    fn move_cursor(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_line = index.min(self.file.line_count().saturating_sub(1));
        self.list_state.scroll_to_reveal_item(self.selected_line);
        cx.notify();
    }

    /// The rows a comment would cover: just the cursor, or the range being built.
    fn selected_rows(&self) -> std::ops::RangeInclusive<usize> {
        match self.selection_anchor {
            Some(anchor) if anchor <= self.selected_line => anchor..=self.selected_line,
            Some(anchor) => self.selected_line..=anchor,
            None => self.selected_line..=self.selected_line,
        }
    }

    /// Grows the selection from wherever it started.
    fn extend_selection(&mut self, to: usize, cx: &mut Context<Self>) {
        if self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.selected_line);
        }
        self.move_cursor(to, cx);
    }

    fn extend_selection_down(
        &mut self,
        _: &ExtendSelectionDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection(self.selected_line.saturating_add(1), cx);
    }

    fn extend_selection_up(
        &mut self,
        _: &ExtendSelectionUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection(self.selected_line.saturating_sub(1), cx);
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
        if self.comment_rows.as_ref() == Some(&self.selected_rows()) {
            self.comment_rows = None;
            window.focus(&self.focus_handle);
        } else {
            self.open_composer(self.selected_rows(), window, cx);
        }
        self.list_state.scroll_to_reveal_item(self.selected_line);
        cx.notify();
    }

    /// Scrolls a row into view and selects it, without opening anything.
    ///
    /// Used when a finding is chosen in the panel: the reviewer wants to see the
    /// line it is about before deciding.
    pub fn reveal_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_line = row.min(self.file.line_count().saturating_sub(1));
        self.selection_anchor = None;
        self.list_state.scroll_to_reveal_item(self.selected_line);
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Opens the composer on a row pre-filled with text of the caller's choosing.
    ///
    /// The one caller is accepting a finding onto a line that already holds a
    /// draft: overwriting the reviewer's words is refused, so both texts are put in
    /// front of them instead and nothing is saved until they commit.
    pub fn open_composer_with(
        &mut self,
        row: usize,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_line = row.min(self.file.line_count().saturating_sub(1));
        self.selection_anchor = None;
        self.comment_rows = Some(self.selected_line..=self.selected_line);
        self.list_state.scroll_to_reveal_item(self.selected_line);
        self.comment_editor
            .update(cx, |editor, cx| editor.load(text, cx));
        let editor_focus = self.comment_editor.read(cx).focus_handle.clone();
        window.focus(&editor_focus);
        cx.notify();
    }

    /// Opens the composer over a span, showing the draft already there if any.
    ///
    /// A range's draft lives at its last row, which is also where a single-line
    /// draft on that row would live. Extending a selection over an existing
    /// comment edits it rather than starting a rival.
    fn open_composer(
        &mut self,
        rows: std::ops::RangeInclusive<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let end_row = *rows.end();
        self.comment_rows = Some(rows);
        let existing = self
            .drafts
            .at(self.file_index, end_row)
            .map(|draft| draft.body.clone())
            .unwrap_or_default();
        self.comment_editor
            .update(cx, |editor, cx| editor.load(existing, cx));
        let editor_focus = self.comment_editor.read(cx).focus_handle.clone();
        window.focus(&editor_focus);
    }

    /// Discards the row's draft and closes the composer.
    fn discard_draft(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.comment_rows = None;
        self.comment_editor.update(cx, CommentEditor::clear);
        window.focus(&self.focus_handle);
        cx.emit(DiffViewEvent::Discarded { row });
        cx.notify();
    }

    /// Dismisses the composer. Bound to `escape` in the composer's own context,
    /// since `c` is now reserved for typing while the composer has focus.
    fn close_comment(&mut self, _: &CloseComment, window: &mut Window, cx: &mut Context<Self>) {
        if self.comment_rows.take().is_some() {
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
            .border_color(rgb(theme::accent::TEXT))
            .bg(rgb(theme::surface::RAISED))
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
                                    .text_color(rgb(theme::accent::TEXT))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(SharedString::from(comment.author.to_string())),
                            )
                            .child(
                                div()
                                    .text_color(rgb(theme::text::TERTIARY))
                                    .child(SharedString::from(comment.created_at.to_string())),
                            )
                            .when(comment.is_multiline(), |header| {
                                header.child(div().text_color(rgb(theme::text::TERTIARY)).child(
                                    format!(
                                        "lines {}-{}",
                                        comment.start_line.unwrap_or_default(),
                                        comment.line.unwrap_or_default(),
                                    ),
                                ))
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(theme::text::SECONDARY))
                            .child(SharedString::from(comment.body.to_string())),
                    )
            }))
            .when(replies > 0, |thread| {
                thread.child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme::text::TERTIARY))
                        .child(format!(
                            "{replies} repl{}",
                            if replies == 1 { "y" } else { "ies" }
                        )),
                )
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
            in_selection,
        } = row;
        // While the composer is open the draft is being edited in it, so showing
        // it read-only underneath as well would duplicate the same text.
        let resting_draft = draft.filter(|_| !show_comment);
        let draft_exists = draft.is_some();
        // Diff terrain from the design system: low-chroma fills that read as
        // landscape at forty rows a screen, sharing no hue with severity.
        let (row_bg, marker_color, text_color) = match line.kind {
            DiffLineKind::Context => (
                rgb(theme::surface::INSET),
                rgb(theme::text::FAINT),
                rgb(theme::text::SECONDARY),
            ),
            DiffLineKind::Addition => (
                rgb(theme::diff::add::BG),
                rgb(theme::diff::add::MARK),
                rgb(theme::diff::add::FG),
            ),
            DiffLineKind::Deletion => (
                rgb(theme::diff::del::BG),
                rgb(theme::diff::del::MARK),
                rgb(theme::diff::del::FG),
            ),
            DiffLineKind::NoNewlineMarker => (
                rgb(theme::surface::INSET),
                rgb(theme::severity::WARNING),
                rgb(theme::text::TERTIARY),
            ),
        };

        // The accent rail: two pixels down the left of any row a comment is
        // attached to, or that the composer is about to attach one to. Without it,
        // the only cue that a line carries a comment is a background tint that is
        // easy to miss and invisible once the thread scrolls past. The reviewer
        // could not see where a comment would land. It sits inside the gutter so
        // the content column never shifts sideways when a comment appears.
        let rail = if selected || show_comment || in_selection {
            Some(rgb(theme::accent::BASE))
        } else if draft_exists {
            // A draft a model proposed keeps its own hue, so provenance is legible
            // in the diff and not only in the panel.
            Some(rgb(
                if draft.is_some_and(domain::DraftComment::is_proposed) {
                    theme::proposed::BASE
                } else {
                    theme::accent::DIM
                },
            ))
        } else if !threads.is_empty() {
            Some(rgb(theme::border::DEFAULT))
        } else {
            None
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
            .bg(if selected {
                rgb(theme::surface::SELECTED)
            } else if in_selection {
                // A range under construction is visible without looking selected.
                rgb(theme::surface::HOVER)
            } else {
                row_bg
            })
            // Drawn above the first row of its hunk, so every hunk in a file is
            // labelled rather than only the first.
            .children(hunk_header.map(|header| {
                div()
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px_3()
                    .flex()
                    .items_center()
                    .bg(rgb(theme::diff::hunk::BG))
                    .text_color(rgb(theme::diff::hunk::FG))
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
                    // Always occupies its two pixels, coloured or not, so a row
                    // gaining a comment does not move the numbers beside it.
                    .child(
                        div()
                            .w(px(theme::RAIL_WIDTH))
                            .h_full()
                            .flex_shrink_0()
                            .when_some(rail, gpui::Styled::bg),
                    )
                    .child(
                        div()
                            .w(px(GUTTER_WIDTH - theme::RAIL_WIDTH))
                            .pr_2()
                            .text_right()
                            .text_color(rgb(theme::text::FAINT))
                            .child(old_number),
                    )
                    .child(
                        div()
                            .w(px(GUTTER_WIDTH))
                            .pr_2()
                            .text_right()
                            .text_color(rgb(theme::text::FAINT))
                            .child(new_number),
                    )
                    .child(div().w(px(20.0)).text_color(marker_color).child(marker))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            // Added and removed content carries its own tint, so
                            // the terrain reads even where the row fill is subtle.
                            .text_color(text_color)
                            .child(text),
                    )
                    .when(!threads.is_empty(), |row| {
                        row.child(
                            div()
                                .mr_2()
                                .px_1()
                                .rounded_sm()
                                .bg(rgb(theme::accent::DIM))
                                .text_xs()
                                .text_color(rgb(theme::accent::TEXT))
                                .child(format!("{}", threads.len())),
                        )
                    })
                    .when(draft.is_some(), |row| {
                        row.child(
                            div()
                                .mr_2()
                                .px_1()
                                .rounded_sm()
                                .bg(rgb(theme::severity::WARNING_DIM))
                                .text_xs()
                                .text_color(rgb(theme::severity::WARNING_TEXT))
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
                                .bg(rgb(theme::accent::BASE))
                                .text_xs()
                                .text_color(rgb(theme::text::ON_ACCENT))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    cx.stop_propagation();
                                    comment_view.update(cx, |this, cx| {
                                        this.selected_line = index;
                                        this.selection_anchor = None;
                                        this.open_composer(index..=index, window, cx);
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
                        .border_color(rgb(theme::severity::WARNING))
                        .bg(rgb(theme::surface::OVERLAY))
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
                                        .text_color(rgb(theme::severity::WARNING_TEXT))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("Your draft"),
                                )
                                .when(draft.is_stale, |header| {
                                    header.child(
                                        div()
                                            .text_color(rgb(theme::severity::ERROR_TEXT))
                                            .child("needs re-anchoring"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(theme::text::PRIMARY))
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
                                .border_color(rgb(theme::border::STRONG))
                                .text_color(rgb(theme::text::SECONDARY))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    cx.stop_propagation();
                                    close_view.update(cx, |this, cx| {
                                        this.comment_rows = None;
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
                                .border_color(rgb(theme::severity::ERROR_DIM))
                                .text_color(rgb(theme::severity::ERROR_TEXT))
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
        // appear. It is also the only place the reviewer can act on it.
        let stale_drafts = self
            .drafts
            .stale()
            .filter(|draft| draft.anchor.path == self.file.path)
            .cloned()
            .collect::<Vec<_>>();
        let selected_line = self.selected_line;
        let comment_rows = self.comment_rows.clone();
        let selected_rows = self.selected_rows();
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
            || SharedString::from("-"),
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
            .on_action(cx.listener(Self::extend_selection_down))
            .on_action(cx.listener(Self::extend_selection_up))
            .on_action(cx.listener(Self::toggle_comment))
            .on_action(cx.listener(Self::close_comment))
            .on_action(cx.listener(Self::copy_selected_line))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::surface::BASE))
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
                    .border_color(rgb(theme::border::DEFAULT))
                    .bg(rgb(theme::surface::BASE))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_color(rgb(theme::text::PRIMARY))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("ZReview"),
                            )
                            .child(div().text_color(rgb(theme::text::SECONDARY)).child(path)),
                    )
                    .child(div().text_color(rgb(theme::text::SECONDARY)).child(format!(
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
                    .bg(rgb(theme::diff::hunk::BG))
                    .text_color(rgb(theme::accent::TEXT))
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
                                    .text_color(rgb(theme::text::PRIMARY))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(reason.label()),
                            )
                            .child(
                                div()
                                    .max_w(px(420.0))
                                    .text_xs()
                                    .text_color(rgb(theme::text::TERTIARY))
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
                                        show_comment: comment_rows
                                            .as_ref()
                                            .is_some_and(|rows| *rows.end() == index),
                                        in_selection: selected_rows.contains(&index),
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
                            .border_color(rgb(theme::border::DEFAULT))
                            .bg(rgb(theme::surface::BASE))
                            .text_color(rgb(theme::text::SECONDARY))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .text_color(rgb(theme::text::PRIMARY))
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
                                        .text_color(rgb(theme::severity::WARNING_TEXT))
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
                                            .text_color(rgb(theme::severity::WARNING))
                                            .child(format!("{} not on a line", unplaced.len())),
                                    )
                                    .children(unplaced.iter().map(|unplaced| {
                                        let root = unplaced.thread.root();
                                        div()
                                            .p_2()
                                            .rounded_md()
                                            .bg(rgb(theme::surface::RAISED))
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(theme::severity::WARNING))
                                                    .child(unplaced.reason.to_string()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(theme::accent::TEXT))
                                                    .child(SharedString::from(
                                                        root.author.to_string(),
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(theme::text::SECONDARY))
                                                    .child(SharedString::from(
                                                        root.body.to_string(),
                                                    )),
                                            )
                                    }))
                            })
                            // A stale draft is text the reviewer wrote that
                            // currently cannot be submitted. It is shown here with
                            // the one action that fixes that.
                            .when(!stale_drafts.is_empty(), |panel| {
                                panel
                                    .child(
                                        div()
                                            .mt_2()
                                            .text_xs()
                                            .text_color(rgb(theme::severity::ERROR_TEXT))
                                            .child(format!(
                                                "{} draft{} need re-anchoring",
                                                stale_drafts.len(),
                                                if stale_drafts.len() == 1 { "" } else { "s" },
                                            )),
                                    )
                                    .children(stale_drafts.iter().map(|draft| {
                                        let move_view = panel_view.clone();
                                        let stale = draft.anchor.clone();
                                        div()
                                            .p_2()
                                            .rounded_md()
                                            .border_l_2()
                                            .border_color(rgb(theme::severity::ERROR_TEXT))
                                            .bg(rgb(theme::surface::OVERLAY))
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(theme::text::SECONDARY))
                                                    .child(format!(
                                                        "was {} line {}",
                                                        draft.anchor.side, draft.anchor.line,
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(theme::text::PRIMARY))
                                                    .child(SharedString::from(draft.body.clone())),
                                            )
                                            .child(
                                                div()
                                                    .id(("reanchor", draft.anchor.line))
                                                    .mt_1()
                                                    .px_2()
                                                    .py_1()
                                                    .rounded_sm()
                                                    .bg(rgb(theme::accent::BASE))
                                                    .text_xs()
                                                    .text_color(rgb(theme::text::ON_ACCENT))
                                                    .cursor_pointer()
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_, _window, cx| {
                                                            cx.stop_propagation();
                                                            let stale = stale.clone();
                                                            move_view.update(cx, |this, cx| {
                                                                cx.emit(
                                                                    DiffViewEvent::Reanchored {
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
                            .child(
                                div()
                                    .mt_4()
                                    .text_xs()
                                    .text_color(rgb(theme::text::TERTIARY))
                                    .child(format!(
                                        "Row {} · j/k move · c comment · esc close",
                                        selected_line + 1
                                    )),
                            ),
                    ),
            )
    }
}

/// What the review view asks its owner to do.
///
/// Running a review needs a backend and a handle to the window, neither of which
/// this view has, so it reports rather than acts.
pub(crate) enum ReviewViewEvent {
    /// The reviewer asked for an automated review. Whoever owns the backend runs
    /// it; this view only knows it was asked for.
    ReviewRequested,
}

/// The review a reviewer is working through: files on the left, diff in the
/// middle, findings on the right.
///
/// Every piece of state it draws lives in the shared [`SessionModel`]; this view
/// dispatches keys and mouse into it and renders what comes back.
pub(crate) struct ReviewView {
    model: Arc<Mutex<SessionModel>>,
    diff_view: Entity<DiffView>,
    summary_editor: Entity<CommentEditor>,
    file_list_state: ListState,
    /// Held so draft edits keep reaching the model.
    _diff_subscription: Subscription,
    /// Held so summary edits keep reaching the model.
    _summary_subscription: Subscription,
}

impl EventEmitter<ReviewViewEvent> for ReviewView {}

impl ReviewView {
    #[must_use]
    pub fn new(
        model: Arc<Mutex<SessionModel>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (file, file_index, comments, drafts, file_count, summary) = {
            let guard = lock(&model);
            let session = loaded_review(&guard).session();
            (
                Arc::new(session.selected_file().clone()),
                session.selected_file_index(),
                session.shared_comments(),
                session.shared_drafts(),
                session.files().len(),
                session.summary().to_owned(),
            )
        };
        let diff_view = cx.new(|cx| DiffView::new(file, file_index, comments, drafts, window, cx));
        let diff_subscription = cx.subscribe(&diff_view, Self::on_diff_event);

        // The summary reuses the composer, so it inherits the same
        // keybinding isolation the inline editor needed.
        let summary_editor = cx.new(|cx| CommentEditor::with_content(summary, cx));
        let summary_subscription =
            cx.subscribe(&summary_editor, |this, editor, _: &CommentEdited, cx| {
                let body = editor.read(cx).content().to_owned();
                lock(&this.model).summary_edited(body);
                cx.notify();
            });

        Self {
            model,
            diff_view,
            summary_editor,
            file_list_state: ListState::new(file_count, ListAlignment::Top, px(36.0)),
            _diff_subscription: diff_subscription,
            _summary_subscription: summary_subscription,
        }
    }

    /// Opens or closes the guidance section.
    pub fn toggle_guidance_panel(&mut self, cx: &mut Context<Self>) {
        lock(&self.model).toggle_guidance_panel();
        cx.notify();
    }

    /// Turns one guidance file on or off for the next run.
    pub fn toggle_guidance(&mut self, path: &str, cx: &mut Context<Self>) {
        let toggled = lock(&self.model).toggle_guidance(path);
        if toggled {
            cx.notify();
        }
    }

    /// Asks the running review to stop.
    pub fn cancel_review(&mut self, cx: &mut Context<Self>) {
        lock(&self.model).cancel_review();
        cx.notify();
    }

    /// Scrolls the diff to a finding's line and selects it in the panel.
    pub fn reveal_finding(&mut self, id: FindingId, window: &mut Window, cx: &mut Context<Self>) {
        let location = lock(&self.model).reveal_finding(id);
        // A finding about the whole change has nowhere to scroll to.
        if let Some(location) = location {
            self.show_file(location.file, cx);
            self.diff_view
                .update(cx, |view, cx| view.reveal_row(location.row, window, cx));
        }
        cx.notify();
    }

    fn run_review(&mut self, _: &RunReview, _: &mut Window, cx: &mut Context<Self>) {
        let running = lock(&self.model)
            .review()
            .is_some_and(|review| review.run().is_running());
        if running {
            return;
        }
        cx.emit(ReviewViewEvent::ReviewRequested);
    }

    fn select_next_finding(
        &mut self,
        _: &SelectNextFinding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = lock(&self.model)
            .review()
            .and_then(ReviewModel::next_finding);
        if let Some(next) = next {
            self.reveal_finding(next, window, cx);
        }
    }

    fn accept_selected_finding(
        &mut self,
        _: &AcceptFinding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = lock(&self.model)
            .review()
            .and_then(ReviewModel::selected_finding);
        if let Some(id) = selected {
            self.accept_finding_by_id(id, window, cx);
        }
    }

    fn dismiss_selected_finding(
        &mut self,
        _: &DismissFinding,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = lock(&self.model)
            .review()
            .and_then(ReviewModel::selected_finding);
        if let Some(id) = selected {
            self.dismiss_finding_by_id(id, cx);
        }
    }

    /// Accepts a finding, or hands the reviewer the composer when it cannot be
    /// accepted without overwriting their words.
    pub fn accept_finding_by_id(
        &mut self,
        id: FindingId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Bound before the match so the model is not still locked when the
        // handling of a disposition reaches back into it.
        let disposition = lock(&self.model).accept_finding(id);
        match disposition {
            FindingDisposition::Drafted => {
                self.publish_drafts(cx);
                cx.notify();
            }
            FindingDisposition::Composer { location, seed } => {
                self.show_file(location.file, cx);
                self.diff_view.update(cx, |view, cx| {
                    view.open_composer_with(location.row, seed, window, cx);
                });
                cx.notify();
            }
            FindingDisposition::Summary { body } => {
                self.summary_editor
                    .update(cx, |editor, cx| editor.load(body, cx));
                self.publish_drafts(cx);
                cx.notify();
            }
            FindingDisposition::Unknown => {}
        }
    }

    pub fn dismiss_finding_by_id(&mut self, id: FindingId, cx: &mut Context<Self>) {
        let dismissed = lock(&self.model).dismiss_finding(id);
        if dismissed {
            self.publish_drafts(cx);
            cx.notify();
        }
    }

    /// Hands the diff the drafts it should now be drawing.
    fn publish_drafts(&mut self, cx: &mut Context<Self>) {
        let drafts = lock(&self.model)
            .review()
            .map(|review| review.session().shared_drafts());
        if let Some(drafts) = drafts {
            self.diff_view
                .update(cx, |view, cx| view.set_drafts(drafts, cx));
        }
    }

    /// Clears what a forge has accepted out of the composer and the diff.
    fn submission_cleared(&mut self, cx: &mut Context<Self>) {
        self.summary_editor
            .update(cx, |editor, cx| editor.load(String::new(), cx));
        self.publish_drafts(cx);
        cx.notify();
    }

    /// Applies a draft change to the model, then republishes it for rendering.
    fn on_diff_event(
        &mut self,
        _diff_view: Entity<DiffView>,
        event: &DiffViewEvent,
        cx: &mut Context<Self>,
    ) {
        let changed = {
            let mut model = lock(&self.model);
            match event {
                DiffViewEvent::Edited { rows, body } => {
                    model.draft_edited(rows.clone(), body.clone())
                }
                DiffViewEvent::Discarded { row } => model.draft_discarded(*row),
                DiffViewEvent::Reanchored { stale, row } => model.draft_reanchored(stale, *row),
            }
        };
        if changed {
            self.publish_drafts(cx);
            cx.notify();
        }
    }

    /// Switches the displayed file without moving focus.
    ///
    /// Focus belongs where the reviewer put it: clicking a finding in the panel
    /// should not yank the keyboard into the diff.
    fn show_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let file = {
            let mut model = lock(&self.model);
            if !model.select_file(index) {
                return;
            }
            Arc::new(loaded_review(&model).session().selected_file().clone())
        };
        self.diff_view
            .update(cx, |view, cx| view.set_file(file, index, cx));
        self.file_list_state.scroll_to_reveal_item(index);
    }

    fn select_file(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.show_file(index, cx);
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
        let next = {
            let model = lock(&self.model);
            let session = loaded_review(&model).session();
            session
                .selected_file_index()
                .saturating_add(1)
                .min(session.files().len().saturating_sub(1))
        };
        self.select_file(next, window, cx);
    }

    fn select_previous_file(
        &mut self,
        _: &SelectPreviousFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = {
            let model = lock(&self.model);
            loaded_review(&model)
                .session()
                .selected_file_index()
                .saturating_sub(1)
        };
        self.select_file(previous, window, cx);
    }

    fn toggle_viewed(&mut self, _: &ToggleViewed, _: &mut Window, cx: &mut Context<Self>) {
        lock(&self.model).toggle_viewed();
        cx.notify();
    }

    /// The summary field and the three ways to submit.
    ///
    /// Each event is its own button rather than a menu, so choosing to approve is
    /// as deliberate as choosing to request changes.
    fn render_submit_bar(&self, session: &ReviewSession, cx: &mut Context<Self>) -> gpui::Div {
        let drafts = session.drafts();
        let stale = drafts.stale_count();
        let ready = drafts.len() - stale;
        let can_submit = session.source().head_sha().is_some();
        let review_view = cx.entity();

        div()
            .h(px(92.0))
            .flex_shrink_0()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .border_t_1()
            .border_color(rgb(theme::border::DEFAULT))
            .bg(rgb(theme::surface::BASE))
            .font_family("SF Mono")
            .text_size(px(13.0))
            .child(
                div()
                    .w(px(150.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_color(rgb(theme::text::PRIMARY))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("{ready} to submit")),
                    )
                    .when(stale > 0, |counts| {
                        counts.child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme::severity::ERROR_TEXT))
                                .child(format!("{stale} not anchored")),
                        )
                    }),
            )
            .child(div().flex_1().min_w_0().child(self.summary_editor.clone()))
            .children(can_submit.then(|| {
                div().flex_shrink_0().flex().gap_2().children(
                    [
                        (ReviewEvent::Comment, rgb(theme::accent::BASE)),
                        (ReviewEvent::Approve, rgb(theme::severity::SUCCESS)),
                        (ReviewEvent::RequestChanges, rgb(theme::severity::ERROR)),
                    ]
                    .map(|(event, colour)| {
                        let view = review_view.clone();
                        div()
                            .id(SharedString::from(format!(
                                "submit-{}",
                                event.github_value()
                            )))
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(colour)
                            .text_xs()
                            .text_color(rgb(theme::text::ON_ACCENT))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                                cx.stop_propagation();
                                view.update(cx, |review, cx| {
                                    lock(&review.model).request_submission(event);
                                    cx.notify();
                                });
                            })
                            .child(event.label())
                    }),
                )
            }))
    }

    /// The sidebar header: what is under review, and its overall counts.
    fn render_source_header(session: &ReviewSession) -> gpui::Div {
        let (label, title) = match session.source() {
            SessionSource::Demo => (
                SharedString::from("Generated fixture"),
                SharedString::from("Diff virtualization demo"),
            ),
            SessionSource::LocalComparison {
                base_sha, head_sha, ..
            } => (
                SharedString::from("Local comparison"),
                // `...` is the merge-base notation this comparison actually uses.
                SharedString::from(format!("{}...{}", short_sha(base_sha), short_sha(head_sha))),
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
        let file_count = session.files().len();
        let viewed_count = session.viewed_count();
        let thread_count = session.comments().thread_count();

        div()
            .flex_shrink_0()
            .px_3()
            .py_3()
            .flex()
            .flex_col()
            .justify_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(theme::border::DEFAULT))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::severity::INFO))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(label),
            )
            .child(
                div()
                    .text_color(rgb(theme::text::PRIMARY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(title),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::text::TERTIARY))
                    .child(format!(
                        "{file_count} files · {viewed_count} viewed · {thread_count} conversations"
                    )),
            )
            // Conversations that would not load, or drafts that are not being
            // saved, must be visible: a session that quietly lacks either looks
            // exactly like one that has nothing to show.
            .children(session.warnings().iter().map(|warning| {
                div()
                    .text_xs()
                    .text_color(rgb(theme::severity::WARNING))
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
            FileStatus::Added => ("A", rgb(theme::severity::SUCCESS)),
            FileStatus::Deleted => ("D", rgb(theme::severity::ERROR_TEXT)),
            FileStatus::Modified => ("M", rgb(theme::severity::WARNING)),
            FileStatus::Renamed => ("R", rgb(theme::severity::INFO)),
            FileStatus::Copied => ("C", rgb(theme::proposed::BASE)),
            FileStatus::TypeChanged => ("T", rgb(theme::severity::WARNING)),
            FileStatus::Unmerged => ("U", rgb(theme::severity::ERROR)),
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
                rgb(theme::surface::SELECTED)
            } else {
                rgb(theme::surface::BASE)
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
                    .text_color(rgb(theme::text::SECONDARY))
                    .child(path),
            )
            .when(threads > 0, |row| {
                row.child(
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(rgb(theme::accent::DIM))
                        .text_xs()
                        .text_color(rgb(theme::accent::TEXT))
                        .child(format!("{threads}")),
                )
            })
            .when(file.is_binary, |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme::text::TERTIARY))
                        .child("binary"),
                )
            })
            .when(!file.is_binary, |row| {
                row.child(
                    div()
                        .flex()
                        .gap_1()
                        .text_xs()
                        .child(
                            div()
                                .text_color(rgb(theme::severity::SUCCESS))
                                .child(format!("+{additions}")),
                        )
                        .child(
                            div()
                                .text_color(rgb(theme::severity::ERROR_TEXT))
                                .child(format!("-{deletions}")),
                        ),
                )
            })
            .when(viewed, |row| {
                row.child(div().text_color(rgb(theme::severity::SUCCESS)).child("✓"))
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
        let review_view = cx.entity();
        // Cloned before the file list takes ownership of its own copy.
        let panel_view = review_view.clone();
        // Held across this render, so callees must not lock the model themselves.
        let model = lock(&self.model);
        let review = loaded_review(&model);
        let session = review.session();
        let files = session.shared_files();
        let selected = session.selected_file_index();
        let viewed = (0..files.len())
            .map(|index| session.is_viewed(index))
            .collect::<Vec<_>>();
        let comments = session.shared_comments();
        let header = Self::render_source_header(session);
        let submit_bar = self.render_submit_bar(session, cx);
        // Only takes space once there is something to say.
        let panel = review.findings_panel_visible().then(|| {
            findings::render(
                session,
                review.run(),
                review.selected_finding(),
                review.guidance_expanded(),
                &panel_view,
            )
        });

        div()
            .id("review-session")
            .key_context("ReviewSession")
            .on_action(cx.listener(Self::select_next_file))
            .on_action(cx.listener(Self::select_previous_file))
            .on_action(cx.listener(Self::toggle_viewed))
            .on_action(cx.listener(Self::run_review))
            .on_action(cx.listener(Self::select_next_finding))
            .on_action(cx.listener(Self::accept_selected_finding))
            .on_action(cx.listener(Self::dismiss_selected_finding))
            .size_full()
            .flex()
            .bg(rgb(theme::surface::BASE))
            .child(
                div()
                    .w(px(290.0))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(rgb(theme::border::DEFAULT))
                    .bg(rgb(theme::surface::BASE))
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
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().flex_1().min_h_0().child(self.diff_view.clone()))
                    .child(submit_bar),
            )
            .children(panel)
    }
}

/// How a review run is started.
///
/// Installed by whoever owns a backend, after the window exists. A view cannot
/// hold a handle to the window it lives in at construction time. Absent until then,
/// and absent for good in a build with no backend, in which case asking for a review
/// does nothing rather than failing.
pub type ReviewLauncher = Box<dyn Fn(ReviewSession, &mut App)>;

/// The root view: the loading screen, the failure screen, or the review.
///
/// The window opens on this before any Git or GitHub work starts, so a slow or
/// failing load is something the reviewer watches rather than a terminal they may
/// not be looking at.
pub struct SessionView {
    model: Arc<Mutex<SessionModel>>,
    /// The review, once the session is loaded.
    review: Option<LoadedReview>,
    /// How to start a review, once something has said how.
    review_launcher: Option<ReviewLauncher>,
    focus_handle: FocusHandle,
}

/// The review view, and what keeps it talking to the session view.
///
/// Held together because they are only correct together: a subscription outliving
/// the review it watches is dead weight, and a review with none is deaf.
struct LoadedReview {
    view: Entity<ReviewView>,
    /// Held so the review's requests and its repaints keep arriving.
    _subscriptions: [Subscription; 2],
}

impl SessionView {
    /// Opens on the loading screen for a request that has not started yet.
    #[must_use]
    pub fn loading(description: impl Into<String>, cx: &mut Context<Self>) -> Self {
        Self {
            model: Arc::new(Mutex::new(SessionModel::loading(description))),
            review: None,
            review_launcher: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// The state every view of this session shares.
    #[must_use]
    pub fn model(&self) -> Arc<Mutex<SessionModel>> {
        Arc::clone(&self.model)
    }

    /// Says how to run a review. Without this, asking for one does nothing.
    pub fn set_review_launcher(&mut self, launcher: ReviewLauncher) {
        self.review_launcher = Some(launcher);
    }

    /// Records the stage the loader has reached.
    pub fn set_stage(&mut self, label: impl Into<String>, cx: &mut Context<Self>) {
        if lock(&self.model).set_stage(label) {
            cx.notify();
        }
    }

    /// Moves to the loaded session, or to the failure that stopped it.
    pub fn finish(
        &mut self,
        result: Result<LoadedSession, SessionFailure>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let loaded = result.is_ok();
        lock(&self.model).finish(result);
        if loaded {
            let view = cx.new(|cx| ReviewView::new(Arc::clone(&self.model), window, cx));
            let subscriptions = [
                cx.subscribe(&view, |this, _review, event, cx| {
                    this.on_review_event(event, cx);
                }),
                // The banners above the review are drawn from state the review
                // changes, so this view repaints whenever it does.
                cx.observe(&view, |_, _, cx| cx.notify()),
            ];
            // Hand focus to the diff so its keybindings work immediately.
            let focus_handle = view.read(cx).diff_view.read(cx).focus_handle.clone();
            window.focus(&focus_handle);
            self.review = Some(LoadedReview {
                view,
                _subscriptions: subscriptions,
            });
        } else {
            window.focus(&self.focus_handle);
            self.review = None;
        }
        cx.notify();
    }

    /// Starts the review the panel asked for.
    fn on_review_event(&mut self, event: &ReviewViewEvent, cx: &mut Context<Self>) {
        match event {
            ReviewViewEvent::ReviewRequested => {
                // Read out here rather than inside the launcher, which is handed
                // the same model and would deadlock on a lock still held.
                let Some(session) = lock(&self.model).review_request() else {
                    return;
                };
                if let Some(launcher) = self.review_launcher.take() {
                    launcher(session, cx);
                    self.review_launcher = Some(launcher);
                }
                // The panel has to show the run it just asked for.
                cx.notify();
            }
        }
    }

    fn cancel_submission(&mut self, cx: &mut Context<Self>) {
        lock(&self.model).cancel_submission();
        cx.notify();
    }

    /// Posts the confirmed review.
    ///
    /// The request is sent on a background thread because it is network I/O, and
    /// local drafts are forgotten only after the forge has accepted it. Until
    /// then the local copy is the only copy.
    fn send_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(PendingSend {
            submission,
            submitter,
        }) = lock(&self.model).begin_send()
        else {
            return;
        };
        cx.notify();

        cx.spawn(async move |this, cx| {
            let posted = {
                let submission = submission.clone();
                cx.background_executor()
                    .spawn(async move { submitter.submit(&submission) })
                    .await
            };

            this.update(cx, |this, cx| {
                let accepted = posted.is_ok();
                lock(&this.model).complete_send(&submission, posted);
                // Only now is it safe for the view to forget them too.
                if accepted && let Some(loaded) = &this.review {
                    loaded.view.update(cx, ReviewView::submission_cleared);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Everything about the submission that belongs above the diff.
    ///
    /// The confirmation is a full panel rather than a small dialog because PLAN
    /// requires the reviewer to see every inline comment, the summary, the event,
    /// and the pinned head before anything is posted.
    fn render_submission_banner(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let view = cx.entity();
        let panel = || {
            div()
                .flex_shrink_0()
                .max_h(px(420.0))
                .overflow_hidden()
                .px_4()
                .py_3()
                .flex()
                .flex_col()
                .gap_2()
                .border_b_1()
                .border_color(rgb(theme::border::DEFAULT))
                .bg(rgb(theme::surface::BASE))
                .font_family("SF Mono")
                .text_size(px(13.0))
        };

        match lock(&self.model).submission() {
            SubmissionState::Idle => None,

            SubmissionState::Sending => Some(
                panel()
                    .child(
                        div()
                            .text_color(rgb(theme::accent::TEXT))
                            .child("Submitting the review..."),
                    )
                    .into_any(),
            ),

            SubmissionState::Sent(outcome) => Some(
                panel()
                    .child(
                        div()
                            .text_color(rgb(theme::severity::SUCCESS))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!(
                                "Submitted as {} with {} inline comment{}",
                                outcome.state,
                                outcome.comment_count,
                                if outcome.comment_count == 1 { "" } else { "s" },
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::text::SECONDARY))
                            .child(SharedString::from(outcome.url.clone())),
                    )
                    .into_any(),
            ),

            SubmissionState::Failed(failure) => Some(
                panel()
                    .child(
                        div()
                            .text_color(rgb(theme::severity::ERROR_TEXT))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(SharedString::from(failure.summary.clone())),
                    )
                    .children(failure.remediation.as_ref().map(|remediation| {
                        div()
                            .text_color(rgb(theme::severity::WARNING_TEXT))
                            .child(SharedString::from(remediation.clone()))
                    }))
                    .children(failure.detail.as_ref().map(|detail| {
                        div()
                            .text_xs()
                            .text_color(rgb(theme::text::SECONDARY))
                            .child(SharedString::from(detail.clone()))
                    }))
                    .into_any(),
            ),

            SubmissionState::Confirming(submission) => {
                Some(Self::render_confirmation(submission, panel(), &view))
            }
        }
    }

    /// The full request, laid out for approval.
    #[allow(clippy::too_many_lines)]
    fn render_confirmation(
        submission: &ReviewSubmission,
        panel: gpui::Div,
        view: &Entity<Self>,
    ) -> gpui::AnyElement {
        let send_view = view.clone();
        let cancel_view = view.clone();
        panel
            .child(
                div()
                    .text_color(rgb(theme::text::PRIMARY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!(
                        "{} with {} inline comment{}",
                        submission.event.label(),
                        submission.comments.len(),
                        if submission.comments.len() == 1 {
                            ""
                        } else {
                            "s"
                        },
                    )),
            )
            // The head is shown because it is what the review will be
            // pinned to, and therefore what it can still be rejected
            // for.
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::text::TERTIARY))
                    .child(format!("pinned to {}", short_sha(&submission.head_sha))),
            )
            .children((!submission.body.is_empty()).then(|| {
                div()
                    .p_2()
                    .rounded_md()
                    .bg(rgb(theme::surface::RAISED))
                    .text_color(rgb(theme::text::SECONDARY))
                    .child(SharedString::from(submission.body.clone()))
            }))
            .children(submission.comments.iter().map(|comment| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .border_l_2()
                    .border_color(rgb(theme::accent::BASE))
                    .bg(rgb(theme::surface::RAISED))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::text::SECONDARY))
                            .child(format!(
                                "{} {} line {}",
                                comment.path, comment.side, comment.line,
                            )),
                    )
                    .child(
                        div()
                            .text_color(rgb(theme::text::PRIMARY))
                            .child(SharedString::from(comment.body.clone())),
                    )
            }))
            // Shown, never hidden: a reviewer must not believe these
            // were posted.
            .children((!submission.excluded.is_empty()).then(|| {
                div()
                    .mt_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_color(rgb(theme::severity::ERROR_TEXT))
                            .child(format!(
                                "{} draft{} will NOT be posted",
                                submission.excluded.len(),
                                if submission.excluded.len() == 1 {
                                    ""
                                } else {
                                    "s"
                                },
                            )),
                    )
                    .children(
                        submission
                            .excluded
                            .iter()
                            .map(|ExcludedDraft { draft, reason }| {
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme::text::SECONDARY))
                                    .child(format!(
                                        "{} line {} ({reason}): {}",
                                        draft.anchor.path, draft.anchor.line, draft.body,
                                    ))
                            }),
                    )
            }))
            .child(
                div()
                    .mt_2()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("confirm-submit")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(theme::accent::BASE))
                            .text_color(rgb(theme::text::ON_ACCENT))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                                cx.stop_propagation();
                                send_view.update(cx, SessionView::send_confirmed);
                            })
                            .child("Post this review to GitHub"),
                    )
                    .child(
                        div()
                            .id("cancel-submit")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(theme::border::STRONG))
                            .text_color(rgb(theme::text::SECONDARY))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                                cx.stop_propagation();
                                cancel_view.update(cx, SessionView::cancel_submission);
                            })
                            .child("Cancel"),
                    ),
            )
            .into_any()
    }

    fn render_centered(children: Vec<gpui::AnyElement>) -> gpui::Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(rgb(theme::surface::BASE))
            .font_family("SF Mono")
            .text_size(px(13.0))
            .children(children)
    }
}

impl Focusable for SessionView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.review {
            // Hand focus to the diff so its keybindings work immediately.
            Some(loaded) => loaded.view.read(cx).diff_view.read(cx).focus_handle.clone(),
            None => self.focus_handle.clone(),
        }
    }
}

impl Render for SessionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let banner = self.render_submission_banner(cx);
        // Held across this render, so callees must not lock the model themselves.
        let model = lock(&self.model);

        match model.phase() {
            SessionPhase::Loading { description, stage } => Self::render_centered(vec![
                div()
                    .text_color(rgb(theme::text::PRIMARY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("Opening {description}"))
                    .into_any(),
                div()
                    .text_color(rgb(theme::accent::TEXT))
                    .child(format!("{stage}..."))
                    .into_any(),
            ])
            .into_any(),

            SessionPhase::Failed(failure) => Self::render_centered(vec![
                div()
                    .max_w(px(680.0))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_color(rgb(theme::severity::ERROR_TEXT))
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
                            .border_color(rgb(theme::severity::WARNING))
                            .bg(rgb(theme::surface::RAISED))
                            .text_color(rgb(theme::severity::WARNING_TEXT))
                            .child(SharedString::from(remediation.clone()))
                    }))
                    .children(failure.detail.as_ref().map(|detail| {
                        div()
                            .text_xs()
                            .text_color(rgb(theme::text::SECONDARY))
                            .child(SharedString::from(detail.clone()))
                    }))
                    .into_any(),
            ])
            .into_any(),

            // A banner rather than a line in the sidebar: writes failing means
            // the reviewer's work is being lost as they type, which outranks
            // anything else on screen.
            SessionPhase::Ready(_) => div()
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(theme::surface::BASE))
                .children(model.draft_write_failure().map(|failure| {
                    div()
                        .flex_shrink_0()
                        .px_4()
                        .py_2()
                        .bg(rgb(theme::severity::ERROR_DIM))
                        .text_color(rgb(theme::severity::ERROR_TEXT))
                        .font_family("SF Mono")
                        .text_size(px(13.0))
                        .child(format!("Drafts are not being saved: {failure}"))
                }))
                .children(banner)
                .children(
                    self.review
                        .as_ref()
                        .map(|loaded| div().flex_1().min_h_0().child(loaded.view.clone())),
                )
                .into_any(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

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
            SessionSource::LocalComparison {
                repository_root: std::path::PathBuf::from("/tmp/repository"),
                base_sha: Arc::clone(&head_sha),
                diff_base_sha: Arc::clone(&head_sha),
                head_sha,
            },
            files.into(),
        )
        .unwrap()
    }

    /// A review view on a model that has just become ready.
    fn ready_review(
        cx: &mut TestAppContext,
        session: ReviewSession,
    ) -> (Entity<ReviewView>, &mut gpui::VisualTestContext) {
        let model = Arc::new(Mutex::new(SessionModel::loading("a review")));
        lock(&model).finish(Ok(LoadedSession::unsaved(session)));
        cx.add_window_view(move |window, cx| ReviewView::new(model, window, cx))
    }

    /// Reads the session a review view is drawing.
    fn with_session<R>(
        view: &Entity<ReviewView>,
        app: &App,
        read: impl FnOnce(&ReviewSession) -> R,
    ) -> R {
        let model = lock(&view.read(app).model);
        read(loaded_review(&model).session())
    }

    fn body_at(view: &Entity<ReviewView>, app: &App, file: usize, row: usize) -> Option<String> {
        with_session(view, app, |session| {
            session.draft_at(file, row).map(|draft| draft.body.clone())
        })
    }

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
        assert_eq!(cx.read(|app| view.read(app).selected_line), 1);

        cx.dispatch_action(ToggleComment);
        cx.simulate_input("hello");

        assert_eq!(
            cx.read(|app| view.read(app).comment_rows.clone()),
            Some(1..=1)
        );
        assert_eq!(
            cx.read(|app| view.read(app).comment_editor.read(app).content().to_owned()),
            "hello",
        );
    }

    /// The composer renders inside `DiffView`, so navigation bindings used to
    /// swallow every `j`, `k`, and `c` typed into a review comment. `c` also
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
            cx.read(|app| view.read(app).comment_editor.read(app).content().to_owned()),
            "jerky code",
        );
        // Typing must not have navigated the diff or dismissed the composer.
        assert_eq!(cx.read(|app| view.read(app).selected_line), 0);
        assert_eq!(
            cx.read(|app| view.read(app).comment_rows.clone()),
            Some(0..=0)
        );
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
        assert_eq!(cx.read(|app| view.read(app).selected_line), 0);

        cx.simulate_keystrokes("escape");
        assert_eq!(cx.read(|app| view.read(app).comment_rows.clone()), None);

        // With the composer gone, the same key navigates again.
        cx.simulate_input("j");
        assert_eq!(cx.read(|app| view.read(app).selected_line), 1);
    }

    #[gpui::test]
    fn switches_files_and_tracks_viewed_state(cx: &mut TestAppContext) {
        cx.update(init);
        let mut first = DiffFile::demo(20);
        first.path = "src/first.rs".into();
        let mut second = DiffFile::demo(30);
        second.path = "src/second.rs".into();
        let session = ReviewSession::new(SessionSource::Demo, vec![first, second].into()).unwrap();
        let (view, cx) = ready_review(cx, session);

        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });
        cx.dispatch_action(SelectNextFile);
        cx.dispatch_action(ToggleViewed);

        assert_eq!(
            cx.read(|app| with_session(&view, app, ReviewSession::selected_file_index)),
            1,
        );
        assert!(cx.read(|app| with_session(&view, app, |session| session.is_viewed(1))));
        assert_eq!(
            cx.read(|app| { view.read(app).diff_view.read(app).file.path.to_string() }),
            "src/second.rs",
        );
    }

    /// The diff takes focus as soon as the session is ready, so its keybindings
    /// work without a click.
    #[gpui::test]
    fn finishing_focuses_the_diff(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) =
            cx.add_window_view(|_window, cx| SessionView::loading("pull request #42", cx));

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
        cx.dispatch_action(SelectNextLine);

        assert_eq!(
            cx.read(|app| {
                view.read(app)
                    .review
                    .as_ref()
                    .expect("the session is ready")
                    .view
                    .read(app)
                    .diff_view
                    .read(app)
                    .selected_line
            }),
            1,
        );
    }

    fn open_composer_on(
        cx: &mut TestAppContext,
    ) -> (Entity<ReviewView>, &mut gpui::VisualTestContext) {
        let (view, cx) = ready_review(cx, repository_backed_session(&["src/review.rs"]));
        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });
        cx.dispatch_action(ToggleComment);
        (view, cx)
    }

    fn composed_text(view: &Entity<ReviewView>, cx: &mut gpui::VisualTestContext) -> String {
        cx.read(|app| {
            view.read(app)
                .diff_view
                .read(app)
                .comment_editor
                .read(app)
                .content()
                .to_owned()
        })
    }

    /// The old composer could only append and backspace, so a typo in the middle
    /// of a comment could not be fixed.
    #[gpui::test]
    fn the_caret_can_be_moved_and_text_inserted_mid_word(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = open_composer_on(cx);

        cx.simulate_input("helo world");
        // Back to just after "hel".
        for _ in 0..7 {
            cx.simulate_keystrokes("left");
        }
        cx.simulate_input("l");

        assert_eq!(composed_text(&view, cx), "hello world");
    }

    #[gpui::test]
    fn a_selection_is_replaced_by_what_is_typed(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = open_composer_on(cx);

        cx.simulate_input("hello world");
        for _ in 0..5 {
            cx.simulate_keystrokes("shift-left");
        }
        cx.simulate_input("there");

        assert_eq!(composed_text(&view, cx), "hello there");
    }

    #[gpui::test]
    fn select_all_then_typing_replaces_everything(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = open_composer_on(cx);

        cx.simulate_input("first attempt");
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("second");

        assert_eq!(composed_text(&view, cx), "second");
    }

    #[gpui::test]
    fn a_comment_can_span_several_lines(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = open_composer_on(cx);

        cx.simulate_input("first");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("second");

        assert_eq!(composed_text(&view, cx), "first\nsecond");

        // Vertical movement lands on the line above, not at the start of the text.
        cx.simulate_keystrokes("up");
        cx.simulate_input("!");
        assert_eq!(composed_text(&view, cx), "first!\nsecond");
    }

    #[gpui::test]
    fn home_and_end_move_within_the_line(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = open_composer_on(cx);

        cx.simulate_input("one");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("two");
        cx.simulate_keystrokes("home");
        cx.simulate_input("[");

        assert_eq!(composed_text(&view, cx), "one\n[two");
    }

    #[gpui::test]
    fn cut_and_paste_move_the_selection(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = open_composer_on(cx);

        cx.simulate_input("keep move");
        // Selects " move", the space included.
        for _ in 0..5 {
            cx.simulate_keystrokes("shift-left");
        }
        cx.simulate_keystrokes("cmd-x");
        assert_eq!(composed_text(&view, cx), "keep");

        cx.simulate_keystrokes("cmd-v");
        cx.simulate_keystrokes("cmd-v");
        assert_eq!(composed_text(&view, cx), "keep move move");
    }

    /// Every edit path has to reach the draft, not just plain typing.
    #[gpui::test]
    fn editing_with_the_keyboard_updates_the_stored_draft(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = open_composer_on(cx);

        cx.simulate_input("helo");
        cx.simulate_keystrokes("left");
        cx.simulate_input("l");
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_keystrokes("backspace");

        // Emptying the composer removes the draft, not just its text.
        assert_eq!(cx.read(|app| body_at(&view, app, 0, 0)), None);

        cx.simulate_input("done");

        assert_eq!(
            cx.read(|app| body_at(&view, app, 0, 0)),
            Some("done".to_owned()),
        );
    }

    /// Extending the selection and commenting produces one range draft, not
    /// several single-line ones.
    #[gpui::test]
    fn shift_navigation_builds_a_range_comment(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = ready_review(cx, repository_backed_session(&["src/review.rs"]));
        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });

        // Rows 0..=2 are context lines in the fixture's single hunk.
        cx.simulate_keystrokes("shift-j");
        cx.simulate_keystrokes("shift-j");
        assert_eq!(
            cx.read(|app| view.read(app).diff_view.read(app).selected_rows()),
            0..=2,
        );

        cx.dispatch_action(ToggleComment);
        cx.simulate_input("this block");

        let draft = cx
            .read(|app| with_session(&view, app, |session| session.draft_at(0, 2).cloned()))
            .expect("the draft is keyed at the span's last row");
        assert_eq!(draft.body, "this block");
        assert!(draft.anchor.is_multiline());
        assert_eq!(draft.anchor.start_line, Some(1));
        assert_eq!(draft.anchor.line, 3);
        assert_eq!(
            cx.read(|app| with_session(&view, app, |session| session.drafts().len())),
            1,
        );
    }

    /// Plain navigation after extending must abandon the range rather than leave it
    /// quietly attached to the next comment.
    #[gpui::test]
    fn moving_without_shift_collapses_the_selection(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = ready_review(cx, repository_backed_session(&["src/review.rs"]));
        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });

        cx.simulate_keystrokes("shift-j");
        cx.simulate_keystrokes("j");
        assert_eq!(
            cx.read(|app| view.read(app).diff_view.read(app).selected_rows()),
            2..=2,
        );

        cx.dispatch_action(ToggleComment);
        cx.simulate_input("one line");
        let multiline = cx.read(|app| {
            with_session(&view, app, |session| {
                session
                    .draft_at(0, 2)
                    .map(|draft| draft.anchor.is_multiline())
            })
        });
        assert_eq!(multiline, Some(false));
    }

    /// Closing the composer and switching files used to lose the text. It is now
    /// stored on the session, so it survives both.
    #[gpui::test]
    fn a_draft_survives_closing_the_composer_and_changing_files(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = ready_review(
            cx,
            repository_backed_session(&["src/first.rs", "src/second.rs"]),
        );
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
            cx.read(|app| body_at(&view, app, 0, 0)),
            Some("a thought".to_owned()),
        );
    }

    /// Reopening a line has to show what is already there, or the reviewer would
    /// silently start a second comment over the top of the first.
    #[gpui::test]
    fn reopening_the_composer_loads_the_existing_draft(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = open_composer_on(cx);

        cx.simulate_input("first half");
        cx.simulate_keystrokes("escape");
        cx.dispatch_action(ToggleComment);

        assert_eq!(composed_text(&view, cx), "first half");

        // Appending continues the same draft rather than starting another.
        cx.simulate_input(" and second");
        assert_eq!(
            cx.read(|app| body_at(&view, app, 0, 0)),
            Some("first half and second".to_owned()),
        );
        assert_eq!(
            cx.read(|app| with_session(&view, app, |session| session.drafts().len())),
            1,
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
        assert_eq!(cx.read(|app| view.read(app).selected_line), 0);

        // And there is no row to comment on.
        cx.dispatch_action(ToggleComment);
        assert_eq!(
            cx.read(|app| view.read(app).comment_rows.clone()),
            Some(0..=0)
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
        let source = SessionSource::LocalComparison {
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

        let (view, cx) = ready_review(cx, session);
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
            cx.read(|app| view.read(app).diff_view.read(app).selected_line),
            6,
        );
        assert_eq!(composed_text(&view, cx), "also check the edge case");
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
        let session = ReviewSession::new(SessionSource::Demo, vec![first, second].into()).unwrap();
        let (view, cx) = ready_review(cx, session);

        cx.update(|window, app| {
            let focus = view.read(app).diff_view.read(app).focus_handle.clone();
            window.focus(&focus);
        });
        cx.dispatch_action(ToggleComment);
        cx.simulate_keystrokes("cmd-shift-j");

        assert_eq!(
            cx.read(|app| with_session(&view, app, ReviewSession::selected_file_index)),
            0,
        );
        assert_eq!(
            cx.read(|app| view.read(app).diff_view.read(app).comment_rows.clone()),
            Some(0..=0),
        );

        // Dismissing the composer hands file navigation back.
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("cmd-shift-j");
        assert_eq!(
            cx.read(|app| with_session(&view, app, ReviewSession::selected_file_index)),
            1,
        );
    }
}
