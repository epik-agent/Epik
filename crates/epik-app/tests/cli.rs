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
        assert_eq!(
            target,
            Path::new(env!("CARGO_BIN_EXE_epik-app")),
            "{output:?}"
        );
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
    #[cfg(target_os = "linux")]
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
}
