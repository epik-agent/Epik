//! Removing builds' clones, which are scratch space.
//!
//! Everything a build produces is on GitHub or in its record under
//! `~/.epik/runs`, so a clone under `~/.epik/builds` is removed when its
//! build ends, together with the git worktrees its agents made, wherever
//! they are. Clones left by an app that crashed are swept away when the
//! next one starts.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The directory holding every build's clone, `<root>/builds`.
pub fn builds_dir(root: &Path) -> PathBuf {
    root.join("builds")
}

/// Removes every git worktree registered to the clone at `clone`, then the
/// clone itself. Nothing is written outside the clone and the worktrees it
/// lists. A clone that isn't a git repository, or is already gone, is
/// simply removed.
pub fn remove_clone(clone: &Path) -> io::Result<()> {
    if clone.join(".git").exists() {
        for worktree in worktrees(clone)? {
            git(clone, &["worktree", "remove", "--force"], Some(&worktree))?;
        }
    }
    match fs::remove_dir_all(clone) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

/// Removes everything under `<root>/builds`. Called when the app starts,
/// before any build can run, so whatever is there is left over.
pub fn sweep_builds(root: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(builds_dir(root)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            remove_clone(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// The worktrees registered to `clone`, other than the clone itself, from
/// `git worktree list --porcelain`.
fn worktrees(clone: &Path) -> io::Result<Vec<PathBuf>> {
    let listing = git(clone, &["worktree", "list", "--porcelain"], None)?;
    Ok(parse_worktrees(&listing))
}

/// The paths in `git worktree list --porcelain`'s output, without the
/// first, which is the main worktree.
fn parse_worktrees(listing: &str) -> Vec<PathBuf> {
    listing
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .skip(1)
        .map(PathBuf::from)
        .collect()
}

/// Runs `git -C <clone> <args> [path]` and returns its stdout.
fn git(clone: &Path, args: &[&str], path: Option<&Path>) -> io::Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(clone).args(args);
    if let Some(path) = path {
        command.arg(path);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Runs git in `dir`, panicking if it fails.
    fn run(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }

    /// A git repository at `dir` with one commit.
    pub(crate) fn repository(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        run(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("README.md"), "hello\n").unwrap();
        run(dir, &["add", "README.md"]);
        run(
            dir,
            &[
                "-c",
                "user.name=Epik",
                "-c",
                "user.email=epik@example.com",
                "commit",
                "-q",
                "-m",
                "first",
            ],
        );
    }

    #[test]
    fn a_clone_and_all_its_worktrees_are_removed() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let clone = root.path().join("builds").join("run");
        repository(&clone);
        let inside = clone.join(".claude").join("worktrees").join("issue-2");
        let outside = elsewhere.path().join("issue-3");
        for (worktree, branch) in [(&inside, "issue-2"), (&outside, "issue-3")] {
            let path = worktree.to_str().unwrap();
            run(&clone, &["worktree", "add", "-q", "-b", branch, path]);
            // Work an agent left uncommitted.
            fs::write(worktree.join("scratch.txt"), "draft\n").unwrap();
        }
        let neighbor = elsewhere.path().join("keep.txt");
        fs::write(&neighbor, "mine\n").unwrap();

        remove_clone(&clone).unwrap();

        assert!(!clone.exists());
        assert!(!inside.exists());
        assert!(!outside.exists());
        assert!(neighbor.exists(), "nothing else is touched");
    }

    #[test]
    fn a_clone_that_isnt_a_repository_or_is_gone_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let clone = root.path().join("plain");
        fs::create_dir_all(clone.join("sub")).unwrap();

        remove_clone(&clone).unwrap();
        remove_clone(&clone).unwrap();

        assert!(!clone.exists());
    }

    #[test]
    fn the_sweep_removes_every_leftover_build() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds_dir(root.path());
        repository(&builds.join("crashed"));
        fs::create_dir_all(builds.join("older").join("deep")).unwrap();
        fs::write(builds.join("stray-file"), "").unwrap();
        let record = root.path().join("runs").join("crashed");
        fs::create_dir_all(&record).unwrap();

        sweep_builds(root.path()).unwrap();

        assert_eq!(fs::read_dir(&builds).unwrap().count(), 0);
        assert!(record.exists(), "records are kept");
    }

    #[test]
    fn the_sweep_of_a_root_without_builds_does_nothing() {
        let root = tempfile::tempdir().unwrap();

        sweep_builds(root.path()).unwrap();
    }

    #[test]
    fn the_main_worktree_is_not_listed() {
        let listing = "worktree /c\nHEAD abc\nbranch refs/heads/main\n\n\
                       worktree /c/w\nHEAD def\nbranch refs/heads/w\n\n\
                       worktree /elsewhere\nHEAD 123\ndetached\nlocked\n";

        assert_eq!(
            parse_worktrees(listing),
            [PathBuf::from("/c/w"), PathBuf::from("/elsewhere")]
        );
    }
}
