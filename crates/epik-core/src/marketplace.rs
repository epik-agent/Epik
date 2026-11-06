//! The plugin's marketplace at `~/.epik/marketplace`, which the app
//! carries.
//!
//! Claude Code installs the Epik plugin from this directory, so the plugin
//! is always the one the app was built with: upgrading the app upgrades
//! the plugin. The app makes the directory hold exactly its embedded files
//! every time it starts. An app running from a build's clone leaves the
//! directory alone, as it does the link to the app ([`crate::self_link`]).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};

use crate::self_link;

/// The repository's marketplace file.
static CLAUDE_PLUGIN: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../.claude-plugin");

/// The repository's plugin.
static PLUGIN: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../plugin");

/// Where the marketplace is, `<root>/marketplace`.
pub fn path(root: &Path) -> PathBuf {
    root.join("marketplace")
}

/// What [`refresh`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refreshed {
    /// The directory already held exactly the app's files.
    Kept,
    /// A file was written or removed.
    Changed,
    /// The app runs from a build's clone, so the directory was left as it
    /// was.
    FromClone,
}

/// Makes `<root>/marketplace` hold exactly the app's files, unless `exe`,
/// the running app, is in a build's clone.
pub fn refresh(root: &Path, exe: &Path) -> io::Result<Refreshed> {
    if self_link::in_a_clone(root, exe) {
        return Ok(Refreshed::FromClone);
    }
    let dir = path(root);
    let files = files();
    let mut changed = remove_strays(&dir, &dir, &files)?;
    for (name, bytes) in &files {
        let file = dir.join(name);
        if fs::read(&file).is_ok_and(|old| old == *bytes) {
            continue;
        }
        fs::create_dir_all(file.parent().expect("a file is in a directory"))?;
        fs::write(&file, bytes)?;
        changed = true;
    }
    Ok(if changed {
        Refreshed::Changed
    } else {
        Refreshed::Kept
    })
}

/// The app's files, by their paths in the marketplace.
fn files() -> BTreeMap<PathBuf, &'static [u8]> {
    let mut files = BTreeMap::new();
    add(&CLAUDE_PLUGIN, Path::new(".claude-plugin"), &mut files);
    add(&PLUGIN, Path::new("plugin"), &mut files);
    files
}

/// Adds every file in `dir`, at `under`, to `files`.
fn add(dir: &'static Dir, under: &Path, files: &mut BTreeMap<PathBuf, &'static [u8]>) {
    for file in dir.files() {
        files.insert(under.join(file.path()), file.contents());
    }
    for sub in dir.dirs() {
        add(sub, under, files);
    }
}

/// Removes everything in `here`, within the marketplace `dir`, that is
/// neither one of `files` nor a directory holding one. Says whether it
/// removed anything.
fn remove_strays(
    dir: &Path,
    here: &Path,
    files: &BTreeMap<PathBuf, &'static [u8]>,
) -> io::Result<bool> {
    let entries = match fs::read_dir(here) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut removed = false;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = path.strip_prefix(dir).expect("it is in the marketplace");
        if entry.file_type()?.is_dir() {
            if files.keys().any(|file| file.starts_with(name)) {
                removed |= remove_strays(dir, &path, files)?;
            } else {
                fs::remove_dir_all(&path)?;
                removed = true;
            }
        } else if !files.contains_key(name) {
            fs::remove_file(&path)?;
            removed = true;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::builds_dir;

    fn app(root: &Path) -> PathBuf {
        root.join("Epik")
    }

    fn marketplace_file(root: &Path) -> PathBuf {
        path(root).join(".claude-plugin").join("marketplace.json")
    }

    fn manifest(root: &Path) -> PathBuf {
        path(root)
            .join("plugin")
            .join(".claude-plugin")
            .join("plugin.json")
    }

    #[test]
    fn the_directory_is_made_with_the_apps_files() {
        let root = tempfile::tempdir().unwrap();

        assert_eq!(
            refresh(root.path(), &app(root.path())).unwrap(),
            Refreshed::Changed
        );

        assert_eq!(
            fs::read_to_string(marketplace_file(root.path())).unwrap(),
            include_str!("../../../.claude-plugin/marketplace.json")
        );
        assert_eq!(
            fs::read_to_string(manifest(root.path())).unwrap(),
            include_str!("../../../plugin/.claude-plugin/plugin.json")
        );
        assert_eq!(
            fs::read_to_string(path(root.path()).join("plugin/skills/issue/procedure.md")).unwrap(),
            include_str!("../../../plugin/skills/issue/procedure.md")
        );
    }

    #[test]
    fn an_unchanged_directory_is_kept() {
        let root = tempfile::tempdir().unwrap();
        refresh(root.path(), &app(root.path())).unwrap();

        assert_eq!(
            refresh(root.path(), &app(root.path())).unwrap(),
            Refreshed::Kept
        );
    }

    #[test]
    fn a_stale_file_is_removed() {
        let root = tempfile::tempdir().unwrap();
        refresh(root.path(), &app(root.path())).unwrap();
        let stale = path(root.path()).join("plugin").join("skills").join("gone");
        fs::create_dir_all(&stale).unwrap();
        fs::write(stale.join("SKILL.md"), "an old skill").unwrap();
        fs::write(path(root.path()).join("notes.txt"), "stray").unwrap();

        assert_eq!(
            refresh(root.path(), &app(root.path())).unwrap(),
            Refreshed::Changed
        );

        assert!(!stale.exists());
        assert!(!path(root.path()).join("notes.txt").exists());
        assert!(manifest(root.path()).is_file());
    }

    #[test]
    fn a_changed_file_is_rewritten() {
        let root = tempfile::tempdir().unwrap();
        refresh(root.path(), &app(root.path())).unwrap();
        fs::write(manifest(root.path()), "{}").unwrap();

        assert_eq!(
            refresh(root.path(), &app(root.path())).unwrap(),
            Refreshed::Changed
        );

        assert_eq!(
            fs::read_to_string(manifest(root.path())).unwrap(),
            include_str!("../../../plugin/.claude-plugin/plugin.json")
        );
    }

    #[test]
    fn an_app_in_a_clone_leaves_the_directory_alone() {
        let root = tempfile::tempdir().unwrap();
        let target = builds_dir(root.path()).join("a-build").join("target");
        fs::create_dir_all(&target).unwrap();
        let exe = target.join("epik-app");
        fs::write(&exe, "the clone's app").unwrap();

        assert_eq!(refresh(root.path(), &exe).unwrap(), Refreshed::FromClone);

        assert!(!path(root.path()).exists());
    }
}
