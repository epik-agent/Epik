//! The agent and model each role runs with.
//!
//! A role is a kind of work: coding an issue, or reviewing a build's pull
//! requests. The user chooses an agent for each, and a model or the
//! agent's default, and Epik keeps the choice in `<root>/settings.json`:
//!
//! ```json
//! {"coding": {"agent": "claude", "model": null}, "review": null}
//! ```
//!
//! `review` is `null` while review is off. Until the file is written, the
//! settings are [`Settings::default`]. A `config.toml` beside it, left by
//! Epik's earlier Rust version, is not read.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub use epik_view::{AgentSettings as Settings, Role};

use crate::agent::AgentProfile;

/// The file the settings are kept in.
pub const FILE: &str = "settings.json";

/// The ids of the agents Epik can run.
pub const AGENTS: &[&str] = &["claude", "codex"];

fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// The profile of the agent `role` runs, or why there is none: it is an
/// agent Epik doesn't know.
pub fn profile(role: &Role) -> Result<AgentProfile, String> {
    match role.agent.as_str() {
        "claude" => Ok(AgentProfile::claude()),
        "codex" => Ok(AgentProfile::codex()),
        agent => Err(format!(
            "{agent:?} is not an agent Epik knows: the agents are {}",
            AGENTS.join(", ")
        )),
    }
}

/// What people call the agent whose id is `agent`, such as `Codex`, or
/// its id if Epik doesn't know it.
pub fn name(agent: &str) -> String {
    profile(&Role {
        agent: agent.to_owned(),
        model: None,
    })
    .map_or_else(|_| agent.to_owned(), |profile| profile.name)
}

/// Why `settings` can't be used, if an agent in them is one Epik doesn't
/// know.
fn unknown_agent(settings: &Settings) -> Option<String> {
    let roles = [Some(&settings.coding), settings.review.as_ref()];
    roles
        .into_iter()
        .flatten()
        .find_map(|role| profile(role).err())
}

/// The settings, or the defaults if they have never been written. A file
/// naming an agent Epik doesn't know is an error.
pub fn read(root: &Path) -> io::Result<Settings> {
    let settings = match fs::read_to_string(path(root)) {
        Ok(text) => serde_json::from_str(&text)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => return Err(error),
    };
    match unknown_agent(&settings) {
        Some(reason) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {reason}", path(root).display()),
        )),
        None => Ok(settings),
    }
}

