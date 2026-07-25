//! Finding the guidance a repository wants its code reviewed against.
//!
//! Repositories already say how they want to be reviewed, in `AGENTS.md`,
//! `CLAUDE.md`, `CONTRIBUTING.md` and the like. Discovery reads those so a review
//! can be held to the project's own standards rather than to generic ones.
//!
//! Two properties this module is built around, both from PLAN section 8:
//!
//! - **It is read-only.** Nothing here executes anything. Discovering a
//!   repository's guidance is not consent to run its commands, and opening a
//!   repository must never do so. There is no code path from here to a
//!   subprocess.
//! - **It is transparent.** Every file found is reported with where it came from
//!   and what it applies to, and every file *skipped* is reported with why. A
//!   reviewer has to be able to see exactly what would be sent to a model before
//!   it is, so silently dropping something is worse than not finding it.

use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use globset::{Glob, GlobSetBuilder};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Guidance filenames looked for by convention, in the order they are reported.
const CONVENTIONAL_NAMES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "CONTRIBUTING.md",
    "STYLEGUIDE.md",
    "STYLE_GUIDE.md",
];

/// Conventional files that also apply when found in a subdirectory.
///
/// A nested `CONTRIBUTING.md` is usually about contributing to the project as a
/// whole rather than about the code beside it, so only the agent-instruction
/// conventions are treated as directory-scoped.
const NESTED_NAMES: &[&str] = &["AGENTS.md", "CLAUDE.md"];

const COPILOT_INSTRUCTIONS: &str = ".github/copilot-instructions.md";
const INSTRUCTIONS_DIRECTORY: &str = ".github/instructions";

/// Largest single guidance file that will be used.
///
/// Something larger is nearly always generated or vendored, and would crowd out
/// the diff in a model's context.
pub const MAX_FILE_BYTES: usize = 64 * 1024;

/// Largest total guidance that will be used.
pub const MAX_TOTAL_BYTES: usize = 256 * 1024;

/// What a piece of guidance applies to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GuidanceScope {
    /// Every reviewed file.
    Repository,
    /// Reviewed files under a directory, from a nested convention file.
    Directory(Arc<str>),
    /// Reviewed files matching globs, from a path-scoped instruction file.
    Paths(Vec<String>),
}

impl Display for GuidanceScope {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Repository => formatter.write_str("whole repository"),
            Self::Directory(directory) => write!(formatter, "{directory}/"),
            Self::Paths(globs) => write!(formatter, "{}", globs.join(", ")),
        }
    }
}

/// How a piece of guidance came to be included.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuidanceSource {
    /// Found by its conventional name.
    Convention,
    /// Named by `.zreview.toml`.
    Configured,
}

impl Display for GuidanceSource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Convention => "by convention",
            Self::Configured => "from .zreview.toml",
        })
    }
}

/// One guidance file that will be used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuidanceFile {
    /// Repository-relative path, for display.
    pub path: Arc<str>,
    pub scope: GuidanceScope,
    pub source: GuidanceSource,
    pub content: String,
    /// SHA-256 of the content, so a review run can record exactly what it was
    /// given and a later run can tell whether it changed.
    pub content_hash: String,
    /// Whether this file will actually be sent. Discovery includes everything it
    /// finds; the reviewer can turn any of it off.
    pub included: bool,
}

impl GuidanceFile {
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.content.len()
    }

    /// Whether this guidance applies to a reviewed file.
    ///
    /// # Errors
    ///
    /// Returns the offending glob when a path-scoped pattern cannot be compiled.
    pub fn applies_to(&self, reviewed_path: &str) -> Result<bool, String> {
        match &self.scope {
            GuidanceScope::Repository => Ok(true),
            GuidanceScope::Directory(directory) => {
                Ok(reviewed_path.starts_with(&format!("{directory}/")))
            }
            GuidanceScope::Paths(globs) => {
                let mut builder = GlobSetBuilder::new();
                for glob in globs {
                    builder.add(Glob::new(glob).map_err(|error| error.to_string())?);
                }
                let set = builder.build().map_err(|error| error.to_string())?;
                Ok(set.is_match(reviewed_path))
            }
        }
    }
}

