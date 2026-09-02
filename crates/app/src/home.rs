//! What Home shows and every action that changes it, with no window in it.
//!
//! Home reads its repositories from a settings file and resolves each one to a
//! GitHub repository, but neither the file nor Git is touched here. A refresh
//! arrives already read and already validated, so this model is all decisions
//! and no I/O, and effects on the file come back as return values.

use std::path::{Path, PathBuf};

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

    #[must_use]
    const fn is_failed(&self) -> bool {
        matches!(self.outcome, RepositoryOutcome::Failed { .. })
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

/// Everything Home is, and every action that changes it.
///
/// Held behind a lock and shared by whatever is displaying it, as the session
/// model is.
#[derive(Debug, Default)]
pub struct HomeModel {
    repositories: Vec<RepositoryEntry>,
    failure: Option<SessionFailure>,
    write_failure: Option<SessionFailure>,
    footer_expanded: bool,
    refusals: Vec<Refusal>,
}

impl HomeModel {
    /// Starts empty, before the settings file has been read.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes what a refresh read, or the failure that stopped it reading.
    ///
    /// A failed read empties the list, because everything in it came from the
    /// file that could not be read.
    pub fn refreshed(&mut self, result: Result<Vec<RepositoryEntry>, SessionFailure>) {
        match result {
            Ok(repositories) => {
                self.repositories = repositories;
                self.failure = None;
            }
            Err(failure) => {
                self.repositories.clear();
                self.failure = Some(failure);
            }
        }
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
    /// Only a settings file that could not be read, or no home directory to look
    /// for one in. A write that failed is [`Self::write_failure`].
    #[must_use]
    pub const fn failure(&self) -> Option<&SessionFailure> {
        self.failure.as_ref()
    }

    /// Why the last write did not reach the file, if it did not.
    #[must_use]
    pub const fn write_failure(&self) -> Option<&SessionFailure> {
        self.write_failure.as_ref()
    }

    /// How many configured clones could not be resolved.
    #[must_use]
    pub fn failed_count(&self) -> usize {
        self.repositories
            .iter()
            .filter(|entry| entry.is_failed())
            .count()
    }

    /// The header's count line, absent while there is nothing to count.
    ///
    /// No pull requests have been fetched yet, so the count of them is zero.
    #[must_use]
    pub fn count_line(&self) -> Option<String> {
        (!self.repositories.is_empty()).then(|| {
            format!(
                "0 pull requests across {}",
                repository_count(self.repositories.len())
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
        self.failure.is_some().then(|| Refusal {
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
}
