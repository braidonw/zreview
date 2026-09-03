//! Data the frontend sees, and the pure projections that build it from a session.
//!
//! Nothing here touches Tauri. Every projection takes a `&ReviewSession` (or a
//! `&SessionFailure`) and returns a value, which is what keeps them testable
//! without a running app.

use app::{PullRequestId, ReviewModel, ReviewRunState};
use domain::{
    DiffFile, DiffLineKind, DiffSide, EmptyDiffReason, FileStatus, GuidanceSelection,
    ReviewSession, SessionFailure, SessionSource,
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

/// The one Session the window holds, in front of Home or alive behind it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, specta::Type)]
pub struct OpenSessionDto {
    /// What is being opened, which is all the loading screen can say before the
    /// load reaches the pull request itself.
    pub description: String,
    /// `owner/name#number` of the row this Session was opened from, which the
    /// header slot reads and Home marks that row by. Absent for a Session the
    /// command line opened, and its absence is what says there is no Home to go
    /// back to.
    pub row_identity: Option<String>,
}

/// Which screen the window shows, and the Session it is holding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, specta::Type)]
pub enum WindowDto {
    /// Home, with the Session alive behind it when there is one. That Session's
    /// tree stays mounted and hidden, which is what makes returning instant.
    Home { alive: Option<OpenSessionDto> },
    /// The Session, in front of Home or with no Home behind it at all.
    Session { session: OpenSessionDto },
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

/// One guidance file discovery found, as the panel draws its row.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct GuidanceEntryDto {
    /// Repository-relative path, which is also how a finding cites it.
    pub path: String,
    /// What it applies to, already rendered, e.g. "whole repository".
    pub scope: String,
    pub kilobytes: u32,
    /// Whether the next run will send it.
    pub included: bool,
}

/// Something discovery found and will not use, and why.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct GuidanceSkipDto {
    pub path: String,
    pub reason: String,
}

/// The guidance section at the top of the panel.
#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(tag = "kind")]
pub enum GuidanceDto {
    /// Discovery ran and found nothing. Saying so is not the same as showing
    /// nothing. A reviewer needs to tell "this repository states no conventions"
    /// from "guidance was never looked for".
    NothingFound { note: String },
    Discovered {
        /// The one line that stays on screen whether or not the section is open.
        summary: String,
        expanded: bool,
        entries: Vec<GuidanceEntryDto>,
        skipped: Vec<GuidanceSkipDto>,
        /// What configuration keeps out of the review, when it keeps anything out.
        excluded: Option<String>,
    },
}

/// How far the current review run has got.
#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(tag = "state")]
pub enum ReviewRunDto {
    Idle,
    Running {
        /// The backend's most recent progress line.
        detail: String,
    },
    Complete {
        accepted: u32,
        rejected: u32,
        /// Claims suppressed because the reviewer dismissed them before.
        suppressed: u32,
    },
    Failed {
        summary: String,
        remediation: Option<String>,
    },
}

/// What the panel says when there is no finding to show.
///
/// Every run state has one. An empty panel with no explanation is the failure
/// this projection exists to avoid.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct PanelNoteDto {
    pub heading: String,
    pub detail: Option<String>,
}

/// The caveats under the panel: what was refused, and what was never looked at.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct PanelFooterDto {
    /// Claims that did not survive checking against the diff.
    pub refused: Option<String>,
    /// Present when a completed run did not see the whole change.
    pub not_reviewed: Option<String>,
    /// The files that run did not see, named under the count that describes them.
    pub unreviewed: Vec<String>,
}

/// The Session's right-hand panel: what a review is held to, and how far the run
/// that populates it has got.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct ReviewPanelDto {
    /// How many times the panel has changed, so a snapshot that was read before
    /// a change can be told from one that carries it.
    pub revision: u32,
    /// "Review" before there is anything to act on, otherwise the finding count.
    pub heading: String,
    pub guidance: GuidanceDto,
    pub run: ReviewRunDto,
    pub note: Option<PanelNoteDto>,
    pub footer: Option<PanelFooterDto>,
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

/// Which severity a status column paints itself in.
///
/// Named rather than coloured, so the values themselves stay in the one style
/// sheet that owns them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, specta::Type)]
pub enum StatusToneDto {
    Success,
    Error,
    Warning,
    Muted,
}

/// One status column's words and the weight they carry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, specta::Type)]
pub struct RowStatusDto {
    pub label: String,
    pub tone: StatusToneDto,
}

