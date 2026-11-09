//! The link at `~/.epik/bin/epik-app` to the running Epik app.
//!
//! Each platform's installer puts the app somewhere else, so programs that
//! need it, chiefly Claude Code running the plugin's MCP server, find it at
//! one fixed path instead. The app refreshes the link every time it starts:
//! a symbolic link on macOS and Linux; on Windows, where symbolic links
//! need privileges, a copy. Not a hard link: Claude Code runs the link, and
//! a running hard link to the installed app would keep its installer from
//! replacing it. An app running from a build's clone leaves the link alone:
//! clones are temporary, so the link must never lead into one.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cleanup::builds_dir;

/// The link's file name.
pub const NAME: &str = if cfg!(windows) {
    "epik-app.exe"
} else {
    "epik-app"
};

/// Where the link is, `<root>/bin/epik-app` (`epik-app.exe` on Windows).
pub fn path(root: &Path) -> PathBuf {
    root.join("bin").join(NAME)
}

/// What [`refresh`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refreshed {
    /// The link already led to the app.
    Kept,
    /// There was no link, so one was made.
    Made,
    /// The link led elsewhere, or was a stale copy, so it was replaced.
    Replaced,
    /// The app runs from a build's clone, so the link was left as it was.
    FromClone,
}

/// Makes `<root>/bin/epik-app` lead to `exe`, the running app, leaving a
/// link that already does, and leaving the link whatever it is if `exe` is
/// in a build's clone.
pub fn refresh(root: &Path, exe: &Path) -> io::Result<Refreshed> {
    let link = path(root);
    // An app started through the link may be told it is the link. Its
    // real file is what the link must lead to, never the link itself.
    let exe = &real_path(exe);
    if in_a_clone(root, exe) {
        return Ok(Refreshed::FromClone);
    }
    if fs::canonicalize(&link).is_ok_and(|resolved| &resolved == exe) {
        return Ok(Refreshed::Kept);
    }
    fs::create_dir_all(link.parent().expect("the link is in a directory"))?;
    let existed = fs::symlink_metadata(&link).is_ok();
    if existed {
        if leads_to(&link, exe)? {
            return Ok(Refreshed::Kept);
        }
        retire(&link)?;
    }
    make(&link, exe)?;
    Ok(if existed {
        Refreshed::Replaced
    } else {
        Refreshed::Made
    })
}

/// The file `exe` really is, following the link and any other symbolic
/// links, or `exe` itself if it can't be resolved. An app started through
/// the link is told it is the link; macOS finds an app's bundle, and so its
/// name, only from the executable's real path.
pub fn real_path(exe: &Path) -> PathBuf {
    fs::canonicalize(exe).unwrap_or_else(|_| exe.to_owned())
}

/// Whether `exe` is in one of the build clones under `root`.
pub fn in_a_clone(root: &Path, exe: &Path) -> bool {
    let exe = real_path(exe);
    fs::canonicalize(builds_dir(root)).is_ok_and(|builds| exe.starts_with(builds))
}

/// Moves an old link out of the way. On Windows a running program can't be
/// deleted, as the link is while `epik-app mcp` runs, but it can be
/// renamed; the renamed copies are deleted once nothing runs them.
fn retire(link: &Path) -> io::Result<()> {
    if cfg!(windows) {
        let dir = link.parent().expect("the link is in a directory");
        for entry in fs::read_dir(dir)?.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("epik-app.old-")
            {
                let _ = fs::remove_file(entry.path());
            }
        }
        let aside = dir.join(format!("epik-app.old-{}.exe", std::process::id()));
        fs::rename(link, &aside)?;
        let _ = fs::remove_file(aside);
        Ok(())
    } else {
        fs::remove_file(link)
    }
}

#[cfg(unix)]
fn leads_to(link: &Path, exe: &Path) -> io::Result<bool> {
    Ok(fs::read_link(link).is_ok_and(|target| target == exe))
}

#[cfg(unix)]
fn make(link: &Path, exe: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(exe, link)
}

/// Whether the link is the app: the same bytes, as a fresh copy has.
#[cfg(not(unix))]
fn leads_to(link: &Path, exe: &Path) -> io::Result<bool> {
    let (link_meta, exe_meta) = (fs::metadata(link)?, fs::metadata(exe)?);
    if link_meta.len() != exe_meta.len() {
        return Ok(false);
    }
    Ok(fs::read(link)? == fs::read(exe)?)
}

