//! What must stand before a run may start.
//!
//! `epik-worker` preflights a config-derived manifest before any run and
//! refuses with a structured [`CapabilityStatus`] naming the capability and
//! the locations searched — before a worktree exists. Refusals are
//! vocabulary, never log prose: the types up here are serde and wasm-clean,
//! rendered by doors, and reusable by the daemon's health story when that
//! arrives; the resolution code below them is native.
//!
//! The manifest has layers with different owners. Epik's own layer is git
//! and nothing else: found on `PATH`, held to a version floor, and on macOS
//! vouched for by `xcode-select -p` — never by spawning Apple's shim, which
//! answers with an install dialog. The per-agent layer is whatever binaries
//! the configured wrapper declares ([`CodingAgent::binaries`]), resolved
//! explicitly — the configured spelling, `PATH` as fallback — rather than
//! left to spawn-time mechanism, because launchd's `PATH` is not a login
//! shell's. The keystore layer is the GitHub token on its rails. Two layers
//! are deliberately absent: the target repository's toolchain is the user's
//! responsibility, with `Blocked` as the honest failure at run time, and
//! provider reachability is the wrapped agent's own affair — Claude Code
//! carries its own login.
//!
//! Boot integrity — unparseable config, an unwritable Epik home — is the
//! only fatal-at-startup class and is not a capability: those fail the
//! process, these refuse and report.
//!
//! [`CodingAgent::binaries`]: crate::agent::CodingAgent::binaries

use std::fmt;

use serde::{Deserialize, Serialize};

#[cfg(feature = "native")]
use std::env;
#[cfg(feature = "native")]
use std::ffi::OsStr;
#[cfg(feature = "native")]
use std::path::{Path, PathBuf};
#[cfg(feature = "native")]
use std::process::Command;

#[cfg(feature = "native")]
use crate::keystore::{GITHUB_ACCOUNT, GITHUB_OVERRIDE_ENV, KeyStore, SERVICE};

/// A capability the manifest resolves, named by the layer that owns it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Capability {
    /// Epik's own layer: the one binary the harness itself spawns.
    Git,
    /// The per-agent layer: a binary the configured wrapper declared, in
    /// the wrapper's own spelling.
    Agent { binary: String },
    /// The keystore layer: the GitHub token, on its rails.
    GithubToken,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git => write!(f, "git"),
            Self::Agent { binary } => write!(f, "the agent's {binary}"),
            Self::GithubToken => write!(f, "the GitHub token"),
        }
    }
}

/// Where a capability stands. Three states, on the keystore's
/// [`Resolved`](crate::keystore::Resolved) precedent: absent and unfit are
/// different situations, and a refusal should say which it is in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Standing {
    /// Found and adequate, at the location named.
    Present { at: String },
    /// Nowhere to be found: every location searched, named.
    Absent { searched: Vec<String> },
    /// Found — or reached for — but no use: a git beneath the version
    /// floor, a shim with nothing behind it, a keyring that will not
    /// answer.
    Unfit { at: String, reason: String },
}

/// One capability's resolved standing: what a preflight refusal is made of,
/// and what a health endpoint will serve.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityStatus {
    pub capability: Capability,
    pub standing: Standing,
}

impl fmt::Display for CapabilityStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.standing {
            Standing::Present { at } => write!(f, "{} at {at}", self.capability),
            Standing::Absent { searched } if searched.is_empty() => {
                write!(f, "{} is absent, with nowhere to search", self.capability)
            }
            Standing::Absent { searched } => write!(
                f,
                "{} is absent: searched {}",
                self.capability,
                searched.join(", ")
            ),
            Standing::Unfit { at, reason } => {
                write!(f, "{} at {at} is unfit: {reason}", self.capability)
            }
        }
    }
}

/// The version floor git is held to. A platform statement rather than a
/// feature inventory — every verb the harness spawns long predates it, and
/// the oldest supported platforms all clear it — so raising it is a
/// decision, never an accident.
#[cfg(feature = "native")]
const FLOOR: (u32, u32) = (2, 30);