/// Why a candidate was not used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkipReason {
    TooLarge {
        bytes: usize,
        limit: usize,
    },
    /// Excluded by `.zreview.toml`.
    ExcludedByConfig,
    /// Reading it failed.
    Unreadable(String),
    /// The total guidance budget was already spent.
    BudgetSpent {
        limit: usize,
    },
    /// A configured include pattern is not a valid glob.
    InvalidPattern(String),
    /// The path escapes the repository.
    OutsideRepository,
}

impl Display for SkipReason {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => {
                write!(formatter, "{bytes} bytes, over the {limit}-byte limit")
            }
            Self::ExcludedByConfig => formatter.write_str("excluded by .zreview.toml"),
            Self::Unreadable(error) => write!(formatter, "could not be read: {error}"),
            Self::BudgetSpent { limit } => {
                write!(
                    formatter,
                    "the {limit}-byte guidance budget was already full"
                )
            }
            Self::InvalidPattern(error) => write!(formatter, "invalid pattern: {error}"),
            Self::OutsideRepository => formatter.write_str("outside the repository"),
        }
    }
}

/// Something found but not used, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedGuidance {
    pub path: Arc<str>,
    pub reason: SkipReason,
}

/// Everything discovery found for one snapshot.
#[derive(Clone, Debug, Default)]
pub struct Guidance {
    files: Vec<GuidanceFile>,
    skipped: Vec<SkippedGuidance>,
    /// Reviewed-file exclusions from `.zreview.toml`, which are about what gets
    /// reviewed rather than what guides it.
    excluded_files: Vec<String>,
}

impl Guidance {
    #[must_use]
    pub fn files(&self) -> &[GuidanceFile] {
        &self.files
    }

    /// Guidance that will actually be sent.
    pub fn included(&self) -> impl Iterator<Item = &GuidanceFile> {
        self.files.iter().filter(|file| file.included)
    }

    /// Candidates that were found and not used, each with a reason.
    #[must_use]
    pub fn skipped(&self) -> &[SkippedGuidance] {
        &self.skipped
    }

    #[must_use]
    pub fn included_bytes(&self) -> usize {
        self.included().map(GuidanceFile::bytes).sum()
    }

    /// Turns one file on or off by path, reporting whether it was found.
    pub fn set_included(&mut self, path: &str, included: bool) -> bool {
        let Some(file) = self
            .files
            .iter_mut()
            .find(|file| file.path.as_ref() == path)
        else {
            return false;
        };
        file.included = included;
        true
    }

    /// The guidance that applies to a reviewed file, in report order.
    pub fn for_reviewed_path(&self, reviewed_path: &str) -> impl Iterator<Item = &GuidanceFile> {
        self.included()
            .filter(move |file| file.applies_to(reviewed_path).unwrap_or(false))
    }

    /// Whether a reviewed file was excluded from review by configuration.
    ///
    /// # Errors
    ///
    /// Returns the offending pattern when an exclusion cannot be compiled.
    pub fn excludes_reviewed_path(&self, reviewed_path: &str) -> Result<bool, String> {
        if self.excluded_files.is_empty() {
            return Ok(false);
        }
        let mut builder = GlobSetBuilder::new();
        for pattern in &self.excluded_files {
            builder.add(Glob::new(pattern).map_err(|error| error.to_string())?);
        }
        Ok(builder
            .build()
            .map_err(|error| error.to_string())?
            .is_match(reviewed_path))
    }
}

/// `.zreview.toml`, which overrides the conventional defaults.
#[derive(Debug, Default, Deserialize)]
struct RepositoryConfig {
    #[serde(default)]
    review: ReviewConfig,
}

#[derive(Debug, Default, Deserialize)]
struct ReviewConfig {
    /// Extra guidance files to include, as globs.
    #[serde(default)]
    instructions: Vec<String>,
    /// Conventional guidance to leave out, as globs.
    #[serde(default)]
    exclude_instructions: Vec<String>,
    /// Reviewed files to leave out of the review entirely.
    #[serde(default)]
    exclude_files: Vec<String>,
}

