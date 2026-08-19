//! Where refs live and how to write them.
//!
//! A [`Forge`] is the seam between the merge machinery and whatever
//! hosts a repository: the remote a feature branch is pushed to, and
//! the credentials git pushes with. Thin by design in this slice — pull
//! requests and check runs grow here only when the review conversation
//! arrives; a thin trait that names the right boundary beats a fat one
//! that names the wrong boundary.
//!
//! [`GitHub`] implements it with the token already on the
//! [`keystore`](crate::keystore) rails, so pushing needs nothing
//! installed on the machine. [`push`] is the one writing verb: the
//! credentials answer git's askpass through a script that dies with the
//! push and environment variables that die with the process — never
//! argv, never a file with the secret in it — and ambient credential
//! helpers are switched off so the forge's own credentials are the ones
//! that speak. A test implements the trait with a bare directory and no
//! credentials at all.

use std::path::Path;

use crate::keystore::Secret;

/// Where refs live and how to write them: the remote, and the
/// credentials git pushes with.
pub trait Forge {
    /// The remote refs are written to — anything `git push` takes: an
    /// HTTPS URL, or an absolute local path in tests.
    fn remote(&self) -> String;

    /// The credentials for that remote. `None` pushes bare, which is
    /// what a file remote wants.
    fn credentials(&self) -> Option<Credentials>;
}

/// What git's askpass asks for: a username, and the secret that answers
/// for it.
#[derive(Clone, Debug)]
pub struct Credentials {
    pub username: String,
    pub secret: Secret,
}

/// GitHub as a forge: one repository, written over HTTPS as the
/// `x-access-token` user with the token from the keystore. The API
/// client in [`github`](crate::github) is the same system's other face;
/// this one only knows where the refs are.
#[derive(Clone, Debug)]
pub struct GitHub {
    pub repo: crate::github::Repo,
    pub token: Secret,
}

impl Forge for GitHub {
    fn remote(&self) -> String {
        format!("https://github.com/{}.git", self.repo)
    }

    fn credentials(&self) -> Option<Credentials> {
        Some(Credentials {
            username: "x-access-token".to_owned(),
            secret: self.token.clone(),
        })
    }
}

/// The askpass script: git calls it once for the username and once for
/// the password, saying which in its one argument, and it answers from
/// the environment — the secret is never written into the script.
const ASKPASS: &str = "#!/bin/sh\n\
case \"$1\" in\n\
  Username*) printf '%s\\n' \"$EPIK_GIT_USERNAME\" ;;\n\
  *) printf '%s\\n' \"$EPIK_GIT_PASSWORD\" ;;\n\
esac\n";

/// The askpass script on disk, for exactly as long as one push runs.
struct Askpass(std::path::PathBuf);

impl Askpass {
    fn new() -> Result<Self, String> {
        use std::os::unix::fs::PermissionsExt;
        // The clock alone can collide when two pushes start in the same
        // tick, and a shared path would let one Drop delete the other's
        // live script mid-auth — a process-wide count settles it.
        static NTH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "epik-askpass-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default(),
            NTH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&path, ASKPASS)
            .map_err(|error| format!("could not write the askpass script: {error}"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("could not mark the askpass script executable: {error}"))?;
        Ok(Self(path))
    }

    fn path(&self) -> Result<&str, String> {
        self.0
            .to_str()
            .ok_or_else(|| "the askpass path is not valid unicode".to_owned())
    }
}

