//! The universal macOS `.dmg`: built as the release workflow builds it,
//! then checked for what the environment promised.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub type Error = Box<dyn std::error::Error>;

/// What the build environment promised the `.dmg` would be, read from the
/// same variables the Tauri bundler reads. The check demands each promise.
#[derive(Debug, PartialEq, Eq)]
pub struct Promise {
    pub signed: bool,
    pub notarized: bool,
}

impl Promise {
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Self {
        let set = |name| var(name).is_some();
        let apple_id = set("APPLE_ID") && set("APPLE_PASSWORD") && set("APPLE_TEAM_ID");
        let api_key = set("APPLE_API_KEY") && set("APPLE_API_ISSUER");
        Promise {
            signed: set("APPLE_SIGNING_IDENTITY"),
            notarized: apple_id || api_key,
        }
    }
}

/// Both architectures in `lipo -archs` output.
fn universal(archs: &str) -> bool {
    let archs: Vec<_> = archs.split_whitespace().collect();
    archs.contains(&"arm64") && archs.contains(&"x86_64")
}

/// Gatekeeper's assessment names a notarized source.
fn notarized(spctl: &str) -> bool {
    spctl
        .lines()
        .any(|line| line.trim() == "source=Notarized Developer ID")
}

fn app_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../epik-app")
}

fn bundle_dir() -> PathBuf {
    app_dir().join("../../target/universal-apple-darwin/release/bundle/dmg")
}

/// Builds the `.dmg` as the release workflow does.
pub fn build() -> Result<PathBuf, Error> {
    let status = Command::new("cargo")
        .args([
            "tauri",
            "build",
            "--target",
            "universal-apple-darwin",
            "--bundles",
            "dmg",
        ])
        .current_dir(app_dir())
        .status()?;
    if !status.success() {
        return Err("cargo tauri build failed".into());
    }
    built()
}

/// The one `.dmg` the build left behind.
pub fn built() -> Result<PathBuf, Error> {
    let dir = bundle_dir();
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut dmgs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension() == Some(OsStr::new("dmg")))
        .collect();
    match dmgs.len() {
        1 => Ok(dmgs.remove(0)),
        n => Err(format!("expected one .dmg in {}, found {n}", dir.display()).into()),
    }
}

/// Checks the `.dmg` against what the environment promised: universal
/// always, signed and notarized when the credentials were there.
pub fn check(dmg: &Path) -> Result<(), Error> {
    let promise = Promise::from_env(|name| std::env::var(name).ok());
    println!("Checking {}", dmg.display());
    let mount = Mount::attach(dmg)?;
    let app = mount.path.join("Epik.app");

    let archs = run(
        "lipo",
        &[
            "-archs".as_ref(),
            app.join("Contents/MacOS/epik-app").as_os_str(),
        ],
    )?;
    println!("Architectures: {}", archs.trim());
    if !universal(&archs) {
        return Err(format!("the app is not universal: {}", archs.trim()).into());
    }

    if promise.signed {
        run(
            "codesign",
            &[
                "--verify".as_ref(),
                "--deep".as_ref(),
                "--strict".as_ref(),
                app.as_os_str(),
            ],
        )?;
        let details = run("codesign", &["-dvv".as_ref(), app.as_os_str()])?;
        for line in details
            .lines()
            .filter(|l| l.starts_with("Authority=") || l.starts_with("TeamIdentifier="))
        {
            println!("{line}");
        }
    } else {
        println!("No APPLE_SIGNING_IDENTITY: the signature is unchecked.");
    }

    if promise.notarized {
        let assessment = run(
            "spctl",
            &[
                "--assess".as_ref(),
                "--type".as_ref(),
                "execute".as_ref(),
                "-vv".as_ref(),
                app.as_os_str(),
            ],
        )?;
        print!("{assessment}");
        if !notarized(&assessment) {
            return Err("Gatekeeper does not see a notarized app".into());
        }
        run(
            "xcrun",
            &["stapler".as_ref(), "validate".as_ref(), app.as_os_str()],
        )?;
        println!("Notarized and stapled.");
    } else {
        println!("No notarization credentials: notarization is unchecked.");
    }
    Ok(())
}

/// Runs a command, returning everything it printed; a failure's output is
/// the error.
fn run(program: &str, args: &[&OsStr]) -> Result<String, Error> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(text)
    } else {
        Err(format!("{program} {}: {}", output.status, text.trim()).into())
    }
}

/// A mounted `.dmg`, detached when dropped.
struct Mount {
    path: PathBuf,
}

impl Mount {
    fn attach(dmg: &Path) -> Result<Self, Error> {
        let path = std::env::temp_dir().join(format!("epik-dmg-{}", std::process::id()));
        run(
            "hdiutil",
            &[
                "attach".as_ref(),
                "-nobrowse".as_ref(),
                "-readonly".as_ref(),
                "-mountpoint".as_ref(),
                path.as_os_str(),
                dmg.as_os_str(),
            ],
        )?;
        Ok(Mount { path })
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-quiet"])
            .arg(&self.path)
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(set: &'a [&'a str]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| set.contains(&name).then(|| "x".into())
    }

    #[test]
    fn nothing_promised_without_credentials() {
        assert_eq!(
            Promise::from_env(env(&[])),
            Promise {
                signed: false,
                notarized: false
            }
        );
    }

    #[test]
    fn an_identity_promises_a_signature() {
        assert_eq!(
            Promise::from_env(env(&["APPLE_SIGNING_IDENTITY"])),
            Promise {
                signed: true,
                notarized: false
            }
        );
    }

    #[test]
    fn an_apple_id_team_and_password_promise_notarization() {
        let vars = [
            "APPLE_SIGNING_IDENTITY",
            "APPLE_ID",
            "APPLE_PASSWORD",
            "APPLE_TEAM_ID",
        ];
        assert_eq!(
            Promise::from_env(env(&vars)),
            Promise {
                signed: true,
                notarized: true
            }
        );
    }

    #[test]
    fn an_apple_id_without_a_team_promises_nothing_more() {
        let vars = ["APPLE_SIGNING_IDENTITY", "APPLE_ID", "APPLE_PASSWORD"];
        assert_eq!(
            Promise::from_env(env(&vars)),
            Promise {
                signed: true,
                notarized: false
            }
        );
    }

    #[test]
    fn an_api_key_and_issuer_promise_notarization() {
        let vars = [
            "APPLE_SIGNING_IDENTITY",
            "APPLE_API_KEY",
            "APPLE_API_ISSUER",
        ];
        assert_eq!(
            Promise::from_env(env(&vars)),
            Promise {
                signed: true,
                notarized: true
            }
        );
    }

    #[test]
    fn universal_means_both_architectures() {
        assert!(universal("x86_64 arm64"));
        assert!(universal("arm64 x86_64\n"));
        assert!(!universal("arm64"));
        assert!(!universal("x86_64"));
    }

    #[test]
    fn notarized_is_what_gatekeeper_says() {
        let accepted = "/tmp/epik/Epik.app: accepted\nsource=Notarized Developer ID\norigin=Developer ID Application: William McNeill (JAYV3P4GAY)\n";
        assert!(notarized(accepted));
        let unnotarized = "/tmp/epik/Epik.app: accepted\nsource=Developer ID\n";
        assert!(!notarized(unnotarized));
        let rejected = "/tmp/epik/Epik.app: rejected\nsource=Unnotarized Developer ID\n";
        assert!(!notarized(rejected));
    }
}
