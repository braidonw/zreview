use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
    sync::Arc,
};

use domain::{DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStatus};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonMode {
    /// Compare the two commits directly (`base..head`).
    Direct,
    /// Compare the merge base of the commits with head (`base...head`).
    MergeBase,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComparisonDiff {
    pub repository_root: PathBuf,
    pub base_sha: Arc<str>,
    pub head_sha: Arc<str>,
    pub files: Arc<[DiffFile]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitRemote {
    pub name: String,
    pub urls: Vec<String>,
}

#[derive(Debug, Error)]
pub enum GitError {
    #[error("failed to execute git in {repository}: {source}")]
    Execute {
        repository: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("git {operation} failed with status {status}: {stderr}")]
    Command {
        operation: &'static str,
        status: i32,
        stderr: String,
    },

    #[error("git returned non-UTF-8 {kind}")]
    NonUtf8 { kind: &'static str },

    #[error("git returned an invalid object id for {revision:?}: {value:?}")]
    InvalidObjectId { revision: String, value: String },

    #[error("git returned an unsupported file status {0:?}")]
    UnsupportedStatus(String),

    #[error("git returned an unsafe repository-relative path {0:?}")]
    InvalidPath(String),

    #[error("invalid patch for {path}: {message}")]
    InvalidPatch { path: String, message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ChangedPath {
    status: FileStatus,
    old_path: Option<String>,
    path: String,
}

/// Loads a complete local comparison without invoking a shell or an external diff driver.
///
/// # Errors
///
/// Returns [`GitError`] when the repository or revisions are invalid, Git cannot
/// produce the comparison, a path is unsafe/non-UTF-8, or a patch is malformed.
pub fn load_comparison(
    repository: impl AsRef<Path>,
    base: &str,
    head: &str,
    mode: ComparisonMode,
) -> Result<ComparisonDiff, GitError> {
    let repository_root = repository_root(repository.as_ref())?;
    let base_sha = resolve_commit(&repository_root, base)?;
    let head_sha = resolve_commit(&repository_root, head)?;
    let diff_base = match mode {
        ComparisonMode::Direct => base_sha.clone(),
        ComparisonMode::MergeBase => merge_base(&repository_root, &base_sha, &head_sha)?,
    };

    let changed = list_changed_paths(&repository_root, &diff_base, &head_sha)?;
    let files = changed
        .into_iter()
        .map(|changed| load_file_diff(&repository_root, &diff_base, &head_sha, changed))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ComparisonDiff {
        repository_root,
        base_sha: base_sha.into(),
        head_sha: head_sha.into(),
        files: files.into(),
    })
}

/// Resolves a path inside a repository to its worktree root.
///
/// # Errors
///
/// Returns [`GitError`] when Git cannot inspect the path or returns non-UTF-8 output.
pub fn repository_root(repository: &Path) -> Result<PathBuf, GitError> {
    let output = run_git(repository, "rev-parse", ["rev-parse", "--show-toplevel"])?;
    let root = output_text(output, "repository root")?;
    Ok(PathBuf::from(root.trim_end()))
}

/// Resolves a revision to a full commit object ID.
///
/// # Errors
///
/// Returns [`GitError`] when the revision is invalid or is not a commit.
pub fn resolve_commit(repository: &Path, revision: &str) -> Result<String, GitError> {
    let commit_expression = format!("{revision}^{{commit}}");
    let output = run_git(
        repository,
        "rev-parse",
        [
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--end-of-options"),
            OsStr::new(&commit_expression),
        ],
    )?;
    let value = output_text(output, "object id")?.trim().to_owned();
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError::InvalidObjectId {
            revision: revision.to_owned(),
            value,
        });
    }
    Ok(value)
}

/// Lists configured remotes and all of their fetch URLs.
///
/// # Errors
///
/// Returns [`GitError`] when Git cannot read the remote configuration or it contains
/// non-UTF-8 names/URLs.
pub fn remotes(repository: &Path) -> Result<Vec<GitRemote>, GitError> {
    let root = repository_root(repository)?;
    let output = run_git(&root, "remote", ["remote"])?;
    let names = output_text(output, "remote names")?;
    names
        .lines()
        .map(|name| {
            let config_key = format!("remote.{name}.url");
            let output = run_git(
                &root,
                "config --get-all",
                ["config", "--get-all", "--", &config_key],
            )?;
            let urls = output_text(output, "remote URLs")?
                .lines()
                .map(str::to_owned)
                .collect();
            Ok(GitRemote {
                name: name.to_owned(),
                urls,
            })
        })
        .collect()
}

/// Fetches explicit refspecs without updating `FETCH_HEAD` or user branches.
///
/// # Errors
///
/// Returns [`GitError`] when the remote/refspecs are invalid or the fetch fails.
pub fn fetch_refspecs(
    repository: &Path,
    remote: &str,
    refspecs: &[String],
) -> Result<(), GitError> {
    let mut args = vec![
        "fetch".to_owned(),
        "--no-tags".to_owned(),
        "--no-write-fetch-head".to_owned(),
        "--".to_owned(),
        remote.to_owned(),
    ];
    args.extend(refspecs.iter().cloned());
    run_git(repository, "fetch", args.iter().map(String::as_str))?;
    Ok(())
}

fn merge_base(repository: &Path, base: &str, head: &str) -> Result<String, GitError> {
    let output = run_git(repository, "merge-base", ["merge-base", "--", base, head])?;
    let value = output_text(output, "merge base")?.trim().to_owned();
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError::InvalidObjectId {
            revision: format!("merge-base({base}, {head})"),
            value,
        });
    }
    Ok(value)
}

