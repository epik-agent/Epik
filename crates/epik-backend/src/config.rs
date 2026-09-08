//! The configuration file, the startup that converges on it, and its
//! door to the settings window.
//!
//! Epik keeps one file, [`FILE`], in the platform's configuration
//! directory for the app — Tauri's `app_config_dir`, so each operating
//! system's own convention holds. [`converge`] makes the directory and
//! the file appear when they are absent and reads a file that is already
//! there without touching it, so a first run and a deleted directory are
//! the same case. The file as
//! first written is [`starting`]: the chat model, because the source
//! names that default, and nothing else. What the file can state is
//! [`Config`], the library's shape; the path and the format are this
//! crate's.
//!
//! [`config_read`] and [`config_write`] carry a [`Config`] across IPC —
//! no secrets in it, nothing to guard. A write reaches two places in one
//! command: the file, through [`save`], and the `Config`
//! in managed state, replaced only once the file has taken the new
//! values, so the next chat turn and the next build use them with no
//! restart and a failed write leaves what was running untouched.
//!
//! [`models_list`] and [`github_login`] need a secret and take none.
//! The window would have to ask the keyring for the key and send it
//! back over IPC; instead the backend reads it from the keyring itself,
//! so the crossings `secrets` counts — two, and exactly two — stay two.
//! Their results are discovery, never load-bearing: a provider that will
//! not answer is a reason the tab renders, and nothing else changes.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use anyhow::Context;
use epik::chat::{ANTHROPIC_MODEL, ChatError, ModelInfo};
use epik::config::{Config, Model};
use epik::github::GitHub;
use epik::keystore::{KeyStore, Resolved, Secret};
use tauri::{AppHandle, Manager, State};

use crate::chat::{API_KEY_NAME, GITHUB_TOKEN_NAME};
use crate::secrets::OsKeyring;

/// The configuration file's name inside [`root`].
const FILE: &str = "config.toml";

/// The app's configuration directory, as the platform has it: the one
/// place that path is asked for.
///
/// # Errors
///
/// A platform that names no configuration directory — never a fallback
/// somewhere else.
fn root(app: &AppHandle) -> anyhow::Result<PathBuf> {
    app.path()
        .app_config_dir()
        .context("no configuration directory on this platform")
}

/// The file as first written: the chat model, spelled from the constant,
/// and nothing else — no constant in the source names an agent model.
fn starting() -> Config {
    Config {
        model: Model {
            chat: Some(ANTHROPIC_MODEL.to_owned()),
            agent: None,
        },
        github: epik::config::GitHub::default(),
    }
}

/// Writes `config` to `root/config.toml`. The one writer of the file's
/// format, so convergence and anything that edits the configuration
/// afterward can never disagree about what the file looks like.
///
/// # Errors
///
/// The write that failed, naming the path.
fn save(root: &Path, config: &Config) -> anyhow::Result<()> {
    let path = root.join(FILE);
    let text = toml::to_string(config).context("could not serialize the configuration")?;
    std::fs::write(&path, text).with_context(|| format!("could not write {}", path.display()))
}

/// [`converge_at`] over [`root`].
///
/// # Errors
///
/// No configuration directory, or whatever `converge_at` reports.
pub(crate) fn converge(app: &AppHandle) -> anyhow::Result<Config> {
    converge_at(&root(app)?)
}

