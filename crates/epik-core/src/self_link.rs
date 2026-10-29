//! The link at `~/.epik/bin/epik-app` to the running Epik app.
//!
//! Each platform's installer puts the app somewhere else, so programs that
//! need it, chiefly Claude Code running the plugin's MCP server, find it at
//! one fixed path instead. The app refreshes the link every time it starts:
//! a symbolic link on macOS and Linux; on Windows, where symbolic links
//! need privileges, a hard link where possible and a copy otherwise.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
}

/// Makes `<root>/bin/epik-app` lead to `exe`, the running app, leaving a
/// link that already does.
pub fn refresh(root: &Path, exe: &Path) -> io::Result<Refreshed> {
    let link = path(root);
    fs::create_dir_all(link.parent().expect("the link is in a directory"))?;
    let existed = fs::symlink_metadata(&link).is_ok();
    if existed {
        if leads_to(&link, exe)? {
            return Ok(Refreshed::Kept);
        }
        fs::remove_file(&link)?;
    }
    make(&link, exe)?;
    Ok(if existed {
        Refreshed::Replaced
    } else {
        Refreshed::Made
    })
}

#[cfg(unix)]
fn leads_to(link: &Path, exe: &Path) -> io::Result<bool> {
    Ok(fs::read_link(link).is_ok_and(|target| target == exe))
}

#[cfg(unix)]
fn make(link: &Path, exe: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(exe, link)
}

/// Whether the link is the app: the same bytes, as a hard link or a fresh
/// copy has.
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
    fs::hard_link(exe, link).or_else(|_| fs::copy(exe, link).map(|_| ()))
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
    fn a_link_to_an_app_that_is_gone_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let old = app(root.path(), "Old Epik", "the old app");
        refresh(root.path(), &old).unwrap();
        fs::remove_file(&old).unwrap();
        let new = app(root.path(), "Epik", "the app");

        assert_eq!(refresh(root.path(), &new).unwrap(), Refreshed::Replaced);

        assert_eq!(contents(&path(root.path())), "the app");
    }
}
