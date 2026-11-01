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

        // The fake gh finds no issue, so the build starts and ends at once.
        let response = ipc::request(
            &address(home.path()),
            &ipc::Request::Build {
                repo: "epik-agent/nothing".to_owned(),
                number: 1,
                branch: None,
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
    /// is complete. The fake gh answers nothing else.
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
