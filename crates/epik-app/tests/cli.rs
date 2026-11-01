use std::process::Command;

#[test]
fn version_flag_prints_the_version_and_exits_zero() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_epik-app"))
        .arg("--version")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .expect("epik-app runs");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("epik-app {}\n", epik_core::version())
    );
}

/// `epik-app --headless` with its home in `home`, so its root and socket
/// are there; on macOS and Linux only, since Windows's pipe is shared by
/// every app on the machine.
#[cfg(unix)]
mod headless {
    use std::path::Path;
    use std::process::{Child, Command, Output, Stdio};
    use std::time::{Duration, Instant};

    use epik_core::ipc::{self, Address};

    fn headless(home: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_epik-app"));
        command
            .arg("--headless")
            .env("HOME", home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn address(home: &Path) -> Address {
        Address::in_root(&home.join(".epik"))
    }

    /// Waits for the app with its home in `home` to answer `ping`, and
    /// returns the version it answers with.
    fn wait_for_ping(home: &Path, app: &mut Child) -> String {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(version) = ipc::running(&address(home)) {
                return version;
            }
            assert!(app.try_wait().unwrap().is_none(), "the app exited");
            assert!(Instant::now() < deadline, "the app never answered");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Sends `app` SIGTERM and waits for it.
    fn terminate(app: Child) -> Output {
        let status = Command::new("kill")
            .arg(app.id().to_string())
            .status()
            .unwrap();
        assert!(status.success());
        app.wait_with_output().unwrap()
    }

    #[test]
    fn it_answers_ping_and_exits_on_sigterm() {
        let home = tempfile::tempdir().unwrap();
        let mut app = headless(home.path()).spawn().unwrap();

        let version = wait_for_ping(home.path(), &mut app);

        assert_eq!(version, epik_core::version());
        let output = terminate(app);
        assert!(output.status.success(), "{output:?}");
        assert!(ipc::running(&address(home.path())).is_none());
    }

    #[test]
    fn a_second_app_says_one_is_running_and_exits_one() {
        let home = tempfile::tempdir().unwrap();
        let mut first = headless(home.path()).spawn().unwrap();
        wait_for_ping(home.path(), &mut first);

        let second = headless(home.path()).output().unwrap();

        assert_eq!(second.status.code(), Some(1), "{second:?}");
        let stderr = String::from_utf8(second.stderr).unwrap();
        assert!(stderr.contains("already running"), "{stderr}");
        assert!(ipc::running(&address(home.path())).is_some());
        terminate(first);
    }

    #[test]
    fn it_links_itself_at_epik_bin() {
        let home = tempfile::tempdir().unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_epik-app"))
            .arg("--version")
            .env("HOME", home.path())
            .output()
            .unwrap();

        let link = home.path().join(".epik").join("bin").join("epik-app");
        let target = std::fs::read_link(&link).unwrap();
        let app = Path::new(env!("CARGO_BIN_EXE_epik-app"))
            .canonicalize()
            .unwrap();
        assert_eq!(target, app, "{output:?}");
    }

    #[test]
    fn it_removes_leftover_clones_before_serving() {
        let home = tempfile::tempdir().unwrap();
        let builds = home.path().join(".epik").join("builds");
        std::fs::create_dir_all(builds.join("crashed-run").join("src")).unwrap();
        let mut app = headless(home.path()).spawn().unwrap();

        wait_for_ping(home.path(), &mut app);

        assert_eq!(std::fs::read_dir(&builds).unwrap().count(), 0);
        terminate(app);
    }

    #[test]
    fn a_second_app_leaves_the_first_ones_clones_alone() {
        let home = tempfile::tempdir().unwrap();
        let mut first = headless(home.path()).spawn().unwrap();
        wait_for_ping(home.path(), &mut first);
        let clone = home.path().join(".epik").join("builds").join("running");
        std::fs::create_dir_all(&clone).unwrap();

        let second = headless(home.path()).output().unwrap();

        assert_eq!(second.status.code(), Some(1), "{second:?}");
        assert!(clone.exists());
        terminate(first);
    }

    /// With no tray, as on Linux without a display, the app with no flags
    /// says so and runs headless.
    #[cfg(all(target_os = "linux", feature = "desktop"))]
    #[test]
    fn without_a_display_it_runs_headless_and_says_so() {
        let home = tempfile::tempdir().unwrap();
        let mut app = Command::new(env!("CARGO_BIN_EXE_epik-app"))
            .env("HOME", home.path())
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        wait_for_ping(home.path(), &mut app);

        let output = terminate(app);
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("there is no tray (no display)"), "{stderr}");
    }

    /// Built without the desktop, the app with no flags runs headless.
    #[cfg(not(feature = "desktop"))]
    #[test]
    fn without_the_desktop_no_argument_runs_headless() {
        let home = tempfile::tempdir().unwrap();
        let mut app = Command::new(env!("CARGO_BIN_EXE_epik-app"))
            .env("HOME", home.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let version = wait_for_ping(home.path(), &mut app);

        assert_eq!(version, epik_core::version());
        let output = terminate(app);
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("running headless"), "{stderr}");
    }

    /// Built without the desktop, the app doesn't keep the machine awake,
    /// so a build's start and end say nothing about it.
    #[cfg(not(feature = "desktop"))]
    #[test]
    fn without_the_desktop_a_build_says_nothing_about_keeping_awake() {
        let home = tempfile::tempdir().unwrap();
        set_up_fakes(home.path());
        let mut app = headless(home.path()).spawn().unwrap();
        wait_for_ping(home.path(), &mut app);

        // The fake agent passes its check, and the fake gh finds no issue,
        // so the build starts and ends at once.
        let response = ipc::request(
            &address(home.path()),
            &ipc::Request::Build {
                repo: "epik-agent/nothing".to_owned(),
                number: 1,
                branch: None,
                roles: None,
            },
        )
        .unwrap();

        assert!(
            matches!(response, ipc::Response::Error { .. }),
            "{response:?}"
        );
        let output = terminate(app);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("1 build(s) running"), "{stderr}");
        assert!(stderr.contains("0 build(s) running"), "{stderr}");
        assert!(!stderr.contains("awake"), "{stderr}");
        assert!(!stderr.contains("sleep"), "{stderr}");
    }

    /// Puts fake `gh`, `claude` and `node` in `home`'s `~/.local/bin`, and
    /// has git push with gh, so that the setup Epik checks before a build
    /// is complete. The fake gh answers nothing else. A fake `npx` runs an
    /// ACP agent that opens a session in `auto` mode and does nothing more,
    /// which is all Claude Code's check before a build asks.
    #[cfg(not(feature = "desktop"))]
    fn set_up_fakes(home: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let bin = home.join(".local").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fakes = [
            (
                "gh",
                r#"case "$*" in
  --version) echo "gh version 2.63.0 (2024-11-27)" ;;
  "auth status") echo "  - Token scopes: 'project', 'repo'" ;;
  *) echo "no such thing" >&2; exit 1 ;;
esac"#,
            ),
            (
                "claude",
                r#"case "$*" in
  --version) echo "2.1.312 (Claude Code)" ;;
  "auth status") echo '{"loggedIn": true}' ;;
  "plugin list --json") echo '[]' ;;
  *) exit 1 ;;
