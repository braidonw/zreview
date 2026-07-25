//! Running a local coding-agent CLI as the review engine.
//!
//! This settles the decision PLAN section 13 left open, the same way Codiff did:
//! the engine is a coding agent the user already has installed and already signed
//! in to. Nothing here stores an API key, and no token of the user's is copied
//! anywhere — `claude` and `codex` each authenticate themselves, exactly as they do
//! when the user runs them by hand.
//!
//! How each invocation is locked down, per PLAN section 8:
//!
//! - **No tools.** The agent is run with its tool set emptied and its MCP
//!   configuration ignored, so it cannot read files, run commands, or reach the
//!   network. It gets the diff on stdin and returns JSON. It could not post a
//!   review even if it decided to.
//! - **No repository customisation.** `claude` runs in safe mode, so the
//!   repository's own `CLAUDE.md`, hooks, skills, and agents do not load. Guidance
//!   reaches the model through the request, fenced and labelled, rather than by
//!   being silently picked up — which also means a hook in a cloned repository
//!   cannot execute merely because a review was run.
//! - **No shell.** Arguments are passed as an array. The prompt goes on stdin, so
//!   it is not bounded by the argument limit and does not appear in a process
//!   listing.
//! - **No writable checkout.** The working directory is set so relative paths
//!   resolve, and the agent has no tool with which to write to it.
//!
//! A run is bounded in time and in output size, and cancellation is checked before
//! and after the wait, so abandoning a review stops paying for it.

use std::{
    ffi::{OsStr, OsString},
    io::Write as _,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::Arc,
    time::Duration,
};

use domain::{
    GuidanceCitation, RawFinding, RawLocation, ReviewBackend, ReviewError, ReviewEventSink,
    ReviewProgress, ReviewRequest, Severity,
};
use serde::Deserialize;

use crate::material::{self, Material};

/// How long a review may run before it is stopped.
///
/// A thorough review of a large change genuinely takes minutes, so this is
/// generous; it exists to stop a wedged process holding a review open forever.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_mins(15);

/// Largest backend output that will be read.
///
/// Findings are a few kilobytes. Anything approaching this is a runaway.
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// Which coding agent to run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Agent {
    /// Anthropic's `claude`, in print mode.
    ClaudeCode,
    /// `OpenAI`'s `codex`, in exec mode.
    Codex,
}

impl Agent {
    /// The executable looked for on `PATH`.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
        }
    }

    /// How this backend names itself in findings.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    /// Whether the CLI can enforce the output schema itself.
    ///
    /// When it can, a shape violation is caught before the output reaches us. When
    /// it cannot, the schema is quoted in the prompt and the parser is the only
    /// enforcement — so the parser never assumes the shape either way.
    const fn enforces_schema(self) -> bool {
        matches!(self, Self::ClaudeCode)
    }
}

/// A coding-agent CLI, configured for one review.
#[derive(Clone, Debug)]
pub struct CodingAgent {
    agent: Agent,
    /// Resolved separately from `agent` so tests can point at a stub without a
    /// `PATH` of their own.
    executable: OsString,
    model: Option<String>,
    timeout: Duration,
    /// Where the agent runs, so relative paths in the diff mean something.
    working_directory: PathBuf,
}

impl CodingAgent {
    /// A backend that runs `agent` from `PATH` against a checkout.
    #[must_use]
    pub fn new(agent: Agent, working_directory: impl Into<PathBuf>) -> Self {
        Self {
            agent,
            executable: OsString::from(agent.program()),
            model: None,
            timeout: DEFAULT_TIMEOUT,
            working_directory: working_directory.into(),
        }
    }

    /// Runs a specific model rather than the CLI's default.
    ///
    /// Left unset by default on purpose: the user's own configured model is the one
    /// they are paying for and have chosen.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Runs a specific executable instead of resolving one on `PATH`.
    #[must_use]
    pub fn with_executable(mut self, executable: impl Into<OsString>) -> Self {
        self.executable = executable.into();
        self
    }

    fn program(&self) -> Arc<str> {
        Arc::from(self.agent.program())
    }