impl Drop for Askpass {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Pushes `branch` of the repository in `directory` to `forge`'s
/// remote, by explicit refspec so no upstream configuration is needed
/// or touched.
///
/// # Errors
///
/// Git's own words: the remote unreachable, the credentials refused,
/// the ref not fast-forwardable.
pub fn push(directory: &Path, branch: &str, forge: &impl Forge) -> Result<(), String> {
    let directory = directory
        .to_str()
        .ok_or("the workspace path is not valid unicode")?;
    let remote = forge.remote();
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    match forge.credentials() {
        None => {
            crate::git::plumbing(&["-C", directory, "push", "--", &remote, &refspec])?;
        }
        Some(credentials) => {
            let askpass = Askpass::new()?;
            // credential.helper= empties the helper list: the machine's
            // keychain must not answer for the forge's own credentials.
            crate::git::plumbing_with(
                &[
                    "-C",
                    directory,
                    "-c",
                    "credential.helper=",
                    "push",
                    "--",
                    &remote,
                    &refspec,
                ],
                &[
                    ("GIT_ASKPASS", askpass.path()?),
                    ("EPIK_GIT_USERNAME", &credentials.username),
                    // The one place these bytes leave their Secret: into
                    // the environment of one git child, gone with it.
                    ("EPIK_GIT_PASSWORD", credentials.secret.reveal()),
                ],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    /// A forge for tests: a bare directory, no credentials — pushing to
    /// it is pushing to a path.
    struct Local(String);

    impl Forge for Local {
        fn remote(&self) -> String {
            self.0.clone()
        }

        fn credentials(&self) -> Option<Credentials> {
            None
        }
    }

    /// A forge whose remote is a loopback HTTP listener that demands
    /// authentication — how the askpass rails are proven without any
    /// network or any GitHub.
    struct Guarded(String);

    impl Forge for Guarded {
        fn remote(&self) -> String {
            self.0.clone()
        }

        fn credentials(&self) -> Option<Credentials> {
            Some(Credentials {
                username: "x-access-token".to_owned(),
                secret: "ghp_sesame".into(),
            })
        }
    }

    /// A scratch directory that cleans up after itself.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "epik-forge-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn join(&self, name: &str) -> String {
            self.0.join(name).to_str().unwrap().to_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A working repository with one commit, plus a bare remote.
    fn seeded(scratch: &Scratch) -> (String, String) {
        let work = scratch.join("work");
        let bare = scratch.join("remote.git");
        let git = |args: &[&str]| crate::git::plumbing(args).unwrap();
        git(&["init", "--initial-branch=main", &work]);
        git(&["-C", &work, "config", "user.name", "Test"]);
        git(&["-C", &work, "config", "user.email", "test@example.com"]);
        git(&["-C", &work, "config", "commit.gpgsign", "false"]);
        std::fs::write(std::path::Path::new(&work).join("hello.txt"), "hello\n").unwrap();
        git(&["-C", &work, "add", "hello.txt"]);
        git(&["-C", &work, "commit", "-m", "the first commit"]);
        git(&["init", "--bare", "--initial-branch=main", &bare]);
        (work, bare)
    }

    #[test]
    fn github_pushes_its_repository_over_https_as_the_token_user() {
        let forge = GitHub {
            repo: crate::github::Repo::new("epik-agent", "Epik"),
            token: "ghp_sesame".into(),
        };
        assert_eq!(forge.remote(), "https://github.com/epik-agent/Epik.git");
        let credentials = forge.credentials().unwrap();
        assert_eq!(credentials.username, "x-access-token");
        assert_eq!(credentials.secret.reveal(), "ghp_sesame");
    }

    #[test]
    fn a_push_writes_the_branch_to_a_bare_remote() {
        let scratch = Scratch::new("push");
        let (work, bare) = seeded(&scratch);

        push(std::path::Path::new(&work), "main", &Local(bare.clone())).unwrap();

        let verified = git2::Repository::open(&bare).unwrap();
        let pushed = verified
            .find_branch("main", git2::BranchType::Local)
            .unwrap();
        assert_eq!(
            pushed
                .get()
                .peel_to_commit()
                .unwrap()
                .message()
                .unwrap()
                .trim(),
            "the first commit"
        );
    }

    #[test]
    fn a_remote_that_is_not_there_fails_in_gits_own_words() {
        let scratch = Scratch::new("nowhere");
        let (work, _) = seeded(&scratch);
        let gone = scratch.join("gone.git");

        let error = push(std::path::Path::new(&work), "main", &Local(gone)).unwrap_err();
        assert!(error.contains("gone.git"), "{error}");
    }

    #[test]
    fn the_askpass_script_answers_username_and_password_from_the_environment() {
        let askpass = Askpass::new().unwrap();
        let ask = |prompt: &str| {
            let output = std::process::Command::new(askpass.path().unwrap())
                .arg(prompt)
                .env("EPIK_GIT_USERNAME", "x-access-token")
                .env("EPIK_GIT_PASSWORD", "ghp_sesame")
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap()
        };
        assert_eq!(
            ask("Username for 'https://github.com': "),
            "x-access-token\n"
        );
        assert_eq!(
            ask("Password for 'https://x-access-token@github.com': "),
            "ghp_sesame\n"
        );
        assert!(
            !ASKPASS.contains("sesame"),
            "the script itself carries no secret"
        );
    }

    /// The whole rail, no GitHub: a loopback listener demands Basic
    /// authentication, and what arrives is the forge's own credentials —
    /// which never touched argv, only askpass and the environment.
    #[test]
    fn a_credentialed_push_answers_the_challenge_with_the_token() {
        let scratch = Scratch::new("challenge");
        let (work, _) = seeded(&scratch);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/repo.git", listener.local_addr().unwrap());
        let (seen_in, seen) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let mut reader = BufReader::new(stream);
                let mut authorization = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Authorization: ") {
                        authorization = Some(value.trim().to_owned());
                    }
                }
                let mut stream = reader.into_inner();
                let _ = stream.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\n\
                      WWW-Authenticate: Basic realm=\"epik\"\r\n\
                      Content-Length: 0\r\nConnection: close\r\n\r\n",
                );
                if let Some(authorization) = authorization {
                    let _ = seen_in.send(authorization);
                    break;
                }
            }
        });

        let error = push(std::path::Path::new(&work), "main", &Guarded(url)).unwrap_err();
        assert!(
            !error.contains("ghp_sesame"),
            "the failure never quotes the secret: {error}"
        );

        let authorization = seen
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("git re-asked with credentials");
        let encoded = base64("x-access-token:ghp_sesame");
        assert_eq!(authorization, format!("Basic {encoded}"));
    }

    /// Just enough base64 to spell one expected header.
    fn base64(plain: &str) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in plain.as_bytes().chunks(3) {
            let bits = chunk.iter().enumerate().fold(0u32, |bits, (i, byte)| {
                bits | u32::from(*byte) << (16 - 8 * i)
            });
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(char::from(ALPHABET[(bits >> (18 - 6 * i)) as usize & 63]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[test]
    fn the_hand_rolled_base64_spells_the_reference_values() {
        assert_eq!(base64("f"), "Zg==");
        assert_eq!(base64("fo"), "Zm8=");
        assert_eq!(base64("foo"), "Zm9v");
        assert_eq!(base64("user:pass"), "dXNlcjpwYXNz");
    }

    /// A drop is enough to take the script off disk.
    #[test]
    fn the_askpass_script_dies_with_its_push() {
        let path = {
            let askpass = Askpass::new().unwrap();
            assert!(askpass.0.is_file());
            askpass.0.clone()
        };
        assert!(!path.exists());
    }
}
