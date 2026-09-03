//! What Home shows and every action that changes it, with no window in it.
//!
//! Home reads its repositories from a settings file and resolves each one to a
//! GitHub repository, but neither the file nor Git is touched here. A refresh
//! arrives already read and already validated, so this model is all decisions
//! and no I/O, and effects on the file come back as return values.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use domain::SessionFailure;

/// What resolving one clone found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepositoryOutcome {
    /// The worktree root the clone resolved to, and the GitHub repository its
    /// remotes name.
    Valid { root: PathBuf, slug: String },
    /// Why Home cannot list this clone.
    Failed { reason: String },
}

/// One clone and what resolving it found.
///
/// The path is the one the settings file lists, or the one the picker returned,
/// which is not always the worktree root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryEntry {
    pub path: PathBuf,
    pub outcome: RepositoryOutcome,
}

impl RepositoryEntry {
    /// The GitHub repository this clone points at, absent when it did not resolve.
    #[must_use]
    pub fn slug(&self) -> Option<&str> {
        match &self.outcome {
            RepositoryOutcome::Valid { slug, .. } => Some(slug),
            RepositoryOutcome::Failed { .. } => None,
        }
    }

    /// Why this clone cannot be listed, absent when it resolved.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match &self.outcome {
            RepositoryOutcome::Valid { .. } => None,
            RepositoryOutcome::Failed { reason } => Some(reason),
        }
    }

    /// The worktree root, when there is one, which is what identifies a clone.
    fn root(&self) -> Option<&Path> {
        match &self.outcome {
            RepositoryOutcome::Valid { root, .. } => Some(root),
            RepositoryOutcome::Failed { .. } => None,
        }
    }
}

/// Something Home would not do, and why.
///
/// Replaced by the next Add, so a selection that was partly refused says which
/// part and what was wrong with it until the reviewer picks again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refusal {
    /// The folder that was picked, or the settings file when the action itself
    /// was refused.
    pub path: PathBuf,
    pub reason: String,
}

/// The repository list to write to the settings file, in the order it goes in.
///
/// Handed back rather than written here, so this model never touches the file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsWrite {
    pub repositories: Vec<PathBuf>,
}

/// What a pull request wants from the reviewer, which is what Home groups by.
///
/// The groups are always rendered, and always in this order, so an empty one
/// states that nothing is waiting rather than leaving it to be inferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HomeGroup {
    ToReview,
    ToAddress,
    WaitingOnOthers,
}

impl HomeGroup {
    pub const ALL: [Self; 3] = [Self::ToReview, Self::ToAddress, Self::WaitingOnOthers];

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::ToReview => "To review",
            Self::ToAddress => "To address",
            Self::WaitingOnOthers => "Waiting on others",
        }
    }

    /// The one line an empty group shows in place of rows.
    #[must_use]
    pub const fn empty_copy(self) -> &'static str {
        match self {
            Self::ToReview => "Nothing waiting for your review.",
            Self::ToAddress => "Nothing to address.",
            Self::WaitingOnOthers => "Nothing waiting on others.",
        }
    }
}

/// Which of Home's two searches a pull request came back from.
///
/// The search is what decides whether a row is one the reviewer was asked to
/// look at or one of their own, which no field on the pull request says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HomeSearch {
    ReviewRequested,
    Authored,
}

/// GitHub's combined state for every check on a head commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckState {
    Expected,
    Error,
    Failure,
    Pending,
    Success,
}

/// GitHub's standing verdict on a pull request, folding in branch protection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewDecision {
    ChangesRequested,
    Approved,
    ReviewRequired,
}

/// One fetched pull request, carrying only what grouping and rows read.
///
/// Deliberately not the forge's own type. Whoever fetches reduces threads and
/// reviews to the two questions grouping asks of them, which keeps this model
/// free of the API those answers came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchedPullRequest {
    pub search: HomeSearch,
    /// `owner/name`, which is also what the row's identity reads.
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    /// `None` once GitHub has forgotten the account.
    pub author_login: Option<String>,
    pub updated_at_ms: i64,
    pub head_sha: String,
    /// The commit the viewer's own latest review was left against.
    pub viewer_latest_review_sha: Option<String>,
    /// `None` when the head has no checks at all, which reads differently from
    /// a pending one.
    pub check_state: Option<CheckState>,
    pub review_decision: Option<ReviewDecision>,
    /// Whether any standing review asks the author for changes.
    pub changes_requested: bool,
    /// Whether any unresolved thread was last spoken in by somebody else.
    pub thread_awaiting_reply: bool,
}

/// What one repository's fetch found, or why it found nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryFetch {
    pub slug: String,
    pub outcome: Result<Vec<FetchedPullRequest>, String>,
}

/// What a row says about its checks, or nothing when there are none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckStatus {
    Passing,
    Failing,
    Running,
}

impl CheckStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Passing => "checks passing",
            Self::Failing => "checks failing",
            Self::Running => "checks running",
        }
    }
}

/// What a row says about its reviews, or nothing when it has none to report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewStatus {
    ChangesRequested,
    Approved,
    ReviewedThisHead,
}

impl ReviewStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ChangesRequested => "changes requested",
            Self::Approved => "approved",
            Self::ReviewedThisHead => "you reviewed this head",
        }
    }
}

/// One pull request as Home lists it, in the group it belongs to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HomeRow {
    pub group: HomeGroup,
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author_login: Option<String>,
    pub updated_at_ms: i64,
    pub review_status: Option<ReviewStatus>,
    pub check_status: Option<CheckStatus>,
    /// Unsent Drafts on this pull request, regardless of which head they were
    /// written against. Absent means none, and a store failure blanks it too,
    /// so neither reads as a zero.
    pub drafts: Option<usize>,
}

impl HomeRow {
    /// `owner/name#number`, which is how a row names its pull request.
    #[must_use]
    pub fn identity(&self) -> String {
        format!("{}#{}", self.repository, self.number)
    }

    /// The scope this row's Drafts are stored under.
    #[must_use]
    pub fn draft_scope(&self) -> String {
        domain::github_draft_scope(&self.repository, self.number)
    }

    /// "1 draft" or "N drafts", the words the badge shows, absent for a blank cell.
    #[must_use]
    pub fn drafts_label(&self) -> Option<String> {
        self.drafts.map(|count| {
            if count == 1 {
                "1 draft".to_owned()
            } else {
                format!("{count} drafts")
            }
        })
    }
}

/// One configured repository whose pull requests could not be fetched.
///
/// Named by path as well as by slug, because the line it shows carries Remove,
/// and Remove names the entry the settings file holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchFailure {
    pub path: PathBuf,
    pub slug: String,
    pub reason: String,
}

/// How current the list is, which the header stamp reads.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RefreshState {
    /// Nothing has been fetched yet, so there is no stamp to show.
    #[default]
    NeverRefreshed,
    Refreshing {
        done: usize,
        total: usize,
    },
    /// Epoch milliseconds at which the last refresh settled.
    Refreshed {
        at_ms: i64,
    },
    /// Nothing loaded at all, from the preflight, the settings file, or every
    /// configured repository failing.
    Failed,
}

/// Everything Home is, and every action that changes it.
///
/// Held behind a lock and shared by whatever is displaying it, as the session
/// model is.
#[derive(Debug, Default)]
pub struct HomeModel {
    repositories: Vec<RepositoryEntry>,
    settings_failure: Option<SessionFailure>,
    /// What stopped the last refresh, from the preflight or from the refresh
    /// giving up part way.
    refresh_failure: Option<SessionFailure>,
    write_failure: Option<SessionFailure>,
    /// Why the last Drafts read failed, if it did. Left standing until a read
    /// that succeeds, unlike a row's own count, which a failure blanks at once.
    drafts_failure: Option<SessionFailure>,
    footer_expanded: bool,
    refusals: Vec<Refusal>,
    /// Every listed row, in the order Home renders them, which is also the
    /// order the cursor walks.
    rows: Vec<HomeRow>,
    fetch_failures: Vec<FetchFailure>,
    refresh: RefreshState,
    cursor: usize,
    /// How many repositories this refresh has loaded and lost, which is what
    /// separates "nothing loaded" from a list with a hole in it.
    loaded_repositories: usize,
    failed_repositories: usize,
}

impl HomeModel {
    /// Starts empty, before the settings file has been read.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes what a refresh read, or the failure that stopped it reading.
    ///
    /// A failed read empties the list and ends the refresh as failed, because
    /// everything in the list came from the file that could not be read. A read
    /// that worked drops whatever a repository no longer listed had contributed.
    pub fn refreshed(&mut self, result: Result<Vec<RepositoryEntry>, SessionFailure>) {
        match result {
            Ok(repositories) => {
                self.repositories = repositories;
                self.settings_failure = None;
                self.drop_unlisted_rows();
            }
            Err(failure) => {
                self.repositories.clear();
                self.settings_failure = Some(failure);
                self.rows.clear();
                self.fetch_failures.clear();
                self.refresh = RefreshState::Failed;
            }
        }
        self.clamp_cursor();
    }

