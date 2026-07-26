//! Runs a review against the coding agent actually installed on this machine.
//!
//! Ignored by default, because it launches a real model, costs real money, and
//! needs a signed-in CLI — none of which belong in `cargo test`. It exists because
//! the unit tests drive stub executables, which prove the parser handles the shapes
//! we thought of and nothing about what a model really returns. The first run of
//! this test found a live bug: a model cites guidance as `"AGENTS.md: the rule it
//! applied"` rather than as a bare path, and exact-path matching dropped the
//! citation silently.
//!
//! Run it with:
//!
//! ```text
//! env -u ANTHROPIC_API_KEY cargo test -p review --test real_agent -- --ignored --nocapture
//! ```
//!
//! `env -u ANTHROPIC_API_KEY` is not incidental. That variable overrides a
//! signed-in subscription, so leaving it set sends the review to whatever account
//! the key belongs to.

use std::sync::Arc;

use domain::{
    ChangeCounts, DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStatus, FindingOrigin, Findings,
    GuidanceExcerpt, IgnoreProgress, ReviewBackend, ReviewProgress, ReviewRequest,
};
use review::{Agent, CodingAgent};

const HEAD: &str = "0f7a1c2d3e4f5a6b";
const BASE: &str = "9e8d7c6b5a493827";

/// A file with a bug a reviewer should find, and a guidance file that names it.
///
/// Deliberately obvious. This test asks whether the plumbing works, not whether the
/// model is clever, and a subtle bug would make a failure ambiguous.
fn changed_file() -> DiffFile {
    let lines = vec![
        DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(10),
            new_line: Some(10),
            text: Arc::from("impl Queue {"),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(11),
            text: Arc::from("    pub fn first_two(&self) -> (u32, u32) {"),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(12),
            text: Arc::from("        (self.items[0], self.items[1])"),
        },
        DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(13),
            text: Arc::from("    }"),
        },
        DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(11),
            new_line: Some(14),
            text: Arc::from("}"),
        },
    ];
    DiffFile {
        path: Arc::from("src/queue.rs"),
        old_path: None,
        status: FileStatus::Modified,
        is_binary: false,
        hunks: Arc::from(vec![DiffHunk {
            header: Arc::from("@@ -10,2 +10,5 @@"),
            old_start: 10,
            new_start: 10,
            line_range: 0..lines.len(),
        }]),
        counts: ChangeCounts::of(&lines),
        lines: Arc::from(lines),
    }
}

