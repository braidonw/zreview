#![allow(clippy::unreadable_literal)]

mod findings;
mod text;
pub mod theme;

pub use findings::ReviewRunState;
pub use text::TextBuffer;

use std::sync::{Arc, atomic::AtomicBool};

use domain::{
    ChangeCounts, CommentThread, DiffAnchor, DiffFile, DiffLine, DiffLineKind, DraftComment,
    Drafts, ExcludedDraft, FileStatus, FindingAcceptance, FindingId, FindingProvenance, Findings,
    LoadStage, LoadedSession, PlacedComments, ReviewEvent, ReviewSession, ReviewStateSink,
    ReviewSubmission, ReviewSubmitter, SessionFailure, SessionSource, SubmissionOutcome,
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
pub struct CommentEdited;

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
                        .when(highlighted, |piece| piece.bg(rgb(0x1d4ed8)))
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
            .bg(rgb(0xf8fafc))
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
                rgb(0x3b82f6)
            } else {
                rgb(0x334155)
            })
            .bg(rgb(0x111827))
            .text_color(rgb(0xe5e7eb))
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
                        .text_color(rgb(0x6b7280))
                        .child("Write a review comment…"),
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

/// What the diff view asks the session to do about drafts.
///
/// The diff view renders a read-only snapshot and never mutates the session
/// directly, so the session stays the single owner of draft state.
pub enum DiffViewEvent {
    /// The composer's text changed. `rows` is the span it covers, which is one row
    /// for an ordinary comment.
    DraftEdited {
        rows: std::ops::RangeInclusive<usize>,
        body: String,
    },
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
            cx.emit(DiffViewEvent::DraftEdited { rows, body });
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
    /// draft on that row would live — so extending a selection over an existing
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
        cx.emit(DiffViewEvent::DraftDiscarded { row });
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
        // easy to miss and invisible once the thread scrolls past — the reviewer
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
            .on_action(cx.listener(Self::extend_selection_down))
            .on_action(cx.listener(Self::extend_selection_up))
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

/// What the review view asks its owner to do.
///
/// Persistence and submission both live outside this view, so it reports rather
/// than acts — which is also what keeps `crates/ui` free of a database and a
/// forge.
pub enum ReviewViewEvent {
    /// A draft was written or removed at an anchor.
    DraftChanged {
        anchor: DiffAnchor,
        /// `None` when the draft was removed.
        draft: Option<DraftComment>,
    },
    /// The review summary changed.
    SummaryChanged { body: String },
    /// The reviewer asked to submit. Nothing is posted until they confirm.
    SubmitRequested { event: ReviewEvent },
    /// The reviewer asked for an automated review. Whoever owns the backend runs
    /// it; this view only knows it was asked for.
    ReviewRequested,
    /// A finding became a draft, and its provenance needs persisting alongside it.
    FindingAccepted {
        anchor: DiffAnchor,
        body: String,
        provenance: FindingProvenance,
    },
    /// A claim was rejected, and the decision needs remembering.
    FindingDismissed { fingerprint: String },
}

pub struct ReviewView {
    session: ReviewSession,
    diff_view: Entity<DiffView>,
    summary_editor: Entity<CommentEditor>,
    file_list_state: ListState,
    /// How far the current review run has got.
    run: ReviewRunState,
    /// Which finding the reviewer is looking at.
    selected_finding: Option<FindingId>,
    /// Whether the guidance section is open.
    ///
    /// Open before the first run, because PLAN wants what will be sent seen before
    /// it is sent; collapsed afterwards, when the findings are what matters.
    guidance_expanded: bool,
    /// Held so draft edits keep reaching the session.
    _diff_subscription: Subscription,
    /// Held so summary edits keep reaching the session.
    _summary_subscription: Subscription,
}

impl EventEmitter<ReviewViewEvent> for ReviewView {}

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
        let diff_subscription = cx.subscribe(&diff_view, Self::on_diff_event);

        // The summary reuses the composer, so it inherits the same
        // keybinding isolation the inline editor needed.
        let existing_summary = session.summary().to_owned();
        let summary_editor = cx.new(|cx| CommentEditor::with_content(existing_summary, cx));
        let summary_subscription =
            cx.subscribe(&summary_editor, |this, editor, _: &CommentEdited, cx| {
                let body = editor.read(cx).content().to_owned();
                this.session.set_summary(body.clone());
                cx.emit(ReviewViewEvent::SummaryChanged { body });
                cx.notify();
            });

