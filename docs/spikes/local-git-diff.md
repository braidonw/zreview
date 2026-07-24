# Local Git diff parser

## Goal

Load a complete reviewable comparison from local Git objects and map every rendered row to stable old/new line coordinates without relying on GitHub patch responses.

## Implementation

`crates/git` now:

- resolves repository roots and full 40-character commit IDs;
- supports direct (`base..head`) and merge-base (`base...head`) comparisons;
- invokes `git` with argument arrays rather than a shell;
- disables external diff drivers, colors, and text-conversion filters;
- reads NUL-delimited file status/path output;
- detects additions, deletions, modifications, renames, copies, type changes, and binary files;
- uses literal pathspecs for names that Git could otherwise interpret as pathspec magic;
- parses multiple unified hunks into flattened `DiffLine` rows;
- assigns old/right line coordinates to context, deletion, and addition rows;
- preserves missing-final-newline markers as typed rows;
- validates hunk line counts before returning a file;
- rejects absolute, parent-traversing, malformed, and non-UTF-8 repository paths.

The app accepts a repository and revisions and renders the complete changed-file session:

```bash
cargo run -p zreview -- /path/to/repository main
cargo run -p zreview -- /path/to/repository base-branch feature-branch
```

## Tests

Golden parser tests cover multiple hunks, line coordinates, missing-final-newline markers, malformed hunk counts, and NUL-delimited rename records.

An integration test creates a real temporary repository containing a modified rename, deletion, added text file, and binary file, commits both snapshots, and verifies the parsed comparison.

A launch smoke test also loaded and displayed a real temporary repository comparison successfully.

## Current limitations

- Files are loaded with one Git patch subprocess per changed path. This favors correctness for the first implementation but should be batched or concurrency-bounded before supporting very large PRs.
- Domain paths are UTF-8 strings. Git permits arbitrary path bytes, although GitHub review paths are expected to be UTF-8-compatible.
- Combined merge diffs and working-tree changes are not part of this comparison API.
- Binary files are identified but not rendered.

## Next step

Implement the GitHub/`gh` metadata and PR-ref loading layer.
