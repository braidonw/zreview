//! The GraphQL path Home uses to fetch pull requests for a set of repositories.
//!
//! Home asks about many repositories at once, so they are batched into one
//! `gh api graphql` call each and the answer comes back as a per-repository
//! outcome. Grouping those rows into Home's three groups happens in the app
//! crate, so every field the grouping reads is carried here rather than fetched
//! a second time.

use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::{GithubClient, GithubError, RepositorySlug, classify_failure, parse_full_name};

/// Repositories per `gh api graphql` call.
///
/// GitHub documents a 256 character ceiling on a search query and at most five
/// boolean operators, and eight `repo:` terms is what fits under the advanced
/// syntax the query plan was measured against.
const REPOSITORIES_PER_BATCH: usize = 8;

/// GitHub serves at most 1,000 search results, which is twenty pages of fifty.
/// Anything past that is a runaway rather than a long list.
const MAX_SEARCH_PAGES: usize = 20;

/// A pull request with more than a thousand review threads is not something this
/// screen can render, so paging stops rather than looping.
const MAX_THREAD_PAGES: usize = 20;

/// How long one `gh api graphql` call may take before it is killed.
pub const DEFAULT_GRAPHQL_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a running `gh` is checked for having exited.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// GitHub's placeholder for a deleted account, matching what its own UI shows.
const DELETED_USER: &str = "ghost";

const REVIEW_REQUESTED_FILTER: &str = "is:pr is:open draft:false review-requested:@me";
const AUTHORED_FILTER: &str = "is:pr is:open author:@me";

/// Everything Home needs for one set of repositories.
#[derive(Debug)]
pub struct HomeFetch {
    /// The authenticated account, read from the same query as the rows.
    ///
    /// `None` when no batch got as far as an answer.
    pub viewer_login: Option<String>,
    /// The point budget as of the last call that answered.
    pub rate_limit: Option<RateLimit>,
    /// One entry per distinct requested repository, in the order asked for.
    pub repositories: Vec<HomeRepository>,
}

/// One requested repository's pull requests, or why they could not be fetched.
#[derive(Clone, Debug)]
pub struct HomeRepository {
    pub repository: RepositorySlug,
    /// Shared because one failure belongs to every repository in its batch and
    /// [`GithubError`] is not `Clone`.
    pub pull_requests: Result<Vec<HomePullRequest>, Arc<GithubError>>,
}

/// Which of the two searches returned a row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HomeSearch {
    ReviewRequested,
    Authored,
}

/// One open pull request, with every field Home's grouping reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HomePullRequest {
    pub search: HomeSearch,
    pub repository: RepositorySlug,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub draft: bool,
    pub updated_at: String,
    pub head_oid: String,
    pub author_login: String,
    /// The commit the viewer's own latest review was left against, when they
    /// have reviewed and GitHub still knows which commit that was.
    pub viewer_latest_review_oid: Option<String>,
    /// `None` when the head has no checks at all, which reads differently from
    /// a pending one.
    pub check_state: Option<StatusCheckState>,
    /// Authored rows only. `None` on a pull request nobody has reviewed.
    pub review_decision: Option<ReviewDecision>,
    /// Authored rows only. One entry per reviewer, dismissals already dropped.
    pub latest_opinionated_reviews: Vec<OpinionatedReview>,
    /// Authored rows only.
    pub review_threads: Vec<ReviewThread>,
}

/// A reviewer's standing verdict on a pull request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpinionatedReview {
    pub state: ReviewState,
    pub author_login: String,
}

/// One review thread, reduced to what "is the author being waited on" needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewThread {
    pub resolved: bool,
    /// `None` only when the thread carries no comments at all.
    pub last_comment_author_login: Option<String>,
}

/// GitHub's `StatusState`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StatusCheckState {
    Expected,
    Error,
    Failure,
    Pending,
    Success,
}

/// GitHub's `PullRequestReviewState`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewState {
    Pending,
    Commented,
    Approved,
    ChangesRequested,
    Dismissed,
}

/// GitHub's `PullRequestReviewDecision`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewDecision {
    ChangesRequested,
    Approved,
    ReviewRequired,
}

/// What the last answered call reported about the GraphQL point budget.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateLimit {
    pub cost: u32,
    pub remaining: u32,
    pub reset_at: String,
}

impl GithubClient {
    /// Fetches the pull requests Home lists for `repositories`.
    ///
    /// Repositories are queried eight at a time. A batch that fails as a whole
    /// marks every repository in it failed with the same classified error, while
    /// an error naming one repository fails only that one, so a single
    /// unauthorized organisation never hides the rest of the list.
    ///
    /// Duplicates are collapsed, keeping the first occurrence, so there is one
    /// entry per distinct repository.
    #[must_use]
    pub fn fetch_home_pull_requests(&self, repositories: &[RepositorySlug]) -> HomeFetch {
        let requested = distinct(repositories);
        let mut fetch = HomeFetch {
            viewer_login: None,
            rate_limit: None,
            repositories: Vec::with_capacity(requested.len()),
        };

        for batch in requested.chunks(REPOSITORIES_PER_BATCH) {
            let fetched = self.fetch_batch(batch);
            if fetch.viewer_login.is_none() {
                fetch.viewer_login = fetched.viewer_login;
            }
            if fetched.rate_limit.is_some() {
                fetch.rate_limit = fetched.rate_limit;
            }
            for (repository, rows) in batch.iter().zip(fetched.pull_requests) {
                fetch.repositories.push(HomeRepository {
                    repository: repository.clone(),
                    pull_requests: rows.map(strip_cursors),
                });
            }
        }

        fetch
    }

    /// Runs one batch, turning a whole-batch failure into that same failure for
    /// every repository in it.
    fn fetch_batch(&self, batch: &[RepositorySlug]) -> Batch {
        match self.collect_batch(batch) {
            Ok(batch_result) => batch_result,
            Err(failure) => {
                let shared = Arc::new(failure);
                Batch {
                    viewer_login: None,
                    rate_limit: None,
                    pull_requests: batch.iter().map(|_| Err(Arc::clone(&shared))).collect(),
                }
            }
        }
    }

    fn collect_batch(&self, batch: &[RepositorySlug]) -> Result<Batch, GithubError> {
        let to_review_query = search_query(REVIEW_REQUESTED_FILTER, batch);
        let authored_query = search_query(AUTHORED_FILTER, batch);

        let payload = self.graphql(
            &[
                ("query", &batch_document()),
                ("toReview", &to_review_query),
                ("authored", &authored_query),
            ],
            &[],
        )?;

        let mut collected = Collected::default();
        let cursors = collected.absorb_batch(payload)?;
        if let Some(cursor) = cursors.to_review {
            self.page(
                HomeSearch::ReviewRequested,
                &to_review_query,
                cursor,
                &mut collected,
            )?;
        }
        if let Some(cursor) = cursors.authored {
            self.page(
                HomeSearch::Authored,
                &authored_query,
                cursor,
                &mut collected,
            )?;
        }

        let mut batch_result = collected.distribute(batch)?;
        self.complete_threads(batch, &mut batch_result);
        Ok(batch_result)
    }

