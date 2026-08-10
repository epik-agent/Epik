//! What Epik was told about the models it can talk to.
//!
//! The file lives at `~/.epik/config.toml` and is edited by hand: there is no
//! settings UI, by decision — edit and restart, and the status bar names the
//! active model so you can see which one took. It lists providers, says which
//! is active, and carries the system prompt. Every host converges the file
//! into existence at startup ([`Config::converge`]), so there is always one
//! on disk to edit. The `[worker]` table is opt-in
//! and says what the run machinery conducts: which repository — its one
//! required field — and which coding-agent binary. Without the table there
//! is no worker and no launch verb, because a run aimed at a repository
//! nobody named must be unrepresentable.
//!
//! It never carries a key. Keys live where the operating system keeps
//! secrets; see [`crate::keystore`]. A key written into this file is not read
//! by anything.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

/// The minimal Epik persona: the default system prompt, and the reason the
/// window's first words are "Hello, I'm Epik."
pub const PERSONA: &str = "You are Epik, a software-design partner. Open by introducing yourself: \
     \"Hello, I'm Epik.\"";

/// Overrides where Epik keeps its things. Chiefly so tests need not have — or
/// touch — a home directory.
pub const HOME_ENV: &str = "EPIK_HOME";

const CONFIG_FILE: &str = "config.toml";

/// Where Epik keeps its things: `$EPIK_HOME`, or `~/.epik`.
///
/// # Errors
///
/// Returns an error when neither is discoverable.
pub fn home() -> Result<PathBuf> {
    if let Some(home) = env::var_os(HOME_ENV) {
        return Ok(PathBuf::from(home));
    }
    // `HOME` on Unix, `USERPROFILE` on Windows. Reading the environment
    // directly rather than through a crate keeps this compiling for wasm32,
    // where it will simply find nothing.
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".epik"))
        .ok_or_else(|| anyhow!("no home directory: set {HOME_ENV} to say where Epik should live"))
}

/// An OpenAI-compatible endpoint and a model served on it. Two strings is the
/// whole of what talking to a provider takes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Provider {
    pub base_url: String,
    pub model: String,
}

/// Everything Epik reads out of `config.toml`.
///
/// Missing fields fall back to the defaults, so a file naming one provider is
/// a complete config, and a file that does not exist yet is too.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Config {
    /// Which entry of `providers` to talk to.
    pub active: String,
    pub system_prompt: String,
    // Tables last: TOML requires every scalar to be emitted before any table.
    /// The run machinery's aim, when the user has stated one. `None` is a
    /// config with no worker and no launch verb — never a default
    /// repository reached for on the user's behalf.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<Worker>,
    pub providers: BTreeMap<String, Provider>,
}

/// What the run machinery conducts, and with what: `epik-worker`'s job, and
/// the window's launch verb, provisioned identically from this one table.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Worker {
    /// The repository whose issues get implemented, spelled `owner/name`
    /// the way GitHub spells it. The section's one required field: runs
    /// mutate this repository, so it is always the user's own word.
    pub repo: String,
    /// The coding agent's binary. An explicit path is the reliable spelling
    /// — launchd's `PATH` is not a login shell's — and a bare name falls
    /// back to `PATH` resolution.
    #[serde(default = "claude")]
    pub agent: String,
    /// Where the repository is cloned from, when it is not GitHub's own
    /// address for `repo` — an enterprise host, a mirror, a test's local
    /// origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Where the GitHub API answers, when it is not `api.github.com`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
}

/// The unstated agent: `claude` off `PATH`.
fn claude() -> String {
    "claude".to_owned()
}

/// The derivations both run hosts make from the table, in one place: the
/// worker binary and the window's launcher provision the same run the same
/// way because they call the same three verbs.
#[cfg(feature = "native")]
impl Worker {
    /// The repository, parsed from its `owner/name` spelling.
    ///
    /// # Errors
    ///
    /// Returns an error when `repo` is not `owner/name` — a config to fix,
    /// never one to guess at.
    pub fn repository(&self) -> Result<crate::github::Repo> {
        crate::github::Repo::parse(&self.repo).ok_or_else(|| {
            anyhow!(
                "worker.repo {:?} is not an owner/name repository",
                self.repo
            )
        })
    }

