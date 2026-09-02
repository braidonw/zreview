//! Data the frontend sees, and the pure projections that build it from a session.
//!
//! Nothing here touches Tauri. Every projection takes a `&ReviewSession` (or a
//! `&SessionFailure`) and returns a value, which is what keeps them testable
//! without a running app.

use domain::{
    DiffFile, DiffLineKind, DiffSide, EmptyDiffReason, FileStatus, ReviewSession, SessionFailure,
    SessionSource,
};
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, specta::Type)]
pub enum DiffSideDto {
    Left,
    Right,
}

impl From<DiffSide> for DiffSideDto {
    fn from(side: DiffSide) -> Self {
        match side {
            DiffSide::Left => Self::Left,
            DiffSide::Right => Self::Right,
        }
    }
}

impl From<DiffSideDto> for DiffSide {
    fn from(side: DiffSideDto) -> Self {
        match side {
            DiffSideDto::Left => Self::Left,
            DiffSideDto::Right => Self::Right,
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
    pub warnings: Vec<SessionFailureDto>,
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
}

/// A draft resolved to the row it is drawn on.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct AnchoredDraftDto {
    pub row: u32,
    pub body: String,
    pub is_proposed: bool,
}

/// A draft whose anchor no longer resolves against the current diff.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct StaleDraftDto {
    pub path: String,
    pub side: DiffSideDto,
    pub line: u32,
    pub body: String,
    /// Where the draft used to sit, formatted for display, e.g. "was RIGHT line 42".
    pub location: String,
}

/// Every draft that belongs to one file, projected for a per-keystroke response.
///
/// Deliberately narrow. Refetching a whole [`FileDetailDto`] on every keystroke
/// would re-send up to 100,000 rows for a single character typed.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct DraftsDto {
    pub file_index: u32,
    pub anchored: Vec<AnchoredDraftDto>,
    pub stale: Vec<StaleDraftDto>,
    pub file_draft_count: u32,
    pub write_failure: Option<String>,
}

/// The outcome of an edit, discard, or reanchor on the composer.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct DraftEditOutcomeDto {
    pub accepted: bool,
    pub drafts: DraftsDto,
}

/// Why a file shows no diff rows.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct EmptyReasonDto {
    pub label: String,
    pub detail: String,
}

impl From<EmptyDiffReason> for EmptyReasonDto {
    fn from(reason: EmptyDiffReason) -> Self {
        Self {
            label: reason.label().to_owned(),
            detail: reason.detail().to_owned(),
        }
    }
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct FileDetailDto {
    pub index: u32,
    pub path: String,
    pub rows: Vec<RowDto>,
    pub drafts: DraftsDto,
    pub empty_reason: Option<EmptyReasonDto>,
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
        warnings: session.warnings().iter().map(Into::into).collect(),
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
        drafts: project_drafts(session, index),
        empty_reason: file.empty_reason().map(Into::into),
    })
}

fn project_row(session: &ReviewSession, file_index: usize, file: &DiffFile, row: usize) -> RowDto {
    let line = &file.lines[row];
    RowDto {
        kind: line.kind.into(),
        old_line: line.old_line,
        new_line: line.new_line,
        text: line.text.to_string(),
        hunk_header: file.hunk_header_at(row).map(ToString::to_string),
        thread_count: as_u32(session.comments().threads_at(file_index, row).len()),
    }
}