/// Resolves the manifest, layer by layer: Epik's own git, the agent's
/// declared binaries, the GitHub token.
///
/// `Ok` is the token in hand — a passed preflight leaves the caller
/// provisioned, and the one keystore read serves check and credential both,
/// because on macOS a keyring read can be a permission dialog. The token
/// rides in the `Ok` and never in the vocabulary.
///
/// # Errors
///
/// Every refusal in the first layer that could not stand. Later layers go
/// unconsulted — a preflight already refusing has no business popping a
/// keyring dialog.
#[cfg(feature = "native")]
pub fn manifest(
    binaries: &[PathBuf],
    token_override: Option<String>,
    store: &dyn KeyStore,
) -> Result<String, Vec<CapabilityStatus>> {
    held(vec![git()])?;
    held(binaries.iter().map(|spelling| binary(spelling)).collect())?;
    let (status, token) = github_token(token_override, store);
    token.ok_or_else(|| vec![status])
}

/// One layer, held to: every capability in it must be present, and the
/// refusals of a layer that cannot stand are the whole answer.
#[cfg(feature = "native")]
fn held(layer: Vec<CapabilityStatus>) -> Result<(), Vec<CapabilityStatus>> {
    let refused: Vec<CapabilityStatus> = layer
        .into_iter()
        .filter(|status| !matches!(status.standing, Standing::Present { .. }))
        .collect();
    if refused.is_empty() {
        Ok(())
    } else {
        Err(refused)
    }
}

/// Epik's own layer: git on `PATH`, above the floor.
#[cfg(feature = "native")]
#[must_use]
pub fn git() -> CapabilityStatus {
    let (searched, found) = hunted(Path::new("git"), &env::var_os("PATH").unwrap_or_default());
    let standing = found.map_or(Standing::Absent { searched }, |at| {
        shimmed(&at).unwrap_or_else(|| versioned(&at))
    });
    CapabilityStatus {
        capability: Capability::Git,
        standing,
    }
}

/// The per-agent layer, one binary at a time: the wrapper's declared
/// spelling, resolved explicitly — a path answers for itself, a bare name
/// hunts `PATH` — never left to spawn-time mechanism.
#[cfg(feature = "native")]
#[must_use]
pub fn binary(spelling: &Path) -> CapabilityStatus {
    let (searched, found) = hunted(spelling, &env::var_os("PATH").unwrap_or_default());
    CapabilityStatus {
        capability: Capability::Agent {
            binary: spelling.display().to_string(),
        },
        standing: found.map_or(Standing::Absent { searched }, |at| Standing::Present {
            at: at.display().to_string(),
        }),
    }
}

/// Where the GitHub token stands, and the token itself when it is in hand.
///
/// The keystore rails — `$EPIK_GITHUB_TOKEN`, then the keyring — walked
/// stop by stop, so a refusal can name where it looked, where
/// [`Keys::github_token`](crate::keystore::Keys::github_token) collapses
/// the same walk into a bare `Resolved`.
#[cfg(feature = "native")]
#[must_use]
pub fn github_token(
    token_override: Option<String>,
    store: &dyn KeyStore,
) -> (CapabilityStatus, Option<String>) {
    let env_stop = format!("${GITHUB_OVERRIDE_ENV}");
    let keyring = format!("the {SERVICE}/{GITHUB_ACCOUNT} keyring entry");
    let (standing, token) = match token_override.filter(|token| !token.is_empty()) {
        Some(token) => (Standing::Present { at: env_stop }, Some(token)),
        None => match store.get(GITHUB_ACCOUNT) {
            // An empty entry is filtered like the empty override: a token
            // that is no token refuses here, never as a 401 mid-run.
            Ok(Some(token)) if !token.is_empty() => {
                (Standing::Present { at: keyring }, Some(token))
            }
            Ok(_) => (
                Standing::Absent {
                    searched: vec![env_stop, keyring],
                },
                None,
            ),
            Err(error) => (
                Standing::Unfit {
                    at: keyring,
                    reason: format!("{error:#}"),
                },
                None,
            ),
        },
    };
    (
        CapabilityStatus {
            capability: Capability::GithubToken,
            standing,
        },
        token,
    )
}

