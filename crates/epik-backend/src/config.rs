//! The configuration's door to the settings window, and the two lookups
//! the window makes on the configuration's behalf.
//!
//! [`config_read`] and [`config_write`] carry a [`Config`] across IPC —
//! no secrets in it, nothing to guard. A write reaches two places in one
//! command: the file, through [`epik::config::save`], and the `Config`
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

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use epik::chat::{ChatError, ModelInfo};
use epik::config::Config;
use epik::github::GitHub;
use epik::keystore::{KeyStore, OsKeyring, Resolved, Secret};
use tauri::State;

use crate::chat::{API_KEY_NAME, GITHUB_TOKEN_NAME};

/// [`config_write`] against any root and state, which is what makes it
/// testable. The file first; the state only once the file holds it.
fn write(root: &Path, state: &Mutex<Config>, config: Config) -> Result<(), String> {
    epik::config::save(root, &config).map_err(|error| format!("{error:#}"))?;
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

/// Writes `config` to `~/.epik/config.toml` and makes it the running one.
#[tauri::command]
pub async fn config_write(state: State<'_, Mutex<Config>>, config: Config) -> Result<(), String> {
    let home = epik::config::home().map_err(|error| format!("{error:#}"))?;
    write(&home, &state, config)
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
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use anyhow::anyhow;
    use epik::config::{GitHub, Model};
    use epik::keystore::InMemory;

    /// A fresh directory under the system's temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "epik-backend-config-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Broken;

    impl KeyStore for Broken {
        fn get(&self, _: &str) -> anyhow::Result<Option<Secret>> {
            Err(anyhow!("the keychain is locked"))
        }

        fn set(&mut self, _: &str, _: Secret) -> anyhow::Result<()> {
            Err(anyhow!("the keychain is locked"))
        }
    }

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
        let scratch = Scratch::new();
        let state = Mutex::new(epik::config::starting());

        write(&scratch.0, &state, chosen()).unwrap();

        assert_eq!(
            std::fs::read_to_string(scratch.0.join(epik::config::FILE)).unwrap(),
            "[model]\nchat = \"c\"\n\n[github]\nowner = \"o\"\n"
        );
        assert_eq!(*state.lock().unwrap(), chosen());
    }

    #[test]
    fn a_write_that_fails_names_the_path_and_leaves_the_running_config_alone() {
        let scratch = Scratch::new();
        let root = scratch.0.join("a-file");
        std::fs::write(&root, "").unwrap();
        let state = Mutex::new(epik::config::starting());

        let error = write(&root, &state, chosen()).unwrap_err();

        assert!(error.contains("could not write"), "{error}");
        assert!(error.contains("a-file"), "{error}");
        assert_eq!(*state.lock().unwrap(), epik::config::starting());
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
}
