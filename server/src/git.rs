//! Thin wrappers around the `git` CLI.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// A file inside a git repository.
pub struct RepoFile {
    pub root: PathBuf,
    /// Path relative to `root`, with `/` separators.
    pub relative: String,
}

impl RepoFile {
    pub fn locate(path: &Path) -> Result<Self> {
        let dir = path.parent().context("file has no parent directory")?;
        let root = git(dir, &["rev-parse", "--show-toplevel"])?;
        let root = PathBuf::from(root.trim());
        // Canonicalize both sides so symlinked temp dirs (e.g. /tmp on macOS) match.
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let relative = canonical_path
            .strip_prefix(&canonical_root)
            .with_context(|| format!("{} is outside {}", path.display(), root.display()))?
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        Ok(Self { root, relative })
    }

    /// `git log -L` for 1-based, inclusive lines `start..=end`.
    pub fn line_history(&self, start: u32, end: u32) -> Result<String> {
        let range = format!("-L{start},{end}:{}", self.relative);
        git(
            &self.root,
            &[
                "log",
                "--no-color",
                "--date=format:%Y-%m-%d %H:%M",
                "--format=commit %H%nAuthor: %an <%ae>%nDate:   %ad%n%n    %s%n",
                &range,
            ],
        )
    }

    /// `git show` of one commit, limited to this file.
    pub fn show_commit(&self, sha: &str) -> Result<String> {
        git(
            &self.root,
            &[
                "show",
                "--no-color",
                "--date=format:%Y-%m-%d %H:%M",
                sha,
                "--",
                &self.relative,
            ],
        )
    }

    pub fn summary(&self) -> Result<Option<FileSummary>> {
        let last = git(
            &self.root,
            &[
                "log",
                "-1",
                "--format=%H%x00%an%x00%ar",
                "--",
                &self.relative,
            ],
        )?;
        let mut fields = last.trim().split('\0');
        let (Some(sha), Some(author), Some(when)) = (fields.next(), fields.next(), fields.next())
        else {
            return Ok(None); // Untracked, or no commits yet.
        };
        let authors = git(
            &self.root,
            &["shortlog", "-sn", "HEAD", "--", &self.relative],
        )?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
        Ok(Some(FileSummary {
            last_sha: sha.to_string(),
            last_author: author.to_string(),
            last_when: when.to_string(),
            authors,
        }))
    }
}

pub struct FileSummary {
    pub last_sha: String,
    pub last_author: String,
    pub last_when: String,
    pub authors: usize,
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_PAGER", "cat")
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Ada")
            .env("GIT_AUTHOR_EMAIL", "ada@example.com")
            .env("GIT_COMMITTER_NAME", "Ada")
            .env("GIT_COMMITTER_EMAIL", "ada@example.com")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn history_and_summary() {
        let repo = tempfile::tempdir().unwrap();
        let dir = repo.path();
        run(dir, &["init", "-q"]);
        fs::create_dir(dir.join("src")).unwrap();
        let file = dir.join("src/lib.rs");
        fs::write(&file, "fn a() {}\nfn b() {}\n").unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "-qm", "add a and b"]);
        fs::write(&file, "fn a() {}\nfn b() { todo!() }\n").unwrap();
        run(dir, &["commit", "-qam", "fill in b"]);

        let repo_file = RepoFile::locate(&file).unwrap();
        assert_eq!(repo_file.relative, "src/lib.rs");

        let line_two = repo_file.line_history(2, 2).unwrap();
        assert!(line_two.contains("fill in b"));
        assert!(line_two.contains("add a and b"));
        let line_one = repo_file.line_history(1, 1).unwrap();
        assert!(!line_one.contains("fill in b"));

        let summary = repo_file.summary().unwrap().unwrap();
        assert_eq!(summary.last_author, "Ada");
        assert_eq!(summary.authors, 1);
        assert!(repo_file
            .show_commit(&summary.last_sha)
            .unwrap()
            .contains("todo!()"));
    }
}