impl From<app::CheckStatus> for RowStatusDto {
    fn from(status: app::CheckStatus) -> Self {
        Self {
            label: status.label().to_owned(),
            tone: match status {
                app::CheckStatus::Passing => StatusToneDto::Success,
                app::CheckStatus::Failing => StatusToneDto::Error,
                app::CheckStatus::Running => StatusToneDto::Warning,
            },
        }
    }
}

impl From<app::ReviewStatus> for RowStatusDto {
    fn from(status: app::ReviewStatus) -> Self {
        Self {
            label: status.label().to_owned(),
            tone: match status {
                app::ReviewStatus::Approved => StatusToneDto::Success,
                app::ReviewStatus::ChangesRequested => StatusToneDto::Error,
                app::ReviewStatus::ReviewedThisHead => StatusToneDto::Muted,
            },
        }
    }
}

/// One pull request, as one line of the ledger.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct HomeRowDto {
    /// Where this row sits in the flat order the cursor walks.
    pub index: u32,
    pub title: String,
    pub url: String,
    /// `owner/name`, which opening this row names its pull request by.
    pub repository: String,
    pub number: u32,
    /// `owner/name#number`.
    pub identity: String,
    /// Absent once GitHub has forgotten the account.
    pub author: Option<String>,
    /// When the pull request last moved, as epoch milliseconds, which the age
    /// column is worked out from.
    #[specta(type = specta_typescript::Number)]
    pub updated_at_ms: i64,
    pub review_status: Option<RowStatusDto>,
    pub check_status: Option<RowStatusDto>,
    /// "1 draft" or "N drafts", absent for a blank cell.
    pub drafts_label: Option<String>,
    /// Whether the Session alive behind Home is open on this row's pull
    /// request, which is what the row's accent mark says.
    pub is_alive: bool,
}

/// One of Home's three groups, always rendered whether or not it has rows.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct HomeGroupDto {
    pub title: String,
    pub count: u32,
    /// The one line an empty group shows in place of rows.
    pub empty_copy: String,
    pub rows: Vec<HomeRowDto>,
}

/// How current the list is, which the header stamp reads.
#[derive(Clone, Debug, PartialEq, Serialize, specta::Type)]
pub enum RefreshStateDto {
    /// There is no stamp at all until the first refresh starts.
    NeverRefreshed,
    Refreshing {
        done: u32,
        total: u32,
    },
    /// Epoch milliseconds, which the relative stamp is worked out from.
    Refreshed {
        #[specta(type = specta_typescript::Number)]
        at_ms: i64,
    },
    Failed,
}

impl From<app::RefreshState> for RefreshStateDto {
    fn from(state: app::RefreshState) -> Self {
        match state {
            app::RefreshState::NeverRefreshed => Self::NeverRefreshed,
            app::RefreshState::Refreshing { done, total } => Self::Refreshing {
                done: as_u32(done),
                total: as_u32(total),
            },
            app::RefreshState::Refreshed { at_ms } => Self::Refreshed { at_ms },
            app::RefreshState::Failed => Self::Failed,
        }
    }
}

/// One configured repository whose pull requests could not be fetched.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct FailedRepositoryDto {
    /// The entry in the settings file, which is what Remove names.
    pub path: String,
    pub slug: String,
    pub reason: String,
}

/// Which way the cursor is being moved.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, specta::Type)]
pub enum CursorMoveDto {
    Down,
    Up,
}

/// One configured clone as the footer lists it.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct HomeRepositoryDto {
    pub path: String,
    pub slug: Option<String>,
    /// Why this clone cannot be listed, absent when it resolved.
    pub failure: Option<String>,
}

/// Something Home would not do, and why.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct RefusalDto {
    pub path: String,
    pub reason: String,
}

/// Everything Home renders, from its header to its footer.
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct HomeSnapshotDto {
    /// The header's count line, absent until there is something to count.
    pub count_line: Option<String>,
    pub groups: Vec<HomeGroupDto>,
    /// Where the cursor sits, as a flat index across every rendered row.
    pub cursor: u32,
    pub refresh: RefreshStateDto,
    /// One line each above the list, naming a repository this refresh lost.
    pub failed_repositories: Vec<FailedRepositoryDto>,
    pub repositories: Vec<HomeRepositoryDto>,
    pub footer_summary: String,
    pub footer_expanded: bool,
    pub refusals: Vec<RefusalDto>,
    /// What replaces the list area when Home cannot read its repositories or
    /// cannot use `gh`. The header and footer stay either way, so Add still
    /// works.
    pub failure: Option<SessionFailureDto>,
    /// Why the last write did not reach the file, shown as a line above the
    /// list, which stays because reading it still worked.
    pub write_failure: Option<SessionFailureDto>,
    /// Why the last Drafts read failed, shown as a line above the list beside
    /// the failed repositories, with every row's Drafts column left blank.
    pub drafts_failure: Option<SessionFailureDto>,
}