/// Saves `settings`, unless they name an agent Epik doesn't know. They
/// are written whole, through a temporary file, so a reader never sees
/// half of them.
pub fn write(root: &Path, settings: &Settings) -> io::Result<()> {
    if let Some(reason) = unknown_agent(settings) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, reason));
    }
    fs::create_dir_all(root)?;
    let mut json = serde_json::to_vec_pretty(settings)?;
    json.push(b'\n');
    let temporary = root.join(format!("{FILE}.{}", std::process::id()));
    fs::write(&temporary, json)?;
    fs::rename(temporary, path(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(agent: &str, model: Option<&str>) -> Role {
        Role {
            agent: agent.to_owned(),
            model: model.map(str::to_owned),
        }
    }

    fn files(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn settings_never_written_are_the_defaults() {
        let root = tempfile::tempdir().unwrap();
        let settings = read(&root.path().join("not-yet")).unwrap();
        assert_eq!(settings.coding, role("claude", None));
        assert_eq!(settings.review, None);
    }

    #[test]
    fn settings_written_are_read_back() {
        let root = tempfile::tempdir().unwrap();
        let settings = Settings {
            coding: role("claude", Some("opus")),
            review: Some(role("claude", None)),
        };
        write(root.path(), &settings).unwrap();
        assert_eq!(read(root.path()).unwrap(), settings);
    }

    #[test]
    fn either_role_can_be_codex() {
        let root = tempfile::tempdir().unwrap();
        let settings = Settings {
            coding: role("codex", Some("gpt-6.1-sol")),
            review: Some(role("codex", None)),
        };
        write(root.path(), &settings).unwrap();

        assert_eq!(read(root.path()).unwrap(), settings);
        assert_eq!(profile(&settings.coding).unwrap(), AgentProfile::codex());
    }

    #[test]
    fn the_file_has_the_documented_shape() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(FILE),
            r#"{"coding": {"agent": "claude", "model": "sonnet"}, "review": null}"#,
        )
        .unwrap();
        assert_eq!(
            read(root.path()).unwrap(),
            Settings {
                coding: role("claude", Some("sonnet")),
                review: None,
            }
        );

        write(root.path(), &Settings::default()).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(root.path().join(FILE)).unwrap()).unwrap();
        assert_eq!(
            written,
            serde_json::json!({"coding": {"agent": "claude", "model": null}, "review": null})
        );
    }

    #[test]
    fn an_unknown_agent_in_the_file_is_an_error_naming_the_known_ones() {
        let root = tempfile::tempdir().unwrap();
        for json in [
            r#"{"coding": {"agent": "aider", "model": null}, "review": null}"#,
            r#"{"coding": {"agent": "claude", "model": null}, "review": {"agent": "aider", "model": null}}"#,
        ] {
            fs::write(root.path().join(FILE), json).unwrap();
            let error = read(root.path()).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{json}");
            let message = error.to_string();
            assert!(message.contains("\"aider\""), "{message}");
            assert!(
                message.ends_with("the agents are claude, codex"),
                "{message}"
            );
        }
    }

    #[test]
    fn an_unknown_agent_is_not_written() {
        let root = tempfile::tempdir().unwrap();
        let before = Settings {
            coding: role("claude", Some("opus")),
            review: None,
        };
        write(root.path(), &before).unwrap();

        let refused = Settings {
            coding: role("claude", None),
            review: Some(role("aider", None)),
        };
        let error = write(root.path(), &refused).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("the agents are claude, codex"));
        assert_eq!(read(root.path()).unwrap(), before);
        assert_eq!(files(root.path()), [FILE]);
    }

    #[test]
    fn a_write_leaves_only_the_whole_file() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &Settings::default()).unwrap();
        let settings = Settings {
            coding: role("claude", Some("opus")),
            review: Some(role("claude", Some("sonnet"))),
        };
        write(root.path(), &settings).unwrap();

        assert_eq!(files(root.path()), [FILE]);
        assert_eq!(read(root.path()).unwrap(), settings);
    }

    #[test]
    fn a_write_that_fails_leaves_the_settings_as_they_were() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &Settings::default()).unwrap();
        // The temporary file can't be created where a directory stands.
        let temporary = root.path().join(format!("{FILE}.{}", std::process::id()));
        fs::create_dir(&temporary).unwrap();

        let settings = Settings {
            coding: role("claude", Some("opus")),
            review: None,
        };
        assert!(write(root.path(), &settings).is_err());

        assert_eq!(read(root.path()).unwrap(), Settings::default());
    }

    /// The lines of the section of `markdown` headed `heading`, up to the
    /// next heading of its level or higher. Read by line, since a Windows
    /// checkout ends its lines with `\r\n`.
    fn section(markdown: &str, heading: &str) -> String {
        let level = |line: &str| line.len() - line.trim_start_matches('#').len();
        let mut lines = markdown.lines().skip_while(|line| *line != heading);
        assert!(lines.next().is_some(), "no section {heading:?}");
        lines
            .take_while(|line| !(line.starts_with('#') && level(line) <= level(heading)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_user_docs_describe_every_agent_a_role_can_run() {
        let readme = include_str!("../../../README.md");
        let how_it_works = include_str!("../../../plugin/skills/assistant/how-it-works.md");
        for (doc, text) in [
            ("README.md's Settings", section(readme, "## Settings")),
            ("how-it-works.md's Roles", section(how_it_works, "## Roles")),
        ] {
            for agent in AGENTS {
                let name = name(agent);
                assert!(text.contains(&name), "{doc} doesn't mention {name}");
            }
        }
    }
}