        Self {
            session,
            diff_view,
            summary_editor,
            file_list_state: ListState::new(file_count, ListAlignment::Top, px(36.0)),
            run: ReviewRunState::default(),
            selected_finding: None,
            guidance_expanded: true,
            _diff_subscription: diff_subscription,
            _summary_subscription: summary_subscription,
        }
    }

    /// Records that a review run has started, with the flag that stops it.
    pub fn review_started(&mut self, cancel: Arc<AtomicBool>, cx: &mut Context<Self>) {
        self.run = ReviewRunState::Running {
            detail: SharedString::from("Starting…"),
            cancel,
        };
        cx.notify();
    }

    /// Publishes the backend's latest progress line.
    ///
    /// Ignored unless a run is in flight, so a late report cannot make a finished
    /// review look like it is still going.
    pub fn review_progress(&mut self, line: impl Into<SharedString>, cx: &mut Context<Self>) {
        if let ReviewRunState::Running { detail, .. } = &mut self.run {
            let line = line.into();
            if *detail != line {
                *detail = line;
                cx.notify();
            }
        }
    }

    /// Takes the findings a completed run produced.
    ///
    /// `unreviewed` names files the run did not see — excluded, or too large to fit
    /// in the material. They are carried into the panel so a partial review cannot
    /// present itself as a complete one.
    pub fn review_finished(
        &mut self,
        findings: Findings,
        unreviewed: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let rejected = findings.rejected().len();
        let suppressed = self.session.set_findings(findings);
        let accepted = self.session.findings().len();
        self.selected_finding = self.session.findings().accepted().first().map(|f| f.id);
        self.run = ReviewRunState::Complete {
            accepted,
            rejected,
            suppressed,
            unreviewed: unreviewed.into_iter().map(SharedString::from).collect(),
        };
        // The disclosure has served its purpose; the findings are what the reviewer
        // wants the space for now. The summary line stays visible either way.
        self.guidance_expanded = false;
        cx.notify();
    }

    /// Reports a run that produced nothing, with what to do about it.
    pub fn review_failed(
        &mut self,
        summary: impl Into<SharedString>,
        remediation: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.run = ReviewRunState::Failed {
            summary: summary.into(),
            remediation: remediation.map(SharedString::from),
        };
        cx.notify();
    }

    /// Opens or closes the guidance section.
    pub fn toggle_guidance_panel(&mut self, cx: &mut Context<Self>) {
        self.guidance_expanded = !self.guidance_expanded;
        cx.notify();
    }

    #[must_use]
    pub const fn guidance_expanded(&self) -> bool {
        self.guidance_expanded
    }

    /// Turns one guidance file on or off for the next run.
    ///
    /// Not persisted: it is a decision about this sitting, and a choice that
    /// silently outlived the session would be a worse surprise than re-making it.
    pub fn toggle_guidance(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.session.toggle_guidance(path).is_some() {
            cx.notify();
        }
    }

    /// Asks the running review to stop.
    pub fn cancel_review(&mut self, cx: &mut Context<Self>) {
        self.run.cancel();
        cx.notify();
    }

    #[must_use]
    pub const fn review_run(&self) -> &ReviewRunState {
        &self.run
    }

    #[must_use]
    pub const fn selected_finding(&self) -> Option<FindingId> {
        self.selected_finding
    }

    /// Scrolls the diff to a finding's line and selects it in the panel.
    pub fn reveal_finding(&mut self, id: FindingId, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_finding = Some(id);
        let Some(location) = self
            .session
            .findings()
            .get(id)
            .and_then(|finding| finding.location)
        else {
            // A finding about the whole change has nowhere to scroll to.
            cx.notify();
            return;
        };
        self.select_file_at(location.file, cx);
        self.diff_view
            .update(cx, |view, cx| view.reveal_row(location.row, window, cx));
        cx.notify();
    }

    fn run_review(&mut self, _: &RunReview, _: &mut Window, cx: &mut Context<Self>) {
        if self.run.is_running() {
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
        let findings = self.session.findings();
        if findings.is_empty() {
            return;
        }
        let next = self
            .selected_finding
            .and_then(|current| {
                let position = findings
                    .accepted()
                    .iter()
                    .position(|finding| finding.id == current)?;
                findings.accepted().get(position + 1)
            })
            .or_else(|| findings.accepted().first())
            .map(|finding| finding.id);
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
        if let Some(id) = self.selected_finding {
            self.accept_finding_by_id(id, window, cx);
        }
    }

    fn dismiss_selected_finding(
        &mut self,
        _: &DismissFinding,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.selected_finding {
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
        match self.session.accept_finding(id) {
            FindingAcceptance::Drafted { anchor, body } => {
                let provenance = self
                    .session
                    .drafts()
                    .get(&anchor)
                    .and_then(|draft| draft.provenance.clone());
                self.after_findings_changed(cx);
                if let Some(provenance) = provenance {
                    cx.emit(ReviewViewEvent::FindingAccepted {
                        anchor,
                        body,
                        provenance,
                    });
                }
            }
            FindingAcceptance::Occupied {
                location,
                existing,
                proposed,
                ..
            } => {
                // The reviewer already wrote something here. Both texts go into the
                // composer and they decide; nothing is saved until they do.
                self.selected_finding = Some(id);
                self.select_file_at(location.file, cx);
                let merged = format!("{}\n\n{proposed}", existing.trim_end());
                self.diff_view.update(cx, |view, cx| {
                    view.open_composer_with(location.row, merged, window, cx);
                });
                cx.notify();
            }
            FindingAcceptance::NotInline { proposed } => {
                // Nowhere to anchor it, so it belongs in the review summary — which
                // the reviewer can still edit or delete before submitting.
                let existing = self.session.summary().trim_end().to_owned();
                let merged = if existing.is_empty() {
                    proposed
                } else {
                    format!("{existing}\n\n{proposed}")
                };
                self.session.set_summary(merged.clone());
                self.summary_editor
                    .update(cx, |editor, cx| editor.load(merged.clone(), cx));
                self.session.retire_finding(id);
                self.after_findings_changed(cx);
                cx.emit(ReviewViewEvent::SummaryChanged { body: merged });
            }
            FindingAcceptance::Unknown => {}
        }
    }

    pub fn dismiss_finding_by_id(&mut self, id: FindingId, cx: &mut Context<Self>) {
        if let Some(fingerprint) = self.session.dismiss_finding(id) {
            self.after_findings_changed(cx);
            cx.emit(ReviewViewEvent::FindingDismissed { fingerprint });
        }
    }

    /// Republishes drafts for rendering and moves the selection off a finding that
    /// has been acted on.
    fn after_findings_changed(&mut self, cx: &mut Context<Self>) {
        let drafts = self.session.shared_drafts();
        self.diff_view
            .update(cx, |view, cx| view.set_drafts(drafts, cx));
        self.selected_finding = self
            .session
            .findings()
            .accepted()
            .first()
            .map(|finding| finding.id);
        cx.notify();
    }

    /// Forgets what a forge accepted, and reports the anchors it consumed.
    pub fn mark_submitted(
        &mut self,
        submission: &ReviewSubmission,
        cx: &mut Context<Self>,
    ) -> Vec<DiffAnchor> {
        let anchors = submission.submitted_anchors();
        self.session.mark_submitted(submission);
        self.summary_editor
            .update(cx, |editor, cx| editor.load(String::new(), cx));
        let drafts = self.session.shared_drafts();
        self.diff_view
            .update(cx, |view, cx| view.set_drafts(drafts, cx));
        cx.notify();
        anchors
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
                cx.emit(ReviewViewEvent::DraftChanged {
                    draft: None,
                    anchor: moved.vacated,
                });
                cx.emit(ReviewViewEvent::DraftChanged {
                    draft,
                    anchor: moved.anchored,
                });
                cx.notify();
            }
            return;
        }

        let rows = match event {
            DiffViewEvent::DraftEdited { rows, .. } => rows.clone(),
            DiffViewEvent::DraftDiscarded { row } => *row..=*row,
            DiffViewEvent::DraftReanchored { .. } => return,
        };
        // The anchor is read before the change so a discarded draft still reports
        // which position it was removed from. It covers the whole span, so a range
        // is persisted and cleared as one comment.
        let Some(anchor) = self.session.anchor_for_span(file, rows.clone()) else {
            return;
        };

        match event {
            DiffViewEvent::DraftEdited { body, .. } => {
                self.session.set_draft_over(file, rows, body.clone());
            }
            DiffViewEvent::DraftDiscarded { .. } | DiffViewEvent::DraftReanchored { .. } => {
                self.session.clear_draft(file, *rows.end());
            }
        }

        let drafts = self.session.shared_drafts();
        let draft = drafts.get(&anchor).cloned();
        self.diff_view
            .update(cx, |view, cx| view.set_drafts(drafts, cx));
        cx.emit(ReviewViewEvent::DraftChanged { draft, anchor });
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

    /// Switches the displayed file without moving focus.
    ///
    /// Focus belongs where the reviewer put it: clicking a finding in the panel
    /// should not yank the keyboard into the diff.
    fn select_file_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.session.select_file(index) {
            let file = Arc::new(self.session.selected_file().clone());
            self.diff_view
                .update(cx, |view, cx| view.set_file(file, index, cx));
            self.file_list_state.scroll_to_reveal_item(index);
        }
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

    /// The summary field and the three ways to submit.
    ///
    /// Each event is its own button rather than a menu, so choosing to approve is
    /// as deliberate as choosing to request changes.
    fn render_submit_bar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let drafts = self.session.drafts();
        let stale = drafts.stale_count();
        let ready = drafts.len() - stale;
        let can_submit = self.session.source().head_sha().is_some();
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
            .border_color(rgb(0x1e293b))
            .bg(rgb(0x0f172a))
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
                            .text_color(rgb(0xf8fafc))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("{ready} to submit")),
                    )
                    .when(stale > 0, |counts| {
                        counts.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xf87171))
                                .child(format!("{stale} not anchored")),
                        )
                    }),
            )
            .child(div().flex_1().min_w_0().child(self.summary_editor.clone()))
            .children(can_submit.then(|| {
                div().flex_shrink_0().flex().gap_2().children(
                    [
                        (ReviewEvent::Comment, rgb(0x2563eb)),
                        (ReviewEvent::Approve, rgb(0x15803d)),
                        (ReviewEvent::RequestChanges, rgb(0xb91c1c)),
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
                            .text_color(rgb(0xffffff))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                                cx.stop_propagation();
                                view.update(cx, |_review, cx| {
                                    cx.emit(ReviewViewEvent::SubmitRequested { event });
                                });
                            })
                            .child(event.label())
                    }),
                )
            }))
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
        // Cloned before the file list takes ownership of its own copy.
        let panel_view = review_view.clone();

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
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().flex_1().min_h_0().child(self.diff_view.clone()))
                    .child(self.render_submit_bar(cx)),
            )
            // Only takes space once there is something to say.
            .children(findings::is_visible(&self.session, &self.run).then(|| {
                findings::render(
                    &self.session,
                    &self.run,
                    self.selected_finding,
                    self.guidance_expanded,
                    &panel_view,
                )
            }))
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

