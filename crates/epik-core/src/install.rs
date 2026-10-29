//! Installing private copies of `gh` and Node under `~/.epik/tools/`.
//!
//! Where a tool isn't on the machine, the setup checklist installs Epik's
//! own copy: the current release for this OS and CPU, from the project's
//! own releases, checked against the SHA-256 checksum the project publishes
//! with it before anything is unpacked. An older copy is replaced only once
//! the new one is verified and unpacked. Nothing is installed system-wide.

use std::fs;
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::setup;

/// The largest download accepted. Node's archives are about 50 MB.
const MAX_DOWNLOAD: u64 = 256 * 1024 * 1024;

/// An operating system Epik runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux,
    Windows,
}

/// A CPU Epik runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cpu {
    X64,
    Arm64,
}

/// An operating system and CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    pub cpu: Cpu,
}

impl Platform {
    /// This machine's, or `None` on one Epik doesn't install tools for.
    pub fn current() -> Option<Self> {
        let os = match std::env::consts::OS {
            "macos" => Os::Mac,
            "linux" => Os::Linux,
            "windows" => Os::Windows,
            _ => return None,
        };
        let cpu = match std::env::consts::ARCH {
            "x86_64" => Cpu::X64,
            "aarch64" => Cpu::Arm64,
            _ => return None,
        };
        Some(Self { os, cpu })
    }
}

/// Where releases come from. Tests point these at a local server.
#[derive(Debug, Clone)]
pub struct Sources {
    /// GitHub's REST API, which names `gh`'s latest release.
    pub github_api: String,
    /// GitHub, which serves `gh`'s release files.
    pub github: String,
    /// Node's distribution directory.
    pub node_dist: String,
}

impl Default for Sources {
    fn default() -> Self {
        Self {
            github_api: "https://api.github.com".to_owned(),
            github: "https://github.com".to_owned(),
            node_dist: "https://nodejs.org/dist".to_owned(),
        }
    }
}

/// A release file: its name, where it is, and where its checksum list is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub file: String,
    pub url: String,
    pub checksums: String,
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .user_agent(format!("epik/{}", crate::version()))
        .build()
        .new_agent();
    agent
        .get(url)
        .call()
        .map_err(|error| format!("couldn't download {url}: {error}"))?
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD)
        .read_to_vec()
        .map_err(|error| format!("couldn't download {url}: {error}"))
}

fn get_text(url: &str) -> Result<String, String> {
    String::from_utf8(get(url)?).map_err(|_| format!("{url} isn't text"))
}

/// The latest `gh` release's version, without its `v`.
pub fn latest_gh(sources: &Sources) -> Result<String, String> {
    let url = format!("{}/repos/cli/cli/releases/latest", sources.github_api);
    let release: Value = serde_json::from_str(&get_text(&url)?)
        .map_err(|error| format!("unexpected answer from {url}: {error}"))?;
    release["tag_name"]
        .as_str()
        .map(|tag| tag.trim_start_matches('v').to_owned())
        .ok_or_else(|| format!("{url} names no release"))
}

/// `gh` `version`'s file for `platform`.
pub fn gh_release(sources: &Sources, version: &str, platform: Platform) -> Release {
    let os = match platform.os {
        Os::Mac => "macOS",
        Os::Linux => "linux",
        Os::Windows => "windows",
    };
    let cpu = match platform.cpu {
        Cpu::X64 => "amd64",
        Cpu::Arm64 => "arm64",
    };
    let extension = if platform.os == Os::Linux {
        "tar.gz"
    } else {
        "zip"
    };
    let file = format!("gh_{version}_{os}_{cpu}.{extension}");
    let base = format!("{}/cli/cli/releases/download/v{version}", sources.github);
    Release {
        version: version.to_owned(),
        url: format!("{base}/{file}"),
        checksums: format!("{base}/gh_{version}_checksums.txt"),
        file,
    }
}

