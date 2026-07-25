//! The editing model behind the comment composer.
//!
//! Deliberately free of GPUI: every rule about where the caret goes and what an
//! edit changes is decided here, and can be tested without a window. The view is
//! left with drawing and key dispatch.
//!
//! Offsets are byte offsets into the content and are always on grapheme
//! boundaries, so a caret can never land inside a character or split an emoji.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// Text being edited, with a caret or selection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TextBuffer {
    content: String,
    /// Byte range of the selection. Empty means a caret with no selection.
    selection: Range<usize>,
    /// Whether the caret sits at the start of `selection` rather than its end.
    ///
    /// Tracked so extending a selection grows from the end the reviewer is not
    /// holding, which is what makes shift-arrow behave.
    reversed: bool,
    /// The region an input method is currently composing, if any.
    marked: Option<Range<usize>>,
}

impl TextBuffer {
    #[must_use]
    pub fn new(content: String) -> Self {
        let end = content.len();
        Self {
            content,
            selection: end..end,
            reversed: false,
            marked: None,
        }
    }

    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    #[must_use]
    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    #[must_use]
    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// Where the caret is: the end of the selection the reviewer is moving.
    #[must_use]
    pub fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    #[must_use]
    pub fn selected_text(&self) -> &str {
        &self.content[self.selection.clone()]
    }

    /// Replaces the whole content, putting the caret at the end.
    pub fn set_content(&mut self, content: String) {
        *self = Self::new(content);
    }

    /// Collapses the selection to a caret at `offset`.
    pub fn move_to(&mut self, offset: usize) {
        let offset = self.clamp(offset);
        self.selection = offset..offset;
        self.reversed = false;
    }

    /// Extends the selection to `offset`, keeping the far end fixed.
    pub fn select_to(&mut self, offset: usize) {
        let offset = self.clamp(offset);
        let anchor = if self.reversed {
            self.selection.end
        } else {
            self.selection.start
        };
        if offset < anchor {
            self.selection = offset..anchor;
            self.reversed = true;
        } else {
            self.selection = anchor..offset;
            self.reversed = false;
        }
    }

    pub fn select_all(&mut self) {
        self.selection = 0..self.content.len();
        self.reversed = false;
    }