esac"#,
            ),
            ("node", "echo v26.0.0"),
            (
                "npx",
                r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/^{"jsonrpc":"2.0","id":\([^,]*\),.*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocolVersion":1,"agentCapabilities":{}}}' ;;
    *'"method":"session/new"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"s","modes":{"currentModeId":"auto","availableModes":[{"id":"auto","name":"auto"}]}}}' ;;
  esac
done"#,
            ),
        ];
        for (name, body) in fakes {
            let path = bin.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(
            home.join(".gitconfig"),
            "[credential \"https://github.com\"]\n\thelper = !gh auth git-credential\n",
        )
        .unwrap();
    }
}

/// `epik-app mcp` over its real stdin and stdout.
mod mcp {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    use serde_json::{Value, json};

    /// Sends each of `messages` to a fresh `epik-app mcp` and returns the
    /// replies to the ones with an id.
    fn converse(messages: &[Value]) -> Vec<Value> {
        let home = tempfile::tempdir().unwrap();
        let mut server = Command::new(env!("CARGO_BIN_EXE_epik-app"))
            .arg("mcp")
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = server.stdin.take().unwrap();
        let mut stdout = BufReader::new(server.stdout.take().unwrap());
        let mut replies = Vec::new();
        for message in messages {
            writeln!(stdin, "{message}").unwrap();
            if message.get("id").is_some() {
                let mut line = String::new();
                stdout.read_line(&mut line).unwrap();
                replies.push(serde_json::from_str(&line).unwrap());
            }
        }
        drop(stdin);
        server.wait().unwrap();
        replies
    }

    #[test]
    fn it_answers_initialize_and_lists_the_tools() {
        let replies = converse(&[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"},
            }}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        ]);

        let info = &replies[0]["result"];
        assert_eq!(info["serverInfo"]["name"], "epik", "{info}");
        assert_eq!(info["instructions"], epik_core::mcp::INSTRUCTIONS);
        assert!(info["capabilities"]["tools"].is_object(), "{info}");
        let listed: Vec<Value> = replies[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| {
                json!({
                    "name": tool["name"],
                    "description": tool["description"],
                    "inputSchema": tool["inputSchema"],
                })
            })
            .collect();
        let expected: Value =
            serde_json::from_str(include_str!("../../epik-core/src/mcp/tools.json")).unwrap();
        assert_eq!(Value::from(listed), expected);
    }
}