/// Every draft on one file, resolved to its row where it still anchors, cheap
/// regardless of file size since it walks this file's drafts, never its rows.
///
/// # Panics
///
/// Panics if `file_index` is out of range. Every caller already holds a session
/// whose selected or requested file is known to exist.
#[must_use]
pub fn project_drafts(session: &ReviewSession, file_index: usize) -> DraftsDto {
    let file = session
        .files()
        .get(file_index)
        .expect("file_index is bounds-checked by the caller");
    let anchors = session.anchors();
    let mut anchored = Vec::new();
    let mut stale = Vec::new();
    for draft in session.drafts().for_path(&file.path) {
        if draft.is_stale {
            stale.push(StaleDraftDto {
                path: draft.anchor.path.to_string(),
                side: draft.anchor.side.into(),
                line: draft.anchor.line,
                body: draft.body.clone(),
                location: format!("was {} line {}", draft.anchor.side, draft.anchor.line),
            });
        } else if let Some(location) = anchors.and_then(|index| index.resolve(&draft.anchor).ok()) {
            anchored.push(AnchoredDraftDto {
                row: as_u32(location.row),
                body: draft.body.clone(),
                is_proposed: draft.is_proposed(),
            });
        }
    }
    anchored.sort_by_key(|draft| draft.row);
    let file_draft_count = as_u32(anchored.len() + stale.len());
    DraftsDto {
        file_index: as_u32(file_index),
        anchored,
        stale,
        file_draft_count,
        write_failure: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::DiffFile;

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

    /// A session anchored against a head commit, so it can hold drafts.
    fn anchored_session() -> ReviewSession {
        let head_sha: Arc<str> = "a".repeat(40).into();
        let mut file = DiffFile::demo(40);
        file.path = "src/review.rs".into();
        ReviewSession::new(
            SessionSource::LocalComparison {
                repository_root: std::path::PathBuf::from("/tmp/repository"),
                base_sha: Arc::clone(&head_sha),
                diff_base_sha: Arc::clone(&head_sha),
                head_sha,
            },
            vec![file].into(),
        )
        .unwrap()
    }

    /// Built directly rather than through `session::load`, which needs a live `gh`.
    fn pull_request_session() -> ReviewSession {
        let base_sha: Arc<str> = "a".repeat(40).into();
        let head_sha: Arc<str> = "b".repeat(40).into();
        ReviewSession::new(
            SessionSource::GitHubPullRequest {
                repository_root: std::path::PathBuf::from("/tmp/repository"),
                owner: "acme".into(),
                repository: "widgets".into(),
                number: 42,
                title: "Add the widget factory".into(),
                url: "https://github.com/acme/widgets/pull/42".into(),
                base_ref: "main".into(),
                head_ref: "feature".into(),
                base_sha: Arc::clone(&base_sha),
                recorded_base_sha: base_sha.clone(),
                diff_base_sha: base_sha,
                head_sha,
            },
            vec![DiffFile::demo(1)].into(),
        )
        .unwrap()
    }

    #[test]
    fn snapshot_shows_the_pull_requests_identity_and_title() {
        let session = pull_request_session();
        let snapshot = project_snapshot(&session);

        assert_eq!(snapshot.title, "acme/widgets \u{00B7} PR #42");
        assert_eq!(snapshot.subtitle, "Add the widget factory");
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

    #[test]
    fn project_drafts_lists_anchored_and_stale_drafts_on_the_files_path() {
        let mut session = anchored_session();
        session.set_draft(0, 6, "needs a test");
        let stale = domain::DiffAnchor {
            path: "src/review.rs".into(),
            side: domain::DiffSide::Right,
            line: 9_999,
            start_line: None,
            head_sha: "a".repeat(40).into(),
        };
        session.restore_drafts([(stale, "written last week".to_owned())]);

        let drafts = project_drafts(&session, 0);

        assert_eq!(drafts.file_index, 0);
        assert_eq!(drafts.file_draft_count, 2);
        assert_eq!(drafts.anchored.len(), 1);
        assert_eq!(drafts.anchored[0].row, 6);
        assert_eq!(drafts.anchored[0].body, "needs a test");
        assert!(!drafts.anchored[0].is_proposed);
        assert_eq!(drafts.stale.len(), 1);
        assert_eq!(drafts.stale[0].path, "src/review.rs");
        assert!(matches!(drafts.stale[0].side, DiffSideDto::Right));
        assert_eq!(drafts.stale[0].line, 9_999);
        assert_eq!(drafts.stale[0].body, "written last week");
        assert_eq!(drafts.stale[0].location, "was RIGHT line 9999");
        assert!(drafts.write_failure.is_none());
    }

    #[test]
    fn project_file_carries_the_empty_reason_for_a_binary_file() {
        let session = ReviewSession::new(
            SessionSource::Demo,
            vec![DiffFile {
                path: "image.bin".into(),
                old_path: None,
                status: FileStatus::Modified,
                is_binary: true,
                hunks: Arc::from([]),
                counts: domain::ChangeCounts::default(),
                lines: Arc::from([]),
            }]
            .into(),
        )
        .unwrap();

        let detail = project_file(&session, 0).expect("index zero is in range");

        assert!(detail.rows.is_empty());
        let reason = detail.empty_reason.expect("a binary file explains itself");
        assert_eq!(reason.label, "Binary file");
        assert_eq!(reason.detail, "ZReview does not render binary content yet.");
    }

    #[test]
    fn project_file_carries_no_empty_reason_when_there_are_rows() {
        let session = demo_session();
        let detail = project_file(&session, 0).expect("index zero is in range");
        assert!(detail.empty_reason.is_none());
    }

    #[test]
    fn snapshot_carries_the_sessions_warnings() {
        let mut session = demo_session();
        session.push_warning(SessionFailure::new("drafts are not being saved"));

        let snapshot = project_snapshot(&session);

        assert_eq!(snapshot.warnings.len(), 1);
        assert_eq!(snapshot.warnings[0].summary, "drafts are not being saved");
    }
}