    /// Replaces `range` with `text`, leaving the caret after what was inserted.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let range = self.clamp_range(range);
        self.content.replace_range(range.clone(), text);
        let cursor = range.start + text.len();
        self.selection = cursor..cursor;
        self.reversed = false;
        self.marked = None;
    }

    /// Inserts at the caret, replacing the selection if there is one.
    pub fn insert(&mut self, text: &str) {
        let target = self.marked.take().unwrap_or_else(|| self.selection.clone());
        self.replace(target, text);
    }

    /// Deletes backwards. With a selection, deletes that instead of a character —
    /// which is what every other editor does, and what stops a reviewer losing a
    /// character they meant to replace.
    pub fn backspace(&mut self) {
        if self.selection.is_empty() {
            let cursor = self.cursor();
            self.selection = self.previous_boundary(cursor)..cursor;
        }
        self.replace(self.selection.clone(), "");
    }

    pub fn delete_forward(&mut self) {
        if self.selection.is_empty() {
            let cursor = self.cursor();
            self.selection = cursor..self.next_boundary(cursor);
        }
        self.replace(self.selection.clone(), "");
    }

    /// Moves the caret one grapheme left, or collapses a selection to its start.
    pub fn move_left(&mut self) {
        if self.selection.is_empty() {
            self.move_to(self.previous_boundary(self.cursor()));
        } else {
            self.move_to(self.selection.start);
        }
    }

    pub fn move_right(&mut self) {
        if self.selection.is_empty() {
            self.move_to(self.next_boundary(self.cursor()));
        } else {
            self.move_to(self.selection.end);
        }
    }

    pub fn select_left(&mut self) {
        self.select_to(self.previous_boundary(self.cursor()));
    }

    pub fn select_right(&mut self) {
        self.select_to(self.next_boundary(self.cursor()));
    }

    /// Moves to the start of the caret's line, not the start of the text.
    pub fn move_line_start(&mut self) {
        self.move_to(self.line_start(self.cursor()));
    }

    pub fn move_line_end(&mut self) {
        self.move_to(self.line_end(self.cursor()));
    }

    /// Moves up a line, keeping the column where possible.
    pub fn move_up(&mut self) {
        self.move_to(self.offset_above(self.cursor()));
    }

    pub fn move_down(&mut self) {
        self.move_to(self.offset_below(self.cursor()));
    }

    pub fn select_up(&mut self) {
        self.select_to(self.offset_above(self.cursor()));
    }

    pub fn select_down(&mut self) {
        self.select_to(self.offset_below(self.cursor()));
    }

    /// Begins or updates an input method's composition region.
    ///
    /// The composed text is in the content already so it can be seen while it is
    /// being chosen; `marked` records what will be replaced when it is committed.
    pub fn replace_and_mark(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selection: Option<Range<usize>>,
    ) {
        let target = self.clamp_range(
            range
                .or_else(|| self.marked.clone())
                .unwrap_or_else(|| self.selection.clone()),
        );
        self.content.replace_range(target.clone(), text);

        let composed = target.start..target.start + text.len();
        self.marked = (!text.is_empty()).then_some(composed.clone());
        self.selection = new_selection.map_or(composed.end..composed.end, |selected| {
            self.clamp_range(composed.start + selected.start..composed.start + selected.end)
        });
        self.reversed = false;
    }

    pub fn unmark(&mut self) {
        self.marked = None;
    }

    /// The lines of the content, always at least one.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.content.split('\n')
    }

    /// Byte offset where the line containing `offset` starts.
    #[must_use]
    pub fn line_start(&self, offset: usize) -> usize {
        let offset = self.clamp(offset);
        self.content[..offset]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    /// Byte offset where the line containing `offset` ends, before its newline.
    #[must_use]
    pub fn line_end(&self, offset: usize) -> usize {
        let offset = self.clamp(offset);
        self.content[offset..]
            .find('\n')
            .map_or(self.content.len(), |index| offset + index)
    }

    /// Converts a byte offset to the UTF-16 offset platform input methods use.
    #[must_use]
    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let offset = self.clamp(offset);
        self.content[..offset]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>()
    }

    /// Converts a UTF-16 offset from a platform input method to a byte offset.
    #[must_use]
    pub fn offset_from_utf16(&self, target: usize) -> usize {
        let mut utf16 = 0;
        for (offset, character) in self.content.char_indices() {
            if utf16 >= target {
                return offset;
            }
            utf16 += character.len_utf16();
        }
        self.content.len()
    }

    #[must_use]
    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    #[must_use]
    pub fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn offset_above(&self, offset: usize) -> usize {
        let line_start = self.line_start(offset);
        if line_start == 0 {
            return 0;
        }
        let column = self.column(offset);
        self.offset_at_column(self.line_start(line_start - 1), column)
    }

    fn offset_below(&self, offset: usize) -> usize {
        let line_end = self.line_end(offset);
        if line_end == self.content.len() {
            return line_end;
        }
        let column = self.column(offset);
        self.offset_at_column(line_end + 1, column)
    }

    /// The caret's column, counted in graphemes so a wide character counts once.
    fn column(&self, offset: usize) -> usize {
        let start = self.line_start(offset);
        self.content[start..self.clamp(offset)]
            .graphemes(true)
            .count()
    }

    fn offset_at_column(&self, line_start: usize, column: usize) -> usize {
        let line_end = self.line_end(line_start);
        self.content[line_start..line_end]
            .grapheme_indices(true)
            .nth(column)
            .map_or(line_end, |(index, _)| line_start + index)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    /// Snaps an offset onto a grapheme boundary inside the content.
    ///
    /// Platform input methods and stored offsets can both name positions that no
    /// longer exist, and slicing a `String` off a boundary panics.
    fn clamp(&self, offset: usize) -> usize {
        if offset >= self.content.len() {
            return self.content.len();
        }
        if self.content.is_char_boundary(offset) {
            offset
        } else {
            self.previous_boundary(offset)
        }
    }

    fn clamp_range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.clamp(range.start);
        let end = self.clamp(range.end).max(start);
        start..end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(content: &str) -> TextBuffer {
        TextBuffer::new(content.to_owned())
    }

    #[test]
    fn a_new_buffer_puts_the_caret_at_the_end() {
        let text = buffer("hello");
        assert_eq!(text.cursor(), 5);
        assert!(text.selected_text().is_empty());
    }

    #[test]
    fn typing_inserts_at_the_caret_rather_than_appending() {
        let mut text = buffer("hello world");
        text.move_to(5);
        text.insert(",");

        assert_eq!(text.content(), "hello, world");
        assert_eq!(text.cursor(), 6);
    }

    #[test]
    fn backspace_deletes_the_character_before_the_caret() {
        let mut text = buffer("hello");
        text.move_to(3);
        text.backspace();

        assert_eq!(text.content(), "helo");
        assert_eq!(text.cursor(), 2);
    }

    #[test]
    fn backspace_at_the_start_does_nothing() {
        let mut text = buffer("hello");
        text.move_to(0);
        text.backspace();

        assert_eq!(text.content(), "hello");
        assert_eq!(text.cursor(), 0);
    }

    #[test]
    fn delete_forward_removes_the_character_after_the_caret() {
        let mut text = buffer("hello");
        text.move_to(0);
        text.delete_forward();

        assert_eq!(text.content(), "ello");
        assert_eq!(text.cursor(), 0);
    }

    /// Deleting a selection rather than one character is what stops a reviewer
    /// losing the character they meant to replace.
    #[test]
    fn deleting_with_a_selection_removes_the_selection() {
        let mut text = buffer("hello world");
        text.move_to(0);
        text.select_to(5);
        text.backspace();

        assert_eq!(text.content(), " world");
        assert_eq!(text.cursor(), 0);
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut text = buffer("hello world");
        text.move_to(6);
        text.select_to(11);
        text.insert("there");

        assert_eq!(text.content(), "hello there");
    }

    /// An emoji is several bytes and several chars but one grapheme; moving or
    /// deleting must treat it as one unit.
    #[test]
    fn movement_and_deletion_respect_grapheme_clusters() {
        let mut text = buffer("a👍🏽b");
        text.move_to(0);
        text.move_right();
        let after_first = text.cursor();
        assert_eq!(&text.content()[..after_first], "a");

        text.move_right();
        let after_emoji = text.cursor();
        assert_eq!(&text.content()[..after_emoji], "a👍🏽");

        text.backspace();
        assert_eq!(text.content(), "ab", "the whole cluster went at once");
    }

    #[test]
    fn moving_left_collapses_a_selection_to_its_start() {
        let mut text = buffer("hello");
        text.move_to(1);
        text.select_to(4);

        text.move_left();
        assert_eq!(text.cursor(), 1);
        assert!(text.selected_text().is_empty());
    }

    #[test]
    fn extending_a_selection_grows_from_the_fixed_end() {
        let mut text = buffer("hello");
        text.move_to(2);

        text.select_right();
        text.select_right();
        assert_eq!(text.selected_text(), "ll");

        // Reversing past the anchor selects the other way.
        text.select_left();
        text.select_left();
        text.select_left();
        assert_eq!(text.selected_text(), "e");
        assert_eq!(text.cursor(), 1);
    }

    #[test]
    fn select_all_covers_the_content() {
        let mut text = buffer("hello");
        text.select_all();
        assert_eq!(text.selected_text(), "hello");
    }

    #[test]
    fn home_and_end_work_on_the_caret_line_not_the_whole_text() {
        let mut text = buffer("first\nsecond\nthird");
        text.move_to(8);

        text.move_line_start();
        assert_eq!(text.cursor(), 6);
        text.move_line_end();
        assert_eq!(text.cursor(), 12);
    }

    #[test]
    fn vertical_movement_keeps_the_column() {
        let mut text = buffer("abcdef\nghijkl\nmno");
        text.move_to(3); // column 3 of line one

        text.move_down();
        assert_eq!(text.cursor(), 10, "column 3 of line two");
        text.move_down();
        assert_eq!(text.cursor(), 17, "column 3 of line three");
        text.move_up();
        assert_eq!(text.cursor(), 10);
    }

    /// Moving onto a shorter line lands at its end rather than past it.
    #[test]
    fn vertical_movement_clamps_to_a_shorter_line() {
        let mut text = buffer("longer line\nab\nlonger again");
        text.move_to(8);

        text.move_down();
        assert_eq!(text.cursor(), 14, "the end of the short line");
    }

    #[test]
    fn vertical_movement_stops_at_the_edges() {
        let mut text = buffer("one\ntwo");
        text.move_to(1);
        text.move_up();
        assert_eq!(text.cursor(), 0);

        text.move_to(5);
        text.move_down();
        assert_eq!(text.cursor(), 7);
    }

    #[test]
    fn newlines_are_ordinary_insertions() {
        let mut text = buffer("ab");
        text.move_to(1);
        text.insert("\n");

        assert_eq!(text.content(), "a\nb");
        assert_eq!(text.lines().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn utf16_offsets_round_trip_through_bytes() {
        let text = buffer("a👍🏽b");

        for offset in [0, 1, text.content().len()] {
            let utf16 = text.offset_to_utf16(offset);
            assert_eq!(text.offset_from_utf16(utf16), offset);
        }
        // The cluster is four UTF-16 units and eight bytes.
        assert_eq!(text.offset_to_utf16(text.content().len()), 6);
    }

    /// An input method composes in place, then commits.
    #[test]
    fn marked_text_is_visible_while_it_is_being_composed() {
        let mut text = buffer("");
        text.replace_and_mark(None, "n", None);
        assert_eq!(text.content(), "n");
        assert_eq!(text.marked(), Some(0..1));

        text.replace_and_mark(None, "に", None);
        assert_eq!(
            text.content(),
            "に",
            "the composition was replaced, not appended"
        );
        assert_eq!(text.marked(), Some(0..3));

        // Committing clears the composition.
        text.insert("日");
        assert_eq!(text.content(), "日");
        assert_eq!(text.marked(), None);
    }

    #[test]
    fn an_empty_composition_clears_the_mark() {
        let mut text = buffer("");
        text.replace_and_mark(None, "n", None);
        text.replace_and_mark(None, "", None);

        assert_eq!(text.content(), "");
        assert_eq!(text.marked(), None);
    }

    #[test]
    fn unmarking_leaves_the_text_alone() {
        let mut text = buffer("");
        text.replace_and_mark(None, "n", None);
        text.unmark();

        assert_eq!(text.content(), "n");
        assert_eq!(text.marked(), None);
    }

    /// Offsets from a platform input method cannot be trusted to be boundaries,
    /// and slicing a string off one panics.
    #[test]
    fn offsets_inside_a_character_are_snapped_to_a_boundary() {
        let mut text = buffer("a👍🏽b");

        text.move_to(3); // inside the emoji cluster
        assert!(text.content().is_char_boundary(text.cursor()));

        text.replace(2..4, "");
        assert!(text.content().is_char_boundary(text.cursor()));
    }

    #[test]
    fn offsets_past_the_end_are_clamped() {
        let mut text = buffer("ab");
        text.move_to(99);
        assert_eq!(text.cursor(), 2);

        text.select_to(99);
        assert_eq!(text.selected_text(), "");
    }

    #[test]
    fn setting_content_resets_the_selection() {
        let mut text = buffer("hello");
        text.select_all();
        text.set_content("replaced".to_owned());

        assert_eq!(text.content(), "replaced");
        assert_eq!(text.cursor(), 8);
        assert!(text.selected_text().is_empty());
        assert_eq!(text.marked(), None);
    }
}
