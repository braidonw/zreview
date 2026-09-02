//! Data the frontend sees, and the pure projections that build it from a session.
//!
//! Nothing here touches Tauri. Every projection takes a `&ReviewSession` (or a
//! `&SessionFailure`) and returns a value, which is what keeps them testable
//! without a running app.

use domain::{DiffFile, DiffLineKind, FileStatus, ReviewSession, SessionFailure, SessionSource};
use serde::Serialize;

#[derive(Clone, Debug, Serialize, specta::Type)]
pub enum FileStatusDto {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
}

impl From<FileStatus> for FileStatusDto {
    fn from(status: FileStatus) -> Self {
        match status {
            FileStatus::Added => Self::Added,
            FileStatus::Deleted => Self::Deleted,
            FileStatus::Modified => Self::Modified,
            FileStatus::Renamed => Self::Renamed,
            FileStatus::Copied => Self::Copied,
            FileStatus::TypeChanged => Self::TypeChanged,
            FileStatus::Unmerged => Self::Unmerged,
        }
    }
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub enum DiffLineKindDto {
    Context,
    Addition,
    Deletion,
    NoNewlineMarker,
}

impl From<DiffLineKind> for DiffLineKindDto {
    fn from(kind: DiffLineKind) -> Self {
        match kind {
            DiffLineKind::Context => Self::Context,
            DiffLineKind::Addition => Self::Addition,
            DiffLineKind::Deletion => Self::Deletion,
            DiffLineKind::NoNewlineMarker => Self::NoNewlineMarker,
        }
    }
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct FileSummaryDto {
    pub index: u32,
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatusDto,
    pub is_binary: bool,
    pub additions: u32,
    pub deletions: u32,
    pub viewed: bool,
    pub thread_count: u32,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct SidebarDto {
    pub files: Vec<FileSummaryDto>,
    pub selected_file: u32,
    pub viewed_count: u32,
    pub thread_count: u32,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct SessionSnapshotDto {
    pub title: String,
    pub subtitle: String,
    pub sidebar: SidebarDto,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct RowDto {
    pub kind: DiffLineKindDto,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
    /// The header of the hunk that starts at this row, if one does.
    ///
    /// Rides above the row rather than occupying one of its own, so a row index
    /// always equals its line index.
    pub hunk_header: Option<String>,
    pub thread_count: u32,
    pub has_draft: bool,
    pub draft_is_proposed: bool,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct FileDetailDto {
    pub index: u32,
    pub path: String,
    pub rows: Vec<RowDto>,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct SessionFailureDto {
    pub summary: String,
    pub detail: Option<String>,
    pub remediation: Option<String>,
}

impl From<&SessionFailure> for SessionFailureDto {
    fn from(failure: &SessionFailure) -> Self {
        Self {
            summary: failure.summary.clone(),
            detail: failure.detail.clone(),
            remediation: failure.remediation.clone(),
        }
    }
}

/// A command failure with no detail or remediation beyond its summary.
#[must_use]
pub fn command_failure(summary: impl Into<String>) -> SessionFailureDto {
    SessionFailureDto {
        summary: summary.into(),
        detail: None,
        remediation: None,
    }
}

/// Casts a count or index that is always well within range.
///
/// Specta refuses to export `usize`, since a JS number cannot represent one
/// losslessly. Every value that reaches here is a file or row count, which never
/// approaches `u32::MAX`.
fn as_u32(value: usize) -> u32 {
    u32::try_from(value).expect("file and row counts fit comfortably in a u32")
}

/// The label and title a session's source shows in the sidebar header.
///
/// Mirrors the GPUI sidebar header so both front ends describe a session the
/// same way.
fn source_header(source: &SessionSource) -> (String, String) {
    match source {
        SessionSource::Demo => (
            "Generated fixture".to_owned(),
            "Diff virtualization demo".to_owned(),
        ),
        SessionSource::LocalComparison {
            base_sha, head_sha, ..
        } => (
            "Local comparison".to_owned(),
            format!("{}...{}", short_sha(base_sha), short_sha(head_sha)),
        ),
        SessionSource::GitHubPullRequest {
            owner,
            repository,
            number,
            title,
            ..
        } => (
            format!("{owner}/{repository} \u{00B7} PR #{number}"),
            title.to_string(),
        ),
    }
}

fn short_sha(sha: &str) -> &str {
    match sha.char_indices().nth(7) {
        Some((boundary, _)) => &sha[..boundary],
        None => sha,
    }
}

#[must_use]
pub fn project_snapshot(session: &ReviewSession) -> SessionSnapshotDto {
    let (title, subtitle) = source_header(session.source());
    SessionSnapshotDto {
        title,
        subtitle,
        sidebar: project_sidebar(session),
    }
}

#[must_use]
pub fn project_sidebar(session: &ReviewSession) -> SidebarDto {
    let files = session
        .files()
        .iter()
        .enumerate()
        .map(|(index, file)| project_file_summary(session, index, file))
        .collect();
    SidebarDto {
        files,
        selected_file: as_u32(session.selected_file_index()),
        viewed_count: as_u32(session.viewed_count()),
        thread_count: as_u32(session.comments().thread_count()),
    }
}

fn project_file_summary(session: &ReviewSession, index: usize, file: &DiffFile) -> FileSummaryDto {
    FileSummaryDto {
        index: as_u32(index),
        path: file.path.to_string(),
        old_path: file.old_path.as_ref().map(ToString::to_string),
        status: file.status.into(),
        is_binary: file.is_binary,
        additions: as_u32(file.counts.additions),
        deletions: as_u32(file.counts.deletions),
        viewed: session.is_viewed(index),
        thread_count: as_u32(session.comments().thread_count_for_file(index)),
    }
}

/// The rows and path for one file, or `None` when the index is out of range.
#[must_use]
pub fn project_file(session: &ReviewSession, index: usize) -> Option<FileDetailDto> {
    let file = session.files().get(index)?;
    let rows = (0..file.line_count())
        .map(|row| project_row(session, index, file, row))
        .collect();
    Some(FileDetailDto {
        index: as_u32(index),
        path: file.path.to_string(),
        rows,
    })
}

fn project_row(session: &ReviewSession, file_index: usize, file: &DiffFile, row: usize) -> RowDto {
    let line = &file.lines[row];
    let draft = session.draft_at(file_index, row);
    RowDto {
        kind: line.kind.into(),
        old_line: line.old_line,
        new_line: line.new_line,
        text: line.text.to_string(),
        hunk_header: file.hunk_header_at(row).map(ToString::to_string),
        thread_count: as_u32(session.comments().threads_at(file_index, row).len()),
        has_draft: draft.is_some(),
        draft_is_proposed: draft.is_some_and(domain::DraftComment::is_proposed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_session() -> ReviewSession {
        session::load(
            &session::SessionRequest::Demo,
            &session::ReviewStorage::Disabled,
            &|_stage| {},
        )
        .expect("the generated fixture always loads")
        .session
    }

    #[test]
    fn snapshot_projects_the_demo_sidebar() {
        let session = demo_session();
        let snapshot = project_snapshot(&session);

        assert_eq!(snapshot.title, "Generated fixture");
        assert_eq!(snapshot.subtitle, "Diff virtualization demo");
        assert_eq!(snapshot.sidebar.files.len(), 12);
        assert_eq!(snapshot.sidebar.selected_file, 0);
        assert_eq!(snapshot.sidebar.viewed_count, 0);

        for (index, file) in snapshot.sidebar.files.iter().enumerate() {
            assert_eq!(file.path, format!("src/review_fixture_{index:02}.rs"));
            let expected_status = match index % 4 {
                0 => "Modified",
                1 => "Added",
                2 => "Deleted",
                3 => "Renamed",
                _ => unreachable!("index % 4 is always 0..=3"),
            };
            assert_eq!(format!("{:?}", file.status), expected_status);
            assert!(file.additions > 0 || file.deletions > 0);
        }
    }

    #[test]
    fn file_zero_is_the_stress_file() {
        let session = demo_session();
        let detail = project_file(&session, 0).expect("index zero is in range");

        assert_eq!(detail.rows.len(), 100_000);
        assert!(detail.rows[0].hunk_header.is_some());
        assert!(detail.rows[1..].iter().all(|row| row.hunk_header.is_none()));

        // The demo pattern: every 20th row deletes, the next two add, the rest
        // are context, each carrying coordinates on the sides its kind touches.
        assert!(matches!(detail.rows[5].kind, DiffLineKindDto::Deletion));
        assert!(detail.rows[5].old_line.is_some());
        assert!(detail.rows[5].new_line.is_none());
        assert!(matches!(detail.rows[6].kind, DiffLineKindDto::Addition));
        assert!(detail.rows[6].old_line.is_none());
        assert!(detail.rows[6].new_line.is_some());
        assert!(matches!(detail.rows[0].kind, DiffLineKindDto::Context));
        assert!(detail.rows[0].old_line.is_some());
        assert!(detail.rows[0].new_line.is_some());
    }

    #[test]
    fn file_one_has_two_hundred_twenty_five_rows() {
        let session = demo_session();
        let detail = project_file(&session, 1).expect("index one is in range");

        assert_eq!(detail.rows.len(), 225);
    }

    #[test]
    fn project_file_is_none_out_of_range() {
        let session = demo_session();
        assert!(project_file(&session, 999).is_none());
    }

    #[test]
    fn session_failure_dto_maps_every_field() {
        let failure = SessionFailure {
            summary: "could not load".to_owned(),
            detail: Some("underlying error".to_owned()),
            remediation: Some("try again".to_owned()),
        };

        let dto: SessionFailureDto = (&failure).into();

        assert_eq!(dto.summary, "could not load");
        assert_eq!(dto.detail.as_deref(), Some("underlying error"));
        assert_eq!(dto.remediation.as_deref(), Some("try again"));
    }

    #[test]
    fn short_sha_does_not_panic_on_a_short_string() {
        assert_eq!(short_sha("abc"), "abc");
    }
}
