//! The configuration file, and the startup that converges on it.
//!
//! Epik has a home directory, `~/.epik`, holding one file, [`FILE`].
//! [`converge`] makes the directory and the file appear when they are
//! absent and reads a file that is already there without touching it, so
//! a first run and a deleted `~/.epik` are the same case.
//!
//! An omitted entry means the default, and an omitted section means all
//! of its entries are: every entry is an `Option`, absent reads as `None`,
//! and [`save`] writes `None` as absence, so a section holding nothing
//! leaves no empty `[table]` header behind. The file as first written is
//! [`starting`]: the chat model, because the source names that default,
//! and nothing else.
//!
//! The types are wire shape as well as file shape: [`Config`] crosses the
//! IPC barrier to the settings window and back, so it is compiled
//! everywhere `serde` is. The file itself — its path, its format, the
//! startup that converges on it — is `native`.

#[cfg(feature = "native")]
use std::path::{Path, PathBuf};

#[cfg(feature = "native")]
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::chat::ANTHROPIC_MODEL;

/// The configuration file's name inside [`home`].
#[cfg(feature = "native")]
pub const FILE: &str = "config.toml";

/// `~/.epik`: the one place that path is composed.
///
/// # Errors
///
/// An operating system that names no home directory — never a `.epik`
/// somewhere else.
#[cfg(feature = "native")]
pub fn home() -> Result<PathBuf> {
    std::env::home_dir()
        .map(|home| home.join(".epik"))
        .context("no home directory to keep ~/.epik in")
}

/// Everything the file can state. A section missing from the file is a
/// section of `None`s; a key the file states that is not named here is
/// a parse error, so a file in some other shape is reported rather than
/// quietly read as empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Model::is_default")]
    pub model: Model,
    #[serde(default, skip_serializing_if = "GitHub::is_default")]
    pub github: GitHub,
}

/// `[model]`: which models the chat window and the build Agents speak to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    /// The chat window's model; `None` is [`ANTHROPIC_MODEL`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    /// The build Agent's model; `None` lets the agent CLI pick its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// `[github]`: where a bare repository name is looked for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHub {
    /// The owner a bare repository name settles against; `None` refuses
    /// bare names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

