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
- verifies both namespaced refs still equal the API snapshot before constructing the diff;
- returns a merge-base `ComparisonDiff` plus typed PR metadata.

`SessionSource::GitHubPullRequest` pins repository identity, PR number/title/URL, branch names, and both SHAs. The sidebar displays the repository, PR number, and title.

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
- exact SHA verification and construction of the final local comparison.

No live GitHub credentials or network access are required by the test suite.

## Current limitations

- github.com only; GitHub Enterprise Server is deferred.
- Number-only lookup prefers `origin`; fork ambiguity requires a full PR URL.
- Metadata loading is synchronous before the GPUI window opens. It should move into an async session-loading state before production.
- Existing review comments, checks, reviewers, and review submission are not implemented yet.
- Repository fetches are a trust-sensitive operation and need an explicit repository trust/confirmation UX.

## Next step

Load existing GitHub review comments and model local draft comments with GitHub-valid `path`/`line`/`side` anchors. Keep drafts local; review submission comes after draft persistence and stale-head validation.