/// `epik-app stdio` with its home in a temporary directory, relaying to a
/// test server bound there; on macOS and Linux only, since Windows's pipe
/// is shared by every app on the machine.
#[cfg(unix)]
mod stdio {
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Output, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use epik_core::builds::Builds;
    use epik_core::ipc::{Address, Server};
    use serde_json::Value;

    /// Runs `epik-app stdio` with its home in `home`, writes `input` to its
    /// stdin, and waits for it to end.
    fn relay(home: &Path, input: &str) -> Output {
        let mut stdio = Command::new(env!("CARGO_BIN_EXE_epik-app"))
            .arg("stdio")
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        stdio
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        stdio.wait_with_output().unwrap()
    }

    /// Serves the socket in `home` while `f` runs.
    fn with_server<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        let root = home.join(".epik");
        let server = Server::bind(&Address::in_root(&root)).unwrap();
        let builds = Arc::new(Builds::real(&root));
        let stop = Arc::new(AtomicBool::new(false));
        let serving = thread::spawn({
            let stop = stop.clone();
            move || server.serve(builds, &stop)
        });
        let result = f();
        stop.store(true, Ordering::SeqCst);
        serving.join().unwrap();
        result
    }

    /// Each line of `output`'s stdout, as JSON.
    fn replies(output: &Output) -> Vec<Value> {
        String::from_utf8(output.stdout.clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn it_answers_each_request_in_order() {
        let home = tempfile::tempdir().unwrap();

        let output = with_server(home.path(), || {
            relay(home.path(), "{\"type\": \"ping\"}\n{\"type\": \"runs\"}\n")
        });

        assert!(output.status.success(), "{output:?}");
        let replies = replies(&output);
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert_eq!(replies[0]["type"], "pong", "{replies:?}");
        assert_eq!(replies[0]["version"], epik_core::version());
        assert_eq!(replies[1]["type"], "run_keys", "{replies:?}");
    }

    #[test]
    fn a_line_that_isnt_json_gets_the_servers_bad_request() {
        let home = tempfile::tempdir().unwrap();

        let output = with_server(home.path(), || relay(home.path(), "not json\n"));

        assert!(output.status.success(), "{output:?}");
        let replies = replies(&output);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0]["type"], "error", "{replies:?}");
        let message = replies[0]["message"].as_str().unwrap();
        assert!(message.starts_with("bad request: "), "{message}");
    }

    #[test]
    fn with_no_app_running_the_error_says_how_to_start_one() {
        let home = tempfile::tempdir().unwrap();

        let output = relay(home.path(), "{\"type\": \"ping\"}\n");

        assert!(output.status.success(), "{output:?}");
        let replies = replies(&output);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0]["type"], "error", "{replies:?}");
        let message = replies[0]["message"].as_str().unwrap();
        assert!(message.contains("epik-app --headless"), "{message}");
        assert!(message.contains("service"), "{message}");
    }
}

/// `epik-app setup` with its home in a temporary directory, where nothing
/// is installed or signed in.
mod setup {
    use std::path::Path;
    use std::process::{Command, Output};

    /// Runs `epik-app setup` with `args` and its home in `home`, with no
    /// sign-in from the environment, and its releases from `sources`, if
    /// given.
    fn setup(home: &Path, args: &[&str], sources: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_epik-app"));
        command
            .arg("setup")
            .args(args)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("ANTHROPIC_API_KEY");
        if let Some(base) = sources {
            command
                .env("EPIK_GITHUB_API_URL", format!("{base}/api"))
                .env("EPIK_GITHUB_URL", format!("{base}/gh"))
                .env("EPIK_NODE_DIST_URL", format!("{base}/node"));
        }
        command.output().unwrap()
    }