/// The latest Node release in Epik's major version, for `platform`, from
/// the checksum list of `latest-v26.x`, which names every file.
pub fn node_release(sources: &Sources, platform: Platform) -> Result<Release, String> {
    let base = format!("{}/latest-v{}.x", sources.node_dist, setup::NODE_MAJOR);
    let checksums = format!("{base}/SHASUMS256.txt");
    let list = get_text(&checksums)?;
    let os = match platform.os {
        Os::Mac => "darwin",
        Os::Linux => "linux",
        Os::Windows => "win",
    };
    let cpu = match platform.cpu {
        Cpu::X64 => "x64",
        Cpu::Arm64 => "arm64",
    };
    let extension = if platform.os == Os::Windows {
        "zip"
    } else {
        "tar.gz"
    };
    let suffix = format!("-{os}-{cpu}.{extension}");
    let file = list
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .find(|file| file.starts_with("node-v") && file.ends_with(&suffix))
        .ok_or_else(|| format!("{checksums} lists no Node for {os} {cpu}"))?;
    let version = file
        .trim_start_matches("node-v")
        .trim_end_matches(&suffix)
        .to_owned();
    Ok(Release {
        version,
        url: format!("{base}/{file}"),
        file: file.to_owned(),
        checksums,
    })
}

/// The latest Node release's version for this machine.
pub fn latest_node(sources: &Sources) -> Result<String, String> {
    let platform = Platform::current().ok_or("Epik installs no Node for this machine")?;
    Ok(node_release(sources, platform)?.version)
}

/// The latest releases, as read from the internet at most once a day and
/// kept in `<root>/tools/latest.json`, so checking setup before a build
/// rarely waits on the network.
pub struct Cached {
    pub root: PathBuf,
    pub sources: Sources,
}

/// How long the latest releases are kept before they are read again.
const LATEST_FOR: chrono::TimeDelta = chrono::TimeDelta::days(1);

impl Cached {
    fn file(&self) -> PathBuf {
        setup::tools_dir(&self.root).join("latest.json")
    }

    /// The cached releases, refreshed when a day old. None where neither
    /// the cache nor the internet says.
    fn read(&self) -> Value {
        let cached: Value = fs::read_to_string(self.file())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        let fresh = cached["checked_at"]
            .as_str()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .is_some_and(|at| chrono::Utc::now() - at.to_utc() < LATEST_FOR);
        if fresh {
            return cached;
        }
        let latest = serde_json::json!({
            "checked_at": chrono::Utc::now().to_rfc3339(),
            "gh": latest_gh(&self.sources).ok(),
            "node": latest_node(&self.sources).ok(),
        });
        if latest["gh"].is_null() && latest["node"].is_null() {
            return cached;
        }
        if let Some(dir) = self.file().parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(self.file(), latest.to_string());
        latest
    }
}

impl setup::Latest for Cached {
    fn version(&self, tool: setup::Item) -> Option<String> {
        let key = match tool {
            setup::Item::Gh => "gh",
            setup::Item::Node => "node",
            _ => return None,
        };
        self.read()[key].as_str().map(str::to_owned)
    }
}

/// Downloads `release` and returns its bytes, once their SHA-256 matches
/// the one its checksum list gives for it.
fn download_verified(release: &Release) -> Result<Vec<u8>, String> {
    let list = get_text(&release.checksums)?;
    let expected = list
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(_, file)| file.trim().trim_start_matches('*') == release.file)
        .map(|(sum, _)| sum.to_ascii_lowercase())
        .ok_or_else(|| {
            format!(
                "{} lists no checksum for {}",
                release.checksums, release.file
            )
        })?;
    let bytes = get(&release.url)?;
    let actual: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        return Err(format!(
            "{}'s checksum doesn't match the published one, so it wasn't installed",
            release.file
        ));
    }
    Ok(bytes)
}