    /// Starts a refresh, clearing what stopped the last one.
    ///
    /// Whoever calls this has already established that no other refresh is
    /// running. The guard that orders Home's actions is what decides that, and
    /// a second gate here could only ever disagree with it.
    pub fn begin_refresh(&mut self) {
        self.refresh_failure = None;
        self.refresh = RefreshState::Refreshing { done: 0, total: 0 };
    }

    /// Stops the refresh before it fetched anything, `gh` being unusable.
    ///
    /// The list stays as it was, because nothing about it has been disproved.
    /// It is the failure block in front of it that says why it is not moving.
    pub fn preflight_failed(&mut self, failure: SessionFailure) {
        self.refresh_failure = Some(failure);
        self.refresh = RefreshState::Failed;
    }

    /// Ends a refresh that stopped without ever finishing.
    ///
    /// Called when a refresh is abandoned rather than completed, so the stamp
    /// stops claiming that repositories are still being counted off. A refresh
    /// that did finish is left exactly as it left itself.
    pub fn refresh_abandoned(&mut self) {
        if !matches!(self.refresh, RefreshState::Refreshing { .. }) {
            return;
        }
        self.refresh_failure = Some(
            SessionFailure::new("Home could not finish the refresh")
                .with_remediation("Press r to try again."),
        );
        self.refresh = RefreshState::Failed;
    }

    /// Says how many repositories this refresh is about to fetch.
    ///
    /// The rows the last refresh listed stay up. Each repository replaces its
    /// own as it answers, so a reviewer is never left reading a blank list
    /// while GitHub is asked about it again.
    pub fn fetching(&mut self, total: usize) {
        self.loaded_repositories = 0;
        self.failed_repositories = 0;
        self.refresh = RefreshState::Refreshing { done: 0, total };
    }

    /// Takes what one batch of repositories found, advancing the progress count.
    ///
    /// A repository's answer replaces everything the last refresh had from it,
    /// and nothing from anywhere else.
    pub fn batch_fetched(&mut self, batch: Vec<RepositoryFetch>) {
        for fetch in batch {
            self.rows
                .retain(|row| !same_slug(&row.repository, &fetch.slug));
            self.fetch_failures
                .retain(|failure| !same_slug(&failure.slug, &fetch.slug));
            match fetch.outcome {
                Ok(pull_requests) => {
                    self.loaded_repositories += 1;
                    self.rows
                        .extend(one_row_each(pull_requests).into_iter().map(into_row));
                }
                Err(reason) => {
                    self.failed_repositories += 1;
                    self.record_fetch_failure(&fetch.slug, &reason);
                }
            }
            self.advance_progress();
        }
        self.order_rows();
        self.clamp_cursor();
    }

    /// Ends the refresh, settled at `at_ms` or failed with nothing to show.
    ///
    /// A clone that never resolved counts against it as much as one that could
    /// not be fetched, because either way it listed nothing. A refresh with
    /// nothing configured at all is not a failure. There was nothing to fail.
    pub fn finish_refresh(&mut self, at_ms: i64) {
        let lost = self.failed_repositories + self.unresolved_count();
        self.refresh = if self.loaded_repositories == 0 && lost > 0 {
            RefreshState::Failed
        } else {
            RefreshState::Refreshed { at_ms }
        };
    }

    /// How many configured clones never resolved to a repository at all.
    fn unresolved_count(&self) -> usize {
        self.repositories
            .iter()
            .filter(|entry| entry.slug().is_none())
            .count()
    }

    #[must_use]
    pub const fn refresh_state(&self) -> RefreshState {
        self.refresh
    }

    /// Every listed row, in the order Home renders them.
    #[must_use]
    pub fn rows(&self) -> &[HomeRow] {
        &self.rows
    }

    /// The rows in one group, newest updated first.
    pub fn rows_in(&self, group: HomeGroup) -> impl Iterator<Item = &HomeRow> {
        self.rows.iter().filter(move |row| row.group == group)
    }

    /// Every configured repository this refresh could not fetch.
    #[must_use]
    pub fn fetch_failures(&self) -> &[FetchFailure] {
        &self.fetch_failures
    }

