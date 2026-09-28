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

    /// History of 1-based, inclusive lines `start..=end` (`git log -L`) as
    /// Markdown: one `##` heading per commit, so Zed's Outline panel lists the
    /// commits in the sidebar, each followed by its diff.
    pub fn line_history(&self, start: u32, end: u32) -> Result<String> {
        let range = format!("-L{start},{end}:{}", self.relative);
        let log = git(
            &self.root,
            &[
                "log",
                "--no-color",
                "--format=%x1e%h%x1f%an%x1f%ar%x1f%ad%x1f%s",
                "--date=format:%Y-%m-%d %H:%M",
                &range,
            ],
        )?;

        let mut markdown = format!("# Line history: {} L{start}-{end}\n", self.relative);
        let mut commits = 0;
        for entry in log.split('\x1e').filter(|entry| !entry.trim().is_empty()) {
            let (header, diff) = entry.split_once('\n').unwrap_or((entry, ""));
            let fields: Vec<&str> = header.split('\x1f').collect();
            let [sha, author, relative_date, date, subject] = fields[..] else {
                continue;
            };
            commits += 1;
            markdown.push_str(&format!(
                "\n## {sha} · {author} · {relative_date} · {subject}\n\n{date}\n\n````diff\n{}\n````\n",
                diff.trim()
            ));
        }
        if commits == 0 {
            markdown.push_str("\nNo commits touch these lines yet.\n");
        }
        Ok(markdown)
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
        assert!(line_two.starts_with("# Line history: src/lib.rs L2-2\n"));
        let headings: Vec<&str> = line_two.lines().filter(|l| l.starts_with("## ")).collect();
        assert_eq!(headings.len(), 2, "{line_two}");
        assert!(headings[0].contains("· Ada · ") && headings[0].ends_with("· fill in b"));
        assert!(headings[1].ends_with("· add a and b"));
        assert!(line_two.contains("````diff\n"));
        assert!(line_two.contains("+fn b() { todo!() }"));
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