    /// Follows one alias's cursor until GitHub says there is no more.
    fn page(
        &self,
        search: HomeSearch,
        query: &str,
        first_cursor: String,
        collected: &mut Collected,
    ) -> Result<(), GithubError> {
        let mut cursor = first_cursor;
        for _ in 0..MAX_SEARCH_PAGES {
            let payload = self.graphql(
                &[
                    ("query", &search.page_document()),
                    (search.query_variable(), query),
                    (search.cursor_variable(), &cursor),
                ],
                &[],
            )?;
            match collected.absorb_page(search, payload)? {
                Some(next) => cursor = next,
                None => return Ok(()),
            }
        }
        Err(GithubError::PagingLimit {
            pages: MAX_SEARCH_PAGES,
            subject: "search results",
        })
    }

    /// Fetches the threads that did not fit the fifty the list query asked for.
    ///
    /// A follow-up belongs to one pull request, so a failure fails only that
    /// pull request's repository rather than everything in the batch.
    fn complete_threads(&self, batch: &[RepositorySlug], result: &mut Batch) {
        for (index, repository) in batch.iter().enumerate() {
            let Ok(rows) = &result.pull_requests[index] else {
                continue;
            };
            let pending = rows
                .iter()
                .enumerate()
                .filter_map(|(position, row)| {
                    row.threads_after
                        .clone()
                        .map(|cursor| (position, row.row.number, cursor))
                })
                .collect::<Vec<_>>();

            for (position, number, cursor) in pending {
                match self.remaining_threads(repository, number, cursor) {
                    Ok(threads) => {
                        if let Ok(rows) = &mut result.pull_requests[index] {
                            rows[position].row.review_threads.extend(threads);
                        }
                    }
                    Err(failure) => {
                        result.pull_requests[index] = Err(Arc::new(failure));
                        break;
                    }
                }
            }
        }
    }

    fn remaining_threads(
        &self,
        repository: &RepositorySlug,
        number: u64,
        first_cursor: String,
    ) -> Result<Vec<ReviewThread>, GithubError> {
        let mut threads = Vec::new();
        let mut cursor = first_cursor;
        for _ in 0..MAX_THREAD_PAGES {
            let payload = self.graphql(
                &[
                    ("query", THREADS_DOCUMENT),
                    ("owner", &repository.owner),
                    ("name", &repository.name),
                    ("after", &cursor),
                ],
                &[("number", number)],
            )?;
            let page = payload
                .data
                .repository
                .and_then(|holder| holder.pull_request)
                .ok_or_else(|| GithubError::NotFound {
                    detail: format!(
                        "GitHub returned no pull request {}#{number}",
                        repository.full_name()
                    ),
                })?
                .review_threads;
            let next = page.page_info.next_cursor();
            threads.extend(page.nodes.into_iter().map(ReviewThread::from));
            match next {
                Some(following) => cursor = following,
                None => return Ok(threads),
            }
        }
        Err(GithubError::PagingLimit {
            pages: MAX_THREAD_PAGES,
            subject: "review threads",
        })
    }

    /// Runs one `gh api graphql` call and reads its GraphQL envelope.
    ///
    /// `gh` exits non-zero whenever the envelope carries errors, including the
    /// partial answers this path is built on, so the body is read first and the
    /// exit status only decides what to report when there is no `data` at all.
    fn graphql(
        &self,
        strings: &[(&str, &str)],
        numbers: &[(&str, u64)],
    ) -> Result<Payload, GithubError> {
        let mut arguments = vec!["api".to_owned(), "graphql".to_owned()];
        for (name, value) in strings {
            arguments.push("-f".to_owned());
            arguments.push(format!("{name}={value}"));
        }
        for (name, value) in numbers {
            arguments.push("-F".to_owned());
            arguments.push(format!("{name}={value}"));
        }

        let output = self.run_gh(&arguments)?;
        let status = output.status.code().unwrap_or(-1);
        match serde_json::from_slice::<GraphResponse>(&output.stdout) {
            Ok(response) => match response.data {
                Some(data) => Ok(Payload {
                    data,
                    errors: response.errors,
                    status,
                }),
                None => Err(response
                    .errors
                    .first()
                    .map_or_else(|| classify_failure(&output), |error| error.classify(status))),
            },
            Err(malformed) => {
                if output.status.success() {
                    Err(GithubError::InvalidResponse(malformed))
                } else {
                    Err(classify_failure(&output))
                }
            }
        }
    }

