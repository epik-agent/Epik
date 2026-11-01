//! `epik_core::remote::Ssh` with a fake `ssh`: a shell script that prints a
//! line of shell noise, as a login shell's startup files can, then runs this
//! `epik-app stdio` with its home in a temporary directory, relaying to a
//! test server bound there. On macOS and Linux only, since Windows's pipe is
//! shared by every app on the machine.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use epik_core::builds::Builds;
use epik_core::ipc::{Address, Request, Response, Server};
use epik_core::remote::Ssh;

/// A temporary home with an app serving its socket, and a fake `ssh`
/// beside it.
struct Host {
    dir: tempfile::TempDir,
    stop: Arc<AtomicBool>,
    serving: Option<thread::JoinHandle<()>>,
}

impl Host {
    /// A host whose `ssh` relays to its app through `epik-app stdio`.
    fn new() -> Self {
        Self::with_ssh(&format!(
            "echo 'Welcome to devbox!'\n\
             HOME=\"$DIR/home\" exec '{app}' stdio",
            app = env!("CARGO_BIN_EXE_epik-app"),
        ))
    }

    /// A host whose `ssh` runs `body`, a shell script, after recording its
    /// arguments in `args`, its process ID in `pid`, and a line in
    /// `starts`, each in `$DIR`.
    fn with_ssh(body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = format!(
            "#!/bin/sh\n\
             DIR='{dir}'\n\
             echo \"$@\" > \"$DIR/args\"\n\
             echo $$ > \"$DIR/pid\"\n\
             echo started >> \"$DIR/starts\"\n\
             {body}\n",
            dir = dir.path().display(),
        );
        let ssh = dir.path().join("ssh");
        fs::write(&ssh, script).unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();

        let root = dir.path().join("home").join(".epik");
        let server = Server::bind(&Address::in_root(&root)).unwrap();
        let builds = Arc::new(Builds::real(&root));
        let stop = Arc::new(AtomicBool::new(false));
        let serving = thread::spawn({
            let stop = stop.clone();
            move || server.serve(builds, &stop)
        });
        Self {
            dir,
            stop,
            serving: Some(serving),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// An `Ssh` to `devbox` through this host's fake `ssh`.
    fn ssh(&self) -> Ssh {
        let mut ssh = Ssh::new("devbox");
        ssh.program = self.path("ssh");
        ssh
    }

    /// Stops the app, leaving `epik-app stdio` nothing to relay to.
    fn stop_app(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
        let _ = fs::remove_file(self.path("home").join(".epik").join("epik.sock"));
    }

    /// How many times the fake `ssh` has started.
    fn starts(&self) -> usize {
        fs::read_to_string(self.path("starts"))
            .map(|starts| starts.lines().count())
            .unwrap_or(0)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop_app();
    }
}

fn pong() -> Response {
    Response::Pong {
        version: epik_core::version().to_owned(),
    }
}

fn is_alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .status()
        .unwrap()
        .success()
}

#[test]
fn a_request_gets_its_reply_past_the_shells_noise() {
    let host = Host::new();
    let ssh = host.ssh();

    assert_eq!(ssh.request(&Request::Ping).unwrap(), pong());
    assert_eq!(
        ssh.request(&Request::Runs).unwrap(),
        Response::RunKeys { runs: vec![] }
    );
    assert_eq!(host.starts(), 1, "one ssh process serves every request");
}

#[test]
fn it_runs_ssh_without_prompts_and_the_hosts_app_from_its_home() {
    let host = Host::new();

    host.ssh().request(&Request::Ping).unwrap();

    assert_eq!(
        fs::read_to_string(host.path("args")).unwrap(),
        "-T -o BatchMode=yes -o ServerAliveInterval=30 devbox .epik/bin/epik-app stdio\n"
    );
}

#[test]
fn two_threads_requests_each_get_their_own_reply() {
    let host = Host::new();
    let ssh = Arc::new(host.ssh());

    let threads: Vec<_> = [Request::Ping, Request::Runs]
        .into_iter()
        .map(|request| {
            let ssh = ssh.clone();
            thread::spawn(move || {
                (0..20)
                    .map(|_| ssh.request(&request).unwrap())
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let replies: Vec<Vec<Response>> = threads.into_iter().map(|t| t.join().unwrap()).collect();

    assert!(
        replies[0].iter().all(|reply| *reply == pong()),
        "{replies:?}"
    );
    assert!(
        replies[1]
            .iter()
            .all(|reply| *reply == Response::RunKeys { runs: vec![] }),
        "{replies:?}"
    );
    assert_eq!(host.starts(), 1);
}

#[test]
fn a_killed_connection_restarts() {
    let host = Host::new();
    let ssh = host.ssh();
    ssh.request(&Request::Ping).unwrap();
    let pid = fs::read_to_string(host.path("pid")).unwrap();
    let pid = pid.trim();

    // Killed, `ssh` stays a zombie until `Ssh` reaps it, so there is no
    // waiting for it to be gone.
    assert!(
        Command::new("kill")
            .args(["-9", pid])
            .status()
            .unwrap()
            .success()
    );

    assert_eq!(
        ssh.request(&Request::Runs).unwrap(),
        Response::RunKeys { runs: vec![] }
    );
    assert_eq!(host.starts(), 2);
}

#[test]
fn a_host_whose_app_is_another_minor_version_is_refused() {
    let host = Host::with_ssh(
        "read line\n\
         echo '{\"type\": \"pong\", \"version\": \"0.2.9\"}'\n\
         exec cat > /dev/null",
    );

    let error = host.ssh().request(&Request::Runs).unwrap_err();

    let minor = epik_core::version().rsplit_once('.').unwrap().0;
    assert_eq!(
        error.to_string(),
        format!("devbox runs Epik 0.2.9; install {minor}.x there")
    );
}

#[test]
fn a_host_whose_app_differs_only_in_its_patch_is_accepted() {
    let (minor, _) = epik_core::version().rsplit_once('.').unwrap();
    let host = Host::with_ssh(&format!(
        "read line\n\
         echo '{{\"type\": \"pong\", \"version\": \"{minor}.99\"}}'\n\
         read line\n\
         echo '{{\"type\": \"run_keys\", \"runs\": []}}'\n\
         exec cat > /dev/null",
    ));

    assert_eq!(
        host.ssh().request(&Request::Runs).unwrap(),
        Response::RunKeys { runs: vec![] }
    );
}

#[test]
fn a_connection_failure_carries_ssh_s_last_line_of_stderr() {
    let host = Host::with_ssh(
        "echo 'Warning: something earlier' >&2\n\
         echo 'Host key verification failed.' >&2\n\
         exit 255",
    );

    let error = host.ssh().request(&Request::Ping).unwrap_err();

    assert_eq!(
        error.to_string(),
        "couldn't reach devbox: Host key verification failed."
    );
    assert_eq!(host.starts(), 1, "a failed connection isn't retried");
}

#[test]
fn a_hung_host_times_out() {
    let host = Host::with_ssh("exec sleep 600");
    let mut ssh = host.ssh();
    ssh.timeout = Duration::from_millis(500);

    let started = Instant::now();
    let error = ssh.request(&Request::Ping).unwrap_err();

    assert!(started.elapsed() < Duration::from_secs(10), "{started:?}");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut, "{error}");
    assert_eq!(error.to_string(), "devbox didn't answer within 0.5 seconds");
    let pid = fs::read_to_string(host.path("pid")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while is_alive(pid.trim()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!is_alive(pid.trim()), "the hung ssh is stopped");
}

#[test]
fn it_waits_thirty_seconds_by_default() {
    assert_eq!(Ssh::new("devbox").timeout, Duration::from_secs(30));
}

#[test]
fn an_error_from_the_hosts_bridge_is_the_connections_error() {
    // No app is serving: `epik-app stdio` answers with how to start one.
    let mut host = Host::new();
    host.stop_app();

    let error = host.ssh().request(&Request::Ping).unwrap_err();

    assert!(
        error.to_string().starts_with("no Epik app is running at "),
        "{error}"
    );
}
