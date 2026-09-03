//! A stand-in for `gh`, so the commands that call it can be driven in a test.
//!
//! Answers `auth status` and `api graphql` from files in its own directory,
//! which is what lets one test refuse authentication and another hand back a
//! recorded search result.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use github::GithubClient;
use tempfile::TempDir;

pub(crate) struct FakeGh {
    directory: TempDir,
}

impl FakeGh {
    /// A `gh` that is signed in and has no answer for any query yet.
    pub(crate) fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let script = format!(
            r#"#!/bin/sh
dir="{dir}"
if [ "$1" = "auth" ]; then
  if [ -f "$dir/refuse-auth" ]; then
    echo "You are not logged into any GitHub hosts. Run gh auth login." >&2
    exit 1
  fi
  exit 0
fi
if [ -f "$dir/graphql" ]; then
  cat "$dir/graphql"
  exit 0
fi
echo "gh: no recorded response" >&2
exit 1
"#,
            dir = directory.path().display(),
        );
        let path = directory.path().join("gh");
        fs::write(&path, script).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();

        Self { directory }
    }

    pub(crate) fn client(&self) -> GithubClient {
        GithubClient::new(self.path())
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.directory.path().join("gh")
    }

    /// Makes `gh auth status` report no signed in account.
    pub(crate) fn refuse_authentication(&self) -> &Self {
        fs::write(self.directory.path().join("refuse-auth"), "").unwrap();
        self
    }

    /// Signs `gh` back in, which is what a reviewer does between two refreshes.
    pub(crate) fn allow_authentication(&self) -> &Self {
        fs::remove_file(self.directory.path().join("refuse-auth")).unwrap();
        self
    }

    /// Answers every GraphQL call with `body`.
    pub(crate) fn answer_graphql(&self, body: &str) -> &Self {
        fs::write(self.directory.path().join("graphql"), body).unwrap();
        self
    }
}
