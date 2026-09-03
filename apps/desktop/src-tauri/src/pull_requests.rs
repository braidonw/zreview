//! Fetching Home's pull requests, and reducing them to what its model reads.
//!
//! The Home model takes rows that already answer the questions grouping asks,
//! so the comparisons against the viewer's own login and the shape of GitHub's
//! reviews and threads are settled here, where the forge's types are in scope.

use std::path::Path;

use app::{FetchedPullRequest, RepositoryFetch};
use domain::SessionFailure;
use github::{
    GithubClient, GithubError, HomeFetch, HomePullRequest, HomeRepository, RepositorySlug,
    ReviewState, ReviewThread,
};

/// Confirms `gh` is installed and signed in before a refresh spends anything.
///
/// Run in one configured clone, because that is what the check takes, though
/// what it answers is about the machine rather than the repository.
///
/// # Errors
///
/// Returns what Home shows in place of its list when `gh` cannot be used.
pub(crate) fn preflight(client: &GithubClient, clone_root: &Path) -> Result<(), SessionFailure> {
    client
        .check_authentication(clone_root)
        .map_err(|error| preflight_failure(&error))
}

fn preflight_failure(error: &GithubError) -> SessionFailure {
    SessionFailure::from_error("Home could not use the GitHub CLI", error)
        .with_optional_remediation(error.remediation())
}

/// Fetches `repositories`, handing `on_batch` each batch as it answers.
///
/// Batching and the collapsing of two clones that name one repository belong to
/// the fetch, which is the one place that knows how GitHub compares two names.
pub(crate) fn fetch(
    client: &GithubClient,
    repositories: &[RepositorySlug],
    on_batch: &dyn Fn(Vec<RepositoryFetch>),
) {
    let _fetched = client.fetch_home_pull_requests_with_progress(repositories, &|batch| {
        on_batch(map_batch(batch));
    });
}

/// Reduces one batch's answer to what Home's model reads.
fn map_batch(batch: &HomeFetch) -> Vec<RepositoryFetch> {
    batch
        .repositories
        .iter()
        .map(|repository| map_repository(repository.clone(), batch.viewer_login.as_deref()))
        .collect()
}

/// Reduces one repository's fetch to what Home's model reads.
///
/// A row this cannot read fails the repository it came from rather than
/// disappearing out of a list that would then be short without saying why.
fn map_repository(repository: HomeRepository, viewer_login: Option<&str>) -> RepositoryFetch {
    let slug = repository.repository.full_name();
    let outcome = match repository.pull_requests {
        Ok(pull_requests) => map_pull_requests(pull_requests, viewer_login),
        Err(error) => Err(error.to_string()),
    };
    RepositoryFetch { slug, outcome }
}

fn map_pull_requests(
    pull_requests: Vec<HomePullRequest>,
    viewer_login: Option<&str>,
) -> Result<Vec<FetchedPullRequest>, String> {
    if pull_requests.is_empty() {
        return Ok(Vec::new());
    }
    // Grouping asks whether a thread or a review is the viewer's own, and there
    // is no answering that without knowing which account answered.
    let viewer_login =
        viewer_login.ok_or_else(|| "GitHub did not say who is signed in".to_owned())?;
    pull_requests
        .into_iter()
        .map(|pull_request| map_pull_request(pull_request, viewer_login))
        .collect()
}

/// Reduces one fetched pull request to what Home's model reads.
///
/// # Errors
///
/// Returns the malformed update time when GitHub sent one this cannot read,
/// which would otherwise order the row against every other by an invented time.
fn map_pull_request(
    pull_request: HomePullRequest,
    viewer_login: &str,
) -> Result<FetchedPullRequest, String> {
    let updated_at_ms = epoch_millis(&pull_request.updated_at).ok_or_else(|| {
        format!(
            "GitHub sent an update time this cannot read ({})",
            pull_request.updated_at,
        )
    })?;
    Ok(FetchedPullRequest {
        search: match pull_request.search {
            github::HomeSearch::ReviewRequested => app::HomeSearch::ReviewRequested,
            github::HomeSearch::Authored => app::HomeSearch::Authored,
        },
        repository: pull_request.repository.full_name(),
        number: pull_request.number,
        title: pull_request.title,
        url: pull_request.url,
        author_login: pull_request.author_login,
        updated_at_ms,
        head_sha: pull_request.head_sha,
        viewer_latest_review_sha: pull_request.viewer_latest_review_sha,
        check_state: pull_request.check_state.map(map_check_state),
        review_decision: pull_request.review_decision.map(map_review_decision),
        changes_requested: pull_request
            .latest_opinionated_reviews
            .iter()
            .any(|review| review.state == ReviewState::ChangesRequested),
        thread_awaiting_reply: pull_request
            .review_threads
            .iter()
            .any(|thread| is_awaiting_reply(thread, viewer_login)),
    })
}