    /// Spawns `gh` with stdin closed and kills it if it outlives the timeout.
    fn run_gh(&self, arguments: &[String]) -> Result<Output, GithubError> {
        let mut child = Command::new(&self.gh_executable)
            .args(arguments)
            .env("GH_PROMPT_DISABLED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(spawn_error)?;

        // Both pipes are drained on their own threads, so a large response
        // cannot fill a pipe buffer and deadlock the child against this wait.
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());

        let deadline = Instant::now() + self.graphql_timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Ok(Output {
                        status,
                        stdout: stdout.collect()?,
                        stderr: stderr.collect()?,
                    });
                }
                Ok(None) => {}
                Err(source) => return Err(spawn_error(source)),
            }
            if Instant::now() >= deadline {
                // The readers are left to unwind on their own. Joining them here
                // would wait on any grandchild still holding the pipe open.
                let _ = child.kill();
                let _ = child.wait();
                return Err(GithubError::Timeout {
                    timeout_ms: timeout_ms(self.graphql_timeout),
                });
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

/// One batch's repositories, in the order they were requested.
struct Batch {
    viewer_login: Option<String>,
    rate_limit: Option<RateLimit>,
    pull_requests: Vec<Result<Vec<CollectedRow>, Arc<GithubError>>>,
}

/// A row plus where its remaining review threads start, which is paging state
/// rather than something Home renders.
struct CollectedRow {
    row: HomePullRequest,
    threads_after: Option<String>,
}

/// Everything one batch's calls have returned so far.
#[derive(Default)]
struct Collected {
    rows: Vec<CollectedRow>,
    errors: Vec<GraphError>,
    null_nodes: usize,
    viewer_login: Option<String>,
    rate_limit: Option<RateLimit>,
    status: i32,
}

/// Where each alias's next page starts, when it has one.
struct Cursors {
    to_review: Option<String>,
    authored: Option<String>,
}

impl Collected {
    fn absorb_batch(&mut self, payload: Payload) -> Result<Cursors, GithubError> {
        let Payload {
            data,
            errors,
            status,
        } = payload;
        self.status = status;
        self.errors.extend(errors);
        if let Some(viewer) = data.viewer {
            self.viewer_login = Some(viewer.login);
        }
        if data.rate_limit.is_some() {
            self.rate_limit = data.rate_limit;
        }

        Ok(Cursors {
            to_review: self.absorb_search(HomeSearch::ReviewRequested, data.to_review)?,
            authored: self.absorb_search(HomeSearch::Authored, data.authored)?,
        })
    }

    fn absorb_page(
        &mut self,
        search: HomeSearch,
        payload: Payload,
    ) -> Result<Option<String>, GithubError> {
        let Payload {
            data,
            errors,
            status,
        } = payload;
        self.status = status;
        self.errors.extend(errors);
        if data.rate_limit.is_some() {
            self.rate_limit = data.rate_limit;
        }

        let page = match search {
            HomeSearch::ReviewRequested => data.to_review,
            HomeSearch::Authored => data.authored,
        };
        self.absorb_search(search, page)
    }

    /// Reads one search alias, keeping its rows and counting the nodes GitHub
    /// could not show.
    fn absorb_search(
        &mut self,
        search: HomeSearch,
        page: Option<GraphSearch>,
    ) -> Result<Option<String>, GithubError> {
        let Some(page) = page else {
            // The whole alias is missing, so no repository in this batch has an
            // answer and there is nothing honest to attribute.
            return Err(self.errors.first().map_or_else(
                || GithubError::Command {
                    status: self.status,
                    stderr: format!("GitHub returned no {} results", search.query_variable()),
                },
                |error| error.classify(self.status),
            ));
        };

        let next = page.page_info.next_cursor();
        for node in page.nodes {
            match node {
                Some(node) => self.rows.push(CollectedRow::build(search, node)?),
                None => self.null_nodes += 1,
            }
        }
        Ok(next)
    }

    /// Splits the batch's rows and errors across the repositories that asked for
    /// them.
    fn distribute(self, batch: &[RepositorySlug]) -> Result<Batch, GithubError> {
        if self.null_nodes > 0 && self.errors.is_empty() {
            return Err(GithubError::Command {
                status: self.status,
                stderr: format!(
                    "GitHub hid {} search results without saying why",
                    self.null_nodes
                ),
            });
        }

        let keys = batch
            .iter()
            .map(|repository| repository.full_name().to_lowercase())
            .collect::<Vec<_>>();
        let mut pull_requests = batch
            .iter()
            .map(|_| Ok(Vec::new()))
            .collect::<Vec<Result<Vec<CollectedRow>, Arc<GithubError>>>>();

        for error in &self.errors {
            let named = error.names(batch, &keys);
            if named.is_empty() {
                return Err(error.classify(self.status));
            }
            let shared = Arc::new(error.classify(self.status));
            for index in named {
                pull_requests[index] = Err(Arc::clone(&shared));
            }
        }

        for row in self.rows {
            let key = row.row.repository.full_name().to_lowercase();
            let Some(index) = keys.iter().position(|candidate| *candidate == key) else {
                return Err(GithubError::UnexpectedRepository {
                    expected: batch
                        .iter()
                        .map(RepositorySlug::full_name)
                        .collect::<Vec<_>>()
                        .join(", "),
                    actual: row.row.repository.full_name(),
                });
            };
            if let Ok(rows) = &mut pull_requests[index] {
                rows.push(row);
            }
        }

        Ok(Batch {
            viewer_login: self.viewer_login,
            rate_limit: self.rate_limit,
            pull_requests,
        })
    }
}

impl CollectedRow {
    fn build(search: HomeSearch, node: GraphPullRequest) -> Result<Self, GithubError> {
        let repository = parse_full_name(&node.repository.name_with_owner).ok_or_else(|| {
            GithubError::UnexpectedRepository {
                expected: "owner/name".to_owned(),
                actual: node.repository.name_with_owner.clone(),
            }
        })?;
        let threads = node.review_threads;
        let threads_after = threads
            .as_ref()
            .and_then(|threads| threads.page_info.next_cursor());

        Ok(Self {
            threads_after,
            row: HomePullRequest {
                search,
                repository,
                number: node.number,
                title: node.title,
                url: node.url,
                draft: node.is_draft,
                updated_at: node.updated_at,
                head_oid: node.head_ref_oid,
                author_login: login(node.author),
                viewer_latest_review_oid: node
                    .viewer_latest_review
                    .and_then(|review| review.commit)
                    .map(|commit| commit.oid),
                check_state: node.status_check_rollup.map(|rollup| rollup.state),
                review_decision: node.review_decision,
                latest_opinionated_reviews: node.latest_opinionated_reviews.map_or_else(
                    Vec::new,
                    |reviews| {
                        reviews
                            .nodes
                            .into_iter()
                            .map(OpinionatedReview::from)
                            .collect()
                    },
                ),
                review_threads: threads.map_or_else(Vec::new, |threads| {
                    threads.nodes.into_iter().map(ReviewThread::from).collect()
                }),
            },
        })
    }
}

impl HomeSearch {
    fn page_document(self) -> String {
        match self {
            Self::ReviewRequested => format!("{TO_REVIEW_PAGE_OPERATION}{ROW_FRAGMENT}"),
            Self::Authored => {
                format!("{AUTHORED_PAGE_OPERATION}{ROW_FRAGMENT}{AUTHORED_FRAGMENT}")
            }
        }
    }

    const fn query_variable(self) -> &'static str {
        match self {
            Self::ReviewRequested => "toReview",
            Self::Authored => "authored",
        }
    }

    const fn cursor_variable(self) -> &'static str {
        match self {
            Self::ReviewRequested => "toReviewAfter",
            Self::Authored => "authoredAfter",
        }
    }
}

impl From<GraphOpinionatedReview> for OpinionatedReview {
    fn from(review: GraphOpinionatedReview) -> Self {
        Self {
            state: review.state,
            author_login: login(review.author),
        }
    }
}

impl From<GraphThread> for ReviewThread {
    fn from(thread: GraphThread) -> Self {
        Self {
            resolved: thread.is_resolved,
            last_comment_author_login: thread
                .comments
                .nodes
                .into_iter()
                .next_back()
                .map(|comment| login(comment.author)),
        }
    }
}

fn strip_cursors(rows: Vec<CollectedRow>) -> Vec<HomePullRequest> {
    rows.into_iter().map(|row| row.row).collect()
}

/// Keeps the first occurrence of each repository, comparing the way GitHub does.
fn distinct(repositories: &[RepositorySlug]) -> Vec<RepositorySlug> {
    let mut seen = Vec::new();
    let mut kept = Vec::new();
    for repository in repositories {
        let key = repository.full_name().to_lowercase();
        if !seen.contains(&key) {
            seen.push(key);
            kept.push(repository.clone());
        }
    }
    kept
}

/// Builds one search string.
///
/// The repository clause is written out as an explicit OR because under
/// `ISSUE_ADVANCED` a space between `repo:` terms is an AND, which would match
/// nothing.
fn search_query(filter: &str, repositories: &[RepositorySlug]) -> String {
    let clause = repositories
        .iter()
        .map(|repository| format!("repo:{}", repository.full_name()))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("{filter} ({clause})")
}

fn login(author: Option<GraphActor>) -> String {
    author.map_or_else(|| DELETED_USER.to_owned(), |author| author.login)
}

fn timeout_ms(timeout: Duration) -> u64 {
    u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)
}

/// Distinguishes "gh is not installed" from other failures to run it.
///
/// The GraphQL path names repositories by slug rather than by clone, so `gh`
/// runs in whatever directory the application was started in.
fn spawn_error(source: std::io::Error) -> GithubError {
    let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    crate::execution_error(&directory, source)
}

/// A pipe being read to its end on its own thread.
struct Drain(std::thread::JoinHandle<std::io::Result<Vec<u8>>>);

fn drain(pipe: Option<impl Read + Send + 'static>) -> Drain {
    Drain(std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(mut pipe) = pipe {
            pipe.read_to_end(&mut buffer)?;
        }
        Ok(buffer)
    }))
}