    /// Where the repository is cloned from: the stated `url`, or GitHub's
    /// own address for `repo`. GitHub is the only rendezvous, so the
    /// default is the repository's own address — never a spelling with a
    /// token in it; the token rides the git cache's askpass rails instead.
    #[must_use]
    pub fn clone_url(&self, repo: &crate::github::Repo) -> String {
        self.url
            .clone()
            .unwrap_or_else(|| format!("https://github.com/{repo}.git"))
    }

    /// The GitHub client runs speak through: the stated `api` — an
    /// enterprise host, a test's loopback — or GitHub itself.
    #[must_use]
    pub fn github(&self, token: Option<String>) -> crate::github::GitHub {
        match &self.api {
            Some(api) => crate::github::GitHub::at(api, token),
            None => crate::github::GitHub::new(token),
        }
    }
}

impl Default for Config {
    /// A fresh install points at Anthropic through its OpenAI-compatible
    /// endpoint, and ships the local free path alongside it so that
    /// redirecting to Ollama is a one-word edit.
    fn default() -> Self {
        Self {
            active: "anthropic".to_owned(),
            system_prompt: PERSONA.to_owned(),
            providers: BTreeMap::from([
                (
                    "anthropic".to_owned(),
                    Provider {
                        base_url: "https://api.anthropic.com/v1".to_owned(),
                        model: "claude-sonnet-5".to_owned(),
                    },
                ),
                (
                    "ollama".to_owned(),
                    Provider {
                        base_url: "http://localhost:11434/v1".to_owned(),
                        model: "smollm2:135m".to_owned(),
                    },
                ),
            ]),
            worker: None,
        }
    }
}

impl Config {
    /// Where the config file is, whether or not it exists.
    ///
    /// # Errors
    ///
    /// Returns an error when Epik's home is not discoverable.
    pub fn path() -> Result<PathBuf> {
        Ok(home()?.join(CONFIG_FILE))
    }

