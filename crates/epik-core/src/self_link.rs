//! The link at `~/.epik/bin/epik-app` to the running Epik app.
//!
//! Each platform's installer puts the app somewhere else, so programs that
//! need it, chiefly Claude Code running the plugin's MCP server, find it at
//! one fixed path instead. The app refreshes the link every time it starts:
//! a symbolic link on macOS and Linux; on Windows, where symbolic links
//! need privileges, a copy. Not a hard link: Claude Code runs the link, and
//! a running hard link to the installed app would keep its installer from
//! replacing it.

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
    // An app started through the link may be told it is the link. Its
    // real file is what the link must lead to, never the link itself.
    let exe = &fs::canonicalize(exe).unwrap_or_else(|_| exe.to_owned());
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
}