/// Finds the guidance for a snapshot.
///
/// `changed_paths` are the repository-relative paths under review; they decide
/// which nested and path-scoped guidance applies. Nothing is executed and nothing
/// outside `repository_root` is read.
#[must_use]
pub fn discover(repository_root: &Path, changed_paths: &[&str]) -> Guidance {
    let mut guidance = Guidance::default();
    let config = read_config(repository_root, &mut guidance);
    guidance
        .excluded_files
        .clone_from(&config.review.exclude_files);

    let exclusions = compile_globs(&config.review.exclude_instructions, &mut guidance);
    let mut seen = BTreeSet::new();

    // Repository-wide conventions first: they are the most likely to exist and the
    // most general, so they read first in the panel.
    for name in CONVENTIONAL_NAMES {
        consider(
            repository_root,
            name,
            GuidanceScope::Repository,
            GuidanceSource::Convention,
            exclusions.as_ref(),
            &mut seen,
            &mut guidance,
        );
    }
    consider(
        repository_root,
        COPILOT_INSTRUCTIONS,
        GuidanceScope::Repository,
        GuidanceSource::Convention,
        exclusions.as_ref(),
        &mut seen,
        &mut guidance,
    );

    for (path, scope) in nested_candidates(changed_paths) {
        consider(
            repository_root,
            &path,
            scope,
            GuidanceSource::Convention,
            exclusions.as_ref(),
            &mut seen,
            &mut guidance,
        );
    }

    for (path, scope) in path_scoped_candidates(repository_root) {
        consider(
            repository_root,
            &path,
            scope,
            GuidanceSource::Convention,
            exclusions.as_ref(),
            &mut seen,
            &mut guidance,
        );
    }

    for path in configured_candidates(repository_root, &config.review.instructions, &mut guidance) {
        consider(
            repository_root,
            &path,
            GuidanceScope::Repository,
            GuidanceSource::Configured,
            exclusions.as_ref(),
            &mut seen,
            &mut guidance,
        );
    }

    guidance
}

fn read_config(repository_root: &Path, guidance: &mut Guidance) -> RepositoryConfig {
    let path = repository_root.join(".zreview.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return RepositoryConfig::default();
    };
    // Malformed configuration is reported, not fatal: a reviewer should still be
    // able to read the diff.
    toml::from_str(&text).unwrap_or_else(|error| {
        guidance.skipped.push(SkippedGuidance {
            path: ".zreview.toml".into(),
            reason: SkipReason::Unreadable(error.to_string()),
        });
        RepositoryConfig::default()
    })
}

/// Directories containing a changed file, and their ancestors, paired with the
/// nested convention names to look for in each.
fn nested_candidates(changed_paths: &[&str]) -> Vec<(String, GuidanceScope)> {
    let mut directories = BTreeSet::new();
    for path in changed_paths {
        let mut current = Path::new(path).parent();
        while let Some(directory) = current {
            if directory.as_os_str().is_empty() {
                break;
            }
            directories.insert(directory.to_string_lossy().into_owned());
            current = directory.parent();
        }
    }

    directories
        .into_iter()
        .flat_map(|directory| {
            NESTED_NAMES.iter().map(move |name| {
                (
                    format!("{directory}/{name}"),
                    GuidanceScope::Directory(directory.clone().into()),
                )
            })
        })
        .collect()
}

/// `.github/instructions/*.instructions.md`, scoped by their `applyTo` header.
fn path_scoped_candidates(repository_root: &Path) -> Vec<(String, GuidanceScope)> {
    let directory = repository_root.join(INSTRUCTIONS_DIRECTORY);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".instructions.md") {
            continue;
        }
        let relative = format!("{INSTRUCTIONS_DIRECTORY}/{name}");
        let globs = std::fs::read_to_string(entry.path())
            .ok()
            .and_then(|text| apply_to_globs(&text))
            // Without an `applyTo` header the file says nothing about scope, so it
            // is treated as applying everywhere rather than nowhere.
            .map_or(GuidanceScope::Repository, GuidanceScope::Paths);
        candidates.push((relative, globs));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    candidates
}

/// Reads the `applyTo` globs from a front-matter header.
///
/// Handles the shape GitHub documents — a `---` delimited header with a quoted,
/// comma-separated `applyTo` — rather than being a general YAML parser.
fn apply_to_globs(text: &str) -> Option<Vec<String>> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("applyTo:") {
            let globs = value
                .trim()
                .trim_matches(['"', '\''])
                .split(',')
                .map(|glob| glob.trim().to_owned())
                .filter(|glob| !glob.is_empty())
                .collect::<Vec<_>>();
            return (!globs.is_empty()).then_some(globs);
        }
    }
    None
}