fn list_changed_paths(
    repository: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<ChangedPath>, GitError> {
    let output = run_git(
        repository,
        "diff --name-status",
        [
            "diff",
            "--name-status",
            "-z",
            "--find-renames=50%",
            "--find-copies=50%",
            base,
            head,
            "--",
        ],
    )?;
    parse_name_status(&output.stdout)
}

fn parse_name_status(output: &[u8]) -> Result<Vec<ChangedPath>, GitError> {
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut changed = Vec::new();

    while let Some(raw_status) = fields.next() {
        let status_text = decode(raw_status, "file status")?;
        let status_code = status_text
            .as_bytes()
            .first()
            .copied()
            .ok_or_else(|| GitError::UnsupportedStatus(status_text.to_owned()))?;
        let status = match status_code {
            b'A' => FileStatus::Added,
            b'D' => FileStatus::Deleted,
            b'M' => FileStatus::Modified,
            b'R' => FileStatus::Renamed,
            b'C' => FileStatus::Copied,
            b'T' => FileStatus::TypeChanged,
            b'U' => FileStatus::Unmerged,
            _ => return Err(GitError::UnsupportedStatus(status_text.to_owned())),
        };

        let first_path = fields
            .next()
            .ok_or_else(|| GitError::UnsupportedStatus(status_text.to_owned()))?;
        let first_path = validate_repo_path(decode(first_path, "file path")?)?;
        let (old_path, path) = if matches!(status, FileStatus::Renamed | FileStatus::Copied) {
            let new_path = fields
                .next()
                .ok_or_else(|| GitError::UnsupportedStatus(status_text.to_owned()))?;
            let new_path = validate_repo_path(decode(new_path, "file path")?)?;
            (Some(first_path), new_path)
        } else {
            (None, first_path)
        };

        changed.push(ChangedPath {
            status,
            old_path,
            path,
        });
    }

    Ok(changed)
}

fn validate_repo_path(path: &str) -> Result<String, GitError> {
    let candidate = Path::new(path);
    let unsafe_component = candidate.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if path.is_empty() || candidate.is_absolute() || unsafe_component {
        return Err(GitError::InvalidPath(path.to_owned()));
    }
    Ok(path.to_owned())
}

fn load_file_diff(
    repository: &Path,
    base: &str,
    head: &str,
    changed: ChangedPath,
) -> Result<DiffFile, GitError> {
    let mut args = vec![
        "diff".to_owned(),
        "--patch".to_owned(),
        "--unified=3".to_owned(),
        "--no-color".to_owned(),
        "--no-ext-diff".to_owned(),
        "--no-textconv".to_owned(),
        "--no-prefix".to_owned(),
        "--find-renames=50%".to_owned(),
        "--find-copies=50%".to_owned(),
        base.to_owned(),
        head.to_owned(),
        "--".to_owned(),
    ];
    if let Some(old_path) = &changed.old_path {
        args.push(literal_pathspec(old_path));
    }
    args.push(literal_pathspec(&changed.path));

    let output = run_git(repository, "diff --patch", args.iter().map(String::as_str))?;
    let patch = output_text(output, "patch")?;
    let (hunks, lines, is_binary) = parse_patch(&patch, &changed.path)?;

    Ok(DiffFile {
        path: changed.path.into(),
        old_path: changed.old_path.map(Into::into),
        status: changed.status,
        is_binary,
        hunks: hunks.into(),
        lines: lines.into(),
    })
}

fn literal_pathspec(path: &str) -> String {
    format!(":(literal){path}")
}

fn parse_patch(
    patch: &str,
    file_path: &str,
) -> Result<(Vec<DiffHunk>, Vec<DiffLine>, bool), GitError> {
    let is_binary = patch.lines().any(|line| {
        line.starts_with("Binary files ") || line == "GIT binary patch" || line == "Binary file"
    });
    if is_binary {
        return Ok((Vec::new(), Vec::new(), true));
    }

    let patch_lines = patch.lines().collect::<Vec<_>>();
    let mut hunks = Vec::new();
    let mut lines = Vec::new();
    let mut index = 0;

    while index < patch_lines.len() {
        let header = patch_lines[index];
        if !header.starts_with("@@ -") {
            index += 1;
            continue;
        }

        let (old_start, old_count, new_start, new_count) =
            parse_hunk_header(header).map_err(|message| GitError::InvalidPatch {
                path: file_path.to_owned(),
                message,
            })?;
        let line_start = lines.len();
        let mut old_line = old_start;
        let mut new_line = new_start;
        let mut old_seen = 0_u32;
        let mut new_seen = 0_u32;
        index += 1;

        while index < patch_lines.len() && !patch_lines[index].starts_with("@@ -") {
            let raw = patch_lines[index];
            let (kind, old_coordinate, new_coordinate, text) =
                if let Some(text) = raw.strip_prefix(' ') {
                    let coordinates = (Some(old_line), Some(new_line));
                    old_line += 1;
                    new_line += 1;
                    old_seen += 1;
                    new_seen += 1;
                    (DiffLineKind::Context, coordinates.0, coordinates.1, text)
                } else if let Some(text) = raw.strip_prefix('+') {
                    let coordinate = Some(new_line);
                    new_line += 1;
                    new_seen += 1;
                    (DiffLineKind::Addition, None, coordinate, text)
                } else if let Some(text) = raw.strip_prefix('-') {
                    let coordinate = Some(old_line);
                    old_line += 1;
                    old_seen += 1;
                    (DiffLineKind::Deletion, coordinate, None, text)
                } else if raw == "\\ No newline at end of file" {
                    (
                        DiffLineKind::NoNewlineMarker,
                        None,
                        None,
                        "No newline at end of file",
                    )
                } else if raw.starts_with("diff --git ") {
                    break;
                } else {
                    return Err(GitError::InvalidPatch {
                        path: file_path.to_owned(),
                        message: format!("unexpected line inside hunk: {raw:?}"),
                    });
                };

            lines.push(DiffLine {
                kind,
                old_line: old_coordinate,
                new_line: new_coordinate,
                text: Arc::from(text),
            });
            index += 1;
        }

        if old_seen != old_count || new_seen != new_count {
            return Err(GitError::InvalidPatch {
                path: file_path.to_owned(),
                message: format!(
                    "hunk count mismatch: header expected -{old_count}/+{new_count}, parsed -{old_seen}/+{new_seen}"
                ),
            });
        }

        hunks.push(DiffHunk {
            header: header.into(),
            old_start,
            new_start,
            line_range: line_start..lines.len(),
        });
    }

    Ok((hunks, lines, false))
}

fn parse_hunk_header(header: &str) -> Result<(u32, u32, u32, u32), String> {
    let ranges = header
        .strip_prefix("@@ -")
        .ok_or_else(|| format!("invalid hunk header {header:?}"))?;
    let (old_range, remainder) = ranges
        .split_once(" +")
        .ok_or_else(|| format!("invalid old range in {header:?}"))?;
    let (new_range, _) = remainder
        .split_once(" @@")
        .ok_or_else(|| format!("invalid new range in {header:?}"))?;
    let (old_start, old_count) = parse_hunk_range(old_range)?;
    let (new_start, new_count) = parse_hunk_range(new_range)?;
    Ok((old_start, old_count, new_start, new_count))
}

fn parse_hunk_range(value: &str) -> Result<(u32, u32), String> {
    let (start, count) = value.split_once(',').map_or((value, "1"), |parts| parts);
    let start = start
        .parse::<u32>()
        .map_err(|_| format!("invalid hunk start {start:?}"))?;
    let count = count
        .parse::<u32>()
        .map_err(|_| format!("invalid hunk count {count:?}"))?;
    Ok((start, count))
}

fn run_git<I, S>(repository: &Path, operation: &'static str, args: I) -> Result<Output, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|source| GitError::Execute {
            repository: repository.to_path_buf(),
            source,
        })?;

    if output.status.success() {
        Ok(output)
    } else {
        Err(GitError::Command {
            operation,
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

fn output_text(output: Output, kind: &'static str) -> Result<String, GitError> {
    String::from_utf8(output.stdout).map_err(|_| GitError::NonUtf8 { kind })
}

fn decode<'a>(value: &'a [u8], kind: &'static str) -> Result<&'a str, GitError> {
    std::str::from_utf8(value).map_err(|_| GitError::NonUtf8 { kind })
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command};

    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parses_multiple_hunks_and_no_newline_markers() {
        let patch = include_str!("../tests/fixtures/modified.patch");
        let (hunks, lines, is_binary) = parse_patch(patch, "src/example.rs").unwrap();

        assert!(!is_binary);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].line_range, 0..4);
        assert_eq!(hunks[1].line_range, 4..8);
        assert_eq!(lines[1].kind, DiffLineKind::Deletion);
        assert_eq!(lines[1].old_line, Some(2));
        assert_eq!(lines[2].kind, DiffLineKind::Addition);
        assert_eq!(lines[2].new_line, Some(2));
        assert_eq!(lines[7].kind, DiffLineKind::NoNewlineMarker);
    }

    #[test]
    fn rejects_hunk_count_mismatches() {
        let error = parse_patch("@@ -1,2 +1,2 @@\n only one\n", "bad.txt").unwrap_err();
        assert!(matches!(error, GitError::InvalidPatch { .. }));
        assert!(error.to_string().contains("hunk count mismatch"));
    }

    #[test]
    fn parses_nul_delimited_rename_status() {
        let changed =
            parse_name_status(b"M\0src/lib.rs\0R091\0old name.rs\0new name.rs\0").unwrap();

        assert_eq!(changed.len(), 2);
        assert_eq!(changed[0].status, FileStatus::Modified);
        assert_eq!(changed[1].status, FileStatus::Renamed);
        assert_eq!(changed[1].old_path.as_deref(), Some("old name.rs"));
        assert_eq!(changed[1].path, "new name.rs");
    }

    #[test]
    fn lists_remotes_and_fetches_only_namespaced_refs() {
        let source = TempDir::new().unwrap();
        git(source.path(), ["init", "--quiet"]);
        git(source.path(), ["config", "user.name", "ZReview Test"]);
        git(
            source.path(),
            ["config", "user.email", "zreview@example.invalid"],
        );
        fs::write(source.path().join("README.md"), "source\n").unwrap();
        git(source.path(), ["add", "."]);
        git(source.path(), ["commit", "--quiet", "-m", "source"]);
        let source_head = git_output(source.path(), ["rev-parse", "HEAD"]);
        let source_branch = git_output(source.path(), ["branch", "--show-current"]);

        let target = TempDir::new().unwrap();
        git(target.path(), ["init", "--quiet"]);
        git(
            target.path(),
            ["remote", "add", "origin", source.path().to_str().unwrap()],
        );

        let configured = remotes(target.path()).unwrap();
        assert_eq!(configured[0].name, "origin");
        assert_eq!(configured[0].urls, [source.path().to_str().unwrap()]);

        let destination = "refs/zreview/test/head";
        fetch_refspecs(
            target.path(),
            "origin",
            &[format!("+refs/heads/{source_branch}:{destination}")],
        )
        .unwrap();
        assert_eq!(
            resolve_commit(target.path(), destination).unwrap(),
            source_head
        );
    }

    #[test]
    fn loads_a_real_repository_comparison() {
        let repository = TempDir::new().unwrap();
        git(repository.path(), ["init", "--quiet"]);
        git(repository.path(), ["config", "user.name", "ZReview Test"]);
        git(
            repository.path(),
            ["config", "user.email", "zreview@example.invalid"],
        );

        fs::create_dir(repository.path().join("src")).unwrap();
        fs::write(
            repository.path().join("src/example.rs"),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
        )
        .unwrap();
        fs::write(repository.path().join("delete me.txt"), "delete me\n").unwrap();
        git(repository.path(), ["add", "."]);
        git(repository.path(), ["commit", "--quiet", "-m", "base"]);
        let base = git_output(repository.path(), ["rev-parse", "HEAD"]);

        git(
            repository.path(),
            ["mv", "src/example.rs", "src/renamed example.rs"],
        );
        fs::write(
            repository.path().join("src/renamed example.rs"),
            "one\ntwo changed\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
        )
        .unwrap();
        fs::remove_file(repository.path().join("delete me.txt")).unwrap();
        fs::write(
            repository.path().join("added.txt"),
            "new file without newline",
        )
        .unwrap();
        fs::write(repository.path().join("image.bin"), [0_u8, 1, 2, 3]).unwrap();
        git(repository.path(), ["add", "-A"]);
        git(repository.path(), ["commit", "--quiet", "-m", "head"]);

        let comparison =
            load_comparison(repository.path(), &base, "HEAD", ComparisonMode::Direct).unwrap();

        assert_eq!(comparison.files.len(), 4);
        let renamed = comparison
            .files
            .iter()
            .find(|file| file.status == FileStatus::Renamed)
            .unwrap();
        assert_eq!(renamed.path.as_ref(), "src/renamed example.rs");
        assert_eq!(renamed.old_path.as_deref(), Some("src/example.rs"));
        assert!(
            renamed
                .lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Addition)
        );

        let binary = comparison
            .files
            .iter()
            .find(|file| file.path.as_ref() == "image.bin")
            .unwrap();
        assert!(binary.is_binary);
        assert!(binary.lines.is_empty());
    }

    fn git<I, S>(repository: &Path, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let status = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn git_output<I, S>(repository: &Path, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}