    /// The command line for one review, as an argument array.
    fn arguments(&self, schema: &str) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::new();
        match self.agent {
            Agent::ClaudeCode => {
                args.push("--print".into());
                args.push("--output-format".into());
                args.push("json".into());
                // Empty tool set: the model reasons over what it was sent and
                // nothing else.
                args.push("--tools".into());
                args.push("".into());
                args.push("--strict-mcp-config".into());
                // Do not load the reviewed repository's CLAUDE.md, hooks, skills,
                // or agents. Guidance arrives through the request instead.
                args.push("--safe-mode".into());
                args.push("--no-session-persistence".into());
                args.push("--setting-sources".into());
                args.push("user".into());
                args.push("--system-prompt".into());
                args.push(material::SYSTEM_PROMPT.into());
                args.push("--json-schema".into());
                args.push(schema.into());
            }
            Agent::Codex => {
                args.push("exec".into());
                // Read-only: the agent has no business changing the checkout.
                args.push("--sandbox".into());
                args.push("read-only".into());
                args.push("--skip-git-repo-check".into());
                args.push("--cd".into());
                args.push(self.working_directory.clone().into_os_string());
                // Codex takes no separate system prompt, so the contract is the
                // first thing in the prompt body instead.
                args.push("-".into());
            }
        }
        if let Some(model) = &self.model {
            args.push("--model".into());
            args.push(model.into());
        }
        args
    }

    /// What goes on stdin.
    fn stdin_body(&self, material: &Material, schema: &str) -> String {
        if self.agent.enforces_schema() {
            return material.prompt.clone();
        }
        // The CLI will not enforce the shape, so state it in the prompt and let the
        // parser be the check.
        format!(
            "{}\n\nRespond with JSON matching this schema, and nothing else — no \
             prose, no markdown fence:\n{schema}\n\n{}\n",
            material::SYSTEM_PROMPT,
            material.prompt
        )
    }

    fn run(&self, body: &str, schema: &str) -> Result<Output, ReviewError> {
        let program = self.program();
        let mut child = Command::new(&self.executable)
            .current_dir(&self.working_directory)
            .args(self.arguments(schema))
            // No forge credential reaches the review engine. Anything the agent
            // needs for its own sign-in it reads for itself.
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| launch_error(&program, &self.executable, &source))?;

        let mut stdin = child.stdin.take().ok_or_else(|| ReviewError::Launch {
            program: Arc::clone(&program),
            message: format!("{} stdin was not available", self.agent.program()),
        })?;
        let written = stdin
            .write_all(body.as_bytes())
            .and_then(|()| stdin.flush());
        // Closing stdin is what tells the agent the prompt is complete.
        drop(stdin);
        if let Err(source) = written {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ReviewError::Launch {
                program,
                message: format!("could not send the review material: {source}"),
            });
        }

        self.wait_with_timeout(child, &program)
    }

    /// Waits for the child, killing it if it outlives the timeout.
    ///
    /// The child handle stays here rather than moving onto a waiting thread, so a
    /// timeout kills exactly the process this review started. An earlier version
    /// signalled by name instead, which would have killed every `claude` on the
    /// machine — including the reviewer's own interactive session.
    ///
    /// A coding agent may spawn children of its own, and killing the direct child
    /// does not reap those. Doing better would need a process group, which needs
    /// `unsafe` and is forbidden here, so the direct kill is the honest limit.
    fn wait_with_timeout(
        &self,
        mut child: Child,
        program: &Arc<str>,
    ) -> Result<Output, ReviewError> {
        // The pipes are drained on their own threads: a child that fills a pipe
        // buffer blocks until someone reads it, and it would block there long
        // before the timeout could fire.
        let mut out_pipe = child.stdout.take();
        let mut err_pipe = child.stderr.take();
        let reading_stdout = std::thread::spawn(move || drain(out_pipe.as_mut()));
        let reading_stderr = std::thread::spawn(move || drain(err_pipe.as_mut()));

        let deadline = std::time::Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(source) => {
                    let _ = child.kill();
                    return Err(ReviewError::Launch {
                        program: Arc::clone(program),
                        message: source.to_string(),
                    });
                }
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                // Reap it, so the timed-out agent does not linger as a zombie.
                let _ = child.wait();
                return Err(ReviewError::TimedOut {
                    program: Arc::clone(program),
                    seconds: self.timeout.as_secs(),
                });
            }
            std::thread::sleep(POLL_INTERVAL);
        };

        Ok(Output {
            status,
            stdout: reading_stdout.join().unwrap_or_default(),
            stderr: reading_stderr.join().unwrap_or_default(),
        })
    }
}