#[cfg(not(unix))]
fn make(link: &Path, exe: &Path) -> io::Result<()> {
    fs::copy(exe, link).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the app's executable in `dir`.
    fn app(dir: &Path, name: &str, bytes: &str) -> PathBuf {
        let exe = dir.join(name);
        fs::write(&exe, bytes).unwrap();
        exe
    }

    fn contents(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn the_link_is_made_where_there_is_none() {
        let root = tempfile::tempdir().unwrap();
        let exe = app(root.path(), "Epik", "the app");

        assert_eq!(refresh(root.path(), &exe).unwrap(), Refreshed::Made);

        assert_eq!(contents(&path(root.path())), "the app");
    }

    #[test]
    fn a_link_to_the_app_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let exe = app(root.path(), "Epik", "the app");
        refresh(root.path(), &exe).unwrap();

        assert_eq!(refresh(root.path(), &exe).unwrap(), Refreshed::Kept);

        assert_eq!(contents(&path(root.path())), "the app");
    }

    #[test]
    fn a_stale_link_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let old = app(root.path(), "Old Epik", "the old app");
        refresh(root.path(), &old).unwrap();
        let new = app(root.path(), "New Epik", "the new app");

        assert_eq!(refresh(root.path(), &new).unwrap(), Refreshed::Replaced);

        assert_eq!(contents(&path(root.path())), "the new app");
    }

    #[test]
    fn an_app_started_through_the_link_keeps_it() {
        let root = tempfile::tempdir().unwrap();
        let exe = app(root.path(), "Epik", "the app");
        refresh(root.path(), &exe).unwrap();

        // Started as ~/.epik/bin/epik-app, the app may be told that is
        // where it is.
        assert_eq!(
            refresh(root.path(), &path(root.path())).unwrap(),
            Refreshed::Kept
        );

        assert_eq!(contents(&path(root.path())), "the app");
    }

    #[test]
    fn a_link_to_an_app_that_is_gone_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let old = app(root.path(), "Old Epik", "the old app");
        refresh(root.path(), &old).unwrap();
        fs::remove_file(&old).unwrap();
        let new = app(root.path(), "Epik", "the app");

        assert_eq!(refresh(root.path(), &new).unwrap(), Refreshed::Replaced);

        assert_eq!(contents(&path(root.path())), "the app");
    }

    /// A stand-in for an app built in a clone under `<root>/builds`.
    fn app_in_a_clone(root: &Path) -> PathBuf {
        let target = builds_dir(root).join("a-build").join("target");
        fs::create_dir_all(&target).unwrap();
        app(&target, "epik-app", "the clone's app")
    }

    #[test]
    fn an_app_in_a_clone_makes_no_link() {
        let root = tempfile::tempdir().unwrap();
        let exe = app_in_a_clone(root.path());

        assert_eq!(refresh(root.path(), &exe).unwrap(), Refreshed::FromClone);

        assert!(fs::symlink_metadata(path(root.path())).is_err());
    }

    #[test]
    fn an_app_in_a_clone_leaves_a_link_to_another_app() {
        let root = tempfile::tempdir().unwrap();
        let installed = app(root.path(), "Epik", "the app");
        refresh(root.path(), &installed).unwrap();
        let exe = app_in_a_clone(root.path());

        assert_eq!(refresh(root.path(), &exe).unwrap(), Refreshed::FromClone);

        assert_eq!(contents(&path(root.path())), "the app");
    }

    #[cfg(unix)]
    #[test]
    fn the_real_path_of_the_link_is_the_app() {
        let root = tempfile::tempdir().unwrap();
        let exe = app(root.path(), "Epik", "the app");
        refresh(root.path(), &exe).unwrap();

        assert_eq!(
            real_path(&path(root.path())),
            fs::canonicalize(&exe).unwrap()
        );
    }

    #[test]
    fn the_real_path_of_a_path_that_is_no_link_is_itself() {
        let root = tempfile::tempdir().unwrap();
        let exe = fs::canonicalize(app(root.path(), "Epik", "the app")).unwrap();

        assert_eq!(real_path(&exe), exe);
    }

    /// On macOS a temporary directory is reached through a symbolic link,
    /// so the root may not be spelled the way the executable's real path is.
    #[cfg(unix)]
    #[test]
    fn an_app_in_a_clone_is_recognized_under_a_root_reached_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let root = dir.path().join("linked");
        std::os::unix::fs::symlink(&real, &root).unwrap();
        let exe = app_in_a_clone(&real);

        assert_eq!(refresh(&root, &exe).unwrap(), Refreshed::FromClone);

        assert!(fs::symlink_metadata(path(&root)).is_err());
    }
}