/// Unpacks a `.tar.gz` or `.zip` archive into `dir`.
fn unpack(file: &str, bytes: &[u8], dir: &Path) -> Result<(), String> {
    let failed = |error: &dyn std::fmt::Display| format!("couldn't unpack {file}: {error}");
    fs::create_dir_all(dir).map_err(|e| failed(&e))?;
    if file.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| failed(&e))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| failed(&e))?;
            let Some(relative) = entry.enclosed_name() else {
                continue;
            };
            let path = dir.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&path).map_err(|e| failed(&e))?;
                continue;
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| failed(&e))?;
            }
            let mut contents = Vec::new();
            entry.read_to_end(&mut contents).map_err(|e| failed(&e))?;
            fs::write(&path, contents).map_err(|e| failed(&e))?;
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o777))
                    .map_err(|e| failed(&e))?;
            }
        }
        Ok(())
    } else {
        let gz = flate2::read::GzDecoder::new(bytes);
        tar::Archive::new(gz).unpack(dir).map_err(|e| failed(&e))
    }
}

/// The directory in `unpacked` that holds `program` at `relative`: itself,
/// or its one top-level directory, as release archives have.
fn root_holding(unpacked: &Path, relative: &Path) -> Option<PathBuf> {
    if unpacked.join(relative).is_file() {
        return Some(unpacked.to_owned());
    }
    fs::read_dir(unpacked)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|dir| dir.join(relative).is_file())
}