/// Resolves one spelling: an explicit path answers for itself, a bare name
/// is hunted down `PATH` — every stop collected so a refusal can name them.
#[cfg(feature = "native")]
fn hunted(spelling: &Path, path: &OsStr) -> (Vec<String>, Option<PathBuf>) {
    if spelling
        .parent()
        .is_some_and(|dir| !dir.as_os_str().is_empty())
    {
        return (
            vec![spelling.display().to_string()],
            runnable(spelling).then(|| spelling.to_owned()),
        );
    }
    let mut searched = Vec::new();
    // An empty component never degrades the hunt to the cwd: an unset or
    // empty `PATH` — the launchd case — searches nowhere, and the refusal
    // says so.
    for dir in env::split_paths(path).filter(|dir| !dir.as_os_str().is_empty()) {
        let candidate = dir.join(spelling);
        if runnable(&candidate) {
            return (searched, Some(candidate));
        }
        searched.push(candidate.display().to_string());
    }
    (searched, None)
}

/// Whether a file at `path` could be spawned: a file this process may
/// execute — the kernel's own `access(X_OK)` answer, the question execvp
/// asks — so the hunt walks past what execvp would walk past.
#[cfg(feature = "native")]
fn runnable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        path.is_file() && rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Apple's `/usr/bin/git` is a shim that pops the developer-tools install
/// dialog when nothing stands behind it, so it is never spawned on spec:
/// the developer directory is consulted through `xcode-select -p` first,
/// and only a settled one makes the shim safe to run. `xcode-select`
/// itself is a real binary, dialog-free.
#[cfg(all(feature = "native", target_os = "macos"))]
fn shimmed(at: &Path) -> Option<Standing> {
    (at == Path::new("/usr/bin/git")
        && !Command::new("/usr/bin/xcode-select")
            .arg("-p")
            .output()
            .is_ok_and(|probe| probe.status.success()))
    .then(|| Standing::Unfit {
        at: at.display().to_string(),
        reason: "Apple's shim with no developer tools behind it — `xcode-select -p` names none, \
                 and spawning the shim would pop the install dialog"
            .to_owned(),
    })
}

#[cfg(all(feature = "native", not(target_os = "macos")))]
#[allow(clippy::unnecessary_wraps, clippy::missing_const_for_fn)] // the macOS twin is the shape
fn shimmed(_: &Path) -> Option<Standing> {
    None
}

/// Spawns the git already vouched for and holds it to the floor.
#[cfg(feature = "native")]
fn versioned(at: &Path) -> Standing {
    let where_found = at.display().to_string();
    let output = match Command::new(at).arg("--version").output() {
        Ok(output) => output,
        Err(error) => {
            return Standing::Unfit {
                at: where_found,
                reason: format!("would not start: {error}"),
            };
        }
    };
    let said = String::from_utf8_lossy(&output.stdout);
    let Some((major, minor)) = version(&said) else {
        return Standing::Unfit {
            at: where_found,
            reason: format!("did not name its version: {:?}", said.trim()),
        };
    };
    if (major, minor) < FLOOR {
        return Standing::Unfit {
            at: where_found,
            reason: format!(
                "version {major}.{minor} is beneath the floor {}.{}",
                FLOOR.0, FLOOR.1
            ),
        };
    }
    Standing::Present { at: where_found }
}