    /// Why the clone listed at `path` shows nothing, from resolving it or from
    /// fetching it.
    #[must_use]
    pub fn repository_failure(&self, path: &Path) -> Option<&str> {
        self.repositories
            .iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| {
                entry.reason().or_else(|| {
                    let slug = entry.slug()?;
                    self.fetch_failures
                        .iter()
                        .find(|failure| same_slug(&failure.slug, slug))
                        .map(|failure| failure.reason.as_str())
                })
            })
    }

    /// Where the cursor sits, as a flat index across every rendered row.
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Moves the cursor one row down, stopping at the last one.
    pub fn move_cursor_down(&mut self) {
        self.cursor = (self.cursor + 1).min(self.rows.len().saturating_sub(1));
    }

    /// Moves the cursor one row up, stopping at the first one.
    pub const fn move_cursor_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Files every fetched repository's failure against every clone that points
    /// at it, so two checkouts of one repository each say why they are empty.
    fn record_fetch_failure(&mut self, slug: &str, reason: &str) {
        let paths = self
            .repositories
            .iter()
            .filter(|entry| entry.slug().is_some_and(|listed| same_slug(listed, slug)))
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        self.fetch_failures
            .extend(paths.into_iter().map(|path| FetchFailure {
                path,
                slug: slug.to_owned(),
                reason: reason.to_owned(),
            }));
    }

    fn advance_progress(&mut self) {
        if let RefreshState::Refreshing { done, total } = self.refresh {
            self.refresh = RefreshState::Refreshing {
                done: (done + 1).min(total),
                total,
            };
        }
    }

    /// Groups in their fixed order, and within a group the newest update first.
    fn order_rows(&mut self) {
        self.rows.sort_by(|left, right| {
            group_order(left.group)
                .cmp(&group_order(right.group))
                .then(right.updated_at_ms.cmp(&left.updated_at_ms))
        });
    }

    /// Drops what a repository that is no longer configured contributed.
    ///
    /// A removed clone's rows would otherwise sit in the list until the next
    /// refresh, promising pull requests from somewhere Home no longer reads.
    fn drop_unlisted_rows(&mut self) {
        let listed = self
            .repositories
            .iter()
            .filter_map(RepositoryEntry::slug)
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        self.rows
            .retain(|row| listed.iter().any(|slug| same_slug(slug, &row.repository)));
        self.fetch_failures
            .retain(|failure| listed.iter().any(|slug| same_slug(slug, &failure.slug)));
    }

    fn clamp_cursor(&mut self) {
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    /// Takes what came of writing the settings file.
    ///
    /// Kept apart from [`Self::failure`] because a file that could be read but
    /// not written leaves a list worth showing. The failure is a line above it,
    /// not a screen in front of it, and both can be true at once.
    pub fn write_finished(&mut self, result: Result<(), SessionFailure>) {
        self.write_failure = result.err();
    }

    #[must_use]
    pub fn repositories(&self) -> &[RepositoryEntry] {
        &self.repositories
    }

    /// What stops Home listing anything at all, if anything does.
    ///
    /// A settings file that could not be read, no home directory to look for one
    /// in, or a `gh` the refresh could not get past. A write that failed is
    /// [`Self::write_failure`], which leaves a list worth showing.
    #[must_use]
    pub fn failure(&self) -> Option<&SessionFailure> {
        self.settings_failure
            .as_ref()
            .or(self.refresh_failure.as_ref())
    }

    /// Why the last write did not reach the file, if it did not.
    #[must_use]
    pub const fn write_failure(&self) -> Option<&SessionFailure> {
        self.write_failure.as_ref()
    }

    /// Takes what a refresh's Drafts read found, keyed by [`HomeRow::draft_scope`],
    /// or the failure that stopped it reading.
    ///
    /// A failure blanks every row rather than leaving stale counts standing, so
    /// a Drafts column never shows a number the store could not back up. A
    /// scope absent from a successful read means that pull request has no
    /// Drafts, not that its column keeps whatever it last showed.
    pub fn drafts_read(&mut self, result: Result<HashMap<String, usize>, SessionFailure>) {
        match result {
            Ok(counts) => {
                for row in &mut self.rows {
                    row.drafts = counts.get(&row.draft_scope()).copied();
                }
                self.drafts_failure = None;
            }
            Err(failure) => {
                for row in &mut self.rows {
                    row.drafts = None;
                }
                self.drafts_failure = Some(failure);
            }
        }
    }

    /// Why the last Drafts read failed, if it did.
    #[must_use]
    pub fn drafts_failure(&self) -> Option<&SessionFailure> {
        self.drafts_failure.as_ref()
    }

    /// How many configured clones list nothing, from either cause.
    #[must_use]
    pub fn failed_count(&self) -> usize {
        self.repositories
            .iter()
            .filter(|entry| self.repository_failure(&entry.path).is_some())
            .count()
    }

    /// The header's count line, absent while there is nothing to count.
    #[must_use]
    pub fn count_line(&self) -> Option<String> {
        (!self.repositories.is_empty()).then(|| {
            format!(
                "{} across {}",
                pull_request_count(self.rows.len()),
                repository_count(self.repositories.len()),
            )
        })
    }

    /// Takes the folders a reviewer picked, saying what the file should now hold.
    ///
    /// Every folder that resolved and is not already listed is accepted, so one
    /// bad pick in a selection costs only itself. A folder already listed is
    /// ignored rather than refused, because re-adding a clone is harmless and
    /// says nothing a reviewer needs to act on. Nothing is written when nothing
    /// was accepted.
    #[must_use]
    pub fn add_repositories(
        &mut self,
        settings_path: &Path,
        picked: Vec<RepositoryEntry>,
    ) -> Option<SettingsWrite> {
        if let Some(refusal) = self.unreadable_settings_refusal(settings_path) {
            self.refusals = vec![refusal];
            return None;
        }
        // This selection's refusals replace the last one's, which the reviewer
        // has now answered by picking again.
        self.refusals.clear();
        let mut repositories = self
            .repositories
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let mut accepted = 0_usize;
        for entry in picked {
            match entry.outcome {
                RepositoryOutcome::Valid { root, .. } => {
                    if !self.is_listed(&root) && !repositories.contains(&root) {
                        repositories.push(root);
                        accepted += 1;
                    }
                }
                RepositoryOutcome::Failed { reason } => self.refusals.push(Refusal {
                    path: entry.path,
                    reason,
                }),
            }
        }
        (accepted > 0).then_some(SettingsWrite { repositories })
    }

    /// Drops the entry listed at `path`, saying what the file should now hold.
    ///
    /// Named by path rather than by position, so a click on a list that has been
    /// refreshed underneath it removes the repository it was pointing at or
    /// none at all. Nothing is written when the path is no longer listed.
    #[must_use]
    pub fn remove_repository(
        &mut self,
        settings_path: &Path,
        path: &Path,
    ) -> Option<SettingsWrite> {
        if let Some(refusal) = self.unreadable_settings_refusal(settings_path) {
            self.refusals = vec![refusal];
            return None;
        }
        let repositories = self
            .repositories
            .iter()
            .filter(|entry| entry.path != path)
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        (repositories.len() < self.repositories.len()).then_some(SettingsWrite { repositories })
    }

    /// What the last action would not do, with the reason for each.
    #[must_use]
    pub fn refusals(&self) -> &[Refusal] {
        &self.refusals
    }

    /// Refuses to write over a settings file Home could not read.
    ///
    /// A write replaces the whole file, and what an unreadable one holds is
    /// unknown, so writing would silently drop repositories Home never saw.
    fn unreadable_settings_refusal(&self, settings_path: &Path) -> Option<Refusal> {
        self.settings_failure.is_some().then(|| Refusal {
            path: settings_path.to_path_buf(),
            reason: "fix this file before changing your repositories".to_owned(),
        })
    }

    /// Whether `root` is a clone Home already lists.
    ///
    /// Compared against both the resolved root and the listed path, so a clone
    /// whose entry did not resolve is still recognised as itself.
    fn is_listed(&self, root: &Path) -> bool {
        self.repositories
            .iter()
            .any(|entry| entry.root() == Some(root) || entry.path == root)
    }

    /// Whether the footer is showing every repository or just its summary line.
    ///
    /// Not persisted. It is a decision about this sitting, and a refresh leaves
    /// it alone so the list does not fold up under whoever is reading it.
    #[must_use]
    pub const fn is_footer_expanded(&self) -> bool {
        self.footer_expanded
    }

    pub const fn toggle_footer(&mut self) {
        self.footer_expanded = !self.footer_expanded;
    }

    /// The one line the collapsed footer shows.
    #[must_use]
    pub fn footer_summary(&self) -> String {
        if self.repositories.is_empty() {
            return "No repositories".to_owned();
        }
        let counted = repository_count(self.repositories.len());
        match self.failed_count() {
            0 => counted,
            failed => format!("{counted} \u{00B7} {failed} failed"),
        }
    }
}

/// "1 repository" or "4 repositories", as the count requires.
fn repository_count(count: usize) -> String {
    if count == 1 {
        "1 repository".to_owned()
    } else {
        format!("{count} repositories")
    }
}

/// "1 pull request" or "8 pull requests", as the count requires.
fn pull_request_count(count: usize) -> String {
    if count == 1 {
        "1 pull request".to_owned()
    } else {
        format!("{count} pull requests")
    }
}

/// Where a group sits in the fixed order Home renders them in.
fn group_order(group: HomeGroup) -> usize {
    HomeGroup::ALL
        .iter()
        .position(|listed| *listed == group)
        .expect("every group is in ALL")
}