/// How far a submission has got.
///
/// `Confirming` exists because nothing may be posted without an explicit human
/// action. It holds the exact request that will be sent, so what the reviewer
/// approves is what leaves the machine — not a re-derivation of it.
enum SubmissionState {
    Idle,
    Confirming(Box<ReviewSubmission>),
    Sending,
    Sent(SubmissionOutcome),
    Failed(SessionFailure),
}

/// How a review run is started.
///
/// Installed by whoever owns a backend, after the window exists — a view cannot
/// hold a handle to the window it lives in at construction time. Absent until then,
/// and absent for good in a build with no backend, in which case asking for a review
/// does nothing rather than failing.
pub type ReviewLauncher = Box<dyn Fn(&Entity<ReviewView>, ReviewSession, &mut App)>;

pub struct SessionView {
    state: SessionState,
    /// Where draft changes are written, once the session is ready.
    review_sink: Option<Box<dyn ReviewStateSink>>,
    /// Where a confirmed review is posted. Absent when the session is not a pull
    /// request, in which case submitting is not offered at all.
    submitter: Option<Arc<dyn ReviewSubmitter>>,
    submission: SubmissionState,
    /// How to start a review, once something has said how.
    review_launcher: Option<ReviewLauncher>,
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
            review_sink: None,
            submitter: None,
            submission: SubmissionState::Idle,
            review_launcher: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Says how to run a review. Without this, asking for one does nothing.
    pub fn set_review_launcher(&mut self, launcher: ReviewLauncher) {
        self.review_launcher = Some(launcher);
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
                self.review_sink = loaded.review_sink;
                self.submitter = loaded.submitter;
                let review = cx.new(|cx| ReviewView::new(loaded.session, window, cx));
                let subscription = cx.subscribe(&review, |this, review, event, cx| {
                    this.on_review_event(&review, event, cx);
                });
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
    fn on_review_event(
        &mut self,
        review: &Entity<ReviewView>,
        event: &ReviewViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ReviewViewEvent::DraftChanged { anchor, draft } => {
                if let Some(sink) = &self.review_sink {
                    match draft {
                        Some(draft) => sink.save(anchor, &draft.body),
                        None => sink.discard(anchor),
                    }
                }
            }
            ReviewViewEvent::SummaryChanged { body } => {
                if let Some(sink) = &self.review_sink
                    && let Some(head_sha) = review.read(cx).session().source().head_sha()
                {
                    sink.save_summary(head_sha, body);
                }
            }
            ReviewViewEvent::SubmitRequested { event } => self.begin_confirmation(*event, cx),
            ReviewViewEvent::FindingAccepted {
                anchor,
                body,
                provenance,
            } => {
                if let Some(sink) = &self.review_sink {
                    // Text first, then where it came from: an attribution without
                    // the comment it belongs to is worth nothing.
                    sink.save(anchor, body);
                    sink.save_provenance(anchor, provenance);
                }
            }
            ReviewViewEvent::FindingDismissed { fingerprint } => {
                if let Some(sink) = &self.review_sink
                    && let Some(head_sha) = review.read(cx).session().source().head_sha()
                {
                    sink.dismiss_finding(head_sha, fingerprint);
                }
            }
            ReviewViewEvent::ReviewRequested => {
                if let Some(launcher) = self.review_launcher.take() {
                    let session = review.read(cx).session().clone();
                    launcher(review, session, cx);
                    self.review_launcher = Some(launcher);
                }
            }
        }
        // Renders the alarm if writing has started failing.
        cx.notify();
    }