/// The major and minor of a `git --version` line, Apple flavors included.
#[cfg(feature = "native")]
fn version(said: &str) -> Option<(u32, u32)> {
    let mut numbers = said
        .strip_prefix("git version ")?
        .split(|c: char| !c.is_ascii_digit());
    Some((numbers.next()?.parse().ok()?, numbers.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_round_trips_through_json() {
        let statuses = [
            CapabilityStatus {
                capability: Capability::Git,
                standing: Standing::Present {
                    at: "/usr/bin/git".to_owned(),
                },
            },
            CapabilityStatus {
                capability: Capability::Agent {
                    binary: "claude".to_owned(),
                },
                standing: Standing::Absent {
                    searched: vec!["/opt/bin/claude".to_owned()],
                },
            },
            CapabilityStatus {
                capability: Capability::GithubToken,
                standing: Standing::Unfit {
                    at: "the Epik/github keyring entry".to_owned(),
                    reason: "no secret service".to_owned(),
                },
            },
        ];
        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let back: CapabilityStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn a_refusal_renders_the_capability_and_the_locations_searched() {
        let absent = CapabilityStatus {
            capability: Capability::Git,
            standing: Standing::Absent {
                searched: vec!["/a/git".to_owned(), "/b/git".to_owned()],
            },
        };
        assert_eq!(absent.to_string(), "git is absent: searched /a/git, /b/git");

        let unfit = CapabilityStatus {
            capability: Capability::GithubToken,
            standing: Standing::Unfit {
                at: "the Epik/github keyring entry".to_owned(),
                reason: "no secret service".to_owned(),
            },
        };
        assert_eq!(
            unfit.to_string(),
            "the GitHub token at the Epik/github keyring entry is unfit: no secret service"
        );

        let nowhere = CapabilityStatus {
            capability: Capability::Agent {
                binary: "claude".to_owned(),
            },
            standing: Standing::Absent {
                searched: Vec::new(),
            },
        };
        assert_eq!(
            nowhere.to_string(),
            "the agent's claude is absent, with nowhere to search"
        );
    }
}

#[cfg(all(test, feature = "native"))]
// Test scaffolding is entitled to panic; the allow-unwrap-in-tests clippy
// setting only covers #[test] functions, not the helpers here.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod native_tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::keystore::{Counting, InMemory, KeyStore, Unplugged};

    /// Installs an executable file named `name` in `dir`.
    fn tool(dir: &TempDir, name: &str) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn git_versions_parse_in_plain_and_apple_flavors() {
        assert_eq!(version("git version 2.42.0\n"), Some((2, 42)));
        assert_eq!(
            version("git version 2.39.3 (Apple Git-146)\n"),
            Some((2, 39))
        );
        assert_eq!(version("zsh: command not found: git"), None);
        assert_eq!(version("git version mystery"), None);
    }

    #[test]
    fn a_bare_name_is_hunted_down_path_naming_every_stop() {
        let empty = TempDir::new().unwrap();
        let tools = TempDir::new().unwrap();
        let installed = tool(&tools, "present");
        let path = env::join_paths([empty.path(), tools.path()]).unwrap();

        let (searched, found) = hunted(Path::new("present"), &path);
        assert_eq!(found, Some(installed));
        assert_eq!(
            searched,
            [empty.path().join("present").display().to_string()],
            "the hunt stops where it finds"
        );

        let (searched, found) = hunted(Path::new("missing"), &path);
        assert_eq!(found, None);
        assert_eq!(
            searched,
            [
                empty.path().join("missing").display().to_string(),
                tools.path().join("missing").display().to_string(),
            ],
            "a fruitless hunt names every stop"
        );
    }

    #[test]
    fn an_explicit_path_answers_for_itself_without_consulting_path() {
        let tools = TempDir::new().unwrap();
        let installed = tool(&tools, "claude");
        let decoys = TempDir::new().unwrap();
        tool(&decoys, "claude");
        let path = env::join_paths([decoys.path()]).unwrap();

        let (_, found) = hunted(&installed, &path);
        assert_eq!(found, Some(installed));

        let bogus = tools.path().join("nowhere");
        let (searched, found) = hunted(&bogus, &path);
        assert_eq!(found, None);
        assert_eq!(
            searched,
            [bogus.display().to_string()],
            "an explicit spelling is the whole search"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_without_its_execute_bit_is_not_found() {
        let tools = TempDir::new().unwrap();
        let inert = tools.path().join("tool");
        fs::write(&inert, "#!/bin/sh\n").unwrap();
        let path = env::join_paths([tools.path()]).unwrap();

        let (_, found) = hunted(Path::new("tool"), &path);
        assert_eq!(found, None, "presence is runnability, not existence");
    }

    #[cfg(unix)]
    #[test]
    fn execute_permission_is_the_invoking_users_and_the_hunt_walks_past_it() {
        use std::os::unix::fs::PermissionsExt;
        // Executable by group and other but not by owner: `mode & 0o111` is
        // nonzero, yet execvp — and access(X_OK) — would refuse it for us.
        let decoys = TempDir::new().unwrap();
        let untouchable = decoys.path().join("tool");
        fs::write(&untouchable, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&untouchable, fs::Permissions::from_mode(0o055)).unwrap();
        let tools = TempDir::new().unwrap();
        let real = tool(&tools, "tool");
        let path = env::join_paths([decoys.path(), tools.path()]).unwrap();

        let (searched, found) = hunted(Path::new("tool"), &path);

        assert_eq!(found, Some(real), "the hunt continues where execvp would");
        assert_eq!(searched, [untouchable.display().to_string()]);
    }

    #[test]
    fn an_unset_or_empty_path_searches_nowhere_never_the_cwd() {
        let (searched, found) = hunted(Path::new("git"), OsStr::new(""));
        assert_eq!(found, None);
        assert_eq!(
            searched,
            Vec::<String>::new(),
            "an empty PATH component is not the cwd"
        );
    }

    #[test]
    fn the_token_rails_name_where_they_looked_and_hand_over_what_they_found() {
        // The environment answers first, and the store is never touched —
        // one read per secret is a promise worth keeping.
        let counting = Counting::default();
        let reads = counting.reads();
        let (status, token) = github_token(Some("ghp-env".to_owned()), &counting);
        assert_eq!(token.as_deref(), Some("ghp-env"));
        let Standing::Present { at } = &status.standing else {
            panic!("an environment token is present: {status:?}");
        };
        assert!(at.contains("EPIK_GITHUB_TOKEN"), "{at}");
        assert!(reads.borrow().is_empty(), "the keyring went unconsulted");

        // A stored token comes back naming the keyring.
        let mut stored = InMemory::default();
        stored.set("github", "ghp-kept").unwrap();
        let (status, token) = github_token(None, &stored);
        assert_eq!(token.as_deref(), Some("ghp-kept"));
        assert!(matches!(
            &status.standing,
            Standing::Present { at } if at.contains("Epik/github")
        ));

        // Nothing anywhere names both rails.
        let (status, token) = github_token(None, &InMemory::default());
        assert_eq!(token, None);
        let Standing::Absent { searched } = &status.standing else {
            panic!("no token anywhere is absent: {status:?}");
        };
        assert!(
            searched
                .iter()
                .any(|stop| stop.contains("EPIK_GITHUB_TOKEN"))
        );
        assert!(searched.iter().any(|stop| stop.contains("Epik/github")));

        // An empty override is no override.
        let (status, token) = github_token(Some(String::new()), &InMemory::default());
        assert_eq!(token, None);
        assert!(matches!(status.standing, Standing::Absent { .. }));

        // An empty keyring entry is no token either: absent, the keyring
        // named among the places searched.
        let mut hollow = InMemory::default();
        hollow.set("github", "").unwrap();
        let (status, token) = github_token(None, &hollow);
        assert_eq!(token, None);
        let Standing::Absent { searched } = &status.standing else {
            panic!("an empty entry is absence, not presence: {status:?}");
        };
        assert!(searched.iter().any(|stop| stop.contains("Epik/github")));

        // A keyring that will not answer is unfit, not absent.
        let (status, token) = github_token(None, &Unplugged);
        assert_eq!(token, None);
        assert!(matches!(
            &status.standing,
            Standing::Unfit { reason, .. } if reason.contains("no default store")
        ));
    }

    #[test]
    fn the_manifest_refuses_layer_by_layer_leaving_the_keystore_unconsulted() {
        // Real git on the test machine carries the first layer; the bogus
        // agent binary refuses the second, so the third — a keyring read,
        // which on macOS can be a permission dialog — never happens.
        let counting = Counting::default();
        let reads = counting.reads();

        let refusals = manifest(&[PathBuf::from("/nowhere/claude")], None, &counting).unwrap_err();

        let [refusal] = refusals.as_slice() else {
            panic!("exactly the agent's refusal: {refusals:?}");
        };
        assert_eq!(
            refusal.capability,
            Capability::Agent {
                binary: "/nowhere/claude".to_owned()
            }
        );
        assert!(matches!(&refusal.standing, Standing::Absent { searched }
            if searched == &["/nowhere/claude".to_owned()]));
        assert!(
            reads.borrow().is_empty(),
            "no keyring dialog behind a refusal"
        );
    }

    #[test]
    fn a_passed_manifest_hands_over_the_token() {
        let mut store = InMemory::default();
        store.set("github", "ghp-kept").unwrap();

        let token = manifest(&[], None, &store).unwrap();

        assert_eq!(token, "ghp-kept");
    }
}
