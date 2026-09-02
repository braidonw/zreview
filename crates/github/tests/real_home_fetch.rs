//! Fetches Home's rows from the real GitHub, through the real `gh`.
//!
//! Ignored by default, because it needs an authenticated `gh` and network access,
//! neither of which belongs in `cargo test`. It exists because the unit tests
//! drive a fake `gh` over recorded JSON, which proves the parser handles the
//! shapes we thought of and nothing about what GitHub really returns.
//!
//! Run it with:
//!
//! ```text
//! cargo test -p github --locked -- --ignored real_home_fetch --nocapture
//! ```

use github::{GithubClient, RepositorySlug};

/// The repository this application lives in, so the test needs no fixture.
const REPOSITORY: (&str, &str) = ("braidonw", "zreview");

#[test]
#[ignore = "needs an authenticated gh and network; run with --ignored"]
fn real_home_fetch_returns_rows_a_human_can_check() {
    let repository = RepositorySlug::new(REPOSITORY.0, REPOSITORY.1);
    let fetch = GithubClient::default().fetch_home_pull_requests(std::slice::from_ref(&repository));

    println!("viewer: {:?}", fetch.viewer_login);
    println!("rate limit: {:?}", fetch.rate_limit);

    let rows = match &fetch.repositories[0].pull_requests {
        Ok(rows) => rows,
        Err(error) => panic!(
            "fetching {} failed: {error}\n{}",
            repository.full_name(),
            error.remediation().unwrap_or_default()
        ),
    };

    println!("{} row(s) for {}", rows.len(), repository.full_name());
    for row in rows {
        println!(
            "  [{:?}] #{} {:?} checks={:?} decision={:?} threads={}",
            row.search,
            row.number,
            row.title.chars().take(60).collect::<String>(),
            row.check_state,
            row.review_decision,
            row.review_threads.len(),
        );
    }

    // Nothing is asserted about which pull requests are open: that changes daily.
    // What must hold is that GitHub answered and the repository has an answer.
    assert!(
        fetch.viewer_login.is_some(),
        "the query asks for the viewer login, so an answer names the account",
    );
}
