# Guidance discovery

## Goal

Find the guidance a repository already carries about how it wants to be reviewed,
so a review can be held to the project's own standards rather than to generic
ones.

This is the first half of Phase 4, and the half that does not depend on which
review engine is chosen — PLAN section 13 leaves that open, and none of this
requires the answer.

## What it finds

- root conventions: `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md`, `STYLEGUIDE.md`,
  `STYLE_GUIDE.md`, and `.github/copilot-instructions.md`, scoped to the whole
  repository;
- nested `AGENTS.md` and `CLAUDE.md` in any directory containing a changed file,
  or an ancestor of one, scoped to that directory. A nested `CONTRIBUTING.md` is
  deliberately not treated this way: it is usually about contributing to the
  project rather than about the code beside it;
- `.github/instructions/*.instructions.md`, scoped by the globs in their `applyTo`
  header. A file with no header applies to the whole repository, because declaring
  no scope should not mean applying nowhere;
- anything named by `instructions` in `.zreview.toml`.

`.zreview.toml` also drops conventional guidance through `exclude_instructions`,
and excludes files from review entirely through `exclude_files`.

Guidance beside code that is not under review is not read at all: only directories
containing a changed file, and their ancestors, are searched.

## The two properties it is built around

**Read-only.** Nothing here executes anything, and there is no code path from this
crate to a subprocess. Discovering a repository's guidance is not consent to run
its commands, which PLAN states directly and which matters because the content
being read is untrusted. A test asserts the negative: a repository whose guidance
and configuration both contain shell substitutions gets nothing executed, and the
substitution is read as literal text.

**Transparent.** Every file found is reported with where it came from and what it
applies to. Every candidate *skipped* is reported with why — too large, excluded by
configuration, unreadable, over the total budget, an invalid pattern, or outside
the repository. Silently dropping guidance would be worse than not finding it,
because the reviewer is about to decide whether to send this to a model.

Each file carries a SHA-256 of its content, so a review run can record exactly
what it was given and a later run can tell whether it changed.

## Limits and safety

A single file over 64 KiB and a total over 256 KiB are refused, since something
larger is nearly always generated and would crowd the diff out of a model's
context. Both are reported rather than silently truncated.

A guidance path that climbs out of the checkout is refused: otherwise a repository
could name any file on the machine as review context. The configured-glob walk
skips `.git`, `node_modules`, `target`, `_build` and `deps`, is bounded by depth,
and never follows a symbolic link.

## Tests

Conventional files at the root, an empty repository finding nothing *and*
reporting nothing, content read and hashed stably, nested scoping including
ancestors and untouched directories being left alone, `applyTo` parsing and the
headerless default, configuration adding by glob and excluding by name, reviewed-
file exclusions, malformed configuration being reported rather than fatal, the
per-file and total limits, deduplication, refusal of a path outside the
repository, an invalid pattern, toggling a file off, per-reviewed-path resolution,
and that discovery executes nothing.

Run against a real private repository: found its `AGENTS.md` and `CLAUDE.md`
(24.8 KB and 15.8 KB), both repository-scoped, both applying to each of four
changed files across `lib/`, `test/` and `config/`.

## Current limitations

- No user-level guidance from the platform config directory yet, which PLAN also
  lists.
- `applyTo` parsing handles the documented shape — a `---` delimited header with a
  quoted, comma-separated value — rather than being a general YAML parser.
- Nothing is wired into the app yet: there is no Guidance panel, so discovery runs
  nowhere. That is the next commit.
- `exclude_files` is discovered but not yet applied to the reviewed file list.
- Deterministic checks from `.zreview.toml` `[[review.checks]]` are not read. They
  are the part that needs the trust decision PLAN requires before anything is
  executed, and nothing here can execute anyway.

## Next step

The Guidance panel: show every discovered file, its scope, its size, and whether
it will be sent, with the ability to turn any of it off before a review runs. Then
the backend interface — which needs PLAN section 13 settled.