impl Drain {
    fn collect(self) -> Result<Vec<u8>, GithubError> {
        match self.0.join() {
            Ok(read) => read.map_err(spawn_error),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

/// Whether `text` names this exact repository rather than a longer slug that
/// merely starts with it.
fn mentions_repository(text: &str, full_name: &str) -> bool {
    bounded_starts(text, full_name).any(|start| {
        text.as_bytes()
            .get(start + full_name.len())
            .is_none_or(|byte| !word_byte(*byte))
    })
}

/// Whether `text` names this owner as an organisation, the shape a SAML failure
/// takes when it points at `github.com/orgs/<owner>/sso`.
fn mentions_owner(text: &str, owner: &str) -> bool {
    bounded_starts(text, owner).any(|start| text.as_bytes().get(start + owner.len()) == Some(&b'/'))
}

fn bounded_starts<'a>(text: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(needle)
        .map(|(start, _)| start)
        .filter(move |start| *start == 0 || !word_byte(text.as_bytes()[start - 1]))
}

const fn word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

fn batch_document() -> String {
    format!("{BATCH_OPERATION}{ROW_FRAGMENT}{AUTHORED_FRAGMENT}")
}

const BATCH_OPERATION: &str = r"query($toReview: String!, $authored: String!) {
  viewer { login }
  rateLimit { cost remaining resetAt }
  toReview: search(query: $toReview, type: ISSUE_ADVANCED, first: 50) {
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest { ...Row } }
  }
  authored: search(query: $authored, type: ISSUE_ADVANCED, first: 50) {
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest { ...Row ...Authored } }
  }
}
";

const TO_REVIEW_PAGE_OPERATION: &str = r"query($toReview: String!, $toReviewAfter: String!) {
  rateLimit { cost remaining resetAt }
  toReview: search(query: $toReview, type: ISSUE_ADVANCED, first: 50, after: $toReviewAfter) {
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest { ...Row } }
  }
}
";

const AUTHORED_PAGE_OPERATION: &str = r"query($authored: String!, $authoredAfter: String!) {
  rateLimit { cost remaining resetAt }
  authored: search(query: $authored, type: ISSUE_ADVANCED, first: 50, after: $authoredAfter) {
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest { ...Row ...Authored } }
  }
}
";

const THREADS_DOCUMENT: &str = r"query($owner: String!, $name: String!, $number: Int!, $after: String!) {
  rateLimit { cost remaining resetAt }
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 50, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes { isResolved comments(last: 1) { nodes { author { login } } } }
      }
    }
  }
}
";

const ROW_FRAGMENT: &str = r"fragment Row on PullRequest {
  number title url isDraft updatedAt headRefOid
  repository { nameWithOwner }
  author { login }
  viewerLatestReview { commit { oid } }
  statusCheckRollup { state }
}
";

const AUTHORED_FRAGMENT: &str = r"fragment Authored on PullRequest {
  reviewDecision
  latestOpinionatedReviews(first: 50) { nodes { state author { login } } }
  reviewThreads(first: 50) {
    totalCount
    pageInfo { hasNextPage endCursor }
    nodes { isResolved comments(last: 1) { nodes { author { login } } } }
  }
}
";

/// One answered call, split into what it returned and what it could not.
struct Payload {
    data: GraphData,
    errors: Vec<GraphError>,
    status: i32,
}

#[derive(Debug, Deserialize)]
struct GraphResponse {
    #[serde(default)]
    data: Option<GraphData>,
    #[serde(default)]
    errors: Vec<GraphError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphData {
    #[serde(default)]
    viewer: Option<GraphViewer>,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
    #[serde(default)]
    to_review: Option<GraphSearch>,
    #[serde(default)]
    authored: Option<GraphSearch>,
    #[serde(default)]
    repository: Option<GraphThreadRepository>,
}

#[derive(Debug, Deserialize)]
struct GraphError {
    #[serde(default)]
    message: String,
    #[serde(default, rename = "type")]
    error_type: Option<String>,
}

impl GraphError {
    /// Sorts a GraphQL error into the same categories a failed `gh api` call
    /// lands in, so Home's remediation text matches what a Session shows.
    fn classify(&self, status: i32) -> GithubError {
        let detail = self.message.clone();
        match self.error_type.as_deref() {
            Some("FORBIDDEN") => GithubError::Forbidden { detail },
            Some("NOT_FOUND") => GithubError::NotFound { detail },
            Some("RATE_LIMITED") => GithubError::RateLimited { detail },
            Some("UNPROCESSABLE") => GithubError::Validation { detail },
            Some(_) | None => GithubError::Command {
                status,
                stderr: detail,
            },
        }
    }