/// Everything Home shows, from the model that decided it.
///
/// `alive` is the pull request the Session behind Home is open on, when one is,
/// so the row listing it comes back marked.
#[must_use]
pub fn project_home(home: &app::HomeModel, alive: Option<&PullRequestId>) -> HomeSnapshotDto {
    // Walked in the model's own order, so a row's index is where the cursor
    // finds it rather than something counted twice and hoped to agree.
    let mut index = 0_u32;
    let mut groups = Vec::with_capacity(app::HomeGroup::ALL.len());
    for group in app::HomeGroup::ALL {
        let rows = home
            .rows_in(group)
            .map(|row| {
                let projected = project_home_row(row, index, alive);
                index += 1;
                projected
            })
            .collect::<Vec<_>>();
        groups.push(HomeGroupDto {
            title: group.title().to_owned(),
            count: as_u32(rows.len()),
            empty_copy: group.empty_copy().to_owned(),
            rows,
        });
    }

    HomeSnapshotDto {
        count_line: home.count_line(),
        groups,
        cursor: as_u32(home.cursor()),
        refresh: home.refresh_state().into(),
        failed_repositories: home
            .fetch_failures()
            .iter()
            .map(|failure| FailedRepositoryDto {
                path: failure.path.display().to_string(),
                slug: failure.slug.clone(),
                reason: failure.reason.clone(),
            })
            .collect(),
        repositories: home
            .repositories()
            .iter()
            .map(|entry| HomeRepositoryDto {
                path: entry.path.display().to_string(),
                slug: entry.slug().map(ToOwned::to_owned),
                failure: home.repository_failure(&entry.path).map(ToOwned::to_owned),
            })
            .collect(),
        footer_summary: home.footer_summary(),
        footer_expanded: home.is_footer_expanded(),
        refusals: home
            .refusals()
            .iter()
            .map(|refusal| RefusalDto {
                path: refusal.path.display().to_string(),
                reason: refusal.reason.clone(),
            })
            .collect(),
        failure: home.failure().map(Into::into),
        write_failure: home.write_failure().map(Into::into),
        drafts_failure: home.drafts_failure().map(Into::into),
    }
}