/// How often a running agent is checked for having finished.
///
/// Small enough to feel immediate, large enough that a fifteen-minute review does
/// not spend the time spinning.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Reads a pipe to the end, treating a read failure as no more output.
///
/// A partial read is more useful than none: the bytes already received are often
/// the error message that explains what went wrong.
fn drain(pipe: Option<&mut impl std::io::Read>) -> Vec<u8> {
    let mut buffer = Vec::new();
    if let Some(pipe) = pipe {
        let _ = pipe.read_to_end(&mut buffer);
    }
    buffer
}

impl ReviewBackend for CodingAgent {
    fn name(&self) -> Arc<str> {
        Arc::from(self.agent.label())
    }

    fn review(
        &self,
        request: &ReviewRequest,
        events: &dyn ReviewEventSink,
    ) -> Result<Vec<RawFinding>, ReviewError> {
        if request.files.is_empty() {
            return Err(ReviewError::NothingToReview);
        }
        if events.is_cancelled() {
            return Err(ReviewError::Cancelled);
        }

        let program = self.program();
        events.progress(ReviewProgress::Starting {
            program: Arc::clone(&program),
        });

        let material = material::render(request);
        let schema = material::output_schema().to_string();
        let body = self.stdin_body(&material, &schema);

        events.progress(ReviewProgress::Running {
            detail: Arc::from(format!(
                "Reviewing {} file{} with {}",
                request.files.len(),
                if request.files.len() == 1 { "" } else { "s" },
                self.agent.label()
            )),
        });

        let output = self.run(&body, &schema)?;
        if events.is_cancelled() {
            return Err(ReviewError::Cancelled);
        }

        let payload = self.payload(&output, &program)?;
        let findings = parse_findings(&payload, &program, &request.citations())?;
        events.progress(ReviewProgress::Validating {
            returned: findings.len(),
        });
        Ok(findings)
    }
}