    /// Reads the config, or the defaults when there is no file yet. A fresh
    /// install is a working install.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed.
    /// A config Epik cannot understand is not quietly replaced with one it
    /// invented.
    pub fn read(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
        }
    }

    /// Converges Epik's home: the directory exists, the file exists —
    /// materialized from the defaults when there is none — and the config
    /// is in hand. Every host's first act, so the window and the worker
    /// set up the machine identically.
    ///
    /// Idempotent by construction: an existing file is read, never
    /// rewritten, so a hand-edited config survives every startup byte for
    /// byte — and a deleted home is just the first run over again.
    ///
    /// # Errors
    ///
    /// Returns an error when Epik's home is not discoverable or cannot be
    /// made a directory, when the default cannot be written, or when an
    /// existing file cannot be read or parsed.
    pub fn converge() -> Result<Self> {
        Self::converge_at(&Self::path()?)
    }

    /// [`converge`](Self::converge), at a stated path.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or the default cannot be
    /// written, or when an existing file cannot be read or parsed.
    pub fn converge_at(path: &Path) -> Result<Self> {
        // `symlink_metadata` rather than `exists`: a dangling symlink is
        // read — and comes back as the defaults, unwritten — never written
        // through into its target. A probe that fails for any other reason
        // falls through to the write, which says what is actually wrong.
        // Between the probe and the write another host can only race this
        // one to the same rendered defaults.
        if path.symlink_metadata().is_ok() {
            return Self::read(path);
        }
        let config = Self::default();
        config.save_at(path)?;
        Ok(config)
    }

    /// Writes the config, creating Epik's home if it is not there.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or the file cannot be written.
    pub fn save(&self) -> Result<()> {
        self.save_at(&Self::path()?)
    }

    /// [`save`](Self::save), at a stated path. The write is atomic —
    /// rendered beside the file, then renamed into place — so a crash or a
    /// full disk leaves the old file or none, never a torn one the next
    /// startup would refuse to parse.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or the file cannot be written.
    pub fn save_at(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("rendering the config")?;
        let mut staged = path.as_os_str().to_owned();
        staged.push(".new");
        let staged = PathBuf::from(staged);
        fs::write(&staged, text).with_context(|| format!("writing {}", staged.display()))?;
        fs::rename(&staged, path)
            .with_context(|| format!("renaming {} into place", staged.display()))
    }

    /// The active provider, and the name it is configured under — which is
    /// also the account its key is stored against.
    ///
    /// # Errors
    ///
    /// Returns an error when `active` names a provider that is not listed.
    pub fn provider(&self) -> Result<(&str, &Provider)> {
        self.providers
            .get_key_value(&self.active)
            .map(|(name, provider)| (name.as_str(), provider))
            .ok_or_else(|| {
                let known = self
                    .providers
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow!(
                    "the active provider \"{}\" is not one of the configured ones ({known})",
                    self.active
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_config_is_usable_as_it_stands() {
        let config = Config::default();
        let (name, provider) = config.provider().unwrap();
        assert_eq!(name, "anthropic");
        assert!(provider.base_url.starts_with("https://"));
        assert_eq!(config.system_prompt, PERSONA);
    }

    #[test]
    fn the_config_has_nowhere_to_put_a_key() {
        let rendered = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(
            !rendered.to_lowercase().contains("key"),
            "no field of the config may look like somewhere a secret goes:\n{rendered}"
        );
    }

    #[test]
    fn a_key_written_into_the_file_by_hand_is_read_by_nothing() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "active = \"mine\"\n\
             api_key = \"sk-do-not-put-this-here\"\n\
             [providers.mine]\n\
             base_url = \"http://localhost:1234/v1\"\n\
             model = \"local\"\n",
        )
        .unwrap();

        let config = Config::read(file.path()).unwrap();

        assert_eq!(config.provider().unwrap().0, "mine");
        assert!(
            !format!("{config:?}").contains("sk-do-not-put-this-here"),
            "a key in the file must land nowhere at all"
        );
    }

    #[test]
    fn a_file_naming_one_provider_is_a_whole_config() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "active = \"local\"\n\
             [providers.local]\n\
             base_url = \"http://localhost:11434/v1\"\n\
             model = \"smollm2:135m\"\n",
        )
        .unwrap();

        let config = Config::read(file.path()).unwrap();

        assert_eq!(
            config.system_prompt, PERSONA,
            "an unstated system prompt is the persona, not nothing"
        );
        assert_eq!(config.provider().unwrap().1.model, "smollm2:135m");
    }

    #[test]
    fn no_worker_section_is_no_worker_rather_than_somebody_elses_repository() {
        assert_eq!(
            Config::default().worker,
            None,
            "a run aimed at a repository nobody named must be unrepresentable"
        );
    }

    #[test]
    fn a_worker_section_names_its_repository_and_the_agent_defaults() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "[worker]\nrepo = \"wpm/epik-scratch\"\n").unwrap();

        let config = Config::read(file.path()).unwrap();

        let worker = config.worker.expect("the section was stated");
        assert_eq!(worker.repo, "wpm/epik-scratch");
        assert_eq!(worker.agent, "claude", "an unstated agent is the default");
        assert_eq!(worker.url, None);
        assert_eq!(worker.api, None);
    }

    #[test]
    fn a_worker_section_without_a_repository_is_reported_rather_than_aimed() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "[worker]\nagent = \"/opt/claude/claude\"\n").unwrap();

        let error = Config::read(file.path()).expect_err("repo is the one required field");

        assert!(format!("{error:#}").contains("repo"), "{error:#}");
    }

    #[cfg(feature = "native")]
    #[test]
    fn the_worker_derivations_are_the_same_for_every_host() {
        let worker = Worker {
            repo: "wpm/epik-scratch".to_owned(),
            agent: "claude".to_owned(),
            url: None,
            api: None,
        };

        let repo = worker.repository().unwrap();
        assert_eq!(repo.to_string(), "wpm/epik-scratch");
        assert_eq!(
            worker.clone_url(&repo),
            "https://github.com/wpm/epik-scratch.git",
            "GitHub is the only rendezvous, so its address is the default"
        );

        let stated = Worker {
            url: Some("https://mirror.example/scratch.git".to_owned()),
            ..worker.clone()
        };
        assert_eq!(
            stated.clone_url(&repo),
            "https://mirror.example/scratch.git"
        );

        let bad = Worker {
            repo: "not-a-repository".to_owned(),
            ..worker
        };
        let error = bad.repository().expect_err("a typo is a config to fix");
        assert!(error.to_string().contains("owner/name"), "{error}");
    }

    #[test]
    fn an_absent_file_is_a_fresh_install_rather_than_a_failure() {
        let missing = Path::new("/nonexistent/epik/config.toml");
        assert_eq!(Config::read(missing).unwrap(), Config::default());
    }

    #[test]
    fn pointing_active_at_an_unlisted_provider_says_what_is_listed() {
        let config = Config {
            active: "groq".to_owned(),
            ..Config::default()
        };

        let error = config.provider().expect_err("groq is not configured");

        let message = error.to_string();
        assert!(message.contains("groq"), "{message}");
        assert!(message.contains("anthropic, ollama"), "{message}");
    }

    #[test]
    fn convergence_materializes_the_default_file_where_there_was_no_home() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".epik").join("config.toml");

        let config = Config::converge_at(&path).unwrap();

        assert_eq!(config, Config::default());
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            written,
            toml::to_string_pretty(&Config::default()).unwrap(),
            "the written default is Config::default() rendered to TOML"
        );
        assert!(
            !written.contains("[worker]"),
            "the [worker] table is absent until the user states one:\n{written}"
        );
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1,
            "the staged copy is renamed into place, never left beside the file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_config_symlink_is_never_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere.toml");
        let path = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&target, &path).unwrap();

        let config = Config::converge_at(&path).unwrap();

        assert_eq!(
            config,
            Config::default(),
            "a link to a missing file reads as a fresh install"
        );
        assert!(
            !target.exists(),
            "the defaults must not materialize behind the user's link"
        );
        assert!(
            path.symlink_metadata().unwrap().file_type().is_symlink(),
            "the link itself is left exactly as the user made it"
        );
    }

    #[test]
    fn a_deleted_home_converges_to_the_same_file_as_the_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".epik");
        let path = home.join("config.toml");
        Config::converge_at(&path).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&home).unwrap();

        Config::converge_at(&path).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            first,
            "a deleted home and a fresh install are the same case by construction"
        );
    }

    #[test]
    fn a_hand_edited_config_survives_convergence_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let stated = "active = \"local\"\n\
             [providers.local]\n\
             base_url = \"http://localhost:1234/v1\"\n\
             model = \"local\"\n\
             [worker]\n\
             repo = \"wpm/epik-scratch\"\n";
        std::fs::write(&path, stated).unwrap();

        let config = Config::converge_at(&path).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            stated,
            "an existing config is read, never rewritten"
        );
        assert_eq!(config.provider().unwrap().0, "local");
        assert_eq!(
            config.worker.expect("the section was stated").repo,
            "wpm/epik-scratch"
        );
    }

    #[test]
    fn convergence_reports_a_broken_config_rather_than_replacing_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is not toml = = =").unwrap();

        let error = Config::converge_at(&path).expect_err("broken TOML is an error");

        assert!(format!("{error:#}").contains("parsing"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is not toml = = =",
            "a config Epik cannot understand is not quietly replaced"
        );
    }

    #[test]
    fn the_config_round_trips_through_toml() {
        let config = Config::default();
        let rendered = toml::to_string_pretty(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&rendered).unwrap(), config);

        let with_worker = Config {
            worker: Some(Worker {
                repo: "wpm/epik-scratch".to_owned(),
                agent: "claude".to_owned(),
                url: None,
                api: None,
            }),
            ..config
        };
        let rendered = toml::to_string_pretty(&with_worker).unwrap();
        assert_eq!(toml::from_str::<Config>(&rendered).unwrap(), with_worker);
    }
}
