//! The rails a secret reaches git on: askpass, and the environment.
//!
//! Git asks for a username and a password by running the program
//! `GIT_ASKPASS` names. [`Askpass`] is that program, a script on disk
//! for as long as one git runs, answering from two variables in that
//! git's environment, which die with it. The environment of that one
//! child is the whole of where the secret goes: a [`Command`]'s argv
//! is made of other things, and the script is the same text for every
//! secret. The same environment empties git's list of credential
//! helpers, so the [`Credentials`] given are the ones that answer,
//! ahead of the machine's keychain.
//!
//! [`Command`]: super::commands::Command

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::keystore::Secret;
use crate::temp;

/// What git's askpass asks for: a username, and the secret that answers
/// for it.
#[derive(Clone, Debug)]
pub struct Credentials {
    pub username: String,
    pub secret: Secret,
}

/// Git runs the script once for the username and once for the
/// password, saying which in its one argument.
const SCRIPT: &str = "#!/bin/sh\n\
case \"$1\" in\n\
  Username*) printf '%s\\n' \"$EPIK_GIT_USERNAME\" ;;\n\
  *) printf '%s\\n' \"$EPIK_GIT_PASSWORD\" ;;\n\
esac\n";

/// The askpass script on disk, at a path of its own, until dropped.
pub(super) struct Askpass(PathBuf);

impl Askpass {
    pub(super) fn new() -> Result<Self> {
        let path = temp::unique("epik-askpass");
        // Made executable, and its owner's alone, as it is made.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)
            .context("could not make the askpass script")?;
        let askpass = Self(path);
        file.write_all(SCRIPT.as_bytes())
            .context("could not write the askpass script")?;
        Ok(askpass)
    }

    /// The environment of a git that answers a challenge with
    /// `credentials`, and with nothing else.
    pub(super) fn env(&self, credentials: &Credentials) -> [(String, Secret); 6] {
        let script = self.0.to_string_lossy();
        [
            ("GIT_ASKPASS", Secret::from(&*script)),
            ("EPIK_GIT_USERNAME", Secret::from(&*credentials.username)),
            ("EPIK_GIT_PASSWORD", credentials.secret.clone()),
            // One config entry, as `-c credential.helper=` would give:
            // the empty value empties the list of helpers.
            ("GIT_CONFIG_COUNT", Secret::from("1")),
            ("GIT_CONFIG_KEY_0", Secret::from("credential.helper")),
            ("GIT_CONFIG_VALUE_0", Secret::from("")),
        ]
        .map(|(name, value)| (name.to_owned(), value))
    }
}

impl Drop for Askpass {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::git::commands::{Clone, Command, Fetch, Push, Remote};
    use crate::git::testing::{INTRUDED, INTRUDER, OFFERED, aimed, credentials, guarded, seeded};
    use crate::git::url::Url;
    use crate::git::{Repository, TIMEOUT, run_as, run_within};
    use crate::process::Process;
    use crate::temp::Scratch;

    /// Long enough for git to ask, be refused, and ask again.
    const PATIENCE: Duration = Duration::from_secs(30);

    #[test]
    fn the_script_answers_username_and_password_from_the_environment() {
        let askpass = Askpass::new().unwrap();
        let ask = |prompt: &str| {
            let program = askpass.0.to_str().unwrap();
            let env = askpass.env(&credentials());
            Process::new(program, vec![prompt.to_owned()], "/", env, None)
                .unwrap()
                .finish()
                .unwrap()
                .stdout
                .unwrap()
        };
        assert_eq!(ask("Username for 'https://github.com': "), "x-access-token");
        assert_eq!(
            ask("Password for 'https://x-access-token@github.com': "),
            "ghp_sesame"
        );
        assert!(!SCRIPT.contains("sesame"), "the script carries no secret");
    }

    #[test]
    fn the_script_is_on_disk_until_the_drop() {
        let askpass = Askpass::new().unwrap();
        let path = askpass.0.clone();
        assert!(path.is_file());
        drop(askpass);
        assert!(!path.exists());
    }

    /// The whole rail, with no GitHub: whichever command meets a remote
    /// that demands authentication, what reaches it is the credentials,
    /// which were in the environment and in no argument.
    #[test]
    fn a_clone_a_fetch_and_a_push_each_answer_the_challenge_with_the_credentials() {
        let scratch = Scratch::new("askpass-challenge");
        let (_, work) = seeded(&scratch);
        let into = PathBuf::from(scratch.join("clone"));
        let commands: [fn(Repository, PathBuf, Url) -> Command; 3] = [
            |_, into, url| Command::Clone(Clone::new(url, into, None).unwrap()),
            |work, _, _| Command::Fetch(Fetch::new(work, Remote::origin())),
            |work, _, _| {
                Command::Push(Push::new(work, Remote::origin(), "main".to_owned()).unwrap())
            },
        ];
        for command in commands {
            let (url, seen) = guarded();
            aimed(&work, url.clone());
            let command = command(work.clone(), into.clone(), url.parse().unwrap());
            let argv = command.argv().join(" ");
            assert!(!argv.contains("sesame"), "{argv}");

            let error = run_as(&command, &credentials()).unwrap_err().to_string();
            assert!(error.contains("Authentication failed"), "{error}");
            assert!(!error.contains("sesame"), "{error}");
            assert_eq!(
                seen.recv_timeout(PATIENCE).as_deref(),
                Ok(OFFERED),
                "{argv}"
            );
        }
    }

    /// A clone has no repository yet whose config could name a helper,
    /// so the machine's does, from a file of the test's own. The helper
    /// answers a clone left to it, and the rails' environment puts the
    /// credentials ahead of it.
    #[test]
    fn a_clone_answers_with_the_credentials_ahead_of_a_helper_on_the_machine() {
        let scratch = Scratch::new("askpass-helper");
        let config = format!("[credential]\n\thelper = \"{INTRUDER}\"\n");
        scratch.write("gitconfig", &config);
        let machine = || {
            let config = Secret::from(scratch.join("gitconfig"));
            [("GIT_CONFIG_GLOBAL".to_owned(), config)]
        };
        let clone = |url: String, into: &str| {
            Command::Clone(
                Clone::new(url.parse().unwrap(), scratch.join(into).into(), None).unwrap(),
            )
        };

        let (url, seen) = guarded();
        run_within(&clone(url, "helped"), machine(), TIMEOUT).unwrap();
        assert_eq!(seen.recv_timeout(PATIENCE).as_deref(), Ok(INTRUDED));

        let (url, seen) = guarded();
        let askpass = Askpass::new().unwrap();
        let env = machine().into_iter().chain(askpass.env(&credentials()));
        run_within(&clone(url, "railed"), env, TIMEOUT).unwrap();
        assert_eq!(seen.recv_timeout(PATIENCE).as_deref(), Ok(OFFERED));
    }
}