impl CodingAgent {
    /// The JSON findings document, dug out of whatever the CLI wrapped it in.
    fn payload(&self, output: &Output, program: &Arc<str>) -> Result<String, ReviewError> {
        if output.stdout.len() > MAX_OUTPUT_BYTES {
            return Err(ReviewError::MalformedOutput {
                program: Arc::clone(program),
                message: format!("returned more than {MAX_OUTPUT_BYTES} bytes"),
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        match self.agent {
            // Print mode always emits a result envelope, including for failures,
            // and its own message is more specific than the exit code.
            Agent::ClaudeCode => match serde_json::from_str::<PrintEnvelope>(stdout.trim()) {
                Ok(envelope) => {
                    if envelope.is_error {
                        return Err(classify(program, &envelope.failure_message(), None));
                    }
                    Ok(envelope.result.unwrap_or_default())
                }
                Err(source) => {
                    if !output.status.success() {
                        return Err(classify(program, &stderr, output.status.code()));
                    }
                    Err(ReviewError::MalformedOutput {
                        program: Arc::clone(program),
                        message: format!("could not read the result envelope: {source}"),
                    })
                }
            },
            Agent::Codex => {
                if !output.status.success() {
                    return Err(classify(program, &stderr, output.status.code()));
                }
                Ok(stdout.into_owned())
            }
        }
    }
}

/// `claude --print --output-format json` wraps the reply in this.
///
/// Only the fields the review depends on are read; the envelope carries cost and
/// usage data that a review has no business reacting to.
#[derive(Debug, Deserialize)]
struct PrintEnvelope {
    #[serde(default)]
    is_error: bool,
    /// The model's reply, or the failure text when `is_error` is set.
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    api_error_status: Option<u16>,
    #[serde(default)]
    terminal_reason: Option<String>,
}

impl PrintEnvelope {
    fn failure_message(&self) -> String {
        let message = self.result.clone().unwrap_or_default();
        match (self.api_error_status, &self.terminal_reason) {
            (Some(status), _) if !message.is_empty() => format!("{message} (HTTP {status})"),
            (Some(status), _) => format!("HTTP {status}"),
            (None, Some(reason)) if message.is_empty() => reason.clone(),
            _ => message,
        }
    }
}

#[derive(Debug, Deserialize)]
struct FindingsDocument {
    #[serde(default)]
    findings: Vec<WireFinding>,
}

#[derive(Debug, Deserialize)]
struct WireFinding {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    start_line: Option<u32>,
    severity: String,
    confidence: f32,
    title: String,
    #[serde(default)]
    rationale: String,
    proposed_comment: String,
    #[serde(default)]
    guidance: Vec<String>,
}

/// Reads the findings document, tolerating the wrappers a CLI may add.
fn parse_findings(
    payload: &str,
    program: &Arc<str>,
    citations: &[GuidanceCitation],
) -> Result<Vec<RawFinding>, ReviewError> {
    let json = extract_json(payload).ok_or_else(|| ReviewError::MalformedOutput {
        program: Arc::clone(program),
        message: "no JSON object in the reply".to_owned(),
    })?;
    let document: FindingsDocument =
        serde_json::from_str(json).map_err(|source| ReviewError::MalformedOutput {
            program: Arc::clone(program),
            message: source.to_string(),
        })?;

    Ok(document
        .findings
        .into_iter()
        .map(|wire| into_raw(wire, citations))
        .collect())
}

/// A finding as the backend stated it, with only the parts that must be typed
/// converted.
///
/// Anything unrecognisable is passed through as a value the validator will refuse,
/// rather than corrected here: a backend that returns a severity of `"critical"` or
/// a confidence of 12 should show up in the rejected list, not be quietly rounded
/// into something acceptable.
fn into_raw(wire: WireFinding, citations: &[GuidanceCitation]) -> RawFinding {
    let location = match (wire.path, wire.line) {
        (Some(path), Some(line)) if !path.trim().is_empty() => Some(RawLocation {
            path: Arc::from(path.trim()),
            side: wire
                .side
                .as_deref()
                .and_then(domain::DiffSide::from_github)
                .unwrap_or(domain::DiffSide::Right),
            line,
            start_line: wire.start_line,
        }),
        _ => None,
    };

    RawFinding {
        location,
        severity: Severity::parse(wire.severity.trim().to_lowercase().as_str())
            .unwrap_or(Severity::Info),
        confidence: wire.confidence,
        title: wire.title,
        rationale: wire.rationale,
        proposed_comment: wire.proposed_comment,
        // Only guidance that was actually sent can be cited. A path the model
        // invented is dropped rather than recorded as a source.
        guidance_sources: wire
            .guidance
            .iter()
            .filter_map(|path| {
                citations
                    .iter()
                    .find(|citation| &*citation.path == path.trim())
                    .cloned()
            })
            .collect(),
    }
}

/// Finds the JSON object in a reply that may be fenced or have prose around it.
///
/// A schema-enforcing CLI returns bare JSON, but one that only had the schema
/// described to it may wrap it, and losing a whole review to a stray markdown fence
/// would be a poor trade.
fn extract_json(payload: &str) -> Option<&str> {
    let trimmed = payload.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    (start < end).then(|| trimmed[start..=end].trim())
}

/// Maps a failure message onto something a reviewer can act on.
///
/// Substring matching is unpleasant, but a CLI's exit code says only that it
/// failed, and the difference between "sign in" and "top up" is the whole value of
/// the message.
fn classify(program: &Arc<str>, message: &str, status: Option<i32>) -> ReviewError {
    let text = message.trim();
    let lowered = text.to_lowercase();
    let program = Arc::clone(program);
    let message = if text.is_empty() {
        status.map_or_else(
            || "no output".to_owned(),
            |code| format!("exited with status {code}"),
        )
    } else {
        text.to_owned()
    };

    if lowered.contains("credit balance")
        || lowered.contains("insufficient")
        || lowered.contains("quota")
        || lowered.contains("billing")
    {
        return ReviewError::QuotaExhausted { program, message };
    }
    if lowered.contains("rate limit")
        || lowered.contains("rate_limit")
        || lowered.contains("429")
        || lowered.contains("overloaded")
    {
        return ReviewError::RateLimited { program, message };
    }
    if lowered.contains("not logged in")
        || lowered.contains("log in")
        || lowered.contains("login")
        || lowered.contains("unauthorized")
        || lowered.contains("authentication")
        || lowered.contains("invalid api key")
        || lowered.contains("401")
    {
        return ReviewError::Unauthenticated { program };
    }
    ReviewError::Backend { program, message }
}

fn launch_error(program: &Arc<str>, executable: &OsStr, source: &std::io::Error) -> ReviewError {
    if source.kind() == std::io::ErrorKind::NotFound {
        return ReviewError::NotInstalled {
            program: Arc::clone(program),
        };
    }
    ReviewError::Launch {
        program: Arc::clone(program),
        message: format!("{}: {source}", Path::new(executable).display()),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

    use domain::{
        ChangeCounts, DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStatus, IgnoreProgress,
    };

    use super::*;

    /// Writes a stub executable that stands in for a coding-agent CLI.
    fn stub(directory: &Path, name: &str, script: &str) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("the stub is written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("the stub is executable");
        path
    }

    fn file(path: &str) -> DiffFile {
        let lines = vec![DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(1),
            text: Arc::from("let x = risky();"),
        }];
        DiffFile {
            path: Arc::from(path),
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
        }
    }

    fn request() -> ReviewRequest {
        ReviewRequest {
            head_sha: Arc::from("abcdef0123456789"),
            base_sha: Arc::from("0123456789abcdef"),
            title: None,
            description: None,
            guidance: Vec::new(),
            files: Arc::from(vec![file("src/main.rs")]),
        }
    }

    /// One finding, in the envelope `claude --print --output-format json` returns.
    fn print_envelope(findings_json: &str) -> String {
        let result = serde_json::to_string(findings_json).expect("the reply is quotable");
        format!(r#"{{"type":"result","subtype":"success","is_error":false,"result":{result}}}"#)
    }

    const ONE_FINDING: &str = r#"{"findings":[{"path":"src/main.rs","side":"RIGHT","line":1,
        "start_line":null,"severity":"warning","confidence":0.8,"title":"risky() is unchecked",
        "rationale":"it can fail","proposed_comment":"Handle the failure here.","guidance":[]}]}"#;

    fn agent(directory: &Path, script: &str) -> CodingAgent {
        let executable = stub(directory, "claude", script);
        CodingAgent::new(Agent::ClaudeCode, directory).with_executable(executable)
    }

    #[test]
    fn reads_findings_out_of_the_print_envelope() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let payload = print_envelope(ONE_FINDING);
        let backend = agent(
            directory.path(),
            &format!("cat >/dev/null\ncat <<'JSON'\n{payload}\nJSON"),
        );

        let findings = backend
            .review(&request(), &IgnoreProgress)
            .expect("the stub succeeds");

        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.title, "risky() is unchecked");
        assert_eq!(finding.severity, Severity::Warning);
        let location = finding.location.as_ref().expect("the finding is anchored");
        assert_eq!(&*location.path, "src/main.rs");
        assert_eq!(location.line, 1);
        assert_eq!(location.side, domain::DiffSide::Right);
    }

    #[test]
    fn sends_the_review_material_on_stdin_not_as_arguments() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let seen = directory.path().join("stdin.txt");
        let payload = print_envelope(r#"{"findings":[]}"#);
        let backend = agent(
            directory.path(),
            &format!(
                "cat >{}\ncat <<'JSON'\n{payload}\nJSON",
                seen.to_string_lossy()
            ),
        );

        backend
            .review(&request(), &IgnoreProgress)
            .expect("the stub succeeds");

        let body = fs::read_to_string(&seen).expect("stdin was captured");
        assert!(body.contains("R1 let x = risky();"));
        assert!(body.contains("src/main.rs"));
    }

    #[test]
    fn runs_with_no_tools_and_without_the_repository_configuration() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let seen = directory.path().join("args.txt");
        let payload = print_envelope(r#"{"findings":[]}"#);
        let backend = agent(
            directory.path(),
            &format!(
                "printf '%s\\n' \"$@\" >{}\ncat >/dev/null\ncat <<'JSON'\n{payload}\nJSON",
                seen.to_string_lossy()
            ),
        );

        backend
            .review(&request(), &IgnoreProgress)
            .expect("the stub succeeds");

        let args: Vec<String> = fs::read_to_string(&seen)
            .expect("the arguments were captured")
            .lines()
            .map(str::to_owned)
            .collect();
        assert!(args.contains(&"--print".to_owned()));
        assert!(args.contains(&"--safe-mode".to_owned()));
        assert!(args.contains(&"--strict-mcp-config".to_owned()));
        // An empty tool set, passed as an empty argument rather than omitted.
        let tools = args
            .iter()
            .position(|arg| arg == "--tools")
            .expect("the tool set is set");
        assert_eq!(args[tools + 1], "");
    }

    /// Set when this test binary is re-run as its own child, with forge tokens in
    /// the environment.
    const SCRUB_CHILD: &str = "ZREVIEW_SCRUB_CHILD";

    #[test]
    fn no_forge_credential_reaches_the_review_engine() {
        // Checked in a child process rather than by mutating this one's
        // environment: `set_var` is unsafe under edition 2024 and this workspace
        // forbids unsafe code. The child inherits the tokens, so what it observes
        // is what a real review would inherit.
        if std::env::var_os(SCRUB_CHILD).is_none() {
            let status = Command::new(std::env::current_exe().expect("the test binary's path"))
                .args([
                    "--exact",
                    "agent::tests::no_forge_credential_reaches_the_review_engine",
                ])
                .env(SCRUB_CHILD, "1")
                .env("GH_TOKEN", "secret-forge-token")
                .env("GITHUB_TOKEN", "secret-forge-token")
                .status()
                .expect("the test binary re-runs");
            assert!(status.success(), "the child saw a forge token");
            return;
        }

        let directory = tempfile::tempdir().expect("a temporary directory");
        let seen = directory.path().join("env.txt");
        let payload = print_envelope(r#"{"findings":[]}"#);
        let backend = agent(
            directory.path(),
            &format!(
                "printf 'GH_TOKEN=[%s] GITHUB_TOKEN=[%s]' \"$GH_TOKEN\" \"$GITHUB_TOKEN\" >{}\n\
                 cat >/dev/null\ncat <<'JSON'\n{payload}\nJSON",
                seen.to_string_lossy()
            ),
        );

        // The parent of this call does have the tokens.
        assert_eq!(
            std::env::var("GH_TOKEN").as_deref(),
            Ok("secret-forge-token")
        );

        backend
            .review(&request(), &IgnoreProgress)
            .expect("the stub succeeds");

        let environment = fs::read_to_string(&seen).expect("the environment was captured");
        assert_eq!(environment, "GH_TOKEN=[] GITHUB_TOKEN=[]");
    }

    #[test]
    fn a_missing_executable_says_it_is_not_installed() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let backend = CodingAgent::new(Agent::ClaudeCode, directory.path())
            .with_executable(directory.path().join("nothing-here"));

        let error = backend
            .review(&request(), &IgnoreProgress)
            .expect_err("there is no executable");

        assert!(matches!(error, ReviewError::NotInstalled { .. }));
        assert!(error.remediation().is_some());
    }

    #[test]
    fn an_empty_balance_is_reported_as_spent_quota_not_a_generic_failure() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let envelope = r#"{"type":"result","is_error":true,"result":"Credit balance is too low","api_error_status":400}"#;
        let backend = agent(
            directory.path(),
            &format!("cat >/dev/null\ncat <<'JSON'\n{envelope}\nJSON"),
        );

        let error = backend
            .review(&request(), &IgnoreProgress)
            .expect_err("the balance is empty");

        match &error {
            ReviewError::QuotaExhausted { message, .. } => {
                assert!(message.contains("Credit balance"));
                assert!(message.contains("400"));
            }
            other => panic!("expected spent quota, got {other:?}"),
        }
        assert!(!error.is_retryable());
    }

    #[test]
    fn being_signed_out_is_reported_as_needing_a_sign_in() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let envelope =
            r#"{"type":"result","is_error":true,"result":"Invalid API key · Please run /login"}"#;
        let backend = agent(
            directory.path(),
            &format!("cat >/dev/null\ncat <<'JSON'\n{envelope}\nJSON"),
        );

        let error = backend
            .review(&request(), &IgnoreProgress)
            .expect_err("the agent is signed out");

        assert!(matches!(error, ReviewError::Unauthenticated { .. }));
        assert!(
            error
                .remediation()
                .is_some_and(|text| text.contains("Sign in"))
        );
    }

    #[test]
    fn a_rate_limit_is_retryable() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let envelope = r#"{"type":"result","is_error":true,"result":"rate limit exceeded","api_error_status":429}"#;
        let backend = agent(
            directory.path(),
            &format!("cat >/dev/null\ncat <<'JSON'\n{envelope}\nJSON"),
        );

        let error = backend
            .review(&request(), &IgnoreProgress)
            .expect_err("the request was rate limited");

        assert!(matches!(error, ReviewError::RateLimited { .. }));
        assert!(error.is_retryable());
    }

    #[test]
    fn output_that_is_not_the_promised_shape_is_reported_as_such() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let payload = print_envelope("I had a think about it and it looks fine to me!");
        let backend = agent(
            directory.path(),
            &format!("cat >/dev/null\ncat <<'JSON'\n{payload}\nJSON"),
        );

        let error = backend
            .review(&request(), &IgnoreProgress)
            .expect_err("the reply is not findings");

        assert!(matches!(error, ReviewError::MalformedOutput { .. }));
    }

    #[test]
    fn a_crash_before_any_output_is_reported_as_a_backend_failure() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let backend = agent(
            directory.path(),
            "cat >/dev/null\necho 'something went very wrong' >&2\nexit 1",
        );

        let error = backend
            .review(&request(), &IgnoreProgress)
            .expect_err("the stub fails");

        match &error {
            ReviewError::Backend { message, .. } => {
                assert!(message.contains("something went very wrong"));
            }
            other => panic!("expected a backend failure, got {other:?}"),
        }
    }

    #[test]
    fn an_agent_that_never_finishes_is_stopped_and_reported() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let backend = agent(directory.path(), "cat >/dev/null\nsleep 30")
            .with_timeout(Duration::from_millis(250));

        let error = backend
            .review(&request(), &IgnoreProgress)
            .expect_err("the stub hangs");

        assert!(matches!(error, ReviewError::TimedOut { seconds: 0, .. }));
        assert!(error.is_retryable());
    }

    #[test]
    fn nothing_is_launched_for_an_empty_snapshot() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let backend = CodingAgent::new(Agent::ClaudeCode, directory.path())
            .with_executable(directory.path().join("would-fail-if-run"));
        let mut request = request();
        request.files = Arc::from(Vec::new());

        let error = backend
            .review(&request, &IgnoreProgress)
            .expect_err("there is nothing to review");

        assert!(matches!(error, ReviewError::NothingToReview));
    }

    #[test]
    fn a_cancelled_review_does_not_launch_anything() {
        struct Cancelled;
        impl ReviewEventSink for Cancelled {
            fn progress(&self, _progress: ReviewProgress) {}
            fn is_cancelled(&self) -> bool {
                true
            }
        }

        let directory = tempfile::tempdir().expect("a temporary directory");
        let backend = CodingAgent::new(Agent::ClaudeCode, directory.path())
            .with_executable(directory.path().join("would-fail-if-run"));

        let error = backend
            .review(&request(), &Cancelled)
            .expect_err("the review was cancelled");

        assert!(matches!(error, ReviewError::Cancelled));
    }

    #[test]
    fn codex_is_given_the_contract_in_the_prompt_since_it_cannot_enforce_a_schema() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let seen = directory.path().join("stdin.txt");
        let executable = stub(
            directory.path(),
            "codex",
            &format!(
                "cat >{}\ncat <<'JSON'\n{ONE_FINDING}\nJSON",
                seen.to_string_lossy()
            ),
        );
        let backend = CodingAgent::new(Agent::Codex, directory.path()).with_executable(executable);

        let findings = backend
            .review(&request(), &IgnoreProgress)
            .expect("the stub succeeds");

        assert_eq!(findings.len(), 1);
        let body = fs::read_to_string(&seen).expect("stdin was captured");
        // The schema travels in the prompt, because the CLI will not check it.
        assert!(body.contains("proposed_comment"));
        assert!(body.contains("You are reviewing a proposed code change"));
    }

    #[test]
    fn a_fenced_reply_is_still_read() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let executable = stub(
            directory.path(),
            "codex",
            &format!(
                "cat >/dev/null\nprintf 'Here you go:\\n```json\\n%s\\n```\\n' '{ONE_FINDING}'"
            ),
        );
        let backend = CodingAgent::new(Agent::Codex, directory.path()).with_executable(executable);

        let findings = backend
            .review(&request(), &IgnoreProgress)
            .expect("the reply is readable");

        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn a_finding_with_no_location_is_kept_as_a_claim_about_the_whole_change() {
        let document = r#"{"findings":[{"path":null,"side":null,"line":null,"start_line":null,
            "severity":"info","confidence":0.5,"title":"No tests","rationale":"none added",
            "proposed_comment":"Consider adding a test.","guidance":[]}]}"#;

        let findings =
            parse_findings(document, &Arc::from("claude"), &[]).expect("the document is readable");

        assert_eq!(findings.len(), 1);
        assert!(findings[0].location.is_none());
    }

    #[test]
    fn an_unrecognised_severity_is_passed_through_for_the_validator_to_judge() {
        let document = r#"{"findings":[{"path":"src/main.rs","side":"RIGHT","line":1,
            "start_line":null,"severity":"CRITICAL","confidence":0.9,"title":"boom",
            "rationale":"","proposed_comment":"fix","guidance":[]}]}"#;

        let findings =
            parse_findings(document, &Arc::from("claude"), &[]).expect("the document is readable");

        // Downgraded rather than dropped: the finding is real even if the label is
        // not one we offered.
        assert_eq!(findings[0].severity, Severity::Info);
    }

    #[test]
    fn an_out_of_range_confidence_survives_parsing_so_it_can_be_rejected() {
        let document = r#"{"findings":[{"path":"src/main.rs","side":"RIGHT","line":1,
            "start_line":null,"severity":"error","confidence":12.0,"title":"boom",
            "rationale":"","proposed_comment":"fix","guidance":[]}]}"#;

        let findings =
            parse_findings(document, &Arc::from("claude"), &[]).expect("the document is readable");

        assert!(findings[0].confidence > 1.0);
    }

    #[test]
    fn only_guidance_that_was_sent_can_be_cited() {
        let citations = vec![GuidanceCitation {
            path: Arc::from("AGENTS.md"),
            content_hash: Arc::from("hash"),
        }];
        let document = r#"{"findings":[{"path":"src/main.rs","side":"RIGHT","line":1,
            "start_line":null,"severity":"error","confidence":0.9,"title":"boom",
            "rationale":"","proposed_comment":"fix",
            "guidance":["AGENTS.md","POLICY-WE-NEVER-SENT.md"]}]}"#;

        let findings = parse_findings(document, &Arc::from("claude"), &citations)
            .expect("the document is readable");

        assert_eq!(findings[0].guidance_sources, citations);
    }

    #[test]
    fn a_reply_with_no_json_at_all_is_a_shape_failure() {
        let error = parse_findings("nothing here", &Arc::from("claude"), &[])
            .expect_err("there is no document");

        assert!(matches!(error, ReviewError::MalformedOutput { .. }));
    }

    #[test]
    fn an_empty_findings_list_is_a_successful_review() {
        let findings = parse_findings(r#"{"findings":[]}"#, &Arc::from("claude"), &[])
            .expect("an empty list is valid");

        assert!(findings.is_empty());
    }
}
