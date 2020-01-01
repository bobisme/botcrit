//! Version control system integration for fetching diffs.
//!
//! Supports both jj (Jujutsu) and git repositories.

use std::path::Path;
use std::process::Command;

use crate::diff::ParsedDiff;

/// Detected VCS type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsType {
    Jj,
    Git,
}

/// Detect the VCS type for a directory
#[must_use]
pub fn detect_vcs(path: &Path) -> Option<VcsType> {
    // Check for jj first (it can coexist with git)
    if path.join(".jj").exists() {
        return Some(VcsType::Jj);
    }
    if path.join(".git").exists() {
        return Some(VcsType::Git);
    }
    None
}

/// Get the diff for a specific file between two commits.
///
/// If `to_commit` is None, diffs against the working copy.
#[must_use]
pub fn get_file_diff(
    repo_path: &Path,
    file_path: &str,
    from_commit: &str,
    to_commit: Option<&str>,
) -> Option<ParsedDiff> {
    let vcs = detect_vcs(repo_path)?;

    let output = match vcs {
        VcsType::Jj => get_jj_diff(repo_path, file_path, from_commit, to_commit),
        VcsType::Git => get_git_diff(repo_path, file_path, from_commit, to_commit),
    };

    output.map(|diff_text| ParsedDiff::parse(&diff_text))
}

/// Get diff using jj
fn get_jj_diff(
    repo_path: &Path,
    file_path: &str,
    from_commit: &str,
    to_commit: Option<&str>,
) -> Option<String> {
    let mut cmd = Command::new("jj");
    cmd.current_dir(repo_path);
    cmd.arg("diff");
    cmd.arg("--git"); // Use git diff format

    // jj diff --from <commit> --to <commit> <file>
    cmd.arg("--from").arg(from_commit);

    if let Some(to) = to_commit {
        cmd.arg("--to").arg(to);
    }

    cmd.arg(file_path);

    let output = cmd.output().ok()?;

    if output.status.success() {
        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        if diff.trim().is_empty() {
            None
        } else {
            Some(diff)
        }
    } else {
        None
    }
}

/// Get diff using git
fn get_git_diff(
    repo_path: &Path,
    file_path: &str,
    from_commit: &str,
    to_commit: Option<&str>,
) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo_path);
    cmd.arg("diff");

    // git diff <from>..<to> -- <file>
    // or git diff <from> -- <file> (for working copy)
    if let Some(to) = to_commit {
        cmd.arg(format!("{from_commit}..{to}"));
    } else {
        cmd.arg(from_commit);
    }

    cmd.arg("--").arg(file_path);

    let output = cmd.output().ok()?;

    if output.status.success() {
        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        if diff.trim().is_empty() {
            None
        } else {
            Some(diff)
        }
    } else {
        None
    }
}

/// Get file content at a specific commit.
///
/// Returns the file content as a vector of lines.
pub fn get_file_content(repo_path: &Path, file_path: &str, commit: &str) -> Option<Vec<String>> {
    let vcs = detect_vcs(repo_path)?;

    let output = match vcs {
        VcsType::Jj => {
            // jj file show <file> -r <commit>
            let mut cmd = Command::new("jj");
            cmd.current_dir(repo_path);
            cmd.arg("file").arg("show").arg(file_path);
            cmd.arg("-r").arg(commit);
            cmd.output().ok()?
        }
        VcsType::Git => {
            // git show <commit>:<file>
            let mut cmd = Command::new("git");
            cmd.current_dir(repo_path);
            cmd.arg("show").arg(format!("{commit}:{file_path}"));
            cmd.output().ok()?
        }
    };

    if output.status.success() {
        let content = String::from_utf8_lossy(&output.stdout);
        Some(content.lines().map(String::from).collect())
    } else {
        None
    }
}

/// Get the full diff for all files between two commits.
#[must_use]
pub fn get_full_diff(
    repo_path: &Path,
    from_commit: &str,
    to_commit: Option<&str>,
) -> Option<String> {
    let vcs = detect_vcs(repo_path)?;

    match vcs {
        VcsType::Jj => {
            let mut cmd = Command::new("jj");
            cmd.current_dir(repo_path);
            cmd.arg("diff").arg("--git");
            cmd.arg("--from").arg(from_commit);
            if let Some(to) = to_commit {
                cmd.arg("--to").arg(to);
            }

            let output = cmd.output().ok()?;
            if output.status.success() {
                Some(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                None
            }
        }
        VcsType::Git => {
            let mut cmd = Command::new("git");
            cmd.current_dir(repo_path);
            cmd.arg("diff");

            if let Some(to) = to_commit {
                cmd.arg(format!("{from_commit}..{to}"));
            } else {
                cmd.arg(from_commit);
            }

            let output = cmd.output().ok()?;
            if output.status.success() {
                Some(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_vcs_none() {
        let temp = std::env::temp_dir();
        // Temp dir likely has neither .jj nor .git
        // This test just verifies the function doesn't panic
        let _ = detect_vcs(&temp);
    }
}