/// Installs `release` as `<root>/tools/<name>`, whose `program` is at
/// `relative` inside it. The copy already there, if any, is replaced only
/// once the new one is downloaded, verified and unpacked.
fn install(root: &Path, name: &str, release: &Release, relative: &Path) -> Result<String, String> {
    let bytes = download_verified(release)?;
    let tools = setup::tools_dir(root);
    let staging = tools.join(format!(".{name}-new-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    let result = (|| {
        unpack(&release.file, &bytes, &staging)?;
        let unpacked = root_holding(&staging, relative)
            .ok_or_else(|| format!("{} holds no {}", release.file, relative.display()))?;
        let target = tools.join(name);
        let old = tools.join(format!(".{name}-old-{}", std::process::id()));
        let moved = |e: io::Error| format!("couldn't put {name} in place: {e}");
        if target.exists() {
            fs::rename(&target, &old).map_err(moved)?;
        }
        if let Err(error) = fs::rename(&unpacked, &target) {
            let _ = fs::rename(&old, &target);
            return Err(moved(error));
        }
        let _ = fs::remove_dir_all(&old);
        Ok(release.version.clone())
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Installs the latest `gh` for `platform` as `<root>/tools/gh`, and
/// returns its version.
pub fn install_gh(root: &Path, sources: &Sources, platform: Platform) -> Result<String, String> {
    let version = latest_gh(sources)?;
    let release = gh_release(sources, &version, platform);
    let program = if platform.os == Os::Windows {
        "gh.exe"
    } else {
        "gh"
    };
    install(root, "gh", &release, &Path::new("bin").join(program))
}

/// Installs the latest Node 26 for `platform` as `<root>/tools/node`, and
/// returns its version.
pub fn install_node(root: &Path, sources: &Sources, platform: Platform) -> Result<String, String> {
    let release = node_release(sources, platform)?;
    let relative = if platform.os == Os::Windows {
        PathBuf::from("node.exe")
    } else {
        Path::new("bin").join("node")
    };
    install(root, "node", &release, &relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    const MAC_ARM: Platform = Platform {
        os: Os::Mac,
        cpu: Cpu::Arm64,
    };
    const LINUX_X64: Platform = Platform {
        os: Os::Linux,
        cpu: Cpu::X64,
    };
    const WINDOWS_X64: Platform = Platform {
        os: Os::Windows,
        cpu: Cpu::X64,
    };

    /// A local HTTP server serving `files` by path, and 404 for the rest.
    struct Server {
        base: String,
        files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    }

    impl Server {
        fn new() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let files: Arc<Mutex<BTreeMap<String, Vec<u8>>>> = Arc::default();
            let served = files.clone();
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let mut request = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    if reader.read_line(&mut request).is_err() {
                        continue;
                    }
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                    let body = served.lock().unwrap().get(&path).cloned();
                    let (status, body) = match body {
                        Some(body) => ("200 OK", body),
                        None => ("404 Not Found", b"not found".to_vec()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                }
            });
            Self { base, files }
        }

        fn serve(&self, path: &str, body: impl Into<Vec<u8>>) {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_owned(), body.into());
        }

        fn sources(&self) -> Sources {
            Sources {
                github_api: format!("{}/api", self.base),
                github: format!("{}/gh", self.base),
                node_dist: format!("{}/node", self.base),
            }
        }
    }

    fn sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// A `.tar.gz` holding `files`, each a path and its contents.
    fn tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, contents) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, path, contents.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    /// A `.zip` holding `files`.
    fn zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        for (path, contents) in files {
            writer.start_file(*path, options).unwrap();
            writer.write_all(contents.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// Serves `gh` 2.9.0's release for `platform` as `archive`, with a
    /// checksum list giving `sum`, or the archive's own.
    fn serve_gh(server: &Server, platform: Platform, archive: Vec<u8>, sum: Option<&str>) {
        let release = gh_release(&server.sources(), "2.9.0", platform);
        server.serve(
            "/api/repos/cli/cli/releases/latest",
            r#"{"tag_name": "v2.9.0"}"#,
        );
        let sum = sum.map_or_else(|| sha256(&archive), str::to_owned);
        let base = "/gh/cli/cli/releases/download/v2.9.0";
        server.serve(
            &format!("{base}/gh_2.9.0_checksums.txt"),
            format!(
                "{}  gh_2.9.0_other.zip\n{sum}  {}\n",
                "0".repeat(64),
                release.file
            ),
        );
        server.serve(&format!("{base}/{}", release.file), archive);
    }

    #[test]
    fn the_right_archive_is_chosen_for_each_os_and_cpu() {
        let sources = Sources::default();
        let file = |platform| gh_release(&sources, "2.9.0", platform).file;
        assert_eq!(file(MAC_ARM), "gh_2.9.0_macOS_arm64.zip");
        assert_eq!(file(LINUX_X64), "gh_2.9.0_linux_amd64.tar.gz");
        assert_eq!(file(WINDOWS_X64), "gh_2.9.0_windows_amd64.zip");
        assert_eq!(
            gh_release(&sources, "2.9.0", MAC_ARM).url,
            "https://github.com/cli/cli/releases/download/v2.9.0/gh_2.9.0_macOS_arm64.zip"
        );

        let server = Server::new();
        server.serve(
            "/node/latest-v26.x/SHASUMS256.txt",
            "aa  node-v26.2.0-darwin-arm64.tar.gz\n\
             bb  node-v26.2.0-darwin-x64.tar.gz\n\
             cc  node-v26.2.0-linux-x64.tar.gz\n\
             dd  node-v26.2.0-linux-x64.tar.xz\n\
             ee  node-v26.2.0-win-x64.zip\n\
             ff  node-v26.2.0.tar.gz\n",
        );
        let node = |platform| node_release(&server.sources(), platform).unwrap();
        assert_eq!(node(MAC_ARM).file, "node-v26.2.0-darwin-arm64.tar.gz");
        assert_eq!(node(LINUX_X64).file, "node-v26.2.0-linux-x64.tar.gz");
        assert_eq!(node(WINDOWS_X64).file, "node-v26.2.0-win-x64.zip");
        assert_eq!(node(MAC_ARM).version, "26.2.0");
    }

    #[test]
    fn gh_is_installed_from_a_verified_archive() {
        let server = Server::new();
        let archive = zip(&[("gh_2.9.0_macOS_arm64/bin/gh", "#!/bin/sh\necho gh\n")]);
        serve_gh(&server, MAC_ARM, archive, None);
        let root = tempfile::tempdir().unwrap();

        let version = install_gh(root.path(), &server.sources(), MAC_ARM).unwrap();

        assert_eq!(version, "2.9.0");
        let gh = root.path().join("tools").join("gh").join("bin").join("gh");
        assert_eq!(fs::read_to_string(&gh).unwrap(), "#!/bin/sh\necho gh\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&gh).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }
        let leftovers: Vec<_> = fs::read_dir(root.path().join("tools"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, ["gh"]);
    }

    #[test]
    fn a_checksum_mismatch_installs_nothing_and_keeps_the_old_copy() {
        let server = Server::new();
        let archive = tar_gz(&[("gh_2.9.0_linux_amd64/bin/gh", "new")]);
        serve_gh(&server, LINUX_X64, archive, Some(&"1".repeat(64)));
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("tools").join("gh").join("bin").join("gh");
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        fs::write(&old, "old").unwrap();

        let error = install_gh(root.path(), &server.sources(), LINUX_X64).unwrap_err();

        assert_eq!(
            error,
            "gh_2.9.0_linux_amd64.tar.gz's checksum doesn't match the published one, so it wasn't installed"
        );
        assert_eq!(fs::read_to_string(&old).unwrap(), "old");
    }

    #[test]
    fn an_old_copy_is_replaced_by_a_new_one() {
        let server = Server::new();
        let archive = tar_gz(&[
            ("node-v26.2.0-linux-x64/bin/node", "new node"),
            ("node-v26.2.0-linux-x64/bin/npx", "new npx"),
        ]);
        server.serve(
            "/node/latest-v26.x/SHASUMS256.txt",
            format!("{}  node-v26.2.0-linux-x64.tar.gz\n", sha256(&archive)),
        );
        server.serve("/node/latest-v26.x/node-v26.2.0-linux-x64.tar.gz", archive);
        let root = tempfile::tempdir().unwrap();
        let node = root.path().join("tools").join("node");
        fs::create_dir_all(node.join("bin")).unwrap();
        fs::write(node.join("bin").join("node"), "old node").unwrap();
        fs::write(node.join("stale"), "from the old copy").unwrap();

        let version = install_node(root.path(), &server.sources(), LINUX_X64).unwrap();

        assert_eq!(version, "26.2.0");
        assert_eq!(
            fs::read_to_string(node.join("bin").join("node")).unwrap(),
            "new node"
        );
        assert_eq!(
            fs::read_to_string(node.join("bin").join("npx")).unwrap(),
            "new npx"
        );
        assert!(!node.join("stale").exists());
    }

    #[test]
    fn windows_node_keeps_node_exe_at_the_top() {
        let server = Server::new();
        let archive = zip(&[
            ("node-v26.2.0-win-x64/node.exe", "node"),
            ("node-v26.2.0-win-x64/npx.cmd", "npx"),
        ]);
        server.serve(
            "/node/latest-v26.x/SHASUMS256.txt",
            format!("{}  node-v26.2.0-win-x64.zip\n", sha256(&archive)),
        );
        server.serve("/node/latest-v26.x/node-v26.2.0-win-x64.zip", archive);
        let root = tempfile::tempdir().unwrap();

        install_node(root.path(), &server.sources(), WINDOWS_X64).unwrap();

        assert!(
            root.path()
                .join("tools")
                .join("node")
                .join("node.exe")
                .is_file()
        );
    }

    #[test]
    fn an_archive_without_the_program_installs_nothing() {
        let server = Server::new();
        serve_gh(&server, LINUX_X64, tar_gz(&[("README", "hi")]), None);
        let root = tempfile::tempdir().unwrap();

        let error = install_gh(root.path(), &server.sources(), LINUX_X64).unwrap_err();

        let program = Path::new("bin").join("gh");
        assert_eq!(
            error,
            format!("gh_2.9.0_linux_amd64.tar.gz holds no {}", program.display())
        );
        assert!(!root.path().join("tools").join("gh").exists());
    }

    /// Installs the real releases into a temporary root. Reaches the
    /// internet, so it runs only when asked: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn the_real_releases_install_and_run() {
        let root = tempfile::tempdir().unwrap();
        let platform = Platform::current().unwrap();
        let sources = Sources::default();

        let gh = install_gh(root.path(), &sources, platform).unwrap();
        let node = install_node(root.path(), &sources, platform).unwrap();

        let run = |program: PathBuf| {
            let output = std::process::Command::new(program)
                .arg("--version")
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap()
        };
        assert!(run(setup::private_gh(root.path())).contains(&gh));
        assert!(run(setup::private_node(root.path())).contains(&node));
        assert!(!setup::older(&node, "26.0.0"));
    }
}