impl Model {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl GitHub {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The file as first written: the chat model, spelled from the constant,
/// and nothing else — no constant in the source names an agent model.
#[must_use]
pub fn starting() -> Config {
    Config {
        model: Model {
            chat: Some(ANTHROPIC_MODEL.to_owned()),
            agent: None,
        },
        github: GitHub::default(),
    }
}

/// Writes `config` to `root/config.toml`. The one writer of the file's
/// format, so convergence and anything that edits the configuration
/// afterward can never disagree about what the file looks like.
///
/// # Errors
///
/// The write that failed, naming the path.
#[cfg(feature = "native")]
pub fn save(root: &Path, config: &Config) -> Result<()> {
    let path = root.join(FILE);
    let text = toml::to_string(config).context("could not serialize the configuration")?;
    std::fs::write(&path, text).with_context(|| format!("could not write {}", path.display()))
}

/// [`converge_at`] over [`home`].
///
/// # Errors
///
/// No home directory, or whatever `converge_at` reports.
#[cfg(feature = "native")]
pub fn converge() -> Result<Config> {
    converge_at(&home()?)
}

/// Creates `root`, writes [`starting`] when `root/config.toml` is absent,
/// then reads and parses whatever file is there. A file that cannot be
/// parsed is left exactly as it is.
///
/// # Errors
///
/// Which of creating, writing, reading, or parsing failed, and where.
#[cfg(feature = "native")]
pub fn converge_at(root: &Path) -> Result<Config> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("could not create {}", root.display()))?;
    let path = root.join(FILE);
    if !path.exists() {
        save(root, &starting())?;
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    use crate::testing::Scratch;

    fn root() -> (Scratch, PathBuf) {
        let scratch = Scratch::new("config");
        let root = scratch.0.join("home").join(".epik");
        (scratch, root)
    }

    fn file(root: &Path) -> String {
        std::fs::read_to_string(root.join(FILE)).unwrap()
    }

    #[test]
    fn a_fresh_root_gains_the_directory_and_the_starting_file() {
        let (_dir, root) = root();
        let config = converge_at(&root).unwrap();
        assert!(root.join(FILE).is_file());
        assert_eq!(config, starting());
        assert_eq!(config.model.chat.as_deref(), Some(ANTHROPIC_MODEL));
        assert_eq!(config.model.agent, None);
        assert_eq!(config.github.owner, None);
    }

    #[test]
    fn the_first_file_states_the_chat_constant_and_nothing_else() {
        let (_dir, root) = root();
        converge_at(&root).unwrap();
        assert_eq!(
            file(&root),
            format!("[model]\nchat = \"{ANTHROPIC_MODEL}\"\n")
        );
    }

    #[test]
    fn converging_twice_leaves_the_bytes_unchanged() {
        let (_dir, root) = root();
        converge_at(&root).unwrap();
        let before = file(&root);
        converge_at(&root).unwrap();
        assert_eq!(file(&root), before);
    }

    #[test]
    fn an_existing_file_is_read_and_left_alone() {
        let (_dir, root) = root();
        std::fs::create_dir_all(&root).unwrap();
        let text = "# mine\n[model]\nagent = \"a\"\n\n[github]\nowner = \"o\"\n";
        std::fs::write(root.join(FILE), text).unwrap();
        let config = converge_at(&root).unwrap();
        assert_eq!(
            config,
            Config {
                model: Model {
                    chat: None,
                    agent: Some("a".to_owned()),
                },
                github: GitHub {
                    owner: Some("o".to_owned()),
                },
            }
        );
        assert_eq!(file(&root), text);
    }

    #[test]
    fn every_config_survives_a_save_and_a_read_back() {
        let full = Config {
            model: Model {
                chat: Some("c".to_owned()),
                agent: Some("a".to_owned()),
            },
            github: GitHub {
                owner: Some("o".to_owned()),
            },
        };
        let chat_only = starting();
        let owner_only = Config {
            github: GitHub {
                owner: Some("o".to_owned()),
            },
            ..Config::default()
        };
        for config in [full, chat_only, owner_only, Config::default()] {
            let (_dir, root) = root();
            std::fs::create_dir_all(&root).unwrap();
            save(&root, &config).unwrap();
            assert_eq!(converge_at(&root).unwrap(), config, "{config:?}");
        }
    }

    #[test]
    fn a_config_of_defaults_writes_an_empty_file() {
        let (_dir, root) = root();
        std::fs::create_dir_all(&root).unwrap();
        save(&root, &Config::default()).unwrap();
        assert_eq!(file(&root), "");
    }

    #[test]
    fn omissions_read_as_defaults_throughout() {
        for text in ["", "[model]\n", "[github]\n", "[model]\n[github]\n"] {
            let (_dir, root) = root();
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join(FILE), text).unwrap();
            assert_eq!(converge_at(&root).unwrap(), Config::default(), "{text:?}");
        }
    }

    #[test]
    fn a_file_stating_only_agent_reads_none_for_chat() {
        let (_dir, root) = root();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(FILE), "[model]\nagent = \"a\"\n").unwrap();
        let config = converge_at(&root).unwrap();
        assert_eq!(config.model.chat, None);
        assert_eq!(config.model.agent.as_deref(), Some("a"));
    }

    #[test]
    fn a_file_that_is_not_toml_is_left_as_it_was_and_reported() {
        let (_dir, root) = root();
        std::fs::create_dir_all(&root).unwrap();
        let text = "this is not = = toml";
        std::fs::write(root.join(FILE), text).unwrap();
        let error = converge_at(&root).unwrap_err().to_string();
        assert!(error.contains("could not parse"), "{error}");
        assert!(error.contains(FILE), "{error}");
        assert_eq!(file(&root), text);
    }

    /// A file in a shape this `Config` does not know — an earlier Epik's,
    /// say — is not read as empty; it is reported, and left alone.
    #[test]
    fn a_file_with_keys_this_config_does_not_know_is_reported() {
        let (_dir, root) = root();
        std::fs::create_dir_all(&root).unwrap();
        let text = "active = \"anthropic\"\n\n[providers.anthropic]\nmodel = \"x\"\n";
        std::fs::write(root.join(FILE), text).unwrap();
        let error = format!("{:#}", converge_at(&root).unwrap_err());
        assert!(error.contains("could not parse"), "{error}");
        assert!(error.contains("active"), "{error}");
        assert_eq!(file(&root), text);
    }
}