    /// Which repositories of the batch this error is about.
    ///
    /// A SAML failure names its organisation in the authorization link it offers
    /// rather than the repository, so the owner is the fallback.
    fn names(&self, batch: &[RepositorySlug], keys: &[String]) -> Vec<usize> {
        let text = self.message.to_lowercase();
        let by_repository = keys
            .iter()
            .enumerate()
            .filter(|(_, key)| mentions_repository(&text, key))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if !by_repository.is_empty() {
            return by_repository;
        }
        batch
            .iter()
            .enumerate()
            .filter(|(_, repository)| mentions_owner(&text, &repository.owner.to_lowercase()))
            .map(|(index, _)| index)
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct GraphViewer {
    login: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphSearch {
    page_info: GraphPageInfo,
    nodes: Vec<Option<GraphPullRequest>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphPageInfo {
    has_next_page: bool,
    #[serde(default)]
    end_cursor: Option<String>,
}

impl GraphPageInfo {
    /// GitHub only omits the cursor on an empty connection, which never has a
    /// next page, so there is no case where paging stops early.
    fn next_cursor(&self) -> Option<String> {
        if self.has_next_page {
            self.end_cursor.clone()
        } else {
            None
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphPullRequest {
    number: u64,
    title: String,
    url: String,
    is_draft: bool,
    updated_at: String,
    head_ref_oid: String,
    repository: GraphRepositoryName,
    author: Option<GraphActor>,
    viewer_latest_review: Option<GraphViewerReview>,
    status_check_rollup: Option<GraphStatusCheckRollup>,
    #[serde(default)]
    review_decision: Option<ReviewDecision>,
    #[serde(default)]
    latest_opinionated_reviews: Option<GraphReviewConnection>,
    #[serde(default)]
    review_threads: Option<GraphThreadConnection>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphRepositoryName {
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct GraphActor {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GraphViewerReview {
    commit: Option<GraphCommit>,
}

#[derive(Debug, Deserialize)]
struct GraphCommit {
    oid: String,
}

#[derive(Debug, Deserialize)]
struct GraphStatusCheckRollup {
    state: StatusCheckState,
}

#[derive(Debug, Deserialize)]
struct GraphReviewConnection {
    nodes: Vec<GraphOpinionatedReview>,
}

#[derive(Debug, Deserialize)]
struct GraphOpinionatedReview {
    state: ReviewState,
    author: Option<GraphActor>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphThreadConnection {
    page_info: GraphPageInfo,
    nodes: Vec<GraphThread>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphThread {
    is_resolved: bool,
    comments: GraphCommentConnection,
}

#[derive(Debug, Deserialize)]
struct GraphCommentConnection {
    nodes: Vec<GraphComment>,
}

#[derive(Debug, Deserialize)]
struct GraphComment {
    author: Option<GraphActor>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphThreadRepository {
    pull_request: Option<GraphThreadedPullRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphThreadedPullRequest {
    review_threads: GraphThreadConnection,
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        collections::BTreeMap,
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        time::{Duration, Instant},
    };

    use super::{
        HomeFetch, HomePullRequest, HomeSearch, MAX_SEARCH_PAGES, OpinionatedReview, RateLimit,
        ReviewDecision, ReviewState, ReviewThread, StatusCheckState,
    };
    use crate::{GithubClient, GithubError, RepositorySlug};
    use tempfile::TempDir;

    /// Two repositories over both searches, in the shape GitHub returns.
    const SEARCH_RESPONSE: &str = include_str!("../tests/fixtures/home-search.json");

    const EMPTY_RESPONSE: &str = r#"{"data":{
      "viewer": {"login": "reviewer"},
      "rateLimit": {"cost": 6, "remaining": 4994, "resetAt": "2026-09-02T13:00:00Z"},
      "toReview": {"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": []},
      "authored": {"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": []}
    }}"#;

    #[test]
    fn eight_repositories_are_one_query_with_a_single_or_clause() {
        let fake = FakeGh::new();
        fake.respond(EMPTY_RESPONSE);

        let fetch = GithubClient::new(fake.path()).fetch_home_pull_requests(&slugs(1..=8));

        assert_eq!(fake.calls(), 1, "eight repositories are one query");
        let clause = "(repo:acme/repo-1 OR repo:acme/repo-2 OR repo:acme/repo-3 \
             OR repo:acme/repo-4 OR repo:acme/repo-5 OR repo:acme/repo-6 \
             OR repo:acme/repo-7 OR repo:acme/repo-8)";
        let variables = fake.variables(1);
        assert_eq!(
            variables["toReview"],
            format!("is:pr is:open draft:false review-requested:@me {clause}"),
        );
        assert_eq!(
            variables["authored"],
            format!("is:pr is:open author:@me {clause}"),
        );

        assert_eq!(fetch.viewer_login.as_deref(), Some("reviewer"));
        assert_eq!(fetch.repositories.len(), 8);
        for repository in &fetch.repositories {
            assert!(repository.pull_requests.as_ref().unwrap().is_empty());
        }
    }

    #[test]
    fn nine_repositories_are_split_into_two_batches() {
        let fake = FakeGh::new();
        fake.respond(EMPTY_RESPONSE).respond(EMPTY_RESPONSE);

        let fetch = GithubClient::new(fake.path()).fetch_home_pull_requests(&slugs(1..=9));

        assert_eq!(fake.calls(), 2, "nine repositories are two queries");
        assert!(
            fake.variables(1)["authored"].contains("repo:acme/repo-8)"),
            "the first batch takes eight repositories",
        );
        assert_eq!(
            fake.variables(2)["authored"],
            "is:pr is:open author:@me (repo:acme/repo-9)",
            "the ninth repository queries alone",
        );
        assert_eq!(fetch.repositories.len(), 9);
        assert_eq!(
            fetch.repositories[8].repository,
            RepositorySlug::new("acme", "repo-9"),
        );
    }

    #[test]
    fn an_alias_with_more_results_is_paged_by_cursor_until_exhausted() {
        let fake = FakeGh::new();
        fake.respond(&response(&[
            alias(
                "toReview",
                &[requested_node("acme/widgets", 1)],
                Some("PAGE-2"),
            ),
            alias("authored", &[], None),
        ]))
        .respond(&response(&[alias(
            "toReview",
            &[requested_node("acme/widgets", 2)],
            Some("PAGE-3"),
        )]))
        .respond(&response(&[alias(
            "toReview",
            &[requested_node("acme/widgets", 3)],
            None,
        )]));

        let fetch = GithubClient::new(fake.path())
            .fetch_home_pull_requests(&[RepositorySlug::new("acme", "widgets")]);

        assert_eq!(fake.calls(), 3, "paging follows the cursor to the end");
        let second = fake.variables(2);
        assert_eq!(second["toReviewAfter"], "PAGE-2");
        assert!(
            !second.contains_key("authored"),
            "an exhausted alias is not asked again",
        );
        assert_eq!(fake.variables(3)["toReviewAfter"], "PAGE-3");

        let numbers = rows(&fetch, "acme/widgets")
            .iter()
            .map(|row| row.number)
            .collect::<Vec<_>>();
        assert_eq!(numbers, [1, 2, 3], "every page's rows are kept");
    }

    /// One organisation the token was never authorized for must not empty the
    /// whole screen.
    #[test]
    fn a_forbidden_null_node_fails_only_the_repository_it_names() {
        let fake = FakeGh::new();
        fake.respond_partial(
            &envelope(
                &[
                    alias(
                        "toReview",
                        &["null".to_owned(), requested_node("acme/widgets", 42)],
                        None,
                    ),
                    alias("authored", &[], None),
                ],
                &[saml_error("secure")],
            ),
            "gh: Resource protected by organization SAML enforcement.",
        );

        let fetch = GithubClient::new(fake.path()).fetch_home_pull_requests(&[
            RepositorySlug::new("acme", "widgets"),
            RepositorySlug::new("secure", "vault"),
        ]);

        assert_eq!(rows(&fetch, "acme/widgets").len(), 1, "the rest still load");

        let refused = fetch.repositories[1].pull_requests.as_ref().unwrap_err();
        assert!(
            matches!(&**refused, GithubError::Forbidden { detail }
                if detail.contains("SAML")),
            "unexpected error: {refused}",
        );
        assert!(
            refused.remediation().unwrap().contains("SSO"),
            "the remediation is the one a Session shows",
        );
    }

    /// An error nobody in the batch is named by leaves every row in doubt, so it
    /// is reported rather than quietly dropping whatever did not arrive.
    #[test]
    fn an_error_naming_no_repository_fails_the_whole_batch() {
        let fake = FakeGh::new();
        fake.respond_partial(
            &envelope(
                &[
                    alias("toReview", &["null".to_owned()], None),
                    alias("authored", &[], None),
                ],
                &[r#"{"type": "INTERNAL", "message": "Something went wrong."}"#.to_owned()],
            ),
            "gh: Something went wrong.",
        );

        let fetch = GithubClient::new(fake.path()).fetch_home_pull_requests(&[
            RepositorySlug::new("acme", "widgets"),
            RepositorySlug::new("secure", "vault"),
        ]);

        for repository in &fetch.repositories {
            let error = repository.pull_requests.as_ref().unwrap_err();
            assert!(
                error.to_string().contains("Something went wrong"),
                "unexpected error: {error}",
            );
        }
    }

    /// Nothing arrived, so nothing may be presented as an empty list.
    #[test]
    fn a_batch_that_fails_as_a_whole_fails_every_repository_in_it() {
        let fake = FakeGh::new();
        fake.fail("gh: Bad credentials (HTTP 401)")
            .fail("gh: Bad credentials (HTTP 401)");

        let fetch = GithubClient::new(fake.path()).fetch_home_pull_requests(&slugs(1..=9));

        assert_eq!(fake.calls(), 2, "both batches were attempted");
        assert_eq!(fetch.repositories.len(), 9);
        assert!(fetch.viewer_login.is_none(), "GitHub never answered");
        for repository in &fetch.repositories {
            let error = repository.pull_requests.as_ref().unwrap_err();
            assert!(
                matches!(&**error, GithubError::Unauthenticated { .. }),
                "unexpected error for {}: {error}",
                repository.repository.full_name(),
            );
            assert!(
                error.remediation().unwrap().contains("gh auth login"),
                "the remediation is the one a Session shows",
            );
        }
    }

    /// A page that fails part way through leaves the earlier pages incomplete,
    /// which must not read as a short list.
    #[test]
    fn a_failed_continuation_fails_its_whole_batch() {
        let fake = FakeGh::new();
        fake.respond(&response(&[
            alias(
                "toReview",
                &[requested_node("acme/widgets", 1)],
                Some("PAGE-2"),
            ),
            alias("authored", &[], None),
        ]))
        .fail("gh: Server Error (HTTP 502)");

        let fetch = GithubClient::new(fake.path())
            .fetch_home_pull_requests(&[RepositorySlug::new("acme", "widgets")]);

        let error = fetch.repositories[0].pull_requests.as_ref().unwrap_err();
        assert!(
            matches!(&**error, GithubError::ServerError { status: 502, .. }),
            "unexpected error: {error}",
        );
    }

    /// The list query asks for fifty threads, and "is the author waited on" is
    /// wrong if the unresolved one is the fifty-first.
    #[test]
    fn a_pull_request_with_more_than_fifty_threads_fetches_the_rest() {
        let fake = FakeGh::new();
        fake.respond(&response(&[
            alias("toReview", &[], None),
            alias(
                "authored",
                &[authored_node(91, &[thread(true, "reviewer")], Some("T-2"))],
                None,
            ),
        ]))
        .respond(&threads_response(
            &[thread(false, "maintainer")],
            Some("T-3"),
        ))
        .respond(&threads_response(&[thread(true, "reviewer")], None));

        let fetch = GithubClient::new(fake.path())
            .fetch_home_pull_requests(&[RepositorySlug::new("acme", "widgets")]);

        assert_eq!(fake.calls(), 3, "the follow-up pages to the end");
        let follow_up = fake.variables(2);
        assert_eq!(follow_up["owner"], "acme");
        assert_eq!(follow_up["name"], "widgets");
        assert_eq!(follow_up["number"], "91");
        assert_eq!(follow_up["after"], "T-2");
        assert!(
            fake.arguments(2)
                .windows(2)
                .any(|pair| pair[0] == "-F" && pair[1] == "number=91"),
            "the pull request number is sent as an integer, not a string",
        );
        assert_eq!(fake.variables(3)["after"], "T-3");

        let threads = &rows(&fetch, "acme/widgets")[0].review_threads;
        assert_eq!(
            threads,
            &[
                ReviewThread {
                    resolved: true,
                    last_comment_author_login: Some("reviewer".to_owned()),
                },
                ReviewThread {
                    resolved: false,
                    last_comment_author_login: Some("maintainer".to_owned()),
                },
                ReviewThread {
                    resolved: true,
                    last_comment_author_login: Some("reviewer".to_owned()),
                },
            ],
            "the follow-up threads join the ones the list query returned",
        );
    }

    /// A refresh that hangs would leave Home refreshing forever, so the call is
    /// killed rather than waited on.
    #[test]
    fn a_call_that_outlives_its_timeout_is_killed_and_reported() {
        let fake = FakeGh::new();
        fake.hang(10);

        let started = Instant::now();
        let fetch = GithubClient::new(fake.path())
            .with_graphql_timeout(Duration::from_millis(150))
            .fetch_home_pull_requests(&[RepositorySlug::new("acme", "widgets")]);
        let elapsed = started.elapsed();

        let error = fetch.repositories[0].pull_requests.as_ref().unwrap_err();
        assert!(
            matches!(&**error, GithubError::Timeout { timeout_ms: 150 }),
            "unexpected error: {error}",
        );
        assert!(
            error.remediation().unwrap().contains("network connection"),
            "a timeout has advice to give",
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "the child was waited on rather than killed, after {elapsed:?}",
        );
    }

    #[test]
    fn every_call_runs_non_interactively_with_stdin_closed() {
        let fake = FakeGh::new();
        fake.respond(EMPTY_RESPONSE);

        let fetch = GithubClient::new(fake.path())
            .fetch_home_pull_requests(&[RepositorySlug::new("acme", "widgets")]);

        assert!(fetch.repositories[0].pull_requests.is_ok());
        assert_eq!(
            fake.stdin(1),
            "",
            "gh must see a closed stdin, never the terminal",
        );
        assert_eq!(fake.environment_variable(1), "1", "prompting is disabled");
    }

    /// A follow-up belongs to one pull request, so its failure belongs to that
    /// pull request's repository and not to everything queried alongside it.
    #[test]
    fn a_failed_thread_follow_up_fails_only_its_own_repository() {
        let fake = FakeGh::new();
        fake.respond(&response(&[
            alias("toReview", &[requested_node("acme/gadgets", 7)], None),
            alias(
                "authored",
                &[authored_node(91, &[thread(true, "reviewer")], Some("T-2"))],
                None,
            ),
        ]))
        .fail("gh: Not Found (HTTP 404)");

        let fetch = GithubClient::new(fake.path()).fetch_home_pull_requests(&[
            RepositorySlug::new("acme", "widgets"),
            RepositorySlug::new("acme", "gadgets"),
        ]);

        let error = fetch.repositories[0].pull_requests.as_ref().unwrap_err();
        assert!(
            matches!(&**error, GithubError::NotFound { .. }),
            "unexpected error: {error}",
        );
        assert_eq!(
            rows(&fetch, "acme/gadgets").len(),
            1,
            "the other repository is untouched",
        );
    }

    /// The document is what the measured cost was measured against, so the parts
    /// that set that cost are pinned here.
    #[test]
    fn the_batch_query_matches_the_measured_plan() {
        let fake = FakeGh::new();
        fake.respond(EMPTY_RESPONSE);

        let fetch = GithubClient::new(fake.path())
            .fetch_home_pull_requests(&[RepositorySlug::new("acme", "widgets")]);

        assert!(fetch.repositories[0].pull_requests.is_ok());
        let arguments = fake.arguments(1);
        assert_eq!(arguments[..2], ["api", "graphql"]);

        let document = &fake.variables(1)["query"];
        for required in [
            "type: ISSUE_ADVANCED, first: 50",
            "toReview: search(",
            "authored: search(",
            "viewer { login }",
            "rateLimit { cost remaining resetAt }",
            "pageInfo { hasNextPage endCursor }",
            "viewerLatestReview { commit { oid } }",
            "statusCheckRollup { state }",
            "latestOpinionatedReviews(first: 50)",
            "reviewThreads(first: 50)",
            "comments(last: 1)",
        ] {
            assert!(
                document.contains(required),
                "the document dropped {required}"
            );
        }
    }

    /// GitHub compares slugs without case and answers in its own, so a clone
    /// configured as `Acme/Widgets` must still collect its own rows.
    #[test]
    fn a_row_reaches_its_repository_whatever_case_it_was_configured_in() {
        let fake = FakeGh::new();
        fake.respond(&response(&[
            alias("toReview", &[requested_node("acme/widgets", 42)], None),
            alias("authored", &[], None),
        ]));

        let fetch = GithubClient::new(fake.path())
            .fetch_home_pull_requests(&[RepositorySlug::new("Acme", "Widgets")]);

        assert_eq!(rows(&fetch, "Acme/Widgets").len(), 1);
    }

    /// Two clones of one repository would otherwise list every row twice.
    #[test]
    fn the_same_repository_asked_for_twice_is_queried_once() {
        let fake = FakeGh::new();
        fake.respond(EMPTY_RESPONSE);

        let fetch = GithubClient::new(fake.path()).fetch_home_pull_requests(&[
            RepositorySlug::new("acme", "widgets"),
            RepositorySlug::new("ACME", "WIDGETS"),
        ]);

        assert_eq!(fetch.repositories.len(), 1);
        assert_eq!(
            fake.variables(1)["authored"],
            "is:pr is:open author:@me (repo:acme/widgets)",
        );
    }

    /// A cursor that never ends must stop the refresh rather than spend the
    /// whole rate limit on it.
    #[test]
    fn paging_stops_at_a_ceiling_rather_than_looping_forever() {
        let fake = FakeGh::new();
        fake.respond(&response(&[
            alias("toReview", &[], Some("PAGE")),
            alias("authored", &[], None),
        ]));
        for _ in 0..MAX_SEARCH_PAGES {
            fake.respond(&response(&[alias("toReview", &[], Some("PAGE"))]));
        }

        let fetch = GithubClient::new(fake.path())
            .fetch_home_pull_requests(&[RepositorySlug::new("acme", "widgets")]);

        assert_eq!(
            fake.calls(),
            MAX_SEARCH_PAGES + 1,
            "the ceiling is honoured"
        );
        let error = fetch.repositories[0].pull_requests.as_ref().unwrap_err();
        assert!(
            matches!(&**error, GithubError::PagingLimit { .. }),
            "unexpected error: {error}",
        );
    }

    /// Home groups rows without asking GitHub again, so every field the grouping
    /// reads has to survive the fetch.
    #[test]
    fn a_row_carries_every_field_home_groups_on() {
        let fake = FakeGh::new();
        fake.respond(SEARCH_RESPONSE);

        let fetch = GithubClient::new(fake.path()).fetch_home_pull_requests(&[
            RepositorySlug::new("acme", "widgets"),
            RepositorySlug::new("acme", "gadgets"),
        ]);

        assert_eq!(fetch.viewer_login.as_deref(), Some("reviewer"));
        assert_eq!(
            fetch.rate_limit.as_ref().unwrap(),
            &RateLimit {
                cost: 6,
                remaining: 4994,
                reset_at: "2026-09-02T13:00:00Z".to_owned(),
            },
        );

        let widgets = rows(&fetch, "acme/widgets");
        assert_eq!(widgets.len(), 2, "one requested review and one authored");

        let requested = &widgets[0];
        assert_eq!(requested.search, HomeSearch::ReviewRequested);
        assert_eq!(requested.number, 42);
        assert_eq!(requested.title, "Improve the review flow");
        assert_eq!(requested.url, "https://github.com/acme/widgets/pull/42");
        assert!(!requested.draft);
        assert_eq!(requested.updated_at, "2026-09-02T09:15:00Z");
        assert_eq!(requested.author_login, "contributor");
        assert_eq!(
            requested.viewer_latest_review_oid.as_deref(),
            Some(requested.head_oid.as_str()),
            "the viewer has already reviewed this head",
        );
        assert_eq!(requested.check_state, Some(StatusCheckState::Success));
        assert!(requested.review_decision.is_none());
        assert!(requested.latest_opinionated_reviews.is_empty());
        assert!(requested.review_threads.is_empty());

        let authored = &widgets[1];
        assert_eq!(authored.search, HomeSearch::Authored);
        assert_eq!(authored.number, 91);
        assert_eq!(
            authored.review_decision,
            Some(ReviewDecision::ChangesRequested),
        );
        assert_eq!(
            authored.latest_opinionated_reviews,
            [
                OpinionatedReview {
                    state: ReviewState::ChangesRequested,
                    author_login: "maintainer".to_owned(),
                },
                OpinionatedReview {
                    state: ReviewState::Approved,
                    author_login: "ghost".to_owned(),
                },
            ],
            "a deleted reviewer falls back to GitHub's own placeholder",
        );
        assert_eq!(
            authored.review_threads,
            [
                ReviewThread {
                    resolved: false,
                    last_comment_author_login: Some("maintainer".to_owned()),
                },
                ReviewThread {
                    resolved: true,
                    last_comment_author_login: None,
                },
            ],
        );
        assert_eq!(authored.check_state, Some(StatusCheckState::Failure));

        let gadgets = rows(&fetch, "acme/gadgets");
        assert_eq!(gadgets.len(), 2);
        assert_eq!(
            gadgets[0].author_login, "ghost",
            "a deleted author falls back to the placeholder",
        );
        assert!(
            gadgets[0].viewer_latest_review_oid.is_none(),
            "the viewer has not reviewed this one",
        );
        assert!(
            gadgets[0].check_state.is_none(),
            "no checks reads differently from a pending one",
        );
        assert!(gadgets[1].draft);
    }

    /// Wraps search aliases in the envelope every call returns.
    fn response(aliases: &[String]) -> String {
        envelope(aliases, &[])
    }

    fn envelope(aliases: &[String], errors: &[String]) -> String {
        format!(
            r#"{{"data": {{
              "viewer": {{"login": "reviewer"}},
              "rateLimit": {{"cost": 6, "remaining": 4994, "resetAt": "2026-09-02T13:00:00Z"}},
              {aliases}
            }}, "errors": [{errors}]}}"#,
            aliases = aliases.join(","),
            errors = errors.join(","),
        )
    }

    fn alias(name: &str, nodes: &[String], next: Option<&str>) -> String {
        format!(
            r#""{name}": {{
              "pageInfo": {{"hasNextPage": {has_next}, "endCursor": {cursor}}},
              "nodes": [{nodes}]
            }}"#,
            has_next = next.is_some(),
            cursor = quoted(next),
            nodes = nodes.join(","),
        )
    }

    fn requested_node(full_name: &str, number: u64) -> String {
        format!(
            r#"{{
              "number": {number},
              "title": "Row {number}",
              "url": "https://github.com/{full_name}/pull/{number}",
              "isDraft": false,
              "updatedAt": "2026-09-02T09:15:00Z",
              "headRefOid": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
              "repository": {{"nameWithOwner": "{full_name}"}},
              "author": {{"login": "contributor"}},
              "viewerLatestReview": null,
              "statusCheckRollup": null
            }}"#
        )
    }

    fn authored_node(number: u64, threads: &[String], next: Option<&str>) -> String {
        format!(
            r#"{{
              "number": {number},
              "title": "Row {number}",
              "url": "https://github.com/acme/widgets/pull/{number}",
              "isDraft": false,
              "updatedAt": "2026-09-02T11:40:00Z",
              "headRefOid": "cccccccccccccccccccccccccccccccccccccccc",
              "repository": {{"nameWithOwner": "acme/widgets"}},
              "author": {{"login": "reviewer"}},
              "viewerLatestReview": null,
              "statusCheckRollup": null,
              "reviewDecision": null,
              "latestOpinionatedReviews": {{"nodes": []}},
              "reviewThreads": {{
                "totalCount": 52,
                "pageInfo": {{"hasNextPage": {has_next}, "endCursor": {cursor}}},
                "nodes": [{nodes}]
              }}
            }}"#,
            has_next = next.is_some(),
            cursor = quoted(next),
            nodes = threads.join(","),
        )
    }

    fn thread(resolved: bool, last_comment_author: &str) -> String {
        format!(
            r#"{{"isResolved": {resolved}, "comments": {{"nodes": [{{"author": {{"login": "{last_comment_author}"}}}}]}}}}"#
        )
    }

    fn threads_response(threads: &[String], next: Option<&str>) -> String {
        format!(
            r#"{{"data": {{
              "rateLimit": {{"cost": 1, "remaining": 4993, "resetAt": "2026-09-02T13:00:00Z"}},
              "repository": {{"pullRequest": {{"reviewThreads": {{
                "pageInfo": {{"hasNextPage": {has_next}, "endCursor": {cursor}}},
                "nodes": [{nodes}]
              }}}}}}
            }}}}"#,
            has_next = next.is_some(),
            cursor = quoted(next),
            nodes = threads.join(","),
        )
    }

    /// The error GitHub pairs with a null node when the token has not been
    /// authorized for an organisation's single sign-on.
    fn saml_error(owner: &str) -> String {
        format!(
            r#"{{
              "type": "FORBIDDEN",
              "path": ["toReview", "nodes", 0],
              "message": "Resource protected by organization SAML enforcement. You must grant your Personal Access token access to this organization. Visit https://github.com/orgs/{owner}/sso?authorization_request=ABC to do so."
            }}"#
        )
    }

    fn quoted(value: Option<&str>) -> String {
        value.map_or_else(|| "null".to_owned(), |value| format!("\"{value}\""))
    }

    fn rows<'a>(fetch: &'a HomeFetch, full_name: &str) -> &'a [HomePullRequest] {
        fetch
            .repositories
            .iter()
            .find(|repository| repository.repository.full_name() == full_name)
            .unwrap_or_else(|| panic!("{full_name} was not fetched"))
            .pull_requests
            .as_ref()
            .unwrap()
    }

    fn slugs(numbers: std::ops::RangeInclusive<u32>) -> Vec<RepositorySlug> {
        numbers
            .map(|number| RepositorySlug::new("acme", format!("repo-{number}")))
            .collect()
    }

    /// A stand-in for `gh` that records every invocation and replays one queued
    /// response per call, so a test asserts on the exact call sequence.
    struct FakeGh {
        directory: TempDir,
        queued: Cell<usize>,
    }

    impl FakeGh {
        fn new() -> Self {
            let directory = TempDir::new().unwrap();
            let script = format!(
                r#"#!/bin/sh
dir="{dir}"
call=$(cat "$dir/count" 2>/dev/null || echo 0)
call=$((call + 1))
printf '%s' "$call" > "$dir/count"
printf '%s<<ARG>>' "$@" > "$dir/args-$call"
printf '%s' "$GH_PROMPT_DISABLED" > "$dir/prompt-$call"
if [ -t 0 ]; then printf 'tty' > "$dir/stdin-$call"; else cat > "$dir/stdin-$call"; fi
if [ -f "$dir/sleep-$call" ]; then sleep "$(cat "$dir/sleep-$call")"; fi
if [ ! -f "$dir/response-$call" ] && [ ! -f "$dir/stderr-$call" ]; then
  echo "no recorded response for call $call" >&2
  exit 64
fi
if [ -f "$dir/response-$call" ]; then cat "$dir/response-$call"; fi
if [ -f "$dir/stderr-$call" ]; then cat "$dir/stderr-$call" >&2; fi
if [ -f "$dir/status-$call" ]; then exit "$(cat "$dir/status-$call")"; fi
exit 0
"#,
                dir = directory.path().display(),
            );
            let path = directory.path().join("gh");
            fs::write(&path, script).unwrap();
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();

            Self {
                directory,
                queued: Cell::new(0),
            }
        }

        fn path(&self) -> PathBuf {
            self.directory.path().join("gh")
        }

        fn respond(&self, body: &str) -> &Self {
            let call = self.queue();
            self.write(&format!("response-{call}"), body);
            self
        }

        /// `gh` exits non-zero whenever the envelope carries errors, so a partial
        /// answer arrives alongside a failed status.
        fn respond_partial(&self, body: &str, stderr: &str) -> &Self {
            let call = self.queue();
            self.write(&format!("response-{call}"), body);
            self.write(&format!("stderr-{call}"), stderr);
            self.write(&format!("status-{call}"), "1");
            self
        }

        fn fail(&self, stderr: &str) -> &Self {
            let call = self.queue();
            self.write(&format!("stderr-{call}"), stderr);
            self.write(&format!("status-{call}"), "1");
            self
        }

        /// Queues a call that never answers, so the timeout is what ends it.
        fn hang(&self, seconds: u32) -> &Self {
            let call = self.queue();
            self.write(&format!("sleep-{call}"), &seconds.to_string());
            self.write(&format!("response-{call}"), EMPTY_RESPONSE);
            self
        }

        fn stdin(&self, call: usize) -> String {
            fs::read_to_string(self.directory.path().join(format!("stdin-{call}"))).unwrap()
        }

        fn environment_variable(&self, call: usize) -> String {
            fs::read_to_string(self.directory.path().join(format!("prompt-{call}"))).unwrap()
        }

        fn calls(&self) -> usize {
            fs::read_to_string(self.directory.path().join("count"))
                .map_or(0, |count| count.trim().parse().unwrap())
        }

        /// The `-f` and `-F` fields of one call, which is where the query
        /// document and every GraphQL variable are passed.
        fn variables(&self, call: usize) -> BTreeMap<String, String> {
            self.arguments(call)
                .windows(2)
                .filter(|pair| pair[0] == "-f" || pair[0] == "-F")
                .filter_map(|pair| pair[1].split_once('='))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect()
        }

        fn arguments(&self, call: usize) -> Vec<String> {
            let recorded = fs::read_to_string(self.directory.path().join(format!("args-{call}")))
                .unwrap_or_else(|_| panic!("call {call} was never made"));
            let count = recorded.matches("<<ARG>>").count();
            recorded
                .split("<<ARG>>")
                .take(count)
                .map(ToOwned::to_owned)
                .collect()
        }

        fn write(&self, name: &str, body: &str) {
            fs::write(self.directory.path().join(name), body).unwrap();
        }

        fn queue(&self) -> usize {
            let call = self.queued.get() + 1;
            self.queued.set(call);
            call
        }
    }
}
