use std::{ops::Range, sync::Arc};

/// The semantic role of one row in a unified diff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
}

impl DiffLineKind {
    #[must_use]
    pub const fn marker(self) -> char {
        match self {
            Self::Context => ' ',
            Self::Addition => '+',
            Self::Deletion => '-',
        }
    }
}

/// One source line with stable coordinates on both sides of a diff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: Arc<str>,
}

/// Metadata locating a hunk in a flattened [`DiffFile::lines`] collection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffHunk {
    pub header: Arc<str>,
    pub old_start: u32,
    pub new_start: u32,
    pub line_range: Range<usize>,
}

/// A reviewable file. Lines are flat to make virtualized indexing constant-time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffFile {
    pub path: Arc<str>,
    pub hunks: Arc<[DiffHunk]>,
    pub lines: Arc<[DiffLine]>,
}

impl DiffFile {
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    #[must_use]
    pub fn line(&self, index: usize) -> Option<&DiffLine> {
        self.lines.get(index)
    }

    /// Generates a large deterministic fixture without reading a repository.
    #[must_use]
    pub fn demo(line_count: usize) -> Self {
        let mut lines = Vec::with_capacity(line_count);
        let mut old_line = 1_u32;
        let mut new_line = 1_u32;

        for index in 0..line_count {
            let (kind, old, new) = match index % 20 {
                5 => {
                    let line = old_line;
                    old_line += 1;
                    (DiffLineKind::Deletion, Some(line), None)
                }
                6 | 7 => {
                    let line = new_line;
                    new_line += 1;
                    (DiffLineKind::Addition, None, Some(line))
                }
                _ => {
                    let old = old_line;
                    let new = new_line;
                    old_line += 1;
                    new_line += 1;
                    (DiffLineKind::Context, Some(old), Some(new))
                }
            };

            let text: Arc<str> = match kind {
                DiffLineKind::Addition => format!(
                    "let reviewed_value_{index} = calculate_result(input, ReviewMode::Strict);"
                )
                .into(),
                DiffLineKind::Deletion => {
                    format!("let reviewed_value_{index} = calculate_result(input);").into()
                }
                DiffLineKind::Context => {
                    format!("assert_reviewable(reviewed_value_{index}, repository_guidance);")
                        .into()
                }
            };

            lines.push(DiffLine {
                kind,
                old_line: old,
                new_line: new,
                text,
            });
        }

        Self {
            path: "src/generated_review_fixture.rs".into(),
            hunks: vec![DiffHunk {
                header: format!("@@ -1,{} +1,{} @@", old_line - 1, new_line - 1).into(),
                old_start: 1,
                new_start: 1,
                line_range: 0..line_count,
            }]
            .into(),
            lines: lines.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_has_stable_line_coordinates() {
        let file = DiffFile::demo(100_000);

        assert_eq!(file.line_count(), 100_000);
        assert_eq!(file.hunks[0].line_range, 0..100_000);
        assert_eq!(file.lines[5].kind, DiffLineKind::Deletion);
        assert!(file.lines[5].old_line.is_some());
        assert_eq!(file.lines[5].new_line, None);
        assert_eq!(file.lines[6].kind, DiffLineKind::Addition);
        assert_eq!(file.lines[6].old_line, None);
        assert!(file.lines[6].new_line.is_some());
    }

    #[test]
    fn marker_matches_line_kind() {
        assert_eq!(DiffLineKind::Context.marker(), ' ');
        assert_eq!(DiffLineKind::Addition.marker(), '+');
        assert_eq!(DiffLineKind::Deletion.marker(), '-');
    }
}
