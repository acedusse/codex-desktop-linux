//! Wrapper-repo update detection.
//!
//! Beyond tracking the upstream Codex DMG, the updater can detect when the
//! *wrapper* itself (this repository — new Linux features, patches, fixes) has
//! advanced. Detection is git-based and strictly read-only: it inspects the
//! builder bundle checkout and queries the remote head with `git ls-remote`,
//! and never mutates the user's working tree. The actual rebuild reuses the
//! existing DMG rebuild path against the refreshed checkout.
//!
//! When the builder bundle is a frozen packaged copy (no `.git`), the wrapper
//! axis degrades gracefully: detection reports "not a git checkout" and the
//! caller leaves wrapper updates to a normal package upgrade.

use anyhow::Result;
use std::{path::Path, process::Command};

use crate::changelog;

/// Identity of a wrapper checkout: its current commit and best-effort version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapperVersion {
    /// Full commit SHA of the checkout's `HEAD`.
    pub commit: String,
    /// Semver read from `updater/Cargo.toml`, when available.
    pub version: Option<String>,
}

/// Result of comparing the local checkout against the remote head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapperUpdate {
    pub installed_commit: String,
    pub installed_version: Option<String>,
    pub candidate_commit: String,
    pub candidate_version: Option<String>,
    /// Curated CHANGELOG sections newer than installed, or a git commit-subject
    /// list when the changelog can't be mapped.
    pub changelog: String,
}

/// Runs a read-only git command in `repo`, returning trimmed stdout on success.
fn git_capture(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// True when `repo` is a git working tree.
pub fn is_git_checkout(repo: &Path) -> bool {
    git_capture(repo, &["rev-parse", "--is-inside-work-tree"])
        .map(|value| value == "true")
        .unwrap_or(false)
}

/// Reads the `version = "x.y.z"` value from `updater/Cargo.toml` in the
/// checkout. Best-effort: returns `None` when the file or field is missing.
fn read_wrapper_version(repo: &Path) -> Option<String> {
    let cargo_toml = repo.join("updater").join("Cargo.toml");
    let content = std::fs::read_to_string(cargo_toml).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("version") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Resolves the installed wrapper identity from a checkout.
pub fn installed_wrapper(repo: &Path) -> Option<WrapperVersion> {
    let commit = git_capture(repo, &["rev-parse", "HEAD"])?;
    Some(WrapperVersion {
        commit,
        version: read_wrapper_version(repo),
    })
}

/// Resolves the wrapper repo origin URL from the checkout.
fn origin_url(repo: &Path) -> Option<String> {
    git_capture(repo, &["remote", "get-url", "origin"])
}

/// Queries the remote head commit for `branch` via `git ls-remote`.
///
/// `remote` may be a configured remote name (`origin`) or an explicit URL. When
/// no remote is configured this falls back to the checkout's origin URL.
pub fn fetch_remote_head(repo: &Path, remote: &str, branch: &str) -> Option<String> {
    let resolved_remote = if remote.is_empty() {
        origin_url(repo)?
    } else {
        remote.to_string()
    };
    let output = git_capture(repo, &["ls-remote", &resolved_remote, branch])?;
    // ls-remote prints "<sha>\t<ref>"; take the first whitespace-delimited field.
    output
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_string)
}

/// Fetches the candidate branch into the local object store WITHOUT touching
/// the working tree or current branch. This makes the candidate commit and its
/// `CHANGELOG.md` blob available to `git show` / `git log`. Read-only with
/// respect to the user's checked-out files.
fn fetch_objects(repo: &Path, remote: &str, branch: &str) {
    let resolved_remote = if remote.is_empty() {
        match origin_url(repo) {
            Some(url) => url,
            None => return,
        }
    } else {
        remote.to_string()
    };
    // `git fetch <remote> <branch>` updates FETCH_HEAD and objects only.
    let _ = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["fetch", "--quiet", &resolved_remote, branch])
        .output();
}

/// Reads `CHANGELOG.md` at a specific commit from the object store (the
/// candidate's changelog, which reflects the new version's entries).
fn changelog_at_commit(repo: &Path, commit: &str) -> Option<String> {
    git_capture(repo, &["show", &format!("{commit}:CHANGELOG.md")])
}

/// Builds the "what changed" text for an update. Prefers curated CHANGELOG.md
/// sections (from the candidate commit) newer than `installed_version`; falls
/// back to `git log --oneline installed..candidate` commit subjects when the
/// changelog can't be mapped.
fn build_changelog(
    repo: &Path,
    installed_version: Option<&str>,
    installed_commit: &str,
    candidate_commit: &str,
) -> String {
    if let (Some(version), Some(markdown)) = (
        installed_version,
        changelog_at_commit(repo, candidate_commit),
    ) {
        let sections = changelog::parse_changelog(&markdown);
        if let Some(text) = changelog::sections_newer_than(&sections, version) {
            return text;
        }
    }

    // Fallback: raw commit subjects between the two commits.
    let range = format!("{installed_commit}..{candidate_commit}");
    if let Some(log) = git_capture(repo, &["log", "--oneline", "--no-decorate", &range]) {
        if !log.is_empty() {
            return log;
        }
    }

    "Wrapper updated (no changelog details available).".to_string()
}