    /// The line of `stdout` for the item titled `title`, and the lines
    /// under it saying what to do.
    fn item<'a>(stdout: &'a str, title: &str) -> Vec<&'a str> {
        let mut lines = stdout.lines().skip_while(|line| !line.starts_with(title));
        let first = lines
            .next()
            .unwrap_or_else(|| panic!("no {title} in {stdout}"));
        std::iter::once(first)
            .chain(lines.take_while(|line| line.starts_with(' ')))
            .collect()
    }

    #[test]
    fn on_an_empty_home_it_lists_each_item_and_what_to_do() {
        let home = tempfile::tempdir().unwrap();

        let output = setup(home.path(), &[], None);

        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        for title in [
            "GitHub CLI",
            "GitHub sign-in",
            "Claude Code",
            "Claude sign-in",
            "Node",
            "The Epik plugin",
        ] {
            assert!(stdout.contains(title), "{title}: {stdout}");
        }
        // gh, Claude Code and Node may be on this machine outside the
        // home, but nothing is signed in or installed in it.
        let gh_auth = item(&stdout, "GitHub sign-in");
        assert!(gh_auth[0].contains("missing"), "{stdout}");
        assert!(
            gh_auth.iter().any(|line| line.contains("auth login --web")),
            "{stdout}"
        );
        let claude_auth = item(&stdout, "Claude sign-in");
        assert!(claude_auth[0].contains("missing"), "{stdout}");
        assert!(
            claude_auth.iter().any(|line| line.contains("auth login")),
            "{stdout}"
        );
        let plugin = item(&stdout, "The Epik plugin");
        assert!(plugin[0].contains("missing"), "{stdout}");
        assert!(
            plugin
                .iter()
                .any(|line| line.contains("plugin install epik@epik")),
            "{stdout}"
        );
        let gh = item(&stdout, "GitHub CLI");
        if !gh[0].contains("done") {
            assert_eq!(gh[1].trim(), "run: epik-app setup install gh", "{stdout}");
        }
    }

    #[test]
    fn installing_an_unknown_item_is_refused() {
        let home = tempfile::tempdir().unwrap();

        let output = setup(home.path(), &["install", "emacs"], None);

        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("emacs"), "{stderr}");
        assert!(stderr.contains("gh, node"), "{stderr}");
        assert!(!home.path().join(".epik").join("tools").exists());
    }

    #[test]
    fn installing_a_sign_in_is_refused_and_says_what_to_run() {
        let home = tempfile::tempdir().unwrap();

        let output = setup(home.path(), &["install", "gh-auth"], None);

        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("auth login"), "{stderr}");
    }

    /// Node's release for macOS and Linux, from a local server.
    #[cfg(unix)]
    #[test]
    fn node_is_installed_from_its_verified_release() {
        let server = server::Server::new();
        let archive = server::tar_gz(&[("node-v26.2.0/bin/node", "#!/bin/sh\necho v26.2.0\n")]);
        let sum = server::sha256(&archive);
        let mut list = String::new();
        for platform in ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64"] {
            let file = format!("node-v26.2.0-{platform}.tar.gz");
            list.push_str(&format!("{sum}  {file}\n"));
            server.serve(&format!("/node/latest-v26.x/{file}"), archive.clone());
        }
        server.serve("/node/latest-v26.x/SHASUMS256.txt", list);
        let home = tempfile::tempdir().unwrap();

        let output = setup(home.path(), &["install", "node"], Some(&server.base));

        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains("Installed Node 26.2.0, checked against its published checksum."),
            "{stdout}"
        );
        let node = home.path().join(".epik/tools/node/bin/node");
        assert!(node.is_file());
        let listing = setup(home.path(), &[], Some(&server.base));
        let stdout = String::from_utf8(listing.stdout).unwrap();
        let line = item(&stdout, "Node");
        assert!(line[0].contains("done"), "{stdout}");
        assert!(line[0].contains("Node 26.2.0"), "{stdout}");
    }

    #[cfg(unix)]
    #[test]
    fn a_release_whose_checksum_doesnt_match_isnt_installed() {
        let server = server::Server::new();
        let archive = server::tar_gz(&[("node-v26.2.0/bin/node", "bad")]);
        let mut list = String::new();
        for platform in ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64"] {
            let file = format!("node-v26.2.0-{platform}.tar.gz");
            list.push_str(&format!("{}  {file}\n", "0".repeat(64)));
            server.serve(&format!("/node/latest-v26.x/{file}"), archive.clone());
        }
        server.serve("/node/latest-v26.x/SHASUMS256.txt", list);
        let home = tempfile::tempdir().unwrap();

        let output = setup(home.path(), &["install", "node"], Some(&server.base));

        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("checksum doesn't match the published one"),
            "{stderr}"
        );
        assert!(!home.path().join(".epik/tools/node").exists());
    }

    /// A local HTTP server, as in `install.rs`'s tests.
    #[cfg(unix)]
    mod server {
        use std::collections::BTreeMap;
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};
        use std::thread;

        use sha2::{Digest, Sha256};

        /// Serves `files` by path, and 404 for the rest.
        pub struct Server {
            pub base: String,
            files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        }

        impl Server {
            pub fn new() -> Self {
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

            pub fn serve(&self, path: &str, body: impl Into<Vec<u8>>) {
                self.files
                    .lock()
                    .unwrap()
                    .insert(path.to_owned(), body.into());
            }
        }

        pub fn sha256(bytes: &[u8]) -> String {
            Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        }

        /// A `.tar.gz` holding `files`, each a path and its contents.
        pub fn tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
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
    }
}
