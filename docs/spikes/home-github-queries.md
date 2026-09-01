# GitHub queries for the Home screen

## Goal

Establish, against GitHub's own documentation and the live API, how to fetch the three Home groups (to review, to address, waiting on others) and their row statuses for N configured repositories, and what that costs per refresh.

Everything below was checked on 2026-09-01 with `gh` 2.98.0 against the authenticated account. Introspection and read-only queries were run through `gh api graphql`; nothing was written to GitHub. The group definitions are taken as settled and are not revisited here.

## 1. Team review requests in search

`review-requested:@me` already covers teams. The search docs say of `review-requested:USERNAME` that "if the requested person is on a team that is requested for review, then review requests for that team will also appear in the search results", and that "requested reviewers are no longer listed in the search results after they review a pull request" ([searching issues and pull requests](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests#search-by-pull-request-review-status-and-reviewer)). `user-review-requested:@me` is the narrower form and "matches pull requests that you have directly been asked to review", and `team-review-requested:ORG/TEAM` matches a named team (same page). So one qualifier gives "directly or via a team" and no team enumeration is needed for the list itself.

`gh search prs --review-requested` maps the value to `team-review-requested` when it contains a slash and to `review-requested` otherwise ([prs.go](https://raw.githubusercontent.com/cli/cli/trunk/pkg/cmd/search/prs/prs.go)), which is the same pair of qualifiers.

If the app ever needs the user's teams anyway (for a "requested via team X" label, say), `GET /user/teams` lists "all teams across all of the organizations to which the authenticated user belongs" and needs the `user`, `repo` or `read:org` scope ([list teams for the authenticated user](https://docs.github.com/en/rest/teams/teams?apiVersion=2022-11-28#list-teams-for-the-authenticated-user)). The current `gh` token carries `read:org`, and `gh api user/teams --paginate` ran cleanly (this account belongs to no teams, so it returned an empty array). The GraphQL equivalent is `viewer { organizations { nodes { teams(userLogins: [login]) { nodes { slug } } } } }`, where `userLogins` is documented as "User logins to filter by" on `Organization.teams`.

Two caveats from the team side. When a team has code review assignment enabled, "the team is removed as a reviewer and a specified subset of team members are assigned in the team's place" ([managing code review settings for your team](https://docs.github.com/en/organizations/organizing-members-into-teams/managing-code-review-settings-for-your-team)), so those requests arrive as direct requests. And once a requested reviewer submits a review "they are no longer considered a requested reviewer" ([review requests REST reference](https://docs.github.com/en/rest/pulls/review-requests?apiVersion=2022-11-28)), which is why a PR the user has already reviewed drops out of `review-requested:@me` unless a review is requested again. "You already reviewed this head" therefore only shows up for PRs still in the search, in practice ones re-requested or requested through a team that has not been satisfied.

## 2. Unresolved threads whose latest comment is not mine

`PullRequest.reviewThreads` is "the list of all review threads for this pull request". Each `PullRequestReviewThread` has `isResolved` ("whether this thread has been resolved"), `isOutdated`, and `comments`, "a list of pull request comments associated with the thread" ([GraphQL pulls reference](https://docs.github.com/en/graphql/reference/pulls)). Introspection confirms `comments` takes `first`, `last`, `after`, `before` and `skip`, so `comments(last: 1)` returns only the newest comment of each thread, and each comment carries `author { login }`.

```graphql
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        totalCount
        pageInfo { hasNextPage endCursor }
        nodes {
          isResolved
          isOutdated
          comments(last: 1) {
            nodes { author { login } createdAt }
          }
        }
      }
    }
  }
}
```

Evaluation is `!isResolved && comments.nodes[0].author.login != viewer.login`. Verified on `cli/cli#14056`, where both threads came back `isResolved: true` with their last comment's author, and on a PR the user reviewed, where two unresolved threads correctly reported `braidonw` as the latest commenter. `viewer { login }` can be fetched in the same query rather than shelling out to `gh api user`. Bot authors such as `copilot-pull-request-reviewer` appear as ordinary logins, so nothing special is needed for them.

The same `reviewThreads` selection nests inside a `search` result (section 8), which is how it should be fetched for the list rather than one query per PR.

## 3. A changes-requested review stands

Dismissing a review "changes the status of the review to a review comment" ([dismissing a pull request review](https://docs.github.com/en/pull-requests/how-tos/review-pull-requests/dismissing-a-pull-request-review)), and `PullRequestReviewState` has `DISMISSED`, "a review that has been dismissed", alongside `PENDING`, `COMMENTED`, `APPROVED` and `CHANGES_REQUESTED`.

`PullRequest.latestOpinionatedReviews` is "a list of latest reviews per user associated with the pull request" and does the per-reviewer collapse server side. `latestReviews` is the same "that are not also pending review" but keeps non-opinionated states ([GraphQL pulls reference](https://docs.github.com/en/graphql/reference/pulls)). The docs do not say what happens to a reviewer whose latest review was dismissed, so this was checked live on `microsoft/vscode#333641`, `#333646`, `#333687` and `#333727`. In every case the reviewer whose newest review was `DISMISSED` appears in `latestReviews` with state `DISMISSED` and is absent from `latestOpinionatedReviews`. So the check is simply

```graphql
latestOpinionatedReviews(first: 100) { nodes { state author { login } } }
```

and "a changes-requested review stands" is `any(state == CHANGES_REQUESTED)`. Nothing has to be sorted or de-duplicated on the client, and dismissed reviews are already excluded.

`reviewDecision` ("the current status of this pull request with respect to code review", values `CHANGES_REQUESTED`, `APPROVED`, `REVIEW_REQUIRED`) is cheaper but folds in branch protection. It came back `null` on an open PR with no reviews (`ex-aws/ex_aws#1253`), so it cannot be the only signal. It is worth fetching anyway as the "approval" status for the waiting-on-others group, since `APPROVED` there means what GitHub itself shows.

## 4. You already reviewed this head

`PullRequest.viewerLatestReview` is "the latest review given from the viewer", `PullRequestReview.commit` "identifies the commit associated with this pull request review", and `PullRequest.headRefOid` "identifies the oid of the head ref associated with the pull request, even if the ref has been deleted" ([GraphQL pulls reference](https://docs.github.com/en/graphql/reference/pulls)). REST describes the review's `commit_id` as "the SHA of the commit that needs a review" ([pull request reviews REST reference](https://docs.github.com/en/rest/pulls/reviews?apiVersion=2022-11-28)), which is the same value.

```graphql
headRefOid
viewerLatestReview { state submittedAt commit { oid } }
```

"Already reviewed this head" is `viewerLatestReview != null && viewerLatestReview.commit.oid == headRefOid`. Verified on `Secus-Digital/rdti#164`, where the viewer's `COMMENTED` review carried the same oid as `headRefOid`. `commit` is nullable in the schema, so treat a null commit as "reviewed an unknown head" rather than crashing. `viewerLatestReview` includes `COMMENTED` reviews; if only approvals or change requests should count, filter on `state`.

## 5. Check status

`PullRequest.statusCheckRollup` is "check and status rollup information for the PR's head ref". `StatusCheckRollup` "represents the rollup for both the check runs and status for a commit", its `state` is "the combined status for the commit" of type `StatusState` (`EXPECTED`, `ERROR`, `FAILURE`, `PENDING`, `SUCCESS`), and `contexts` is a connection over the `StatusCheckRollupContext` union of `CheckRun` and `StatusContext` ([GraphQL commits reference](https://docs.github.com/en/graphql/reference/commits)). `CheckRun` exposes `name`, `status` (`CheckStatusState`), `conclusion` (`CheckConclusionState`) and `isRequired`; `StatusContext` exposes `context`, `state` and `isRequired` (introspected). It sits directly on the PR node, so it comes back in the same search query as the list. It is `null` on a PR with no checks at all (`ex-aws/ex_aws#1253`), so the row should render "no checks" for null rather than treating it as pending.

The REST alternative needs two calls per PR. `GET /repos/{owner}/{repo}/commits/{ref}/status` computes `failure` "if any of the contexts report as error or failure", `pending` "if there are no statuses or a context is pending", `success` "if the latest status for all contexts is success", and covers commit statuses only ([combined status](https://docs.github.com/en/rest/commits/statuses?apiVersion=2022-11-28#get-the-combined-status-for-a-specific-reference)). Check runs come from `GET /repos/{owner}/{repo}/commits/{ref}/check-runs` ([list check runs for a Git reference](https://docs.github.com/en/rest/checks/runs?apiVersion=2022-11-28#list-check-runs-for-a-git-reference)). That is 2M extra REST calls per refresh against 5,000 per hour, so GraphQL wins outright here.

## 6. Draft flag and updatedAt

`isDraft` ("identifies if the pull request is a draft") and `updatedAt` ("identifies the date and time when the object was last updated") are plain fields on `PullRequest` and were returned inside `search` nodes in every test. `draft:false` in the search string excludes drafts server side for the to-review group ([searching issues and pull requests](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests#search-by-draft-pull-requests)). `gh search prs --json` also offers `isDraft` and `updatedAt` ([result.go](https://raw.githubusercontent.com/cli/cli/trunk/pkg/search/result.go)), but nothing else this screen needs (no head oid, reviews, threads or checks), which rules it out as the list source.

## 7. Non-interactive use and pagination

`gh api` accepts `graphql` as the endpoint ("graphql to access the GitHub API v4") and takes the query through `-f query=...` or `-F query=@file`, with variables as further `-f`/`-F` fields ([gh api manual](https://cli.github.com/manual/gh_api)). Both `gh api graphql` and `gh search prs` ran with stdin redirected from `/dev/null`, with and without `GH_PROMPT_DISABLED=1`, using the keyring credentials from `gh auth login`. `GH_TOKEN` overrides those when set and `GH_PROMPT_DISABLED` disables "interactive prompting in the terminal" ([gh environment](https://cli.github.com/manual/gh_help_environment)). Neither command prompted. The existing `--method GET` plumbing in `crates/github/src/lib.rs` needs a POST variant for GraphQL, which `gh api graphql` does by default.

Pagination differs sharply between the two.

`gh api graphql --paginate` "requires that the original query accepts an `$endCursor: String` variable and that it fetches the `pageInfo{ hasNextPage, endCursor }` set of fields from a collection" and emits one JSON document per page unless `--slurp` wraps them in an array ([gh api manual](https://cli.github.com/manual/gh_api)). The implementation stops at the first `pageInfo` it meets in the response body ([pagination.go](https://raw.githubusercontent.com/cli/cli/trunk/pkg/cmd/api/pagination.go)), so it can drive exactly one connection. With two aliased searches in one query it would feed the first search's cursor to both. Cursor handling for the Home query therefore belongs in the app, which is a small loop over `pageInfo.hasNextPage` per alias.

`gh search prs` calls REST `GET /search/issues` with `advanced_search=true`, pages at 100 per request and keeps requesting until `--limit` is satisfied ([searcher.go](https://raw.githubusercontent.com/cli/cli/trunk/pkg/search/searcher.go)); `--limit` is rejected outside 1 to 1000 ([shared.go](https://raw.githubusercontent.com/cli/cli/trunk/pkg/cmd/search/shared/shared.go)), matching the REST cap of "up to 1,000 results for each search" ([REST search](https://docs.github.com/en/rest/search/search?apiVersion=2022-11-28)). Those requests draw on the search limit of 30 per minute for authenticated users (same page), confirmed by `x-ratelimit-resource: search` and `x-ratelimit-limit: 30` on a direct `gh api search/issues` call.

GraphQL `search` is the same index behind a different budget. The field "perform[s] a search across resources, returning a maximum of 1,000 results", takes `first`/`last` between 1 and 100, and a `gh api -i graphql` call carrying two searches reported `x-ratelimit-resource: graphql` with `x-ratelimit-limit: 5000`. Search results are not charged against the 30 per minute REST search window when fetched this way.

Two search behaviours matter for a multi-repository query. Under `type: ISSUE` a space between `repo:` qualifiers is an OR, and under `type: ISSUE_ADVANCED` it is an AND, so the advanced form needs `(repo:a OR repo:b)` written out. GitHub's changelog states this directly ("a space between multiple repo, org, and user filter qualifiers is treated as an AND operator ... without advanced search, a space is treated as an OR operator", [March 2025 changelog](https://github.blog/changelog/2025-03-06-github-issues-projects-api-support-for-issues-advanced-search-and-more/)), and it reproduced live. `repo:cli/cli repo:BurntSushi/ripgrep` returned 121 PRs from both repositories under `ISSUE` and zero under `ISSUE_ADVANCED`; the parenthesised OR form returned the same 121 under `ISSUE_ADVANCED`. The advanced syntax is documented as supporting `AND`, `OR` and parentheses nested "up to five levels deep" ([filtering and searching issues and pull requests](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/filtering-and-searching-issues-and-pull-requests)). Separately, the search troubleshooting page says "queries longer than 256 characters are not supported" and allows at most five `AND`, `OR` or `NOT` operators ([troubleshooting search queries](https://docs.github.com/en/search-github/getting-started-with-searching-on-github/troubleshooting-search-queries)). Neither limit was enforced by the API in testing. A 352 character `ISSUE` query naming 13 repositories and a 273 character `ISSUE_ADVANCED` query with eight `OR`s both returned the full 5,663 matches, as did the REST equivalents. The plan below batches repositories anyway so the app stays inside the documented limits.

Search results can also contain nulls. A `reviewed-by:@me` search returned `null` nodes with a `FORBIDDEN` error carrying `saml_failure: true` for PRs in an organisation whose SSO has not been authorised for the token; `gh` printed the data and exited 1. The list parser has to tolerate null nodes and surface the error text rather than failing the whole refresh.

## 8. Call count and rate-limit budget

GraphQL charges "5,000 points per hour per user", where the cost is the number of requests needed to satisfy every connection at its `first`/`last` value, divided by 100 and rounded, with a minimum of 1, and a query may touch at most 500,000 nodes ([GraphQL rate limits](https://docs.github.com/en/graphql/overview/rate-limits-and-node-limits-for-the-graphql-api)). `rateLimit(dryRun: true) { cost nodeCount }` reports the price without running the query (introspected description "If true, calculate the cost for the query without evaluating it"), which is how the numbers below were measured.

One query with two aliased searches (to review, authored) and everything nested inline costs

| PRs per search | latestOpinionatedReviews | reviewThreads | contexts | points | nodes |
| --- | --- | --- | --- | --- | --- |
| 50 | 50 | 50 | 50 | 27 | 12,600 |
| 100 | 100 | 100 | 100 | 104 | 50,200 |
| 50 | 50 | 20 | 50 | 12 | 9,600 |
| 100 | 50 | none | 50 | 3 | 15,200 |
| 100 | 100 | none | 100 | 3 | 30,200 |

`reviewThreads` dominates because `comments(last: 1)` is a connection per thread, so the charge is PRs x threads regardless of how many threads exist. Fetching threads separately for the M authored PRs, 20 per query via aliased `repository { pullRequest(number:) }` with `reviewThreads(first: 100)`, costs 20 points per query, so about 1 point per authored PR.

That gives two workable shapes. Everything inline at 50/50/50/50 is one call and 27 points per refresh, or about 185 refreshes an hour with nothing else running. The split shape is 3 points for the list plus roughly M points for threads, with 1 + ceil(M/20) calls. For the M values a single person actually has open (under 20), the split costs 4 to 23 points and two calls. Either is far inside the budget at a sensible refresh interval, and both leave the REST budget untouched apart from `gh auth status`.

Repositories do not multiply the call count. Both the `ISSUE` and `ISSUE_ADVANCED` forms accept several repositories in one string (section 7), so N is bounded by the documented 256 character limit rather than by calls. At roughly 25 characters per `repo:owner/name` plus ` OR `, about eight repositories fit per query under the advanced syntax; with the legacy `ISSUE` type and space-separated `repo:` terms it is nearer ten. Beyond that, batch repositories into further queries, each paying the same points.

The remaining ceiling is the 1,000 result cap per search and the 100 per page, which only matters if a single search across the configured repositories returns more than 100 open PRs. Then `pageInfo.endCursor` on that alias drives a follow-up query at the same per-page cost.

## Recommended query plan

One `gh api graphql` call per refresh, per batch of at most eight repositories, using `type: ISSUE_ADVANCED` and an explicit `(repo:a OR repo:b ...)` clause so the semantics do not depend on which search index GitHub defaults to. The two searches are aliased in the same document with `first: 50`, and the app pages any alias whose `pageInfo.hasNextPage` is true. `viewer { login }` and `rateLimit { cost remaining resetAt }` ride along for the client-side comparisons and for logging.

```graphql
query($toReview: String!, $authored: String!) {
  viewer { login }
  rateLimit { cost remaining resetAt }
  toReview: search(query: $toReview, type: ISSUE_ADVANCED, first: 50) {
    issueCount
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest { ...Row } }
  }
  authored: search(query: $authored, type: ISSUE_ADVANCED, first: 50) {
    issueCount
    pageInfo { hasNextPage endCursor }
    nodes {
      ... on PullRequest {
        ...Row
        reviewDecision
        latestOpinionatedReviews(first: 50) { nodes { state author { login } } }
        reviewThreads(first: 50) {
          totalCount
          nodes {
            isResolved
            comments(last: 1) { nodes { author { login } } }
          }
        }
      }
    }
  }
}

fragment Row on PullRequest {
  number title url isDraft updatedAt headRefOid
  repository { nameWithOwner }
  author { login }
  viewerLatestReview { state submittedAt commit { oid } }
  statusCheckRollup {
    state
    contexts(first: 50) {
      totalCount
      nodes {
        __typename
        ... on CheckRun { name status conclusion isRequired }
        ... on StatusContext { context state isRequired }
      }
    }
  }
}
```

with `$toReview` = `is:pr is:open draft:false review-requested:@me (repo:... OR repo:...)` and `$authored` = `is:pr is:open author:@me (repo:... OR repo:...)`. Measured cost is 27 points and 12,600 nodes, well under the 5,000 point hour and 500,000 node ceiling.

Grouping is then local. Authored PRs go to "to address" when any `latestOpinionatedReviews` node is `CHANGES_REQUESTED` or any thread is unresolved with a last comment not by `viewer.login`, and otherwise to "waiting on others" with `statusCheckRollup.state` (null meaning no checks) and `reviewDecision` as row statuses. To-review rows show "already reviewed this head" when `viewerLatestReview.commit.oid` equals `headRefOid`. A PR whose `reviewThreads.totalCount` exceeds 50 is the one case that needs a follow-up `repository { pullRequest(number:) { reviewThreads(after:) } }` query, at about 1 point per PR.

If refresh frequency ever makes 27 points feel expensive, drop `reviewThreads` from the list query (3 points) and fetch threads for authored PRs in one aliased query of 20 PRs each. The rest of the plan is unchanged.