/// Creates `root`, writes [`starting`] when `root/config.toml` is absent,
/// then reads and parses whatever file is there. A file that cannot be
/// parsed is left exactly as it is.
///
/// # Errors
///
/// Which of creating, writing, reading, or parsing failed, and where.
fn converge_at(root: &Path) -> anyhow::Result<Config> {
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

/// [`config_write`] against any root and state, which is what makes it
/// testable. The file first; the state only once the file holds it.
fn write(root: &Path, state: &Mutex<Config>, config: Config) -> Result<(), String> {
    save(root, &config).map_err(|error| format!("{error:#}"))?;
    *state.lock().unwrap_or_else(PoisonError::into_inner) = config;
    Ok(())
}

/// The secret filed under `name` in `store`, or why there is none:
/// `absent` for an empty entry, the keyring's own words for a keyring
/// that would not answer.
fn key_from(store: &impl KeyStore, name: &str, absent: impl ToString) -> Result<Secret, String> {
    match store.resolve(name) {
        Resolved::Found(secret) => Ok(secret),
        Resolved::Absent => Err(absent.to_string()),
        Resolved::Unreachable(reason) => Err(reason),
    }
}

/// The configuration as it stands: what the file said at startup, or
/// what the last [`config_write`] made of it.
#[tauri::command]
pub fn config_read(state: State<'_, Mutex<Config>>) -> Config {
    state.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Writes `config` to the configuration file and makes it the running one.
#[tauri::command]
pub async fn config_write(
    app: AppHandle,
    state: State<'_, Mutex<Config>>,
    config: Config,
) -> Result<(), String> {
    let root = root(&app).map_err(|error| format!("{error:#}"))?;
    write(&root, &state, config)
}

/// Runs a keyring-and-network lookup off the async runtime's workers:
/// both are blocking, and the network can take its time, which is why
/// `send_message` gives a turn its own thread.
async fn blocking<T: Send + 'static>(
    lookup: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(lookup)
        .await
        .map_err(|error| format!("the lookup did not finish: {error}"))?
}

/// The models the stored Anthropic key can see, newest first.
#[tauri::command]
pub async fn models_list() -> Result<Vec<ModelInfo>, String> {
    blocking(|| {
        let key = key_from(&OsKeyring, API_KEY_NAME, ChatError::NoKey)?;
        epik::chat::models(&key).map_err(|error| error.to_string())
    })
    .await
}

/// The login of the account the stored GitHub token belongs to.
#[tauri::command]
pub async fn github_login() -> Result<String, String> {
    blocking(|| {
        let token = key_from(
            &OsKeyring,
            GITHUB_TOKEN_NAME,
            epik::github::Error::TokenAbsent,
        )?;
        GitHub::new(Some(token))
            .login()
            .map_err(|error| error.to_string())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    use epik::config::GitHub;
    use epik::keystore::InMemory;

    use crate::testing::Broken;
    use epik::testing::Scratch;

    fn chosen() -> Config {
        Config {
            model: Model {
                chat: Some("c".to_owned()),
                agent: None,
            },
            github: GitHub {
                owner: Some("o".to_owned()),
            },
        }
    }

    #[test]
    fn a_write_states_the_config_in_the_file_and_replaces_the_running_one() {
        let scratch = Scratch::new("config");
        let state = Mutex::new(starting());

        write(&scratch.0, &state, chosen()).unwrap();

        assert_eq!(
            std::fs::read_to_string(scratch.0.join(FILE)).unwrap(),
            "[model]\nchat = \"c\"\n\n[github]\nowner = \"o\"\n"
        );
        assert_eq!(*state.lock().unwrap(), chosen());
    }

    #[test]
    fn a_write_that_fails_names_the_path_and_leaves_the_running_config_alone() {
        let scratch = Scratch::new("config");
        let root = scratch.0.join("a-file");
        std::fs::write(&root, "").unwrap();
        let state = Mutex::new(starting());

        let error = write(&root, &state, chosen()).unwrap_err();

        assert!(error.contains("could not write"), "{error}");
        assert!(error.contains("a-file"), "{error}");
        assert_eq!(*state.lock().unwrap(), starting());
    }

    #[test]
    fn a_stored_key_is_found() {
        let mut store = InMemory::default();
        store.set("k", "sk-1".into()).unwrap();
        assert_eq!(key_from(&store, "k", "absent").unwrap(), "sk-1".into());
    }

    #[test]
    fn an_absent_key_is_the_callers_wording() {
        let error = key_from(&InMemory::default(), "k", ChatError::NoKey).unwrap_err();
        assert_eq!(error, ChatError::NoKey.to_string());
    }

    #[test]
    fn an_unreachable_store_is_its_own_reason() {
        let error = key_from(&Broken, "k", "absent").unwrap_err();
        assert!(error.contains("locked"), "{error}");
    }

    fn root() -> (Scratch, PathBuf) {
        let scratch = Scratch::new("config");
        let root = scratch.0.join("app").join("config");
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
