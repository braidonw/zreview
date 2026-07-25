# GitHub PR loading through `gh`

## Goal

Open a GitHub pull request from an existing local clone, pin it to exact base/head SHAs, and reuse the local Git comparison/session pipeline.

## Implementation

`crates/github` provides an authenticated, token-free integration around the installed GitHub CLI:

- accepts a positive PR number or canonical `https://github.com/<owner>/<repo>/pull/<number>` URL;
- discovers github.com HTTPS and SSH remotes, preferring `origin` for number-only lookup;
- invokes `gh api` with prompts disabled and the pinned REST API version;
- validates the returned PR number, base repository, base/head SHAs, and required metadata;
- selects a local remote matching the PR's base repository;
- fetches the base branch and `refs/pull/<number>/head` into `refs/zreview/github/...`;
- uses `--no-tags` and `--no-write-fetch-head`, and never updates user branches;
- verifies the fetched head still equals the API snapshot before constructing the diff;
- returns a merge-base `ComparisonDiff` plus typed PR metadata.

`SessionSource::GitHubPullRequest` pins repository identity, PR number/title/URL, branch names, and the base tip, merge base, and head SHAs. The sidebar displays the repository, PR number, and title.

### Why `base.sha` is not the comparison base

GitHub pins `pull_request.base.sha` when a PR is created or synchronized; it does
not follow the base branch. On any active repository it is therefore behind the
real branch tip — four of the five newest open PRs on `cli/cli` were behind when
this was checked.

An earlier revision fetched `refs/heads/<base_ref>` and then asserted that the
fetched tip equalled `base.sha`, which made loading fail for most real PRs. The
comparison base is now the merge base of the *current* base branch tip and the
head, matching GitHub's "Files changed" view. `base.sha` is retained on
`PullRequestMetadata` as provenance.

Only the head is verified against the API response, because a head that no longer
matches means the PR was pushed to mid-load, which would silently change what is
under review.

## Usage

```bash
# Current local repository
cargo run -p zreview -- pr 123

# Explicit local clone
cargo run -p zreview -- pr /path/to/repository 123
cargo run -p zreview -- pr /path/to/repository \
  https://github.com/acme/widgets/pull/123
```

Number-only lookup uses the preferred GitHub remote. A full URL should be used for an ambiguous fork setup.

## Tests

Tests cover:

- PR URL and GitHub remote URL parsing;
- metadata normalization using a fake `gh` executable;
- remote enumeration and namespaced ref fetching;
- an end-to-end local simulation in which a canonical GitHub URL is rewritten to a temporary repository containing `refs/pull/42/head`;
- a PR whose recorded `base.sha` has been left behind by an advancing base branch, asserting that loading succeeds and that the base branch's own commit is not part of the review;
- rejection of a head that moved between the API read and the fetch;
- head SHA verification and construction of the final local comparison.

No live GitHub credentials or network access are required by the test suite.

## Current limitations

- github.com only; GitHub Enterprise Server is deferred.
- Number-only lookup prefers `origin`; fork ambiguity requires a full PR URL.
- Metadata loading is synchronous before the GPUI window opens. It should move into an async session-loading state before production.
- Checks, reviewers, and review submission are not implemented yet.
- Repository fetches are a trust-sensitive operation and need an explicit repository trust/confirmation UX.

## Next step

Existing review comments are covered by [review-comments.md](review-comments.md), which also carries the anchor model local drafts will build on.