fn request() -> ReviewRequest {
    ReviewRequest {
        head_sha: Arc::from(HEAD),
        base_sha: Arc::from(BASE),
        title: Some("Add first_two to Queue".to_owned()),
        description: None,
        guidance: vec![GuidanceExcerpt {
            path: Arc::from("AGENTS.md"),
            scope: Arc::from("whole repository"),
            content: "Never index a slice without checking its length first.".to_owned(),
            content_hash: Arc::from(
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
        }],
        files: Arc::from(vec![changed_file()]),
    }
}

/// Whether an API key in the environment would override the signed-in
/// subscription, sending the review to an account the runner did not choose.
fn subscription_is_shadowed() -> bool {
    std::env::var_os("ANTHROPIC_API_KEY").is_some_and(|value| !value.is_empty())
}

/// Reports progress to stdout, so `--nocapture` shows the run happening.
struct Narrate;

impl domain::ReviewEventSink for Narrate {
    fn progress(&self, progress: ReviewProgress) {
        println!("  {progress}");
    }

    fn is_cancelled(&self) -> bool {
        false
    }
}

#[test]
#[ignore = "launches a real model; run with --ignored"]
fn claude_code_finds_a_real_bug_and_anchors_it_to_a_line_in_the_diff() {
    assert!(
        !subscription_is_shadowed(),
        "ANTHROPIC_API_KEY is set, which overrides your signed-in subscription. \
         Re-run with: env -u ANTHROPIC_API_KEY cargo test -p review --test real_agent \
         -- --ignored --nocapture"
    );

    let checkout = tempfile::tempdir().expect("a temporary directory");
    let backend = CodingAgent::new(Agent::ClaudeCode, checkout.path());
    let request = request();

    println!("Running {} …", backend.name());
    let raw = match backend.review(&request, &Narrate) {
        Ok(raw) => raw,
        Err(error) => panic!(
            "the review failed: {error}\n{}",
            error.remediation().unwrap_or_default()
        ),
    };
    println!("{} claim(s) returned", raw.len());

    // Validate exactly as the application does, so this exercises the whole path
    // rather than just the parser.
    let anchors = domain::AnchorIndex::new(&request.files, Arc::from(HEAD));
    let findings = Findings::validate(raw, &anchors, &FindingOrigin::Ai(backend.name()));

    for rejected in findings.rejected() {
        println!("  rejected: {}", rejected.reason);
    }
    for finding in findings.accepted() {
        println!(
            "  [{}] {} ({:?}) — cites {:?}",
            finding.severity,
            finding.title,
            finding.anchor.as_ref().map(|anchor| anchor.line),
            finding
                .guidance_sources
                .iter()
                .map(|source| &*source.path)
                .collect::<Vec<_>>()
        );
    }

    assert!(
        !findings.accepted().is_empty(),
        "a model given an unchecked index and a rule forbidding it should find something"
    );
    assert!(
        findings.rejected().is_empty(),
        "every claim should have survived validation: {:?}",
        findings.rejected()
    );

    let inline = findings
        .accepted()
        .iter()
        .find(|finding| finding.is_inline())
        .expect("at least one finding should anchor to a line");
    let anchor = inline
        .anchor
        .as_ref()
        .expect("an inline finding has an anchor");
    assert_eq!(&*anchor.path, "src/queue.rs");
    // The indexing is on line 12, which is the only line it could sensibly be on.
    assert_eq!(anchor.line, 12, "the finding should anchor to the indexing");

    // The guidance forbids exactly this, so a finding that does not cite it means
    // citations are being lost between the model and us.
    assert!(
        findings.accepted().iter().any(|finding| finding
            .guidance_sources
            .iter()
            .any(|source| &*source.path == "AGENTS.md")),
        "the finding should cite the guidance that forbids it"
    );
}

#[test]
#[ignore = "launches a real model; run with --ignored"]
fn a_clean_change_produces_no_findings() {
    assert!(
        !subscription_is_shadowed(),
        "ANTHROPIC_API_KEY is set; re-run with env -u ANTHROPIC_API_KEY"
    );

    let checkout = tempfile::tempdir().expect("a temporary directory");
    let backend = CodingAgent::new(Agent::ClaudeCode, checkout.path());

    let lines = vec![DiffLine {
        kind: DiffLineKind::Addition,
        old_line: None,
        new_line: Some(1),
        text: Arc::from("//! Types describing a review."),
    }];
    let file = DiffFile {
        path: Arc::from("src/lib.rs"),
        old_path: None,
        status: FileStatus::Modified,
        is_binary: false,
        hunks: Arc::from(vec![DiffHunk {
            header: Arc::from("@@ -0,0 +1 @@"),
            old_start: 0,
            new_start: 1,
            line_range: 0..1,
        }]),
        counts: ChangeCounts::of(&lines),
        lines: Arc::from(lines),
    };

    let mut request = request();
    request.title = Some("Add a module doc comment".to_owned());
    request.guidance = Vec::new();
    request.files = Arc::from(vec![file]);

    let raw = backend
        .review(&request, &IgnoreProgress)
        .expect("the review runs");

    // Not an assertion about the model's taste — an assertion that "nothing found"
    // travels back as an empty list rather than as a failure or as invented noise.
    println!("{} claim(s) on a one-line comment change", raw.len());
    for finding in &raw {
        println!("  {} ({})", finding.title, finding.severity);
    }
}