/// Whether this thread is one the author is being waited on in.
///
/// A thread whose last comment has no author at all counts as somebody else's,
/// because the one thing known about it is that it is not the viewer's.
fn is_awaiting_reply(thread: &ReviewThread, viewer_login: &str) -> bool {
    !thread.resolved && thread.last_comment_author_login.as_deref() != Some(viewer_login)
}

const fn map_check_state(state: github::StatusCheckState) -> app::CheckState {
    match state {
        github::StatusCheckState::Expected => app::CheckState::Expected,
        github::StatusCheckState::Error => app::CheckState::Error,
        github::StatusCheckState::Failure => app::CheckState::Failure,
        github::StatusCheckState::Pending => app::CheckState::Pending,
        github::StatusCheckState::Success => app::CheckState::Success,
    }
}

const fn map_review_decision(decision: github::ReviewDecision) -> app::ReviewDecision {
    match decision {
        github::ReviewDecision::ChangesRequested => app::ReviewDecision::ChangesRequested,
        github::ReviewDecision::Approved => app::ReviewDecision::Approved,
        github::ReviewDecision::ReviewRequired => app::ReviewDecision::ReviewRequired,
    }
}

/// Reads GitHub's `YYYY-MM-DDTHH:MM:SSZ` into epoch milliseconds.
///
/// Returns `None` for anything else, rather than inventing a time for a row
/// whose position in the list is decided by it.
fn epoch_millis(timestamp: &str) -> Option<i64> {
    let bytes = timestamp.as_bytes();
    if bytes.len() != 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' || bytes[19] != b'Z' {
        return None;
    }
    let year = timestamp[0..4].parse::<i64>().ok()?;
    let month = timestamp[5..7].parse::<u32>().ok()?;
    let day = timestamp[8..10].parse::<u32>().ok()?;
    let hour = timestamp[11..13].parse::<i64>().ok()?;
    let minute = timestamp[14..16].parse::<i64>().ok()?;
    // 60 is a leap second, which GitHub can legitimately send.
    let second = timestamp[17..19].parse::<i64>().ok()?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000)
}

/// How many days that month of that year holds.
const fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        // The caller has already refused every month outside the year.
        _ => 0,
    }
}

const fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days between the civil date and 1970-01-01, by Howard Hinnant's algorithm.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use github::{
        HomePullRequest, HomeRepository, HomeSearch, OpinionatedReview, RepositorySlug,
        ReviewDecision, StatusCheckState,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::fake_gh::FakeGh;

    /// One pull request as the GraphQL path returns it.
    fn pull_request(search: HomeSearch, number: u64) -> HomePullRequest {
        HomePullRequest {
            search,
            repository: RepositorySlug::new("acme", "widgets"),
            number,
            title: format!("Pull request {number}"),
            url: format!("https://github.com/acme/widgets/pull/{number}"),
            draft: false,
            updated_at: "2026-09-01T12:34:56Z".to_owned(),
            head_sha: "abc123".to_owned(),
            author_login: Some("mlee".to_owned()),
            viewer_latest_review_sha: None,
            check_state: None,
            review_decision: None,
            latest_opinionated_reviews: Vec::new(),
            review_threads: Vec::new(),
        }
    }

    #[test]
    fn a_fetched_pull_request_carries_every_field_the_grouping_reads() {
        let fetched = map_pull_request(pull_request(HomeSearch::ReviewRequested, 412), "braidonw")
            .expect("a well-formed row maps");

        assert_eq!(fetched.search, app::HomeSearch::ReviewRequested);
        assert_eq!(fetched.repository, "acme/widgets");
        assert_eq!(fetched.number, 412);
        assert_eq!(fetched.title, "Pull request 412");
        assert_eq!(fetched.url, "https://github.com/acme/widgets/pull/412");
        assert_eq!(fetched.author_login.as_deref(), Some("mlee"));
        assert_eq!(fetched.head_sha, "abc123");
        assert_eq!(fetched.updated_at_ms, 1_788_266_096_000);
    }

    /// One authored pull request carrying `threads`, as the viewer sees it.
    fn with_threads(threads: Vec<ReviewThread>) -> FetchedPullRequest {
        let mut authored = pull_request(HomeSearch::Authored, 412);
        authored.review_threads = threads;
        map_pull_request(authored, "braidonw").expect("a well-formed row maps")
    }

    fn thread(resolved: bool, last_comment_author_login: Option<&str>) -> ReviewThread {
        ReviewThread {
            resolved,
            last_comment_author_login: last_comment_author_login.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn an_unresolved_thread_somebody_else_spoke_in_last_is_awaiting_a_reply() {
        let fetched = with_threads(vec![thread(false, Some("priya"))]);

        assert!(fetched.thread_awaiting_reply);
    }

    #[test]
    fn an_unresolved_thread_the_viewer_spoke_in_last_is_not_awaiting_a_reply() {
        let fetched = with_threads(vec![thread(false, Some("braidonw"))]);

        assert!(!fetched.thread_awaiting_reply);
    }

    /// All that is known about a thread with no last author is that the last
    /// word was not the viewer's.
    #[test]
    fn an_unresolved_thread_with_no_last_comment_author_is_awaiting_a_reply() {
        let fetched = with_threads(vec![thread(false, None)]);

        assert!(fetched.thread_awaiting_reply);
    }

    #[test]
    fn a_resolved_thread_is_never_awaiting_a_reply() {
        let fetched = with_threads(vec![thread(true, Some("priya"))]);

        assert!(!fetched.thread_awaiting_reply);
    }

    #[test]
    fn one_unanswered_thread_among_answered_ones_is_enough() {
        let fetched = with_threads(vec![
            thread(true, Some("priya")),
            thread(false, Some("braidonw")),
            thread(false, Some("tomas")),
        ]);

        assert!(fetched.thread_awaiting_reply);
    }

    /// One authored pull request whose standing reviews are `reviews`.
    fn with_reviews(reviews: Vec<OpinionatedReview>) -> FetchedPullRequest {
        let mut authored = pull_request(HomeSearch::Authored, 412);
        authored.latest_opinionated_reviews = reviews;
        map_pull_request(authored, "braidonw").expect("a well-formed row maps")
    }

    fn review(state: ReviewState) -> OpinionatedReview {
        OpinionatedReview {
            state,
            author_login: Some("priya".to_owned()),
        }
    }

    #[test]
    fn a_standing_changes_requested_review_is_carried_to_the_model() {
        let fetched = with_reviews(vec![
            review(ReviewState::Approved),
            review(ReviewState::ChangesRequested),
        ]);

        assert!(fetched.changes_requested);
    }

    #[test]
    fn reviews_that_ask_for_nothing_leave_the_pull_request_alone() {
        let fetched = with_reviews(vec![
            review(ReviewState::Approved),
            review(ReviewState::Commented),
            review(ReviewState::Pending),
            review(ReviewState::Dismissed),
        ]);

        assert!(!fetched.changes_requested);
    }

    #[test]
    fn every_check_state_and_review_decision_reaches_the_model_unchanged() {
        let states = [
            (StatusCheckState::Expected, app::CheckState::Expected),
            (StatusCheckState::Error, app::CheckState::Error),
            (StatusCheckState::Failure, app::CheckState::Failure),
            (StatusCheckState::Pending, app::CheckState::Pending),
            (StatusCheckState::Success, app::CheckState::Success),
        ];
        for (fetched_state, expected) in states {
            let mut row = pull_request(HomeSearch::Authored, 412);
            row.check_state = Some(fetched_state);
            let mapped = map_pull_request(row, "braidonw").expect("a well-formed row maps");
            assert_eq!(mapped.check_state, Some(expected));
        }

        let decisions = [
            (
                ReviewDecision::ChangesRequested,
                app::ReviewDecision::ChangesRequested,
            ),
            (ReviewDecision::Approved, app::ReviewDecision::Approved),
            (
                ReviewDecision::ReviewRequired,
                app::ReviewDecision::ReviewRequired,
            ),
        ];
        for (fetched_decision, expected) in decisions {
            let mut row = pull_request(HomeSearch::Authored, 412);
            row.review_decision = Some(fetched_decision);
            let mapped = map_pull_request(row, "braidonw").expect("a well-formed row maps");
            assert_eq!(mapped.review_decision, Some(expected));
        }
    }

    #[test]
    fn an_authored_search_reaches_the_model_as_an_authored_row() {
        let fetched =
            map_pull_request(pull_request(HomeSearch::Authored, 412), "braidonw").unwrap();

        assert_eq!(fetched.search, app::HomeSearch::Authored);
    }

    /// The row's place in the list is decided by its update time, so a time
    /// that cannot be read is refused rather than guessed at.
    #[test]
    fn an_update_time_github_could_not_have_sent_is_refused() {
        let unreadable = [
            "",
            "2026-09-01",
            "2026-09-01T12:34:56",
            "2026-09-01T12:34:56+01:00",
            "2026-13-01T12:34:56Z",
            "2026-09-32T12:34:56Z",
            "2026-00-01T12:34:56Z",
            "2026-09-00T12:34:56Z",
            "2026-02-31T12:34:56Z",
            "2026-02-29T12:34:56Z",
            "2026-04-31T12:34:56Z",
            "2026-09-01T24:34:56Z",
            "2026-09-01T12:61:56Z",
            "2026-09-01T12:34:61Z",
            "yyyy-mm-ddThh:mm:ssZ",
        ];

        for timestamp in unreadable {
            let mut row = pull_request(HomeSearch::Authored, 412);
            row.updated_at = timestamp.to_owned();

            let refused = map_pull_request(row, "braidonw").expect_err("{timestamp} should refuse");

            assert!(
                refused.contains("update time"),
                "{timestamp} refused with {refused}",
            );
        }
    }

    /// A leap second is a real second GitHub can legitimately send.
    #[test]
    fn a_leap_second_is_read_rather_than_refused() {
        assert_eq!(
            epoch_millis("2016-12-31T23:59:60Z"),
            Some(1_483_228_800_000),
            "the leap second reads as the instant that follows it",
        );
    }

    #[test]
    fn the_last_day_of_every_month_is_read() {
        let last_days = [
            ("2026-01-31T00:00:00Z", true),
            ("2026-02-28T00:00:00Z", true),
            ("2024-02-29T00:00:00Z", true),
            ("2000-02-29T00:00:00Z", true),
            ("1900-02-29T00:00:00Z", false),
            ("2026-03-31T00:00:00Z", true),
            ("2026-04-30T00:00:00Z", true),
            ("2026-04-31T00:00:00Z", false),
            ("2026-06-31T00:00:00Z", false),
            ("2026-09-31T00:00:00Z", false),
            ("2026-11-31T00:00:00Z", false),
            ("2026-12-31T00:00:00Z", true),
        ];

        for (timestamp, readable) in last_days {
            assert_eq!(epoch_millis(timestamp).is_some(), readable, "{timestamp}");
        }
    }

    #[test]
    fn update_times_are_read_as_epoch_milliseconds() {
        let read = [
            ("1970-01-01T00:00:00Z", 0),
            ("1969-12-31T23:59:59Z", -1_000),
            ("2000-02-29T00:00:00Z", 951_782_400_000),
            ("2026-09-01T12:34:56Z", 1_788_266_096_000),
            ("2038-01-19T03:14:08Z", 2_147_483_648_000),
        ];

        for (timestamp, expected) in read {
            assert_eq!(epoch_millis(timestamp), Some(expected), "{timestamp}");
        }
    }

    /// One repository's fetch, having found `rows`.
    fn loaded(rows: Vec<HomePullRequest>) -> HomeRepository {
        HomeRepository {
            repository: RepositorySlug::new("acme", "widgets"),
            pull_requests: Ok(rows),
        }
    }

    #[test]
    fn a_repository_that_answered_carries_its_rows_under_its_slug() {
        let fetched = map_repository(
            loaded(vec![
                pull_request(HomeSearch::ReviewRequested, 412),
                pull_request(HomeSearch::Authored, 398),
            ]),
            Some("braidonw"),
        );

        assert_eq!(fetched.slug, "acme/widgets");
        let rows = fetched.outcome.expect("the repository answered");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].number, 412);
    }

    #[test]
    fn a_repository_that_failed_carries_the_classified_error() {
        let fetched = map_repository(
            HomeRepository {
                repository: RepositorySlug::new("acme", "billing"),
                pull_requests: Err(std::sync::Arc::new(github::GithubError::Forbidden {
                    detail: "SAML enforcement".to_owned(),
                })),
            },
            Some("braidonw"),
        );

        assert_eq!(fetched.slug, "acme/billing");
        let reason = fetched.outcome.expect_err("the repository failed");
        assert!(reason.contains("SAML enforcement"), "{reason}");
    }

    #[test]
    fn one_unreadable_update_time_fails_the_repository_it_came_from() {
        let mut unreadable = pull_request(HomeSearch::Authored, 398);
        unreadable.updated_at = "the day before yesterday".to_owned();

        let fetched = map_repository(
            loaded(vec![
                pull_request(HomeSearch::ReviewRequested, 412),
                unreadable,
            ]),
            Some("braidonw"),
        );

        let reason = fetched.outcome.expect_err("the repository failed");
        assert!(reason.contains("update time"), "{reason}");
    }

    /// Every comparison against the viewer needs a viewer, and inventing one
    /// would put pull requests in the wrong group without saying so.
    #[test]
    fn rows_that_arrived_without_a_signed_in_account_fail_the_repository() {
        let fetched = map_repository(loaded(vec![pull_request(HomeSearch::Authored, 412)]), None);

        let reason = fetched.outcome.expect_err("the repository failed");
        assert!(reason.contains("who is signed in"), "{reason}");
    }

    #[test]
    fn a_repository_that_answered_with_nothing_needs_no_signed_in_account() {
        let fetched = map_repository(loaded(Vec::new()), None);

        assert_eq!(fetched.outcome, Ok(Vec::new()));
    }

    #[test]
    fn a_preflight_against_a_signed_in_gh_lets_the_refresh_go_on() {
        let gh = FakeGh::new();
        let clone = TempDir::new().unwrap();

        assert_eq!(preflight(&gh.client(), clone.path()), Ok(()));
    }

    #[test]
    fn a_preflight_against_an_unauthenticated_gh_says_what_to_fix() {
        let gh = FakeGh::new();
        gh.refuse_authentication();
        let clone = TempDir::new().unwrap();

        let failure = preflight(&gh.client(), clone.path()).expect_err("gh is not signed in");

        assert_eq!(failure.summary, "Home could not use the GitHub CLI");
        assert!(
            failure
                .detail
                .expect("the classified error rides along")
                .contains("not logged into any GitHub hosts"),
        );
        assert!(
            failure
                .remediation
                .expect("signing in is something a reviewer can do")
                .contains("gh auth login"),
        );
    }

    /// The one search result the fake hands back, with one row in each search.
    const RECORDED_SEARCH: &str = r#"{"data":{
        "viewer":{"login":"braidonw"},
        "rateLimit":{"cost":1,"remaining":4999,"resetAt":"2026-09-01T13:00:00Z"},
        "toReview":{"issueCount":1,"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[
          {"number":412,"title":"Retry webhook deliveries","url":"https://github.com/acme/widgets/pull/412",
           "isDraft":false,"updatedAt":"2026-09-01T12:34:56Z","headRefOid":"abc123",
           "repository":{"nameWithOwner":"acme/widgets"},"author":{"login":"mlee"},
           "viewerLatestReview":null,"statusCheckRollup":{"state":"SUCCESS"}}
        ]},
        "authored":{"issueCount":1,"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[
          {"number":398,"title":"Split the renderer","url":"https://github.com/acme/widgets/pull/398",
           "isDraft":false,"updatedAt":"2026-09-01T11:00:00Z","headRefOid":"def456",
           "repository":{"nameWithOwner":"acme/widgets"},"author":{"login":"braidonw"},
           "viewerLatestReview":null,"statusCheckRollup":null,"reviewDecision":"CHANGES_REQUESTED",
           "latestOpinionatedReviews":{"nodes":[{"state":"CHANGES_REQUESTED","author":{"login":"priya"}}]},
           "reviewThreads":{"totalCount":0,"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[]}}
        ]}
    }}"#;

    /// Everything a batch answered, gathered from the reports as they land.
    fn fetch_all(client: &GithubClient, repositories: &[RepositorySlug]) -> Vec<RepositoryFetch> {
        let gathered = std::cell::RefCell::new(Vec::new());
        fetch(client, repositories, &|batch| {
            gathered.borrow_mut().extend(batch);
        });
        gathered.into_inner()
    }

    #[test]
    fn a_fetched_batch_comes_back_as_one_entry_per_repository_with_its_rows() {
        let gh = FakeGh::new();
        gh.answer_graphql(RECORDED_SEARCH);

        let fetched = fetch_all(&gh.client(), &[RepositorySlug::new("acme", "widgets")]);

        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].slug, "acme/widgets");
        let rows = fetched[0]
            .outcome
            .as_ref()
            .expect("the repository answered");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].search, app::HomeSearch::ReviewRequested);
        assert_eq!(rows[0].check_state, Some(app::CheckState::Success));
        assert_eq!(rows[1].search, app::HomeSearch::Authored);
        assert!(rows[1].changes_requested);
    }

    #[test]
    fn a_batch_gh_could_not_answer_fails_every_repository_in_it() {
        let gh = FakeGh::new();

        let fetched = fetch_all(
            &gh.client(),
            &[
                RepositorySlug::new("acme", "widgets"),
                RepositorySlug::new("acme", "billing"),
            ],
        );

        assert_eq!(fetched.len(), 2);
        assert!(fetched.iter().all(|entry| entry.outcome.is_err()));
    }
}