/// Expands the `instructions` globs from `.zreview.toml` into repository paths.
fn configured_candidates(
    repository_root: &Path,
    patterns: &[String],
    guidance: &mut Guidance,
) -> Vec<String> {
    if patterns.is_empty() {
        return Vec::new();
    }
    let set = compile_globs(patterns, guidance);
    let Some(set) = set else {
        return Vec::new();
    };

    let mut matched = BTreeSet::new();
    walk_repository(repository_root, repository_root, &mut |relative| {
        if set.is_match(relative) {
            matched.insert(relative.to_owned());
        }
    });
    matched.into_iter().collect()
}

/// Walks the repository, skipping directories that never hold guidance.
///
/// Bounded by depth so a pathological tree cannot spin, and it never follows
/// symbolic links out of the repository.
fn walk_repository(root: &Path, directory: &Path, visit: &mut impl FnMut(&str)) {
    const SKIP: &[&str] = &[
        ".git",
        "node_modules",
        "target",
        "_build",
        "deps",
        ".elixir_ls",
    ];
    const MAX_DEPTH: usize = 12;

    if directory
        .components()
        .count()
        .saturating_sub(root.components().count())
        > MAX_DEPTH
    {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            if !SKIP.contains(&name.as_str()) {
                walk_repository(root, &path, visit);
            }
        } else if let Ok(relative) = path.strip_prefix(root)
            && let Some(relative) = relative.to_str()
        {
            visit(relative);
        }
    }
}

fn compile_globs(patterns: &[String], guidance: &mut Guidance) -> Option<globset::GlobSet> {
    if patterns.is_empty() {
        return None;
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        match Glob::new(pattern) {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(error) => guidance.skipped.push(SkippedGuidance {
                path: pattern.clone().into(),
                reason: SkipReason::InvalidPattern(error.to_string()),
            }),
        }
    }
    builder.build().ok()
}

/// Reads one candidate, recording it as used or skipped.
#[allow(clippy::too_many_arguments)]
fn consider(
    repository_root: &Path,
    relative: &str,
    scope: GuidanceScope,
    source: GuidanceSource,
    exclusions: Option<&globset::GlobSet>,
    seen: &mut BTreeSet<String>,
    guidance: &mut Guidance,
) {
    if seen.contains(relative) {
        return;
    }
    // A guidance path that climbs out of the repository would let a checkout name
    // any file on the machine as review context.
    if !is_inside_repository(relative) {
        seen.insert(relative.to_owned());
        guidance.skipped.push(SkippedGuidance {
            path: relative.into(),
            reason: SkipReason::OutsideRepository,
        });
        return;
    }

    let path = repository_root.join(relative);
    let Ok(metadata) = std::fs::metadata(&path) else {
        return; // Simply absent; not worth reporting.
    };
    if !metadata.is_file() {
        return;
    }
    seen.insert(relative.to_owned());

    if exclusions.is_some_and(|excluded| excluded.is_match(relative)) {
        guidance.skipped.push(SkippedGuidance {
            path: relative.into(),
            reason: SkipReason::ExcludedByConfig,
        });
        return;
    }

    let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if size > MAX_FILE_BYTES {
        guidance.skipped.push(SkippedGuidance {
            path: relative.into(),
            reason: SkipReason::TooLarge {
                bytes: size,
                limit: MAX_FILE_BYTES,
            },
        });
        return;
    }
    if guidance.included_bytes().saturating_add(size) > MAX_TOTAL_BYTES {
        guidance.skipped.push(SkippedGuidance {
            path: relative.into(),
            reason: SkipReason::BudgetSpent {
                limit: MAX_TOTAL_BYTES,
            },
        });
        return;
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let content_hash = hex(&Sha256::digest(content.as_bytes()));
            guidance.files.push(GuidanceFile {
                path: relative.into(),
                scope,
                source,
                content,
                content_hash,
                included: true,
            });
        }
        Err(error) => guidance.skipped.push(SkippedGuidance {
            path: relative.into(),
            reason: SkipReason::Unreadable(error.to_string()),
        }),
    }
}

/// Lowercase hex of a digest.
///
/// Written out rather than pulling in a hex crate for thirty-two bytes.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn is_inside_repository(relative: &str) -> bool {
    let path = Path::new(relative);
    !path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