/// Detects whether the wrapper repo at `repo` has a newer head than the local
/// checkout. Returns `Ok(None)` when up to date, when `repo` is not a git
/// checkout (packaged frozen bundle), or when the remote can't be reached.
/// Never mutates the working tree.
pub fn detect_wrapper_update(
    repo: &Path,
    remote: &str,
    branch: &str,
) -> Result<Option<WrapperUpdate>> {
    if !is_git_checkout(repo) {
        return Ok(None);
    }

    let Some(installed) = installed_wrapper(repo) else {
        return Ok(None);
    };
    let Some(candidate_commit) = fetch_remote_head(repo, remote, branch) else {
        return Ok(None);
    };

    if candidate_commit == installed.commit {
        return Ok(None);
    }

    // Bring the candidate commit + its CHANGELOG blob into the local object
    // store so the changelog can be read. Does not touch the working tree.
    fetch_objects(repo, remote, branch);

    let changelog = build_changelog(
        repo,
        installed.version.as_deref(),
        &installed.commit,
        &candidate_commit,
    );

    Ok(Some(WrapperUpdate {
        installed_commit: installed.commit,
        installed_version: installed.version,
        candidate_commit,
        candidate_version: None,
        changelog,
    }))
}

/// Convenience for callers that hold a `builder_bundle_root` path.
pub fn detect_from_bundle_root(
    bundle_root: &Path,
    remote: &str,
    branch: &str,
) -> Result<Option<WrapperUpdate>> {
    detect_wrapper_update(bundle_root, remote, branch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    use crate::test_util::env_lock;
    use std::path::PathBuf;

    // Resolve git to an absolute path so these tests don't depend on $PATH,
    // which other tests in the binary mutate concurrently.
    fn git_bin() -> PathBuf {
        if let Some(explicit) = std::env::var_os("GIT") {
            return PathBuf::from(explicit);
        }
        for candidate in ["/usr/bin/git", "/bin/git", "/usr/local/bin/git"] {
            if Path::new(candidate).exists() {
                return PathBuf::from(candidate);
            }
        }
        PathBuf::from("git")
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new(git_bin())
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_clone(origin: &Path, dest: &Path) {
        let status = Command::new(git_bin())
            .args(["clone", "-q"])
            .arg(origin)
            .arg(dest)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git clone");
        assert!(status.status.success(), "git clone failed");
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(dir.join("updater")).unwrap();
        std::fs::write(
            dir.join("updater/Cargo.toml"),
            "[package]\nname = \"codex-update-manager\"\nversion = \"0.8.1\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("CHANGELOG.md"), "# Changelog\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn non_git_dir_reports_no_update() {
        let temp = tempdir().unwrap();
        assert!(!is_git_checkout(temp.path()));
        assert_eq!(
            detect_wrapper_update(temp.path(), "origin", "main").unwrap(),
            None
        );
    }

    #[test]
    fn reads_installed_commit_and_version() {
        let _g = env_lock();
        let temp = tempdir().unwrap();
        init_repo(temp.path());
        let installed = installed_wrapper(temp.path()).expect("installed");
        assert_eq!(installed.version.as_deref(), Some("0.8.1"));
        assert_eq!(installed.commit.len(), 40);
    }

    #[test]
    fn detects_newer_head_against_local_remote() {
        let _g = env_lock();
        // origin repo
        let origin = tempdir().unwrap();
        init_repo(origin.path());

        // clone it
        let clone = tempdir().unwrap();
        let clone_path = clone.path().join("checkout");
        git_clone(origin.path(), &clone_path);

        // advance origin with a changelog bump
        std::fs::write(
            origin.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [0.9.0] - 2026-06-01\n\n### Added\n\n- New wrapper feature.\n",
        )
        .unwrap();
        git(origin.path(), &["add", "-A"]);
        git(origin.path(), &["commit", "-q", "-m", "bump"]);

        // clone still on old head; detect should find the new origin head
        let update = detect_wrapper_update(&clone_path, "origin", "main")
            .unwrap()
            .expect("update detected");
        assert_ne!(update.installed_commit, update.candidate_commit);
        assert_eq!(update.installed_version.as_deref(), Some("0.8.1"));
        // The candidate commit's CHANGELOG has a [0.9.0] section, newer than the
        // installed 0.8.1, so the curated changelog is surfaced.
        assert!(
            update.changelog.contains("New wrapper feature."),
            "changelog was: {}",
            update.changelog
        );
    }

    #[test]
    fn up_to_date_clone_reports_no_update() {
        let _g = env_lock();
        let origin = tempdir().unwrap();
        init_repo(origin.path());
        let clone = tempdir().unwrap();
        let clone_path = clone.path().join("checkout");
        git_clone(origin.path(), &clone_path);
        assert_eq!(
            detect_wrapper_update(&clone_path, "origin", "main").unwrap(),
            None
        );
    }
}