fn project_home_row(row: &app::HomeRow, index: u32, alive: Option<&PullRequestId>) -> HomeRowDto {
    HomeRowDto {
        index,
        is_alive: alive.is_some_and(|pull_request| row.is_on(pull_request)),
        title: row.title.clone(),
        url: row.url.clone(),
        repository: row.repository.clone(),
        number: u32::try_from(row.number).expect("a pull request number fits comfortably in a u32"),
        identity: row.identity(),
        author: row.author_login.clone(),
        updated_at_ms: row.updated_at_ms,
        review_status: row.review_status.map(Into::into),
        check_status: row.check_status.map(Into::into),
        drafts_label: row.drafts_label(),
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

/// The Session's review panel, or `None` when this session has nothing to put one
/// on.
///
/// Mirrors `crates/ui/src/findings.rs`, which is the parity target: the same copy
/// in the same order, read off the same model. The findings list itself is not
/// projected yet.
#[must_use]
pub fn project_panel(review: &ReviewModel) -> Option<ReviewPanelDto> {
    if !review.findings_panel_visible() {
        return None;
    }
    let session = review.session();
    let run = review.run();
    Some(ReviewPanelDto {
        revision: review.revision(),
        heading: match session.findings().len() {
            0 => "Review".to_owned(),
            1 => "1 finding".to_owned(),
            many => format!("{many} findings"),
        },
        guidance: project_guidance(session.guidance(), review.guidance_expanded()),
        run: project_run(run),
        note: project_note(session, run),
        footer: project_footer(session, run),
    })
}

fn project_guidance(guidance: &GuidanceSelection, expanded: bool) -> GuidanceDto {
    if guidance.is_empty() {
        return GuidanceDto::NothingFound {
            note: "No guidance files found. The review will judge the diff alone.".to_owned(),
        };
    }
    let count = guidance.included_count();
    let kilobytes = guidance.included_bytes() / 1024;
    let excluded = guidance.excluded_paths().len();
    GuidanceDto::Discovered {
        summary: if count == 0 {
            "No guidance will be sent".to_owned()
        } else {
            format!(
                "{count} guidance file{} \u{00B7} {kilobytes} KB",
                plural(count)
            )
        },
        expanded,
        entries: guidance
            .entries()
            .iter()
            .map(|entry| GuidanceEntryDto {
                path: entry.path().to_string(),
                scope: entry.excerpt.scope.to_string(),
                kilobytes: as_u32(entry.bytes() / 1024),
                included: entry.included,
            })
            .collect(),
        skipped: guidance
            .skipped()
            .iter()
            .map(|skip| GuidanceSkipDto {
                path: skip.path.to_string(),
                reason: skip.reason.to_string(),
            })
            .collect(),
        excluded: (excluded > 0).then(|| {
            format!(
                "{excluded} file{} excluded from review by .zreview.toml",
                plural(excluded)
            )
        }),
    }
}

/// The plural suffix for a count, so no line ever reads "1 files".
const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn project_run(run: &ReviewRunState) -> ReviewRunDto {
    match run {
        ReviewRunState::Idle => ReviewRunDto::Idle,
        ReviewRunState::Running { detail, .. } => ReviewRunDto::Running {
            detail: detail.clone(),
        },
        ReviewRunState::Complete {
            accepted,
            rejected,
            suppressed,
            ..
        } => ReviewRunDto::Complete {
            accepted: as_u32(*accepted),
            rejected: as_u32(*rejected),
            suppressed: as_u32(*suppressed),
        },
        ReviewRunState::Failed {
            summary,
            remediation,
        } => ReviewRunDto::Failed {
            summary: summary.clone(),
            remediation: remediation.clone(),
        },
    }
}

/// What to say when there is no finding to show.
///
/// Every run state says something. An empty panel with no explanation is the
/// failure this projection exists to avoid.
fn project_note(session: &ReviewSession, run: &ReviewRunState) -> Option<PanelNoteDto> {
    if !session.findings().is_empty() {
        return None;
    }
    let (heading, detail) = match run {
        ReviewRunState::Idle => (
            "No review has been run.".to_owned(),
            Some("Press Review to check this change against the repository's guidance.".to_owned()),
        ),
        ReviewRunState::Running { .. } => ("Reviewing...".to_owned(), None),
        ReviewRunState::Complete {
            rejected,
            suppressed,
            ..
        } => (
            "Nothing to act on.".to_owned(),
            Some(nothing_to_act_on(*rejected, *suppressed)),
        ),
        ReviewRunState::Failed {
            summary,
            remediation,
        } => (summary.clone(), remediation.clone()),
    };
    Some(PanelNoteDto { heading, detail })
}

/// Why a completed run left nothing to act on.
fn nothing_to_act_on(rejected: usize, suppressed: usize) -> String {
    match (rejected, suppressed) {
        (0, 0) => "The review found no problems.".to_owned(),
        (rejected, 0) => format!("{rejected} claim(s) did not survive checking against the diff."),
        (0, suppressed) => format!("{suppressed} previously dismissed claim(s) were hidden."),
        (rejected, suppressed) => format!(
            "{rejected} claim(s) did not check out and {suppressed} were previously dismissed."
        ),
    }
}

/// The caveats: what was refused, and what was never looked at.
fn project_footer(session: &ReviewSession, run: &ReviewRunState) -> Option<PanelFooterDto> {
    let rejected = session.findings().rejected().len();
    let refused = (rejected > 0).then(|| format!("{rejected} claim(s) refused"));
    let ReviewRunState::Complete { unreviewed, .. } = run else {
        return refused.map(|refused| PanelFooterDto {
            refused: Some(refused),
            not_reviewed: None,
            unreviewed: Vec::new(),
        });
    };
    if rejected == 0 && unreviewed.is_empty() {
        return None;
    }
    Some(PanelFooterDto {
        refused,
        not_reviewed: (!unreviewed.is_empty())
            .then(|| format!("{} file(s) not reviewed", unreviewed.len())),
        unreviewed: unreviewed.clone(),
    })
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

    /// Home with two clones configured, one of which did not resolve.
    fn home_with_one_failed_repository() -> app::HomeModel {
        let mut home = app::HomeModel::new();
        home.refreshed(Ok(vec![
            app::RepositoryEntry {
                path: std::path::PathBuf::from("/Developer/zreview"),
                outcome: app::RepositoryOutcome::Valid {
                    root: std::path::PathBuf::from("/Developer/zreview"),
                    slug: "braidonw/zreview".to_owned(),
                },
            },
            app::RepositoryEntry {
                path: std::path::PathBuf::from("/Developer/moved"),
                outcome: app::RepositoryOutcome::Failed {
                    reason: "the folder no longer exists".to_owned(),
                },
            },
        ]));
        home
    }

    #[test]
    fn home_projects_the_three_groups_in_order_with_their_empty_copy() {
        let snapshot = project_home(&app::HomeModel::new(), None);

        let titles = snapshot
            .groups
            .iter()
            .map(|group| group.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(titles, ["To review", "To address", "Waiting on others"]);
        assert!(snapshot.groups.iter().all(|group| group.count == 0));
        assert_eq!(snapshot.groups[1].empty_copy, "Nothing to address.");
    }

    #[test]
    fn home_projects_each_repository_with_its_slug_or_its_reason() {
        let snapshot = project_home(&home_with_one_failed_repository(), None);

        assert_eq!(snapshot.repositories[0].path, "/Developer/zreview");
        assert_eq!(
            snapshot.repositories[0].slug.as_deref(),
            Some("braidonw/zreview")
        );
        assert!(snapshot.repositories[0].failure.is_none());
        assert!(snapshot.repositories[1].slug.is_none());
        assert_eq!(
            snapshot.repositories[1].failure.as_deref(),
            Some("the folder no longer exists")
        );
        assert_eq!(snapshot.footer_summary, "2 repositories \u{b7} 1 failed");
        assert_eq!(
            snapshot.count_line.as_deref(),
            Some("0 pull requests across 2 repositories"),
        );
    }

    #[test]
    fn home_projects_the_whole_home_failure_and_the_footer_that_stays_with_it() {
        let mut home = app::HomeModel::new();
        home.refreshed(Err(SessionFailure::new(
            "Home could not read your settings",
        )));

        let snapshot = project_home(&home, None);

        assert_eq!(
            snapshot
                .failure
                .expect("the failure should be shown")
                .summary,
            "Home could not read your settings",
        );
        assert_eq!(snapshot.footer_summary, "No repositories");
        assert!(snapshot.count_line.is_none());
    }

    #[test]
    fn home_projects_a_rows_drafts_as_its_singular_or_plural_label_or_blank() {
        let mut home = app::HomeModel::new();
        home.refreshed(Ok(vec![app::RepositoryEntry {
            path: std::path::PathBuf::from("/Developer/widgets"),
            outcome: app::RepositoryOutcome::Valid {
                root: std::path::PathBuf::from("/Developer/widgets"),
                slug: "acme/widgets".to_owned(),
            },
        }]));
        home.batch_fetched(vec![app::RepositoryFetch {
            slug: "acme/widgets".to_owned(),
            outcome: Ok(vec![
                app::FetchedPullRequest {
                    search: app::HomeSearch::ReviewRequested,
                    repository: "acme/widgets".to_owned(),
                    number: 412,
                    title: "Retry webhook deliveries".to_owned(),
                    url: "https://github.com/acme/widgets/pull/412".to_owned(),
                    author_login: Some("mlee".to_owned()),
                    updated_at_ms: 100,
                    head_sha: "head".to_owned(),
                    viewer_latest_review_sha: None,
                    check_state: None,
                    review_decision: None,
                    changes_requested: false,
                    thread_awaiting_reply: false,
                },
                app::FetchedPullRequest {
                    search: app::HomeSearch::ReviewRequested,
                    repository: "acme/widgets".to_owned(),
                    number: 398,
                    title: "Split the renderer".to_owned(),
                    url: "https://github.com/acme/widgets/pull/398".to_owned(),
                    author_login: Some("priya".to_owned()),
                    updated_at_ms: 50,
                    head_sha: "head".to_owned(),
                    viewer_latest_review_sha: None,
                    check_state: None,
                    review_decision: None,
                    changes_requested: false,
                    thread_awaiting_reply: false,
                },
            ]),
        }]);
        home.drafts_read(Ok(std::collections::HashMap::from([(
            "github:acme/widgets#412".to_owned(),
            3,
        )])));

        let snapshot = project_home(&home, None);

        let by_identity = |identity: &str| {
            snapshot.groups[0]
                .rows
                .iter()
                .find(|row| row.identity == identity)
                .unwrap()
        };
        assert_eq!(
            by_identity("acme/widgets#412").drafts_label.as_deref(),
            Some("3 drafts")
        );
        assert_eq!(by_identity("acme/widgets#398").drafts_label, None);
    }

    #[test]
    fn home_projects_the_drafts_failure_beside_the_failed_repository_lines() {
        let mut home = app::HomeModel::new();
        home.drafts_read(Err(SessionFailure::new("Drafts could not be read")));

        let snapshot = project_home(&home, None);

        assert_eq!(
            snapshot
                .drafts_failure
                .expect("the failure should be shown")
                .summary,
            "Drafts could not be read",
        );
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
