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

use std::os::unix::fs::PermissionsExt;
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
        std::fs::write(&path, SCRIPT).context("could not write the askpass script")?;
        let askpass = Self(path);
        std::fs::set_permissions(&askpass.0, std::fs::Permissions::from_mode(0o700))
            .context("could not mark the askpass script executable")?;
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
    use crate::git::commands::{Command, Push, Remote};
    use crate::git::push_as;
    use crate::git::testing::{aimed, guarded, seeded};
    use crate::process::Process;
    use crate::temp::Scratch;

    fn credentials() -> Credentials {
        Credentials {
            username: "x-access-token".to_owned(),
            secret: "ghp_sesame".into(),
        }
    }

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

    /// The whole rail, with no GitHub: what reaches a remote that
    /// demands authentication is the credentials, which were in the
    /// environment and in no argument.
    #[test]
    fn a_push_answers_the_challenge_with_the_credentials() {
        let scratch = Scratch::new("askpass-challenge");
        let (_, work) = seeded(&scratch);
        let (url, seen) = guarded();
        aimed(&work, url);
        let push = Push::new(work, Remote::origin(), "main".to_owned()).unwrap();
        let argv = Command::Push(push.clone()).argv().join(" ");
        assert!(!argv.contains("sesame"), "{argv}");

        let error = push_as(push, &credentials()).unwrap_err().to_string();
        assert!(!error.contains("sesame"), "{error}");

        let authorization = seen
            .recv_timeout(Duration::from_secs(30))
            .expect("git asked again, with credentials");
        // Base64 of `x-access-token:ghp_sesame`.
        assert_eq!(authorization, "Basic eC1hY2Nlc3MtdG9rZW46Z2hwX3Nlc2FtZQ==");
    }
}