    /// The loaded review, once there is one.
    fn review(&self) -> Option<&Entity<ReviewView>> {
        match &self.state {
            SessionState::Ready { review, .. } => Some(review),
            _ => None,
        }
    }

    /// Assembles what submitting would post and shows it for approval.
    ///
    /// Deliberately stops here. PLAN requires that nothing is ever posted without
    /// an explicit human submission action, so building the request and sending it
    /// are two separate steps with a person in between.
    fn begin_confirmation(&mut self, event: ReviewEvent, cx: &mut Context<Self>) {
        let Some(review) = self.review().cloned() else {
            return;
        };
        self.submission = match review.read(cx).session().prepare_submission(event) {
            Ok(submission) => SubmissionState::Confirming(Box::new(submission)),
            Err(refused) => SubmissionState::Failed(
                SessionFailure::new("This review cannot be submitted yet")
                    .with_remediation(refused.to_string()),
            ),
        };
        cx.notify();
    }

    fn cancel_submission(&mut self, cx: &mut Context<Self>) {
        self.submission = SubmissionState::Idle;
        cx.notify();
    }

    /// Posts the confirmed review.
    ///
    /// The request is sent on a background thread because it is network I/O, and
    /// local drafts are forgotten only after the forge has accepted it — until
    /// then the local copy is the only copy.
    fn send_confirmed(&mut self, cx: &mut Context<Self>) {
        let (SubmissionState::Confirming(submission), Some(submitter), Some(review)) = (
            &self.submission,
            self.submitter.clone(),
            self.review().cloned(),
        ) else {
            return;
        };
        let submission = submission.clone();
        self.submission = SubmissionState::Sending;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let posted = {
                let submission = submission.clone();
                cx.background_executor()
                    .spawn(async move { submitter.submit(&submission) })
                    .await
            };

            this.update(cx, |this, cx| {
                this.submission = match posted {
                    Ok(outcome) => {
                        // Only now is it safe to forget them.
                        let anchors =
                            review.update(cx, |review, cx| review.mark_submitted(&submission, cx));
                        if let (Some(sink), Some(head_sha)) = (
                            this.review_sink.as_ref(),
                            review.read(cx).session().source().head_sha(),
                        ) {
                            sink.clear_submitted(head_sha, &anchors);
                        }
                        SubmissionState::Sent(outcome)
                    }
                    // Nothing local was touched, so every draft is still there.
                    Err(failure) => SubmissionState::Failed(failure),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The review awaiting confirmation, if one is.
    #[must_use]
    pub fn confirming(&self) -> Option<&ReviewSubmission> {
        match &self.submission {
            SubmissionState::Confirming(submission) => Some(submission),
            _ => None,
        }
    }

    /// The outcome of a review the forge accepted.
    #[must_use]
    pub fn submitted(&self) -> Option<&SubmissionOutcome> {
        match &self.submission {
            SubmissionState::Sent(outcome) => Some(outcome),
            _ => None,
        }
    }

    /// Why submitting failed or was refused.
    #[must_use]
    pub fn submission_failure(&self) -> Option<&SessionFailure> {
        match &self.submission {
            SubmissionState::Failed(failure) => Some(failure),
            _ => None,
        }
    }

    /// The reason drafts are not reaching storage, if they are not.
    #[must_use]
    pub fn draft_write_failure(&self) -> Option<String> {
        self.review_sink.as_ref().and_then(|sink| sink.failure())
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
                .border_color(rgb(0x1e293b))
                .bg(rgb(0x0b1220))
                .font_family("SF Mono")
                .text_size(px(13.0))
        };

        match &self.submission {
            SubmissionState::Idle => None,

            SubmissionState::Sending => Some(
                panel()
                    .child(
                        div()
                            .text_color(rgb(0x93c5fd))
                            .child("Submitting the review…"),
                    )
                    .into_any(),
            ),

            SubmissionState::Sent(outcome) => Some(
                panel()
                    .child(
                        div()
                            .text_color(rgb(0x4ade80))
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
                            .text_color(rgb(0x94a3b8))
                            .child(SharedString::from(outcome.url.clone())),
                    )
                    .into_any(),
            ),

            SubmissionState::Failed(failure) => Some(
                panel()
                    .child(
                        div()
                            .text_color(rgb(0xf87171))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(SharedString::from(failure.summary.clone())),
                    )
                    .children(failure.remediation.as_ref().map(|remediation| {
                        div()
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
                    .text_color(rgb(0xf8fafc))
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
                    .text_color(rgb(0x64748b))
                    .child(format!("pinned to {}", short_sha(&submission.head_sha))),
            )
            .children((!submission.body.is_empty()).then(|| {
                div()
                    .p_2()
                    .rounded_md()
                    .bg(rgb(0x131c31))
                    .text_color(rgb(0xcbd5e1))
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
                    .border_color(rgb(0x2563eb))
                    .bg(rgb(0x131c31))
                    .child(div().text_xs().text_color(rgb(0x94a3b8)).child(format!(
                        "{} {} line {}",
                        comment.path, comment.side, comment.line,
                    )))
                    .child(
                        div()
                            .text_color(rgb(0xe5e7eb))
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
                    .child(div().text_color(rgb(0xf87171)).child(format!(
                        "{} draft{} will NOT be posted",
                        submission.excluded.len(),
                        if submission.excluded.len() == 1 {
                            ""
                        } else {
                            "s"
                        },
                    )))
                    .children(
                        submission
                            .excluded
                            .iter()
                            .map(|ExcludedDraft { draft, reason }| {
                                div().text_xs().text_color(rgb(0x94a3b8)).child(format!(
                                    "{} line {} — {reason}: {}",
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
                            .bg(rgb(0x2563eb))
                            .text_color(rgb(0xffffff))
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
                            .border_color(rgb(0x475569))
                            .text_color(rgb(0xcbd5e1))
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                .children(self.render_submission_banner(cx))
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
            cx.read(|app| view.read(app).comment_editor.read(app).content().to_owned()),
            "jerky code",
        );
        // Typing must not have navigated the diff or dismissed the composer.
        assert_eq!(cx.read(|app| view.read(app).selected_line()), 0);
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
        assert_eq!(cx.read(|app| view.read(app).selected_line()), 0);

        cx.simulate_keystrokes("escape");
        assert_eq!(cx.read(|app| view.read(app).comment_rows.clone()), None);

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

    impl domain::ReviewStateSink for RecordingSink {
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

        fn save_summary(&self, _head_sha: &str, body: &str) {
            self.calls.lock().unwrap().push(format!("summary {body}"));
        }

        fn save_provenance(&self, anchor: &DiffAnchor, provenance: &domain::FindingProvenance) {
            self.calls.lock().unwrap().push(format!(
                "provenance {} {} {}",
                anchor.path, anchor.line, provenance.origin
            ));
        }

        fn dismiss_finding(&self, _head_sha: &str, fingerprint: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("dismiss {fingerprint}"));
        }

        fn clear_submitted(&self, _head_sha: &str, anchors: &[DiffAnchor]) {
            let positions = anchors
                .iter()
                .map(|anchor| format!("{} {}", anchor.path, anchor.line))
                .collect::<Vec<_>>()
                .join(", ");
            self.calls
                .lock()
                .unwrap()
                .push(format!("clear submitted [{positions}]"));
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
        sink: Option<Box<dyn domain::ReviewStateSink>>,
    ) -> (Entity<SessionView>, &mut gpui::VisualTestContext) {
        let (view, cx) = cx.add_window_view(|_window, cx| SessionView::loading("a review", cx));
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.finish(
                    Ok(LoadedSession {
                        session: repository_backed_session(&["src/review.rs"]),
                        review_sink: sink,
                        submitter: None,
                    }),
                    window,
                    cx,
                );
            });
        });
        (view, cx)
    }

    /// Puts one finding on the review, anchored to the row a comment can go on.
    fn give_one_finding(
        review: &Entity<ReviewView>,
        cx: &mut gpui::VisualTestContext,
        title: &str,
    ) -> FindingId {
        cx.update(|_window, app| {
            review.update(app, |view, cx| {
                let anchor = view
                    .session()
                    .anchor_for(0, 1)
                    .expect("row 1 can carry a comment");
                let raw = domain::RawFinding {
                    location: Some(domain::RawLocation {
                        path: anchor.path.clone(),
                        side: anchor.side,
                        line: anchor.line,
                        start_line: None,
                    }),
                    severity: domain::Severity::Warning,
                    confidence: 0.8,
                    title: title.to_owned(),
                    rationale: "because".to_owned(),
                    proposed_comment: "Handle the failure here.".to_owned(),
                    guidance_sources: vec![domain::GuidanceCitation {
                        path: "AGENTS.md".into(),
                        content_hash: "hash".into(),
                    }],
                };
                let anchors = view.session().anchors().expect("anchored").clone();
                let findings = Findings::validate(
                    vec![raw],
                    &anchors,
                    &domain::FindingOrigin::Ai("claude-code".into()),
                );
                let id = findings.accepted()[0].id;
                view.review_finished(findings, Vec::new(), cx);
                id
            })
        })
    }

    #[gpui::test]
    fn accepting_a_finding_writes_the_draft_and_its_provenance_to_the_sink(
        cx: &mut TestAppContext,
    ) {
        cx.update(init);
        let sink = RecordingSink::default();
        let (view, cx) = ready_session_view(cx, Some(Box::new(sink.clone())));
        let review = cx.update(|_window, app| review_of(&view, app));
        let id = give_one_finding(&review, cx, "unchecked index");

        cx.update(|window, app| {
            review.update(app, |view, cx| {
                view.accept_finding_by_id(id, window, cx);
            });
        });

        // The text is written before the note about where it came from.
        assert_eq!(
            sink.calls(),
            [
                "save src/review.rs 2 Handle the failure here.".to_owned(),
                "provenance src/review.rs 2 claude-code".to_owned(),
            ]
        );
        cx.update(|_window, app| {
            let view = review.read(app);
            assert!(view.session().findings().is_empty(), "acted on");
            let draft = view.session().draft_at(0, 1).expect("the draft is there");
            assert!(draft.is_proposed());
        });
    }

    #[gpui::test]
    fn dismissing_a_finding_records_the_decision(cx: &mut TestAppContext) {
        cx.update(init);
        let sink = RecordingSink::default();
        let (view, cx) = ready_session_view(cx, Some(Box::new(sink.clone())));
        let review = cx.update(|_window, app| review_of(&view, app));
        let id = give_one_finding(&review, cx, "unchecked index");

        cx.update(|_window, app| {
            review.update(app, |view, cx| view.dismiss_finding_by_id(id, cx));
        });

        assert_eq!(sink.calls().len(), 1);
        assert!(sink.calls()[0].starts_with("dismiss "));
        cx.update(|_window, app| {
            assert!(review.read(app).session().findings().is_empty());
            assert!(review.read(app).session().drafts().is_empty());
        });
    }

    /// The reviewer's own words are never overwritten; both texts go to the
    /// composer instead, and nothing is saved until they commit.
    #[gpui::test]
    fn accepting_onto_an_occupied_line_opens_the_composer_pre_filled(cx: &mut TestAppContext) {
        cx.update(init);
        let sink = RecordingSink::default();
        let (view, cx) = ready_session_view(cx, Some(Box::new(sink.clone())));
        let review = cx.update(|_window, app| review_of(&view, app));

        // The reviewer writes on the line first.
        cx.dispatch_action(SelectNextLine);
        cx.dispatch_action(ToggleComment);
        cx.simulate_input("mine");
        cx.dispatch_action(CloseComment);
        let id = give_one_finding(&review, cx, "unchecked index");
        let before = sink.calls().len();

        cx.update(|window, app| {
            review.update(app, |view, cx| {
                view.accept_finding_by_id(id, window, cx);
            });
        });

        cx.update(|_window, app| {
            let view = review.read(app);
            // Untouched, and the finding is still waiting.
            assert_eq!(
                view.session().draft_at(0, 1).map(|d| d.body.as_str()),
                Some("mine")
            );
            assert_eq!(view.session().findings().len(), 1);
            // The composer holds both texts for the reviewer to reconcile.
            let composer = view.diff_view.read(app).comment_editor.read(app);
            assert_eq!(composer.content(), "mine\n\nHandle the failure here.");
        });
        assert_eq!(sink.calls().len(), before, "nothing was written");
    }

    #[gpui::test]
    fn a_finding_about_the_whole_change_goes_into_the_summary(cx: &mut TestAppContext) {
        cx.update(init);
        let sink = RecordingSink::default();
        let (view, cx) = ready_session_view(cx, Some(Box::new(sink.clone())));
        let review = cx.update(|_window, app| review_of(&view, app));

        let id = cx.update(|_window, app| {
            review.update(app, |view, cx| {
                let raw = domain::RawFinding {
                    location: None,
                    severity: domain::Severity::Info,
                    confidence: 0.5,
                    title: "no tests".to_owned(),
                    rationale: String::new(),
                    proposed_comment: "Consider adding a test.".to_owned(),
                    guidance_sources: Vec::new(),
                };
                let anchors = view.session().anchors().expect("anchored").clone();
                let findings = Findings::validate(
                    vec![raw],
                    &anchors,
                    &domain::FindingOrigin::Ai("claude-code".into()),
                );
                let id = findings.accepted()[0].id;
                view.review_finished(findings, Vec::new(), cx);
                id
            })
        });

        cx.update(|window, app| {
            review.update(app, |view, cx| view.accept_finding_by_id(id, window, cx));
        });

        cx.update(|_window, app| {
            let view = review.read(app);
            assert_eq!(view.session().summary(), "Consider adding a test.");
            assert!(view.session().drafts().is_empty(), "nowhere to anchor it");
            assert!(view.session().findings().is_empty());
        });
        assert!(sink.calls().iter().any(|call| call.starts_with("summary ")));
    }

    fn guidance_selection() -> domain::GuidanceSelection {
        let entry = |path: &str, content: &str| domain::GuidanceEntry {
            excerpt: domain::GuidanceExcerpt {
                path: path.into(),
                scope: "whole repository".into(),
                content: content.to_owned(),
                content_hash: "hash".into(),
            },
            included: true,
        };
        domain::GuidanceSelection::new(
            vec![
                entry("AGENTS.md", &"a".repeat(2048)),
                entry("CLAUDE.md", "b"),
            ],
            vec![domain::GuidanceSkip {
                path: "HUGE.md".into(),
                reason: "90000 bytes, over the 65536-byte limit".into(),
            }],
            vec!["vendor/lib.rs".into()],
        )
    }

    /// The panel is the disclosure notice, so turning a file off has to change what
    /// a run would send — not just how the row is drawn.
    #[gpui::test]
    fn toggling_guidance_changes_what_would_be_sent(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = ready_session_view(cx, None);
        let review = cx.update(|_window, app| review_of(&view, app));

        cx.update(|_window, app| {
            review.update(app, |view, _cx| {
                view.session.set_guidance(guidance_selection());
            });
        });
        cx.update(|_window, app| {
            let guidance = review.read(app).session().guidance();
            assert_eq!(guidance.included_count(), 2);
            assert_eq!(guidance.included_bytes(), 2049);
        });

        cx.update(|_window, app| {
            review.update(app, |view, cx| view.toggle_guidance("AGENTS.md", cx));
        });

        cx.update(|_window, app| {
            let guidance = review.read(app).session().guidance();
            assert_eq!(guidance.included_count(), 1);
            assert_eq!(guidance.included_bytes(), 1);
            let sent: Vec<_> = guidance
                .included()
                .map(|excerpt| excerpt.path.to_string())
                .collect();
            assert_eq!(sent, vec!["CLAUDE.md".to_owned()]);
        });
    }

    #[gpui::test]
    fn the_guidance_section_starts_open_and_collapses_once_a_run_finishes(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = ready_session_view(cx, None);
        let review = cx.update(|_window, app| review_of(&view, app));

        // Open before a run: PLAN wants what will be sent seen before it is sent.
        cx.update(|_window, app| assert!(review.read(app).guidance_expanded()));

        cx.update(|_window, app| {
            review.update(app, |view, cx| {
                view.review_finished(Findings::default(), Vec::new(), cx);
            });
        });

        cx.update(|_window, app| assert!(!review.read(app).guidance_expanded()));

        // And it can be reopened.
        cx.update(|_window, app| {
            review.update(app, ReviewView::toggle_guidance_panel);
        });
        cx.update(|_window, app| assert!(review.read(app).guidance_expanded()));
    }

    /// The panel carries the only Review button, so it must not depend on there
    /// being something to show yet.
    #[gpui::test]
    fn the_panel_is_reachable_whenever_a_review_is_possible(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = ready_session_view(cx, None);
        let review = cx.update(|_window, app| review_of(&view, app));

        cx.update(|_window, app| {
            let view = review.read(app);
            // A repository-backed snapshot can be reviewed, so the panel — and the
            // only Review button — must be reachable before anything is discovered.
            assert!(findings::is_visible(view.session(), view.review_run()));
        });

        cx.update(|_window, app| {
            review.update(app, |view, cx| {
                view.session.set_guidance(guidance_selection());
                cx.notify();
            });
        });

        cx.update(|_window, app| {
            let view = review.read(app);
            assert!(findings::is_visible(view.session(), view.review_run()));
        });
    }

    /// The fixture has no commit, so there is nothing to review and no panel.
    #[gpui::test]
    fn the_generated_fixture_offers_no_review(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = cx.add_window_view(|_window, cx| SessionView::loading("demo", cx));
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.finish(
                    Ok(LoadedSession {
                        session: ReviewSession::new(
                            SessionSource::Demo,
                            vec![DiffFile::demo(8)].into(),
                        )
                        .expect("the fixture has files"),
                        review_sink: None,
                        submitter: None,
                    }),
                    window,
                    cx,
                );
            });
        });
        let review = cx.update(|_window, app| review_of(&view, app));

        cx.update(|_window, app| {
            let view = review.read(app);
            assert!(!findings::is_visible(view.session(), view.review_run()));
        });
    }

    #[gpui::test]
    fn cancelling_sets_the_flag_the_backend_polls(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = ready_session_view(cx, None);
        let review = cx.update(|_window, app| review_of(&view, app));
        let cancel = Arc::new(AtomicBool::new(false));

        cx.update(|_window, app| {
            review.update(app, |view, cx| {
                view.review_started(Arc::clone(&cancel), cx);
                assert!(view.review_run().is_running());
                view.cancel_review(cx);
            });
        });

        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[gpui::test]
    fn a_failed_run_keeps_its_remediation(cx: &mut TestAppContext) {
        cx.update(init);
        let (view, cx) = ready_session_view(cx, None);
        let review = cx.update(|_window, app| review_of(&view, app));

        cx.update(|_window, app| {
            review.update(app, |view, cx| {
                view.review_failed(
                    "claude is not installed",
                    Some("Install it.".to_owned()),
                    cx,
                );
            });
        });

        cx.update(|_window, app| {
            let ReviewRunState::Failed {
                summary,
                remediation,
            } = review.read(app).review_run()
            else {
                panic!("the run should have failed");
            };
            assert_eq!(summary.as_ref(), "claude is not installed");
            assert_eq!(
                remediation.as_ref().map(SharedString::as_ref),
                Some("Install it.")
            );
        });
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

    fn open_composer_on(
        cx: &mut TestAppContext,
    ) -> (Entity<ReviewView>, &mut gpui::VisualTestContext) {
        let session = repository_backed_session(&["src/review.rs"]);
        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));
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
        cx.simulate_input("done");

        assert_eq!(
            cx.read(|app| view
                .read(app)
                .session()
                .draft_at(0, 0)
                .map(|d| d.body.clone())),
            Some("done".to_owned()),
        );
    }

    /// Extending the selection and commenting produces one range draft, not
    /// several single-line ones.
    #[gpui::test]
    fn shift_navigation_builds_a_range_comment(cx: &mut TestAppContext) {
        cx.update(init);
        let session = repository_backed_session(&["src/review.rs"]);
        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));
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
            .read(|app| view.read(app).session().draft_at(0, 2).cloned())
            .expect("the draft is keyed at the span's last row");
        assert_eq!(draft.body, "this block");
        assert!(draft.anchor.is_multiline());
        assert_eq!(draft.anchor.start_line, Some(1));
        assert_eq!(draft.anchor.line, 3);
        assert_eq!(cx.read(|app| view.read(app).session().drafts().len()), 1);
    }

    /// Plain navigation after extending must abandon the range rather than leave it
    /// quietly attached to the next comment.
    #[gpui::test]
    fn moving_without_shift_collapses_the_selection(cx: &mut TestAppContext) {
        cx.update(init);
        let session = repository_backed_session(&["src/review.rs"]);
        let (view, cx) = cx.add_window_view(|window, cx| ReviewView::new(session, window, cx));
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
        let draft = cx
            .read(|app| view.read(app).session().draft_at(0, 2).cloned())
            .unwrap();
        assert!(!draft.anchor.is_multiline());
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

    /// Records every submission attempt, so a test can prove one did not happen.
    #[derive(Clone, Default)]
    struct RecordingSubmitter {
        posted: Arc<std::sync::Mutex<Vec<ReviewSubmission>>>,
        failure: Option<SessionFailure>,
    }

    impl RecordingSubmitter {
        fn posted(&self) -> Vec<ReviewSubmission> {
            self.posted.lock().unwrap().clone()
        }
    }

    impl ReviewSubmitter for RecordingSubmitter {
        fn submit(
            &self,
            submission: &ReviewSubmission,
        ) -> Result<SubmissionOutcome, SessionFailure> {
            self.posted.lock().unwrap().push(submission.clone());
            self.failure.clone().map_or_else(
                || {
                    Ok(SubmissionOutcome {
                        state: "COMMENTED".to_owned(),
                        url: "https://github.com/acme/widgets/pull/42".to_owned(),
                        comment_count: submission.comments.len(),
                    })
                },
                Err,
            )
        }
    }

    fn submittable_session() -> ReviewSession {
        let head_sha: Arc<str> = "a".repeat(40).into();
        let mut file = DiffFile::demo(40);
        file.path = "src/review.rs".into();
        ReviewSession::new(
            domain::SessionSource::GitHubPullRequest {
                repository_root: std::path::PathBuf::from("/tmp/repository"),
                owner: "acme".into(),
                repository: "widgets".into(),
                number: 42,
                title: "Improve the review flow".into(),
                url: "https://github.com/acme/widgets/pull/42".into(),
                base_ref: "main".into(),
                head_ref: "feature".into(),
                base_sha: Arc::clone(&head_sha),
                recorded_base_sha: Arc::clone(&head_sha),
                diff_base_sha: Arc::clone(&head_sha),
                head_sha,
            },
            vec![file].into(),
        )
        .unwrap()
    }

    fn submittable_view(
        cx: &mut TestAppContext,
        session: ReviewSession,
        submitter: RecordingSubmitter,
        sink: RecordingSink,
    ) -> (Entity<SessionView>, &mut gpui::VisualTestContext) {
        let (view, cx) = cx.add_window_view(|_window, cx| SessionView::loading("a review", cx));
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.finish(
                    Ok(LoadedSession {
                        session,
                        review_sink: Some(Box::new(sink)),
                        submitter: Some(Arc::new(submitter)),
                    }),
                    window,
                    cx,
                );
            });
        });
        (view, cx)
    }

    /// The property the whole design turns on: asking to submit posts nothing.
    #[gpui::test]
    fn requesting_submission_does_not_post_anything(cx: &mut TestAppContext) {
        cx.update(init);
        let submitter = RecordingSubmitter::default();
        let mut session = submittable_session();
        session.set_draft(0, 6, "needs a test");
        session.set_summary("Two notes.");
        let (view, cx) = submittable_view(cx, session, submitter.clone(), RecordingSink::default());

        cx.update(|_window, app| {
            let review = review_of(&view, app);
            review.update(app, |_review, cx| {
                cx.emit(ReviewViewEvent::SubmitRequested {
                    event: ReviewEvent::Comment,
                });
            });
        });

        // Waiting on a human, not on the network.
        assert!(
            submitter.posted().is_empty(),
            "nothing may be posted before confirmation",
        );
        cx.update(|_window, app| {
            let submission = view.read(app).confirming().expect("should be confirming");
            assert_eq!(submission.event, ReviewEvent::Comment);
            assert_eq!(submission.comments.len(), 1);
            assert_eq!(submission.body, "Two notes.");
        });
    }

    #[gpui::test]
    fn confirming_posts_the_review_and_clears_what_was_sent(cx: &mut TestAppContext) {
        cx.update(init);
        let submitter = RecordingSubmitter::default();
        let sink = RecordingSink::default();
        let mut session = submittable_session();
        session.set_draft(0, 6, "needs a test");
        session.set_summary("Two notes.");
        let (view, cx) = submittable_view(cx, session, submitter.clone(), sink.clone());

        cx.update(|_window, app| {
            let review = review_of(&view, app);
            review.update(app, |_review, cx| {
                cx.emit(ReviewViewEvent::SubmitRequested {
                    event: ReviewEvent::Comment,
                });
            });
        });
        cx.update(|_window, app| {
            view.update(app, SessionView::send_confirmed);
        });
        cx.run_until_parked();

        let posted = submitter.posted();
        assert_eq!(posted.len(), 1, "posted exactly once");
        assert_eq!(posted[0].comments.len(), 1);
        assert_eq!(posted[0].head_sha.as_ref(), "a".repeat(40));

        cx.update(|_window, app| {
            assert!(
                view.read(app).submitted().is_some(),
                "should report success"
            );
            let session = review_of(&view, app).read(app).session();
            // Forgotten only after the forge accepted it.
            assert!(session.drafts().is_empty());
            assert_eq!(session.summary(), "");
        });
        assert!(
            sink.calls()
                .iter()
                .any(|call| call.starts_with("clear submitted [src/review.rs 6")),
            "storage should be told what was posted: {:?}",
            sink.calls(),
        );
    }

    /// A failed submission must leave every draft exactly where it was.
    #[gpui::test]
    fn a_failed_submission_keeps_every_draft(cx: &mut TestAppContext) {
        cx.update(init);
        let submitter = RecordingSubmitter {
            failure: Some(
                SessionFailure::new("The pull request moved on")
                    .with_remediation("Your drafts are unchanged."),
            ),
            ..RecordingSubmitter::default()
        };
        let sink = RecordingSink::default();
        let mut session = submittable_session();
        session.set_draft(0, 6, "needs a test");
        session.set_summary("Two notes.");
        let (view, cx) = submittable_view(cx, session, submitter.clone(), sink.clone());

        cx.update(|_window, app| {
            let review = review_of(&view, app);
            review.update(app, |_review, cx| {
                cx.emit(ReviewViewEvent::SubmitRequested {
                    event: ReviewEvent::Comment,
                });
            });
        });
        cx.update(|_window, app| {
            view.update(app, SessionView::send_confirmed);
        });
        cx.run_until_parked();

        cx.update(|_window, app| {
            let failure = view
                .read(app)
                .submission_failure()
                .expect("the failure should be shown");
            assert_eq!(failure.summary, "The pull request moved on");

            let session = review_of(&view, app).read(app).session();
            assert_eq!(session.drafts().len(), 1, "the draft is still here");
            assert_eq!(session.summary(), "Two notes.", "so is the summary");
        });
        assert!(
            !sink.calls().iter().any(|call| call.contains("clear")),
            "nothing may be cleared when the post failed: {:?}",
            sink.calls(),
        );
    }

    #[gpui::test]
    fn a_review_that_cannot_be_assembled_explains_itself_without_posting(cx: &mut TestAppContext) {
        cx.update(init);
        let submitter = RecordingSubmitter::default();
        // A comment review with a draft but no summary: GitHub requires a body.
        let mut session = submittable_session();
        session.set_draft(0, 6, "needs a test");
        let (view, cx) = submittable_view(cx, session, submitter.clone(), RecordingSink::default());

        cx.update(|_window, app| {
            let review = review_of(&view, app);
            review.update(app, |_review, cx| {
                cx.emit(ReviewViewEvent::SubmitRequested {
                    event: ReviewEvent::Comment,
                });
            });
        });

        cx.update(|_window, app| {
            let failure = view.read(app).submission_failure().expect("should refuse");
            assert!(
                failure
                    .remediation
                    .as_ref()
                    .unwrap()
                    .contains("needs a summary"),
                "unexpected remediation: {:?}",
                failure.remediation,
            );
            assert!(view.read(app).confirming().is_none());
        });
        assert!(submitter.posted().is_empty());
    }

    #[gpui::test]
    fn cancelling_a_confirmation_posts_nothing(cx: &mut TestAppContext) {
        cx.update(init);
        let submitter = RecordingSubmitter::default();
        let mut session = submittable_session();
        session.set_summary("Just a note.");
        let (view, cx) = submittable_view(cx, session, submitter.clone(), RecordingSink::default());

        cx.update(|_window, app| {
            let review = review_of(&view, app);
            review.update(app, |_review, cx| {
                cx.emit(ReviewViewEvent::SubmitRequested {
                    event: ReviewEvent::Approve,
                });
            });
        });
        cx.update(|_window, app| {
            view.update(app, SessionView::cancel_submission);
        });
        cx.run_until_parked();

        assert!(submitter.posted().is_empty());
        cx.update(|_window, app| {
            assert!(view.read(app).confirming().is_none());
            // The summary survives cancelling.
            assert_eq!(
                review_of(&view, app).read(app).session().summary(),
                "Just a note.",
            );
        });
    }

    /// Typing in the summary must reach both the session and storage.
    #[gpui::test]
    fn the_summary_is_stored_as_it_is_typed(cx: &mut TestAppContext) {
        cx.update(init);
        let sink = RecordingSink::default();
        let (view, cx) = submittable_view(
            cx,
            submittable_session(),
            RecordingSubmitter::default(),
            sink.clone(),
        );

        cx.update(|window, app| {
            let review = review_of(&view, app);
            let editor = review.read(app).summary_editor.clone();
            window.focus(&editor.read(app).focus_handle);
        });
        cx.simulate_input("ok");

        cx.update(|_window, app| {
            assert_eq!(review_of(&view, app).read(app).session().summary(), "ok");
        });
        assert_eq!(
            sink.calls(),
            ["summary o".to_owned(), "summary ok".to_owned()],
        );
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
            start_line: None,
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
                        review_sink: Some(Box::new(sink.clone())),
                        submitter: None,
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
                .content()
                .to_owned()),
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
            cx.read(|app| view.read(app).diff_view.read(app).comment_rows.clone()),
            Some(0..=0),
        );

        // Dismissing the composer hands file navigation back.
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("cmd-shift-j");
        assert_eq!(cx.read(|app| view.read(app).selected_file_index()), 1);
    }
}