/// Whether two slugs name one repository, which GitHub decides without regard
/// to case, so nothing downstream of it may either.
fn same_slug(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// Keeps one row per pull request, preferring what the authored search said.
///
/// A pull request the reviewer wrote and is also asked to review comes back
/// from both searches. It is theirs to move along, so the authored row is the
/// one that survives, and the spec puts it in the authored groups.
fn one_row_each(pull_requests: Vec<FetchedPullRequest>) -> Vec<FetchedPullRequest> {
    let authored = pull_requests
        .iter()
        .filter(|row| row.search == HomeSearch::Authored)
        .map(|row| (row.repository.clone(), row.number))
        .collect::<Vec<_>>();
    pull_requests
        .into_iter()
        .filter(|row| {
            row.search == HomeSearch::Authored
                || !authored.iter().any(|(repository, number)| {
                    *number == row.number && same_slug(repository, &row.repository)
                })
        })
        .collect()
}

/// Which group a fetched pull request belongs to.
///
/// The search it came back from decides first. Only the reviewer's own pull
/// requests can be waiting on them, and everything asked of them is to review
/// whether or not they have looked at it before.
fn group_of(fetched: &FetchedPullRequest) -> HomeGroup {
    match fetched.search {
        HomeSearch::ReviewRequested => HomeGroup::ToReview,
        HomeSearch::Authored => {
            if fetched.changes_requested || fetched.thread_awaiting_reply {
                HomeGroup::ToAddress
            } else {
                HomeGroup::WaitingOnOthers
            }
        }
    }
}

/// What a row says about its checks, absent when the head has none.
const fn check_status_of(state: Option<CheckState>) -> Option<CheckStatus> {
    match state {
        Some(CheckState::Success) => Some(CheckStatus::Passing),
        Some(CheckState::Failure | CheckState::Error) => Some(CheckStatus::Failing),
        Some(CheckState::Pending | CheckState::Expected) => Some(CheckStatus::Running),
        None => None,
    }
}

/// What a row says about its reviews, absent when it has nothing to report.
///
/// A standing request for changes outranks an approval, and both outrank the
/// reviewer's own note that they have already looked at this head.
fn review_status_of(fetched: &FetchedPullRequest) -> Option<ReviewStatus> {
    if fetched.changes_requested {
        return Some(ReviewStatus::ChangesRequested);
    }
    match fetched.review_decision {
        Some(ReviewDecision::ChangesRequested) => return Some(ReviewStatus::ChangesRequested),
        Some(ReviewDecision::Approved) => return Some(ReviewStatus::Approved),
        // Nobody has reviewed it, which is not something a row has to say.
        Some(ReviewDecision::ReviewRequired) | None => {}
    }
    let reviewed_this_head = fetched
        .viewer_latest_review_sha
        .as_ref()
        .is_some_and(|sha| *sha == fetched.head_sha);
    reviewed_this_head.then_some(ReviewStatus::ReviewedThisHead)
}

fn into_row(fetched: FetchedPullRequest) -> HomeRow {
    HomeRow {
        group: group_of(&fetched),
        review_status: review_status_of(&fetched),
        check_status: check_status_of(fetched.check_state),
        repository: fetched.repository,
        number: fetched.number,
        title: fetched.title,
        url: fetched.url,
        author_login: fetched.author_login,
        updated_at_ms: fetched.updated_at_ms,
        drafts: None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    /// A clone that resolved, listed by the path the settings file holds.
    fn valid(path: &str, slug: &str) -> RepositoryEntry {
        RepositoryEntry {
            path: PathBuf::from(path),
            outcome: RepositoryOutcome::Valid {
                root: PathBuf::from(path),
                slug: slug.to_owned(),
            },
        }
    }

    /// A clone that could not be resolved, with the reason a reviewer sees.
    fn failed(path: &str, reason: &str) -> RepositoryEntry {
        RepositoryEntry {
            path: PathBuf::from(path),
            outcome: RepositoryOutcome::Failed {
                reason: reason.to_owned(),
            },
        }
    }

    /// One fetched pull request, with nothing standing between it and its group.
    fn fetched(search: HomeSearch, number: u64, updated_at_ms: i64) -> FetchedPullRequest {
        FetchedPullRequest {
            search,
            repository: "acme/widgets".to_owned(),
            number,
            title: format!("Pull request {number}"),
            url: format!("https://github.com/acme/widgets/pull/{number}"),
            author_login: Some("mlee".to_owned()),
            updated_at_ms,
            head_sha: "head".to_owned(),
            viewer_latest_review_sha: None,
            check_state: None,
            review_decision: None,
            changes_requested: false,
            thread_awaiting_reply: false,
        }
    }

    /// One repository's fetch, having found `rows`.
    fn loaded(rows: Vec<FetchedPullRequest>) -> RepositoryFetch {
        RepositoryFetch {
            slug: "acme/widgets".to_owned(),
            outcome: Ok(rows),
        }
    }

    #[test]
    fn every_row_from_the_review_requested_search_lands_in_to_review() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![valid("/Developer/widgets", "acme/widgets")]));

        home.batch_fetched(vec![loaded(vec![
            fetched(HomeSearch::ReviewRequested, 412, 200),
            fetched(HomeSearch::ReviewRequested, 398, 100),
        ])]);

        let rows = home.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.group == HomeGroup::ToReview));
    }

    /// Home with one repository configured, listing whatever it fetched.
    fn listing(rows: Vec<FetchedPullRequest>) -> HomeModel {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![valid("/Developer/widgets", "acme/widgets")]));
        home.batch_fetched(vec![loaded(rows)]);
        home
    }

    /// The single row a test that built one expects to find.
    fn only_row(home: &HomeModel) -> &HomeRow {
        assert_eq!(home.rows().len(), 1, "exactly one row was fetched");
        &home.rows()[0]
    }

    /// A pull request the reviewer has already looked at, at this very head.
    #[test]
    fn a_review_request_already_answered_at_this_head_still_lands_in_to_review() {
        let mut already_reviewed = fetched(HomeSearch::ReviewRequested, 398, 100);
        already_reviewed.viewer_latest_review_sha = Some("head".to_owned());

        let home = listing(vec![already_reviewed]);

        let row = only_row(&home);
        assert_eq!(row.group, HomeGroup::ToReview);
        assert_eq!(row.review_status, Some(ReviewStatus::ReviewedThisHead));
    }

    #[test]
    fn a_review_left_against_an_earlier_head_is_no_row_status_at_all() {
        let mut reviewed_before = fetched(HomeSearch::ReviewRequested, 398, 100);
        reviewed_before.viewer_latest_review_sha = Some("an older commit".to_owned());

        let home = listing(vec![reviewed_before]);

        assert_eq!(only_row(&home).review_status, None);
    }

    #[test]
    fn an_authored_pull_request_with_a_standing_changes_requested_review_is_to_address() {
        let mut changes_requested = fetched(HomeSearch::Authored, 412, 100);
        changes_requested.changes_requested = true;

        let home = listing(vec![changes_requested]);

        let row = only_row(&home);
        assert_eq!(row.group, HomeGroup::ToAddress);
        assert_eq!(row.review_status, Some(ReviewStatus::ChangesRequested));
    }

    #[test]
    fn an_authored_pull_request_whose_unresolved_thread_someone_else_answered_last_is_to_address() {
        let mut awaiting_reply = fetched(HomeSearch::Authored, 412, 100);
        awaiting_reply.thread_awaiting_reply = true;

        let home = listing(vec![awaiting_reply]);

        assert_eq!(only_row(&home).group, HomeGroup::ToAddress);
    }

    /// The reviewer having spoken last is what takes it off their hands, which
    /// is the whole difference between the two authored groups.
    #[test]
    fn an_authored_pull_request_whose_unresolved_thread_the_viewer_answered_last_is_waiting() {
        let answered = fetched(HomeSearch::Authored, 412, 100);

        let home = listing(vec![answered]);

        assert_eq!(only_row(&home).group, HomeGroup::WaitingOnOthers);
    }

    #[test]
    fn every_other_authored_pull_request_is_waiting_on_others() {
        let mut approved = fetched(HomeSearch::Authored, 412, 100);
        approved.review_decision = Some(ReviewDecision::Approved);
        approved.check_state = Some(CheckState::Success);

        let home = listing(vec![approved]);

        let row = only_row(&home);
        assert_eq!(row.group, HomeGroup::WaitingOnOthers);
        assert_eq!(row.review_status, Some(ReviewStatus::Approved));
        assert_eq!(row.check_status, Some(CheckStatus::Passing));
    }

    #[test]
    fn rows_are_grouped_in_a_fixed_order_and_newest_updated_first_within_each() {
        let mut changes_requested = fetched(HomeSearch::Authored, 1, 100);
        changes_requested.changes_requested = true;

        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![valid("/Developer/widgets", "acme/widgets")]));
        home.batch_fetched(vec![loaded(vec![
            fetched(HomeSearch::Authored, 2, 300),
            fetched(HomeSearch::ReviewRequested, 3, 200),
            changes_requested,
            fetched(HomeSearch::ReviewRequested, 4, 400),
        ])]);

        let listed = home
            .rows()
            .iter()
            .map(|row| (row.group, row.number))
            .collect::<Vec<_>>();
        assert_eq!(
            listed,
            [
                (HomeGroup::ToReview, 4),
                (HomeGroup::ToReview, 3),
                (HomeGroup::ToAddress, 1),
                (HomeGroup::WaitingOnOthers, 2),
            ],
        );
    }

    #[test]
    fn a_row_names_its_pull_request_as_owner_name_and_number() {
        let home = listing(vec![fetched(HomeSearch::ReviewRequested, 412, 100)]);

        assert_eq!(only_row(&home).identity(), "acme/widgets#412");
    }

    #[test]
    fn every_check_state_maps_to_one_of_three_statuses_or_a_gap() {
        let mapped = [
            (Some(CheckState::Success), Some(CheckStatus::Passing)),
            (Some(CheckState::Failure), Some(CheckStatus::Failing)),
            (Some(CheckState::Error), Some(CheckStatus::Failing)),
            (Some(CheckState::Pending), Some(CheckStatus::Running)),
            (Some(CheckState::Expected), Some(CheckStatus::Running)),
            (None, None),
        ];

        for (state, expected) in mapped {
            let mut row = fetched(HomeSearch::ReviewRequested, 412, 100);
            row.check_state = state;

            let home = listing(vec![row]);

            assert_eq!(only_row(&home).check_status, expected, "{state:?}");
        }
    }

    /// Precedence, top down. A request for changes is what the author has to
    /// act on, and it says so even where GitHub's own decision has moved on.
    #[test]
    fn a_standing_request_for_changes_outranks_an_approval_and_a_review_of_this_head() {
        let mut contested = fetched(HomeSearch::Authored, 412, 100);
        contested.changes_requested = true;
        contested.review_decision = Some(ReviewDecision::Approved);
        contested.viewer_latest_review_sha = Some("head".to_owned());

        let home = listing(vec![contested]);

        assert_eq!(
            only_row(&home).review_status,
            Some(ReviewStatus::ChangesRequested),
        );
    }

    #[test]
    fn an_approval_outranks_a_review_of_this_head() {
        let mut approved = fetched(HomeSearch::Authored, 412, 100);
        approved.review_decision = Some(ReviewDecision::Approved);
        approved.viewer_latest_review_sha = Some("head".to_owned());

        let home = listing(vec![approved]);

        assert_eq!(only_row(&home).review_status, Some(ReviewStatus::Approved));
    }

    /// Nobody has reviewed it, which is not something a row has to say.
    #[test]
    fn a_review_still_required_leaves_the_review_column_empty() {
        let mut untouched = fetched(HomeSearch::Authored, 412, 100);
        untouched.review_decision = Some(ReviewDecision::ReviewRequired);

        let home = listing(vec![untouched]);

        assert_eq!(only_row(&home).review_status, None);
    }

    /// One repository's fetch that could not be made.
    fn unreachable(slug: &str, reason: &str) -> RepositoryFetch {
        RepositoryFetch {
            slug: slug.to_owned(),
            outcome: Err(reason.to_owned()),
        }
    }

    /// One other repository's fetch, having found one pull request.
    fn elsewhere(slug: &str, number: u64, updated_at_ms: i64) -> RepositoryFetch {
        RepositoryFetch {
            slug: slug.to_owned(),
            outcome: Ok(vec![FetchedPullRequest {
                repository: slug.to_owned(),
                ..fetched(HomeSearch::ReviewRequested, number, updated_at_ms)
            }]),
        }
    }

    /// One repository's fetch that found nothing, which is not a failure.
    fn empty(slug: &str) -> RepositoryFetch {
        RepositoryFetch {
            slug: slug.to_owned(),
            outcome: Ok(Vec::new()),
        }
    }

    /// Four configured clones, which is what a batch's progress counts against.
    fn four_repositories() -> HomeModel {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![
            valid("/Developer/zreview", "braidonw/zreview"),
            valid("/Developer/widgets", "acme/widgets"),
            valid("/Developer/billing", "acme/billing"),
            valid("/Developer/tokens", "acme/design-tokens"),
        ]));
        home
    }

    #[test]
    fn there_is_no_stamp_at_all_before_the_first_refresh() {
        let home = HomeModel::new();

        assert_eq!(home.refresh_state(), RefreshState::NeverRefreshed);
    }

    #[test]
    fn a_refresh_counts_off_the_repositories_as_each_batch_lands() {
        let mut home = four_repositories();

        home.begin_refresh();
        assert_eq!(
            home.refresh_state(),
            RefreshState::Refreshing { done: 0, total: 0 },
            "the total is unknown until the settings file has been read",
        );

        home.fetching(4);
        assert_eq!(
            home.refresh_state(),
            RefreshState::Refreshing { done: 0, total: 4 },
        );

        home.batch_fetched(vec![empty("braidonw/zreview"), empty("acme/widgets")]);
        assert_eq!(
            home.refresh_state(),
            RefreshState::Refreshing { done: 2, total: 4 },
        );

        home.batch_fetched(vec![empty("acme/billing"), empty("acme/design-tokens")]);
        home.finish_refresh(1_700_000_000_000);
        assert_eq!(
            home.refresh_state(),
            RefreshState::Refreshed {
                at_ms: 1_700_000_000_000,
            },
        );
    }

    #[test]
    fn a_refresh_where_every_repository_failed_reads_as_failed() {
        let mut home = four_repositories();
        home.begin_refresh();
        home.fetching(2);

        home.batch_fetched(vec![
            unreachable("braidonw/zreview", "GitHub could not be reached"),
            unreachable("acme/widgets", "GitHub could not be reached"),
        ]);
        home.finish_refresh(1_700_000_000_000);

        assert_eq!(home.refresh_state(), RefreshState::Failed);
    }

    /// One repository still answering is a list with a hole in it, not a
    /// failure, and the stamp says when it was fetched.
    #[test]
    fn a_refresh_where_one_repository_answered_still_settles() {
        let mut home = four_repositories();
        home.begin_refresh();
        home.fetching(2);

        home.batch_fetched(vec![
            unreachable("braidonw/zreview", "GitHub refused access"),
            empty("acme/widgets"),
        ]);
        home.finish_refresh(1_700_000_000_000);

        assert_eq!(
            home.refresh_state(),
            RefreshState::Refreshed {
                at_ms: 1_700_000_000_000,
            },
        );
    }

    /// Nothing was configured, so nothing failed.
    #[test]
    fn a_refresh_with_nothing_configured_settles_rather_than_failing() {
        let mut home = HomeModel::new();
        home.begin_refresh();
        home.refreshed(Ok(Vec::new()));
        home.fetching(0);

        home.finish_refresh(1_700_000_000_000);

        assert_eq!(
            home.refresh_state(),
            RefreshState::Refreshed {
                at_ms: 1_700_000_000_000,
            },
        );
    }

    #[test]
    fn a_preflight_failure_is_the_whole_home_failure_and_ends_the_refresh() {
        let mut home = four_repositories();
        home.begin_refresh();

        home.preflight_failed(
            SessionFailure::new("GitHub is not authenticated")
                .with_remediation("Run `gh auth login`, then press r."),
        );

        let failure = home.failure().expect("the preflight failure shows");
        assert_eq!(failure.summary, "GitHub is not authenticated");
        assert_eq!(home.refresh_state(), RefreshState::Failed);
        assert_eq!(
            home.repositories().len(),
            4,
            "an unusable gh says nothing about the configured clones",
        );
    }

    /// The settings file is fine, so there is nothing about it to fix first.
    #[test]
    fn a_preflight_failure_does_not_stand_in_the_way_of_adding_a_repository() {
        let mut home = four_repositories();
        home.begin_refresh();
        home.preflight_failed(SessionFailure::new("GitHub is not authenticated"));

        let write = home.add_repositories(
            &settings_path(),
            vec![picked("/Developer/notes", "/Developer/notes", "acme/notes")],
        );

        assert!(write.is_some(), "the settings file can still be written");
        assert!(home.refusals().is_empty());
    }

    #[test]
    fn the_next_refresh_clears_the_preflight_failure() {
        let mut home = four_repositories();
        home.begin_refresh();
        home.preflight_failed(SessionFailure::new("GitHub is not authenticated"));

        home.begin_refresh();

        assert!(home.failure().is_none());
    }

    #[test]
    fn a_settings_file_that_cannot_be_read_ends_the_refresh_as_failed() {
        let mut home = four_repositories();
        home.begin_refresh();

        home.refreshed(Err(SessionFailure::new(
            "Home could not read your settings",
        )));

        assert_eq!(home.refresh_state(), RefreshState::Failed);
    }

    #[test]
    fn a_repository_that_could_not_be_fetched_names_itself_its_error_and_its_entry() {
        let mut home = four_repositories();
        home.begin_refresh();
        home.fetching(4);

        home.batch_fetched(vec![unreachable(
            "acme/billing",
            "GitHub refused access to acme/billing",
        )]);

        let failures = home.fetch_failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].slug, "acme/billing");
        assert_eq!(failures[0].reason, "GitHub refused access to acme/billing");
        assert_eq!(failures[0].path, Path::new("/Developer/billing"));
    }

    #[test]
    fn a_repository_that_could_not_be_fetched_says_so_in_the_footer_and_the_summary() {
        let mut home = four_repositories();
        home.begin_refresh();
        home.fetching(4);

        home.batch_fetched(vec![unreachable("acme/billing", "GitHub refused access")]);

        assert_eq!(
            home.repository_failure(Path::new("/Developer/billing")),
            Some("GitHub refused access"),
        );
        assert_eq!(
            home.repository_failure(Path::new("/Developer/widgets")),
            None,
        );
        assert_eq!(home.failed_count(), 1);
        assert_eq!(home.footer_summary(), "4 repositories \u{00B7} 1 failed");
    }

    /// A clone that never resolved was never fetched, so its own reason is the
    /// only thing it has to say.
    #[test]
    fn a_clone_that_did_not_resolve_keeps_reporting_its_own_reason() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![failed(
            "/Developer/moved",
            "the folder no longer exists",
        )]));
        home.begin_refresh();
        home.fetching(0);

        assert_eq!(
            home.repository_failure(Path::new("/Developer/moved")),
            Some("the folder no longer exists"),
        );
        assert_eq!(home.failed_count(), 1);
    }

    /// Two checkouts of one repository are two entries with two Removes, and
    /// the failure belongs to both of them.
    #[test]
    fn one_repository_failing_marks_every_clone_that_points_at_it() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![
            valid("/Developer/widgets", "acme/widgets"),
            valid("/Developer/widgets-worktree", "acme/widgets"),
        ]));
        home.begin_refresh();
        home.fetching(1);

        home.batch_fetched(vec![unreachable("acme/widgets", "GitHub refused access")]);

        assert_eq!(home.fetch_failures().len(), 2);
        assert_eq!(home.failed_count(), 2);
    }

    #[test]
    fn a_refresh_that_succeeds_clears_the_failure_the_last_one_reported() {
        let mut home = four_repositories();
        home.begin_refresh();
        home.fetching(1);
        home.batch_fetched(vec![unreachable("acme/billing", "GitHub refused access")]);
        home.finish_refresh(1_700_000_000_000);

        home.begin_refresh();
        home.fetching(1);
        home.batch_fetched(vec![empty("acme/billing")]);
        home.finish_refresh(1_700_000_060_000);

        assert!(home.fetch_failures().is_empty());
        assert_eq!(home.failed_count(), 0);
    }

    /// Home listing one row per group, which is what the cursor walks across.
    fn three_groups() -> HomeModel {
        let mut changes_requested = fetched(HomeSearch::Authored, 2, 200);
        changes_requested.changes_requested = true;
        listing(vec![
            fetched(HomeSearch::ReviewRequested, 1, 300),
            changes_requested,
            fetched(HomeSearch::Authored, 3, 100),
        ])
    }

    #[test]
    fn the_cursor_walks_every_row_across_the_groups() {
        let mut home = three_groups();
        assert_eq!(home.cursor(), 0);

        home.move_cursor_down();
        assert_eq!(home.rows()[home.cursor()].number, 2, "into To address");

        home.move_cursor_down();
        assert_eq!(home.rows()[home.cursor()].number, 3, "into Waiting");

        home.move_cursor_up();
        assert_eq!(home.rows()[home.cursor()].number, 2);
    }

    #[test]
    fn the_cursor_stops_at_the_ends_of_the_list() {
        let mut home = three_groups();

        home.move_cursor_up();
        assert_eq!(home.cursor(), 0);

        for _ in 0..10 {
            home.move_cursor_down();
        }
        assert_eq!(home.cursor(), 2);
    }

    #[test]
    fn the_cursor_is_pulled_back_onto_a_list_a_refresh_shortened() {
        let mut home = three_groups();
        home.move_cursor_down();
        home.move_cursor_down();
        assert_eq!(home.cursor(), 2);

        home.begin_refresh();
        home.fetching(1);
        home.batch_fetched(vec![loaded(vec![fetched(
            HomeSearch::ReviewRequested,
            1,
            300,
        )])]);

        assert_eq!(home.cursor(), 0, "the row it was on is gone");
    }

    #[test]
    fn the_cursor_sits_at_the_top_of_a_list_that_has_emptied() {
        let mut home = three_groups();
        home.move_cursor_down();

        home.begin_refresh();
        home.fetching(1);
        home.batch_fetched(vec![empty("acme/widgets")]);

        assert_eq!(home.cursor(), 0);
        assert!(home.rows().is_empty());
    }

    /// A removed clone takes its rows with it, so the list never promises pull
    /// requests from somewhere Home no longer reads.
    #[test]
    fn removing_a_repository_takes_its_rows_out_of_the_list() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![
            valid("/Developer/widgets", "acme/widgets"),
            valid("/Developer/zreview", "braidonw/zreview"),
        ]));
        home.begin_refresh();
        home.fetching(2);
        home.batch_fetched(vec![
            loaded(vec![fetched(HomeSearch::ReviewRequested, 1, 300)]),
            RepositoryFetch {
                slug: "braidonw/zreview".to_owned(),
                outcome: Ok(vec![FetchedPullRequest {
                    repository: "braidonw/zreview".to_owned(),
                    ..fetched(HomeSearch::ReviewRequested, 2, 200)
                }]),
            },
        ]);
        assert_eq!(home.rows().len(), 2);

        home.refreshed(Ok(vec![valid("/Developer/widgets", "acme/widgets")]));

        assert_eq!(home.rows().len(), 1);
        assert_eq!(home.rows()[0].repository, "acme/widgets");
    }

    /// A pull request the reviewer wrote and is also asked to review, which a
    /// team review request makes possible.
    #[test]
    fn a_pull_request_in_both_searches_is_listed_once_among_the_authored_groups() {
        let mut authored = fetched(HomeSearch::Authored, 412, 100);
        authored.changes_requested = true;

        let home = listing(vec![
            fetched(HomeSearch::ReviewRequested, 412, 100),
            authored,
        ]);

        assert_eq!(home.rows().len(), 1, "one pull request is one row");
        assert_eq!(home.rows()[0].group, HomeGroup::ToAddress);
    }

    #[test]
    fn a_review_request_the_reviewer_asked_changes_of_stays_in_to_review() {
        let mut asked = fetched(HomeSearch::ReviewRequested, 412, 100);
        asked.changes_requested = true;

        let home = listing(vec![asked]);

        let row = only_row(&home);
        assert_eq!(row.group, HomeGroup::ToReview);
        assert_eq!(row.review_status, Some(ReviewStatus::ChangesRequested));
    }

    /// GitHub's own decision is a review status in its own right, on a pull
    /// request whose standing reviews said nothing.
    #[test]
    fn a_changes_requested_decision_alone_is_a_review_status() {
        let mut decided = fetched(HomeSearch::Authored, 412, 100);
        decided.review_decision = Some(ReviewDecision::ChangesRequested);

        let home = listing(vec![decided]);

        let row = only_row(&home);
        assert_eq!(row.review_status, Some(ReviewStatus::ChangesRequested));
        assert_eq!(
            row.group,
            HomeGroup::WaitingOnOthers,
            "grouping stays on the standing reviews the To address rule names",
        );
    }

    /// A remote spelled in mixed case is the same repository, and every later
    /// read of the settings file has to agree that it is.
    #[test]
    fn a_repository_spelled_in_another_case_keeps_its_rows() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![valid("/Developer/widgets", "Acme/Widgets")]));
        home.begin_refresh();
        home.fetching(1);
        home.batch_fetched(vec![loaded(vec![fetched(
            HomeSearch::ReviewRequested,
            412,
            100,
        )])]);
        assert_eq!(home.rows().len(), 1);

        home.refreshed(Ok(vec![valid("/Developer/widgets", "Acme/Widgets")]));

        assert_eq!(home.rows().len(), 1, "the row survives a re-read");
        assert_eq!(
            home.repository_failure(Path::new("/Developer/widgets")),
            None,
        );
    }

    #[test]
    fn two_clones_of_one_repository_spelled_differently_both_carry_its_failure() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![
            valid("/Developer/widgets", "acme/widgets"),
            valid("/Developer/Widgets", "Acme/Widgets"),
        ]));
        home.begin_refresh();
        home.fetching(1);

        home.batch_fetched(vec![unreachable("acme/widgets", "GitHub refused access")]);

        assert_eq!(home.fetch_failures().len(), 2);
        assert_eq!(
            home.refresh_state(),
            RefreshState::Refreshing { done: 1, total: 1 },
            "one repository answered for both clones",
        );
    }

    /// Nothing could be listed, which is a failed refresh however far it got.
    #[test]
    fn a_refresh_of_clones_that_none_of_them_resolved_reads_as_failed() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![failed(
            "/Developer/moved",
            "the folder no longer exists",
        )]));
        home.begin_refresh();
        home.fetching(0);

        home.finish_refresh(1_700_000_000_000);

        assert_eq!(home.refresh_state(), RefreshState::Failed);
    }

    /// The rows a reviewer is reading stay up while the next refresh runs, and
    /// each repository replaces its own as it answers.
    #[test]
    fn a_running_refresh_replaces_the_rows_one_repository_at_a_time() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![
            valid("/Developer/widgets", "acme/widgets"),
            valid("/Developer/zreview", "braidonw/zreview"),
        ]));
        home.begin_refresh();
        home.fetching(2);
        home.batch_fetched(vec![
            loaded(vec![fetched(HomeSearch::ReviewRequested, 1, 300)]),
            elsewhere("braidonw/zreview", 2, 200),
        ]);
        home.finish_refresh(1_700_000_000_000);

        home.begin_refresh();
        home.fetching(2);
        assert_eq!(
            home.rows().len(),
            2,
            "the list a reviewer is reading stays up while the next refresh runs",
        );

        home.batch_fetched(vec![loaded(vec![
            fetched(HomeSearch::ReviewRequested, 1, 300),
            fetched(HomeSearch::ReviewRequested, 3, 400),
        ])]);

        let listed = home
            .rows()
            .iter()
            .map(|row| (row.repository.clone(), row.number))
            .collect::<Vec<_>>();
        assert_eq!(
            listed,
            [
                ("acme/widgets".to_owned(), 3),
                ("acme/widgets".to_owned(), 1),
                ("braidonw/zreview".to_owned(), 2),
            ],
            "only the repository that answered was replaced",
        );
    }

    #[test]
    fn a_repository_that_answers_with_nothing_takes_its_rows_out_of_the_list() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![valid("/Developer/widgets", "acme/widgets")]));
        home.begin_refresh();
        home.fetching(1);
        home.batch_fetched(vec![loaded(vec![fetched(
            HomeSearch::ReviewRequested,
            1,
            300,
        )])]);

        home.begin_refresh();
        home.fetching(1);
        home.batch_fetched(vec![empty("acme/widgets")]);

        assert!(home.rows().is_empty());
    }

    /// A refresh that stopped without finishing leaves a stamp that would
    /// otherwise claim for ever that it is still running.
    #[test]
    fn a_refresh_that_was_abandoned_ends_as_failed_with_a_reason() {
        let mut home = four_repositories();
        home.begin_refresh();
        home.fetching(4);

        home.refresh_abandoned();

        assert_eq!(home.refresh_state(), RefreshState::Failed);
        let failure = home.failure().expect("an abandoned refresh says so");
        assert_eq!(failure.summary, "Home could not finish the refresh");
    }

    #[test]
    fn a_refresh_that_finished_is_not_marked_abandoned() {
        let mut home = four_repositories();
        home.begin_refresh();
        home.fetching(1);
        home.batch_fetched(vec![empty("braidonw/zreview")]);
        home.finish_refresh(1_700_000_000_000);

        home.refresh_abandoned();

        assert_eq!(
            home.refresh_state(),
            RefreshState::Refreshed {
                at_ms: 1_700_000_000_000,
            },
        );
        assert!(home.failure().is_none());
    }

    #[test]
    fn every_status_carries_the_words_its_column_shows() {
        assert_eq!(CheckStatus::Passing.label(), "checks passing");
        assert_eq!(CheckStatus::Failing.label(), "checks failing");
        assert_eq!(CheckStatus::Running.label(), "checks running");
        assert_eq!(ReviewStatus::ChangesRequested.label(), "changes requested");
        assert_eq!(ReviewStatus::Approved.label(), "approved");
        assert_eq!(
            ReviewStatus::ReviewedThisHead.label(),
            "you reviewed this head",
        );
    }

    #[test]
    fn a_refresh_lists_every_configured_repository_with_its_slug() {
        let mut home = HomeModel::new();

        home.refreshed(Ok(vec![
            valid("/Developer/zreview", "braidonw/zreview"),
            valid("/Developer/widgets", "acme/widgets"),
        ]));

        let listed = home.repositories();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].path, Path::new("/Developer/zreview"));
        assert_eq!(listed[0].slug(), Some("braidonw/zreview"));
        assert_eq!(listed[1].slug(), Some("acme/widgets"));
        assert!(home.failure().is_none());
    }

    #[test]
    fn a_repository_that_cannot_be_resolved_keeps_its_reason_and_has_no_slug() {
        let mut home = HomeModel::new();

        home.refreshed(Ok(vec![
            valid("/Developer/zreview", "braidonw/zreview"),
            failed("/Developer/moved", "the folder no longer exists"),
        ]));

        let listed = home.repositories();
        assert_eq!(listed[1].slug(), None);
        assert_eq!(listed[1].reason(), Some("the folder no longer exists"));
        assert_eq!(home.failed_count(), 1);
    }

    #[test]
    fn a_settings_file_that_cannot_be_read_becomes_the_whole_home_failure() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![valid("/Developer/zreview", "braidonw/zreview")]));

        home.refreshed(Err(SessionFailure::new("Could not read your settings")
            .with_remediation(
                "Fix ~/.config/zreview/settings.toml, then press r.",
            )));

        let failure = home.failure().expect("the failure should be held");
        assert_eq!(failure.summary, "Could not read your settings");
        assert!(
            home.repositories().is_empty(),
            "a list read from an unreadable file cannot be trusted",
        );
    }

    #[test]
    fn a_later_successful_refresh_clears_the_whole_home_failure() {
        let mut home = HomeModel::new();
        home.refreshed(Err(SessionFailure::new("Could not read your settings")));

        home.refreshed(Ok(vec![valid("/Developer/zreview", "braidonw/zreview")]));

        assert!(home.failure().is_none());
        assert_eq!(home.repositories().len(), 1);
    }

    #[test]
    fn the_count_line_counts_the_listed_rows_and_the_configured_repositories() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![valid("/Developer/widgets", "acme/widgets")]));

        home.batch_fetched(vec![loaded(vec![
            fetched(HomeSearch::ReviewRequested, 1, 100),
            fetched(HomeSearch::Authored, 2, 200),
        ])]);

        assert_eq!(
            home.count_line().as_deref(),
            Some("2 pull requests across 1 repository"),
        );
    }

    #[test]
    fn the_count_line_counts_one_pull_request_in_the_singular() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![valid("/Developer/widgets", "acme/widgets")]));

        home.batch_fetched(vec![loaded(vec![fetched(
            HomeSearch::ReviewRequested,
            1,
            100,
        )])]);

        assert_eq!(
            home.count_line().as_deref(),
            Some("1 pull request across 1 repository"),
        );
    }

    #[test]
    fn a_group_holds_only_its_own_rows() {
        let mut changes_requested = fetched(HomeSearch::Authored, 1, 100);
        changes_requested.changes_requested = true;
        let home = listing(vec![
            fetched(HomeSearch::ReviewRequested, 2, 300),
            changes_requested,
        ]);

        assert_eq!(home.rows_in(HomeGroup::ToReview).count(), 1);
        assert_eq!(home.rows_in(HomeGroup::ToAddress).count(), 1);
        assert_eq!(home.rows_in(HomeGroup::WaitingOnOthers).count(), 0);
    }

    #[test]
    fn the_count_line_counts_the_configured_repositories() {
        let mut home = HomeModel::new();
        assert_eq!(home.count_line(), None, "nothing to count before a refresh");

        home.refreshed(Ok(vec![valid("/Developer/zreview", "braidonw/zreview")]));
        assert_eq!(
            home.count_line().as_deref(),
            Some("0 pull requests across 1 repository"),
        );

        home.refreshed(Ok(vec![
            valid("/Developer/zreview", "braidonw/zreview"),
            valid("/Developer/widgets", "acme/widgets"),
        ]));
        assert_eq!(
            home.count_line().as_deref(),
            Some("0 pull requests across 2 repositories"),
        );
    }

    #[test]
    fn the_footer_summary_names_the_failed_repositories() {
        let mut home = HomeModel::new();

        home.refreshed(Ok(vec![
            valid("/Developer/zreview", "braidonw/zreview"),
            valid("/Developer/widgets", "acme/widgets"),
            valid("/Developer/billing", "acme/billing"),
            failed("/Developer/moved", "the folder no longer exists"),
        ]));

        assert_eq!(home.footer_summary(), "4 repositories \u{00B7} 1 failed");
    }

    #[test]
    fn the_footer_summary_leaves_out_failures_when_every_repository_resolved() {
        let mut home = HomeModel::new();

        home.refreshed(Ok(vec![valid("/Developer/zreview", "braidonw/zreview")]));

        assert_eq!(home.footer_summary(), "1 repository");
    }

    #[test]
    fn the_footer_summary_reads_no_repositories_before_any_are_configured() {
        let home = HomeModel::new();

        assert_eq!(home.footer_summary(), "No repositories");
    }

    #[test]
    fn the_footer_starts_collapsed_and_the_expansion_outlives_a_refresh() {
        let mut home = HomeModel::new();
        assert!(!home.is_footer_expanded());

        home.toggle_footer();
        assert!(home.is_footer_expanded());

        home.refreshed(Ok(vec![valid("/Developer/zreview", "braidonw/zreview")]));
        assert!(
            home.is_footer_expanded(),
            "a refresh should not close the footer under the reviewer",
        );

        home.toggle_footer();
        assert!(!home.is_footer_expanded());
    }

    #[test]
    fn the_groups_are_in_a_fixed_order_each_with_its_own_empty_copy() {
        let titles = HomeGroup::ALL.map(HomeGroup::title);
        let copy = HomeGroup::ALL.map(HomeGroup::empty_copy);

        assert_eq!(titles, ["To review", "To address", "Waiting on others"]);
        assert_eq!(
            copy,
            [
                "Nothing waiting for your review.",
                "Nothing to address.",
                "Nothing waiting on others.",
            ],
        );
    }

    /// A picked folder that resolved, whose root is not the folder itself.
    fn picked(folder: &str, root: &str, slug: &str) -> RepositoryEntry {
        RepositoryEntry {
            path: PathBuf::from(folder),
            outcome: RepositoryOutcome::Valid {
                root: PathBuf::from(root),
                slug: slug.to_owned(),
            },
        }
    }

    /// The settings file every Add and Remove in these tests would write.
    fn settings_path() -> PathBuf {
        PathBuf::from("/Users/braidon/.config/zreview/settings.toml")
    }

    /// Home with two clones already configured.
    fn configured() -> HomeModel {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![
            valid("/Developer/zreview", "braidonw/zreview"),
            valid("/Developer/widgets", "acme/widgets"),
        ]));
        home
    }

    #[test]
    fn adding_folders_writes_the_existing_entries_then_every_accepted_root() {
        let mut home = configured();

        let write = home
            .add_repositories(
                &settings_path(),
                vec![picked(
                    "/Developer/billing/crates",
                    "/Developer/billing",
                    "acme/billing",
                )],
            )
            .expect("an accepted folder should be written");

        assert_eq!(
            write.repositories,
            [
                PathBuf::from("/Developer/zreview"),
                PathBuf::from("/Developer/widgets"),
                PathBuf::from("/Developer/billing"),
            ],
            "the worktree root is written, not the folder that was picked",
        );
        assert!(home.refusals().is_empty());
    }

    #[test]
    fn a_folder_that_did_not_resolve_is_refused_while_the_rest_proceed() {
        let mut home = configured();

        let write = home
            .add_repositories(
                &settings_path(),
                vec![
                    failed("/Developer/notes", "not a Git repository"),
                    picked("/Developer/billing", "/Developer/billing", "acme/billing"),
                ],
            )
            .expect("the folder that did resolve should still be written");

        assert!(
            write
                .repositories
                .contains(&PathBuf::from("/Developer/billing"))
        );
        let refusals = home.refusals();
        assert_eq!(refusals.len(), 1);
        assert_eq!(refusals[0].path, Path::new("/Developer/notes"));
        assert_eq!(refusals[0].reason, "not a Git repository");
    }

    #[test]
    fn an_add_that_accepts_nothing_writes_nothing_and_still_reports_the_refusal() {
        let mut home = configured();

        let write = home.add_repositories(
            &settings_path(),
            vec![failed("/Developer/notes", "no GitHub remote")],
        );

        assert!(
            write.is_none(),
            "nothing was accepted, so nothing is written"
        );
        assert_eq!(home.refusals().len(), 1);
    }

    #[test]
    fn a_folder_inside_a_clone_that_is_already_listed_is_ignored() {
        let mut home = configured();

        let write = home.add_repositories(
            &settings_path(),
            vec![picked(
                "/Developer/zreview/crates/app",
                "/Developer/zreview",
                "braidonw/zreview",
            )],
        );

        assert!(write.is_none(), "re-adding a listed clone changes nothing");
        assert!(
            home.refusals().is_empty(),
            "a folder already listed is ignored, not refused",
        );
    }

    #[test]
    fn a_folder_already_listed_is_ignored_even_when_its_entry_did_not_resolve() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![failed(
            "/Developer/zreview",
            "the folder no longer exists",
        )]));

        let write = home.add_repositories(
            &settings_path(),
            vec![picked(
                "/Developer/zreview",
                "/Developer/zreview",
                "braidonw/zreview",
            )],
        );

        assert!(write.is_none());
    }

    #[test]
    fn two_picked_folders_inside_one_clone_become_one_entry() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(Vec::new()));

        let write = home
            .add_repositories(
                &settings_path(),
                vec![
                    picked(
                        "/Developer/billing/api",
                        "/Developer/billing",
                        "acme/billing",
                    ),
                    picked(
                        "/Developer/billing/web",
                        "/Developer/billing",
                        "acme/billing",
                    ),
                ],
            )
            .expect("one of the two should be written");

        assert_eq!(write.repositories, [PathBuf::from("/Developer/billing")]);
    }

    #[test]
    fn each_add_replaces_the_refusals_the_last_one_reported() {
        let mut home = configured();
        let _ = home.add_repositories(
            &settings_path(),
            vec![failed("/Developer/notes", "no GitHub remote")],
        );

        let _ = home.add_repositories(
            &settings_path(),
            vec![failed("/Developer/scratch", "not a Git repository")],
        );

        let refusals = home.refusals();
        assert_eq!(refusals.len(), 1, "the earlier refusal has been answered");
        assert_eq!(refusals[0].path, Path::new("/Developer/scratch"));
    }

    /// Home cannot know what a file it could not read holds, so writing over it
    /// would replace repositories it never saw.
    #[test]
    fn adding_over_a_settings_file_that_could_not_be_read_is_refused() {
        let mut home = HomeModel::new();
        home.refreshed(Err(SessionFailure::new(
            "Home could not read your settings",
        )));

        let write = home.add_repositories(
            &settings_path(),
            vec![picked(
                "/Developer/billing",
                "/Developer/billing",
                "acme/billing",
            )],
        );

        assert!(write.is_none(), "nothing may be written over that file");
        let refusals = home.refusals();
        assert_eq!(refusals.len(), 1);
        assert_eq!(refusals[0].path, settings_path());
        assert_eq!(
            refusals[0].reason,
            "fix this file before changing your repositories",
        );
    }

    #[test]
    fn removing_over_a_settings_file_that_could_not_be_read_is_refused() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![valid("/Developer/zreview", "braidonw/zreview")]));
        home.refreshed(Err(SessionFailure::new(
            "Home could not read your settings",
        )));

        let write = home.remove_repository(&settings_path(), Path::new("/Developer/zreview"));

        assert!(write.is_none());
        assert_eq!(home.refusals()[0].path, settings_path());
    }

    #[test]
    fn removing_an_entry_writes_the_list_without_it() {
        let mut home = configured();

        let write = home
            .remove_repository(&settings_path(), Path::new("/Developer/zreview"))
            .expect("a listed entry should be removable");

        assert_eq!(write.repositories, [PathBuf::from("/Developer/widgets")]);
    }

    #[test]
    fn removing_an_entry_that_is_no_longer_listed_writes_nothing() {
        let mut home = configured();

        let write = home.remove_repository(&settings_path(), Path::new("/Developer/billing"));

        assert!(write.is_none());
    }

    #[test]
    fn a_write_failure_is_its_own_line_and_leaves_the_list_standing() {
        let mut home = HomeModel::new();
        home.refreshed(Ok(vec![valid("/Developer/zreview", "braidonw/zreview")]));

        home.write_finished(Err(SessionFailure::new(
            "Home could not save your settings",
        )));

        assert_eq!(
            home.write_failure()
                .expect("the write failure shows")
                .summary,
            "Home could not save your settings",
        );
        assert!(
            home.failure().is_none(),
            "a write failure never replaces the list",
        );
        assert_eq!(home.repositories().len(), 1);
    }

    #[test]
    fn a_read_failure_and_a_write_failure_are_both_visible() {
        let mut home = HomeModel::new();
        home.write_finished(Err(SessionFailure::new(
            "Home could not save your settings",
        )));

        home.refreshed(Err(SessionFailure::new(
            "Home could not read your settings",
        )));

        assert_eq!(
            home.failure().expect("the read failure shows").summary,
            "Home could not read your settings",
        );
        assert_eq!(
            home.write_failure()
                .expect("the write failure shows")
                .summary,
            "Home could not save your settings",
        );
    }

    #[test]
    fn a_successful_write_clears_the_write_failure() {
        let mut home = HomeModel::new();
        home.write_finished(Err(SessionFailure::new(
            "Home could not save your settings",
        )));

        home.write_finished(Ok(()));

        assert!(home.write_failure().is_none());
    }

    #[test]
    fn a_row_with_drafts_against_an_older_head_still_shows_the_count() {
        let mut home = listing(vec![fetched(HomeSearch::ReviewRequested, 412, 100)]);
        home.drafts_read(Ok(HashMap::from([(
            "github:acme/widgets#412".to_owned(),
            3,
        )])));

        assert_eq!(only_row(&home).drafts, Some(3));
    }

    #[test]
    fn a_row_with_no_drafts_shows_a_blank_cell() {
        let mut home = listing(vec![fetched(HomeSearch::ReviewRequested, 412, 100)]);

        home.drafts_read(Ok(HashMap::new()));

        assert_eq!(only_row(&home).drafts, None);
    }

    #[test]
    fn a_store_failure_blanks_every_row_and_names_the_reason_above_the_list() {
        let mut home = listing(vec![
            fetched(HomeSearch::ReviewRequested, 412, 100),
            fetched(HomeSearch::ReviewRequested, 398, 200),
        ]);
        home.drafts_read(Ok(HashMap::from([(
            "github:acme/widgets#412".to_owned(),
            2,
        )])));

        home.drafts_read(Err(SessionFailure::new("Drafts could not be read")));

        assert!(home.rows().iter().all(|row| row.drafts.is_none()));
        assert_eq!(
            home.drafts_failure()
                .expect("the failure should show")
                .summary,
            "Drafts could not be read",
        );
    }

    #[test]
    fn a_read_that_succeeds_after_a_failure_clears_it() {
        let mut home = listing(vec![fetched(HomeSearch::ReviewRequested, 412, 100)]);
        home.drafts_read(Err(SessionFailure::new("Drafts could not be read")));

        home.drafts_read(Ok(HashMap::from([(
            "github:acme/widgets#412".to_owned(),
            1,
        )])));

        assert!(home.drafts_failure().is_none());
        assert_eq!(only_row(&home).drafts, Some(1));
    }

    #[test]
    fn every_draft_count_reads_as_its_singular_or_plural_label() {
        let mut home = listing(vec![fetched(HomeSearch::ReviewRequested, 412, 100)]);

        home.drafts_read(Ok(HashMap::from([(
            "github:acme/widgets#412".to_owned(),
            1,
        )])));
        assert_eq!(only_row(&home).drafts_label().as_deref(), Some("1 draft"));

        home.drafts_read(Ok(HashMap::from([(
            "github:acme/widgets#412".to_owned(),
            4,
        )])));
        assert_eq!(only_row(&home).drafts_label().as_deref(), Some("4 drafts"));

        home.drafts_read(Ok(HashMap::new()));
        assert_eq!(only_row(&home).drafts_label(), None);
    }
}