/// The absolute path of a repository-relative guidance file.
#[must_use]
pub fn guidance_path(repository_root: &Path, relative: &str) -> PathBuf {
    repository_root.join(relative)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn paths(guidance: &Guidance) -> Vec<String> {
        guidance
            .files()
            .iter()
            .map(|file| file.path.to_string())
            .collect()
    }

    #[test]
    fn finds_the_conventional_files_at_the_root() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "AGENTS.md", "agent rules");
        write(repository.path(), "CLAUDE.md", "claude rules");
        write(repository.path(), "CONTRIBUTING.md", "how to contribute");
        write(repository.path(), "README.md", "not guidance");

        let guidance = discover(repository.path(), &["src/lib.rs"]);

        assert_eq!(
            paths(&guidance),
            ["AGENTS.md", "CLAUDE.md", "CONTRIBUTING.md"],
        );
        assert!(
            guidance
                .files()
                .iter()
                .all(|file| file.scope == GuidanceScope::Repository)
        );
        assert!(
            guidance
                .files()
                .iter()
                .all(|file| file.source == GuidanceSource::Convention)
        );
    }

    #[test]
    fn a_repository_with_no_guidance_finds_nothing_and_reports_nothing() {
        let repository = TempDir::new().unwrap();
        let guidance = discover(repository.path(), &["src/lib.rs"]);

        assert!(guidance.files().is_empty());
        assert!(guidance.skipped().is_empty(), "absent is not skipped");
    }

    #[test]
    fn content_is_read_and_hashed() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "AGENTS.md", "prefer clarity");

        let guidance = discover(repository.path(), &[]);
        let file = &guidance.files()[0];

        assert_eq!(file.content, "prefer clarity");
        assert_eq!(file.bytes(), 14);
        // Stable across runs and builds, so a review run can record what it saw.
        assert_eq!(file.content_hash, hex(&Sha256::digest(b"prefer clarity")),);
        assert_eq!(file.content_hash.len(), 64);
    }

    /// Nested guidance applies to the code beside it, not to everything.
    #[test]
    fn nested_guidance_is_scoped_to_its_directory() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "AGENTS.md", "root");
        write(repository.path(), "lib/web/AGENTS.md", "web rules");

        let guidance = discover(repository.path(), &["lib/web/page.ex", "lib/core/thing.ex"]);

        assert_eq!(paths(&guidance), ["AGENTS.md", "lib/web/AGENTS.md"]);
        let nested = &guidance.files()[1];
        assert_eq!(nested.scope, GuidanceScope::Directory("lib/web".into()),);
        assert!(nested.applies_to("lib/web/page.ex").unwrap());
        assert!(!nested.applies_to("lib/core/thing.ex").unwrap());
        // The root file applies to both.
        assert!(guidance.files()[0].applies_to("lib/core/thing.ex").unwrap());
    }

    /// Guidance beside code that is not being reviewed is not read at all.
    #[test]
    fn nested_guidance_for_untouched_directories_is_not_read() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "lib/web/AGENTS.md", "web rules");
        write(repository.path(), "lib/other/AGENTS.md", "other rules");

        let guidance = discover(repository.path(), &["lib/web/page.ex"]);

        assert_eq!(paths(&guidance), ["lib/web/AGENTS.md"]);
    }

    #[test]
    fn ancestor_directories_of_a_changed_file_are_searched() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "lib/AGENTS.md", "lib rules");

        let guidance = discover(repository.path(), &["lib/web/live/page.ex"]);

        assert_eq!(paths(&guidance), ["lib/AGENTS.md"]);
        assert!(
            guidance.files()[0]
                .applies_to("lib/web/live/page.ex")
                .unwrap()
        );
    }

    #[test]
    fn path_scoped_instructions_use_their_apply_to_header() {
        let repository = TempDir::new().unwrap();
        write(
            repository.path(),
            ".github/instructions/elixir.instructions.md",
            "---\napplyTo: \"**/*.ex,**/*.exs\"\n---\nElixir rules",
        );

        let guidance = discover(repository.path(), &["lib/thing.ex"]);
        let file = &guidance.files()[0];

        assert_eq!(
            file.scope,
            GuidanceScope::Paths(vec!["**/*.ex".to_owned(), "**/*.exs".to_owned()]),
        );
        assert!(file.applies_to("lib/thing.ex").unwrap());
        assert!(file.applies_to("test/thing.exs").unwrap());
        assert!(!file.applies_to("README.md").unwrap());
        assert_eq!(
            file.content,
            "---\napplyTo: \"**/*.ex,**/*.exs\"\n---\nElixir rules"
        );
    }

    /// A file that declares no scope is about the whole repository, not nothing.
    #[test]
    fn instructions_without_an_apply_to_header_apply_everywhere() {
        let repository = TempDir::new().unwrap();
        write(
            repository.path(),
            ".github/instructions/general.instructions.md",
            "no front matter here",
        );

        let guidance = discover(repository.path(), &["lib/thing.ex"]);

        assert_eq!(guidance.files()[0].scope, GuidanceScope::Repository);
        assert!(guidance.files()[0].applies_to("anything.txt").unwrap());
    }

    #[test]
    fn copilot_instructions_are_found() {
        let repository = TempDir::new().unwrap();
        write(
            repository.path(),
            ".github/copilot-instructions.md",
            "copilot rules",
        );

        let guidance = discover(repository.path(), &[]);
        assert_eq!(paths(&guidance), [".github/copilot-instructions.md"]);
    }

    #[test]
    fn config_can_exclude_a_conventional_file() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "AGENTS.md", "agent rules");
        write(repository.path(), "CLAUDE.md", "claude rules");
        write(
            repository.path(),
            ".zreview.toml",
            "[review]\nexclude_instructions = [\"CLAUDE.md\"]\n",
        );

        let guidance = discover(repository.path(), &[]);

        assert_eq!(paths(&guidance), ["AGENTS.md"]);
        assert_eq!(
            guidance.skipped(),
            [SkippedGuidance {
                path: "CLAUDE.md".into(),
                reason: SkipReason::ExcludedByConfig,
            }],
        );
    }

    #[test]
    fn config_can_add_guidance_by_glob() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "docs/style/naming.md", "naming rules");
        write(repository.path(), "docs/style/tests.md", "test rules");
        write(repository.path(), "docs/other.md", "not guidance");
        write(
            repository.path(),
            ".zreview.toml",
            "[review]\ninstructions = [\"docs/style/**/*.md\"]\n",
        );

        let guidance = discover(repository.path(), &[]);

        assert_eq!(
            paths(&guidance),
            ["docs/style/naming.md", "docs/style/tests.md"],
        );
        assert!(
            guidance
                .files()
                .iter()
                .all(|file| file.source == GuidanceSource::Configured)
        );
    }

    #[test]
    fn config_can_exclude_reviewed_files() {
        let repository = TempDir::new().unwrap();
        write(
            repository.path(),
            ".zreview.toml",
            "[review]\nexclude_files = [\"vendor/**\", \"**/*.lock\"]\n",
        );

        let guidance = discover(repository.path(), &[]);

        assert!(guidance.excludes_reviewed_path("vendor/thing.ex").unwrap());
        assert!(guidance.excludes_reviewed_path("mix.lock").unwrap());
        assert!(!guidance.excludes_reviewed_path("lib/thing.ex").unwrap());
    }

    /// Malformed configuration must not stop a reviewer reading the diff.
    #[test]
    fn malformed_config_is_reported_rather_than_fatal() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "AGENTS.md", "agent rules");
        write(repository.path(), ".zreview.toml", "this is not toml {{{");

        let guidance = discover(repository.path(), &[]);

        assert_eq!(paths(&guidance), ["AGENTS.md"], "discovery still ran");
        assert!(matches!(
            guidance.skipped().first().map(|skip| &skip.reason),
            Some(SkipReason::Unreadable(_)),
        ));
    }

    #[test]
    fn an_oversized_file_is_skipped_with_its_size() {
        let repository = TempDir::new().unwrap();
        write(
            repository.path(),
            "AGENTS.md",
            &"x".repeat(MAX_FILE_BYTES + 1),
        );

        let guidance = discover(repository.path(), &[]);

        assert!(guidance.files().is_empty());
        assert_eq!(
            guidance.skipped()[0].reason,
            SkipReason::TooLarge {
                bytes: MAX_FILE_BYTES + 1,
                limit: MAX_FILE_BYTES,
            },
        );
    }

    #[test]
    fn the_total_budget_is_enforced_and_reported() {
        let repository = TempDir::new().unwrap();
        // Four files of 64KiB each: the fourth exceeds the 256KiB total.
        let big = "x".repeat(MAX_FILE_BYTES);
        for name in ["AGENTS.md", "CLAUDE.md", "CONTRIBUTING.md", "STYLEGUIDE.md"] {
            write(repository.path(), name, &big);
        }

        let guidance = discover(repository.path(), &[]);

        assert_eq!(guidance.files().len(), 4, "exactly the budget");
        assert_eq!(guidance.included_bytes(), MAX_TOTAL_BYTES);

        // A fifth would not fit.
        write(repository.path(), "STYLE_GUIDE.md", &big);
        let guidance = discover(repository.path(), &[]);
        assert_eq!(guidance.files().len(), 4);
        assert_eq!(
            guidance.skipped()[0].reason,
            SkipReason::BudgetSpent {
                limit: MAX_TOTAL_BYTES
            },
        );
    }

    #[test]
    fn a_file_is_never_reported_twice() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "AGENTS.md", "rules");
        // Also matched by configuration, which must not duplicate it.
        write(
            repository.path(),
            ".zreview.toml",
            "[review]\ninstructions = [\"AGENTS.md\"]\n",
        );

        let guidance = discover(repository.path(), &[]);

        assert_eq!(paths(&guidance), ["AGENTS.md"]);
        assert_eq!(guidance.files()[0].source, GuidanceSource::Convention);
    }

    /// A configured pattern must not be able to name a file outside the checkout.
    #[test]
    fn guidance_outside_the_repository_is_refused() {
        let repository = TempDir::new().unwrap();
        let mut guidance = Guidance::default();
        let mut seen = BTreeSet::new();

        consider(
            repository.path(),
            "../outside.md",
            GuidanceScope::Repository,
            GuidanceSource::Configured,
            None,
            &mut seen,
            &mut guidance,
        );

        assert!(guidance.files().is_empty());
        assert_eq!(guidance.skipped()[0].reason, SkipReason::OutsideRepository);
    }

    #[test]
    fn an_invalid_configured_pattern_is_reported() {
        let repository = TempDir::new().unwrap();
        write(
            repository.path(),
            ".zreview.toml",
            "[review]\ninstructions = [\"[unclosed\"]\n",
        );

        let guidance = discover(repository.path(), &[]);

        assert!(matches!(
            guidance.skipped().first().map(|skip| &skip.reason),
            Some(SkipReason::InvalidPattern(_)),
        ));
    }

    #[test]
    fn guidance_can_be_turned_off_and_stops_being_sent() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "AGENTS.md", "agent rules");
        write(repository.path(), "CLAUDE.md", "claude rules");
        let mut guidance = discover(repository.path(), &[]);

        assert_eq!(guidance.included().count(), 2);
        assert!(guidance.set_included("CLAUDE.md", false));
        assert_eq!(guidance.included().count(), 1);
        assert_eq!(guidance.included_bytes(), 11);
        assert!(!guidance.set_included("ABSENT.md", false));
    }

    #[test]
    fn guidance_for_a_reviewed_path_is_only_what_applies() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "AGENTS.md", "root");
        write(repository.path(), "lib/web/AGENTS.md", "web");
        write(
            repository.path(),
            ".github/instructions/tests.instructions.md",
            "---\napplyTo: \"test/**\"\n---\ntest rules",
        );

        let guidance = discover(
            repository.path(),
            &["lib/web/page.ex", "test/page_test.exs"],
        );

        let for_web: Vec<_> = guidance
            .for_reviewed_path("lib/web/page.ex")
            .map(|file| file.path.to_string())
            .collect();
        assert_eq!(for_web, ["AGENTS.md", "lib/web/AGENTS.md"]);

        let for_test: Vec<_> = guidance
            .for_reviewed_path("test/page_test.exs")
            .map(|file| file.path.to_string())
            .collect();
        assert_eq!(
            for_test,
            ["AGENTS.md", ".github/instructions/tests.instructions.md"],
        );
    }

    /// Discovery reads files and nothing else. A repository cannot get anything
    /// executed by being opened.
    #[test]
    fn discovery_does_not_execute_anything() {
        let repository = TempDir::new().unwrap();
        let marker = repository.path().join("executed");
        write(
            repository.path(),
            "AGENTS.md",
            &format!("$(touch {})", marker.display()),
        );
        write(
            repository.path(),
            ".zreview.toml",
            &format!(
                "[review]\ninstructions = [\"$(touch {}).md\"]\n",
                marker.display()
            ),
        );

        let guidance = discover(repository.path(), &["src/lib.rs"]);

        assert!(!marker.exists(), "nothing may be executed by discovery");
        assert_eq!(
            guidance.files()[0].content,
            format!("$(touch {})", marker.display())
        );
    }
}
