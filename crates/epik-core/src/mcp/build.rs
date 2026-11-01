//! `build_start`, `build_status` and `build_list`: the tools that ask the
//! Epik app, over its socket, to build and to say where its runs are.

use std::io;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Args, Context, Reply, integer, string};
use crate::ipc::{self, Request, Response};
use crate::run::RunKey;

/// The host a run without one is on, as the conversation names it.
const THIS_COMPUTER: &str = "this computer";

/// How often a starting app is pinged.
const PING_INTERVAL: Duration = Duration::from_millis(250);

/// Whether `error` means nothing listens at the app's socket.
fn not_listening(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

/// Sends `request` to the app and returns its reply, starting the app
/// first when it isn't running and `start` is set. A reply of type error is
/// its message.
fn request(context: &Context, request: &Request, start: bool) -> Result<Response, String> {
    let reply = match ipc::request(&context.address, request) {
        Err(error) if not_listening(&error) => {
            if !start {
                return Err("The Epik app isn't running.".to_owned());
            }
            (context.start_app)()
                .map_err(|error| format!("The Epik app couldn't be started: {error}"))?;
            await_app(context)?;
            ipc::request(&context.address, request)
        }
        reply => reply,
    }
    .map_err(|error| format!("The Epik app didn't answer: {error}"))?;
    match reply {
        Response::Error { message } => Err(message),
        reply => Ok(reply),
    }
}

/// Pings the app until it answers, for up to the context's start timeout.
fn await_app(context: &Context) -> Result<(), String> {
    let deadline = Instant::now() + context.start_timeout;
    while ipc::running(&context.address).is_none() {
        if Instant::now() >= deadline {
            return Err(format!(
                "The Epik app didn't start within {} seconds.",
                context.start_timeout.as_secs_f64()
            ));
        }
        thread::sleep(PING_INTERVAL);
    }
    Ok(())
}

fn unexpected(reply: &Response) -> String {
    format!("The Epik app answered unexpectedly: {reply:?}")
}

pub fn start(context: &Context, args: &Args) -> Result<Reply, String> {
    let build = Request::Build {
        repo: string(args, "repo").unwrap_or_default().to_owned(),
        number: integer(args, "number").unwrap_or_default() as u64,
        branch: string(args, "branch").map(str::to_owned),
        roles: None,
        host: string(args, "host").map(str::to_owned),
    };
    match request(context, &build, true)? {
        Response::Started(started) => {
            let mut reply = json!({
                "run_id": started.run,
                "build_id": started.build_id,
                "kind": started.kind,
                "branch": started.branch,
            });
            // A remote build's record is on its host, out of `tail -f`'s reach.
            if started.host.is_none() {
                reply["build_dir"] = json!(started.build_dir);
                reply["events"] = json!(started.build_dir.join(crate::record::EVENTS_FILE));
            }
            reply["host"] = json!(started.host.as_deref().unwrap_or(THIS_COMPUTER));
            Ok(Reply::Json(reply))
        }
        reply => Err(unexpected(&reply)),
    }
}

pub fn status(context: &Context, args: &Args) -> Result<Reply, String> {
    let run: RunKey = string(args, "run_id").unwrap_or_default().parse()?;
    match request(context, &Request::Status { run }, false)? {
        Response::Run(run) => Ok(Reply::Json(hosted(to_json(&run)))),
        reply => Err(unexpected(&reply)),
    }
}

pub fn list(context: &Context, args: &Args) -> Result<Reply, String> {
    let limit = integer(args, "limit").map(|limit| limit as usize);
    match request(context, &Request::List { limit }, false)? {
        Response::Runs { runs } => Ok(Reply::Json(Value::Array(
            runs.iter().map(|run| hosted(to_json(run))).collect(),
        ))),
        reply => Err(unexpected(&reply)),
    }
}

fn to_json(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("a run serializes")
}

/// `run` naming its host, which is this computer unless the app named
/// another.
fn hosted(mut run: Value) -> Value {
    if run.get("host").is_none_or(Value::is_null) {
        run["host"] = json!(THIS_COMPUTER);
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentProfile;
    use crate::builds::{Builds, HangingAgent, Started, fake_github};
    use crate::hosts;
    use crate::ipc::{Address, Server};
    use crate::mcp::gh::Gh;
    use crate::mcp::tests::text;
    use crate::mirrors::Mirrors;
    use crate::record::Kind;
    use chrono::Utc;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// An address no other test uses.
    fn address(dir: &std::path::Path) -> Address {
        if cfg!(windows) {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::SeqCst);
            Address::Pipe(format!("epik-mcp-test-{}-{n}", std::process::id()))
        } else {
            Address::Path(dir.join("epik.sock"))
        }
    }

    fn builds(root: &std::path::Path) -> Builds {
        Builds::new(root, Arc::new(fake_github()), |_: &AgentProfile| {
            Arc::new(HangingAgent)
        })
    }

    /// A host's app, which starts every build it is asked for and records
    /// the requests.
    struct Remote {
        root: tempfile::TempDir,
        builds: Mutex<Vec<Request>>,
    }

    impl Remote {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                root: tempfile::tempdir().unwrap(),
                builds: Mutex::default(),
            })
        }
    }

    impl crate::remote::Client for Remote {
        fn request(&self, request: &Request) -> io::Result<Response> {
            Ok(match request {
                Request::Build { repo, number, .. } => {
                    self.builds.lock().unwrap().push(request.clone());
                    Response::Started(Started {
                        run: RunKey::new(repo, *number),
                        build_id: "20261101T120000Z-abcd".to_owned(),
                        kind: Kind::Issue,
                        branch: "main".to_owned(),
                        build_dir: "/home/me/.epik/runs/owner/repo/7/20261101T120000Z-abcd".into(),
                        host: None,
                    })
                }
                Request::Ping => Response::Pong {
                    version: crate::version().to_owned(),
                },
                other => ipc::records(self.root.path(), other.clone()),
            })
        }
    }

    /// A fake app: the real socket server over builds with a fake GitHub,
    /// started by the MCP server when it first needs it.
    struct FakeApp {
        root: tempfile::TempDir,
        address: Address,
        builds: Arc<Builds>,
        stop: Arc<AtomicBool>,
        starts: Arc<AtomicUsize>,
    }

    impl FakeApp {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let builds = builds(root.path());
            Self::serving(root, builds)
        }

        /// An app that lists the host `devbox`, reached as `remote`.
        fn reaching(remote: &Arc<Remote>) -> Self {
            let root = tempfile::tempdir().unwrap();
            hosts::add(root.path(), "devbox", Utc::now()).unwrap();
            let mut mirrors = Mirrors::new(root.path(), {
                let remote = remote.clone();
                move |_| remote.clone()
            });
            mirrors.idle = Duration::from_secs(3600);
            let builds = builds(root.path()).reaching(Arc::new(mirrors));
            Self::serving(root, builds)
        }

        fn serving(root: tempfile::TempDir, builds: Builds) -> Self {
            Self {
                address: address(root.path()),
                builds: Arc::new(builds),
                root,
                stop: Arc::new(AtomicBool::new(false)),
                starts: Arc::new(AtomicUsize::new(0)),
            }
        }

        /// An MCP server whose `start_app` starts this app.
        fn mcp(&self) -> crate::mcp::Server {
            let (address, builds, stop, starts) = (
                self.address.clone(),
                self.builds.clone(),
                self.stop.clone(),
                self.starts.clone(),
            );
            crate::mcp::Server::new(Context {
                gh: Gh::scripted(vec![], Arc::new(Mutex::new(Vec::new()))),
                root: self.root.path().to_owned(),
                address: self.address.clone(),
                start_app: Box::new(move || {
                    starts.fetch_add(1, Ordering::SeqCst);
                    let server = Server::bind(&address).map_err(io::Error::other)?;
                    let (builds, stop) = (builds.clone(), stop.clone());
                    thread::spawn(move || server.serve(builds, &stop));
                    Ok(())
                }),
                start_timeout: Duration::from_secs(10),
            })
        }
    }

    impl Drop for FakeApp {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.builds.shutdown();
        }
    }

    fn json_of(result: &rmcp::model::CallToolResult) -> Value {
        let (text, error) = text(result);
        assert!(!error, "{text}");
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn build_start_starts_the_app_and_the_build() {
        let app = FakeApp::new();
        let mcp = app.mcp();

        let reply = json_of(
            &mcp.call("build_start", json!({"repo": "owner/repo", "number": 7}))
                .unwrap(),
        );

        assert_eq!(app.starts.load(Ordering::SeqCst), 1);
        assert_eq!(reply["run_id"], "owner/repo#7");
        assert_eq!(reply["kind"], "issue");
        assert_eq!(reply["branch"], "main");
        let build_dir = PathBuf::from(reply["build_dir"].as_str().unwrap());
        assert_eq!(
            PathBuf::from(reply["events"].as_str().unwrap()),
            build_dir.join("events.jsonl")
        );
        assert!(build_dir.join("events.jsonl").is_file());
        assert_eq!(reply["host"], "this computer");
    }

    #[test]
    fn build_start_builds_on_the_host_it_names() {
        let remote = Remote::new();
        let app = FakeApp::reaching(&remote);
        let mcp = app.mcp();

        let reply = json_of(
            &mcp.call(
                "build_start",
                json!({"repo": "owner/repo", "number": 7, "host": "devbox"}),
            )
            .unwrap(),
        );

        let forwarded = remote.builds.lock().unwrap().clone();
        assert!(
            matches!(&forwarded[..], [Request::Build { repo, number: 7, .. }] if repo == "owner/repo"),
            "{forwarded:?}"
        );
        assert_eq!(
            reply,
            json!({
                "run_id": "owner/repo#7",
                "build_id": "20261101T120000Z-abcd",
                "kind": "issue",
                "branch": "main",
                "host": "devbox",
            })
        );
    }

    #[test]
    fn build_start_refuses_a_host_that_isnt_listed() {
        let app = FakeApp::new();
        let mcp = app.mcp();

        let (message, error) = text(
            &mcp.call(
                "build_start",
                json!({"repo": "owner/repo", "number": 7, "host": "devbox"}),
            )
            .unwrap(),
        );

        assert!(error);
        assert_eq!(
            message,
            "couldn't build owner/repo#7: \"devbox\" is not a host: add it in Settings"
        );
    }

    #[test]
    fn a_run_names_its_host_or_this_computer() {
        assert_eq!(
            hosted(json!({"run": "owner/repo#7"})),
            json!({"run": "owner/repo#7", "host": "this computer"})
        );
        assert_eq!(
            hosted(json!({"run": "owner/repo#7", "host": null})),
            json!({"run": "owner/repo#7", "host": "this computer"})
        );
        assert_eq!(
            hosted(json!({"run": "owner/repo#7", "host": "devbox"})),
            json!({"run": "owner/repo#7", "host": "devbox"})
        );
    }

    #[test]
    fn build_status_and_list_describe_runs() {
        let app = FakeApp::new();
        let mcp = app.mcp();
        mcp.call("build_start", json!({"repo": "owner/repo", "number": 7}))
            .unwrap();

        let status = json_of(
            &mcp.call("build_status", json!({"run_id": "owner/repo#7"}))
                .unwrap(),
        );
        let list = json_of(&mcp.call("build_list", json!({})).unwrap());

        assert_eq!(status["run"], "owner/repo#7");
        assert_eq!(status["disposition"], "building");
        assert_eq!(status["builds"].as_array().unwrap().len(), 1);
        assert_eq!(status["host"], "this computer");
        assert!(status.get("type").is_none());
        assert_eq!(list[0], status);
        assert_eq!(app.starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reads_dont_start_the_app() {
        let app = FakeApp::new();
        let mcp = app.mcp();

        for (tool, args) in [
            ("build_status", json!({"run_id": "owner/repo#7"})),
            ("build_list", json!({})),
        ] {
            let (message, error) = text(&mcp.call(tool, args).unwrap());

            assert!(error);
            assert_eq!(message, "The Epik app isn't running.");
        }
        assert_eq!(app.starts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_apps_errors_reach_the_caller() {
        let app = FakeApp::new();
        let mcp = app.mcp();

        let (message, error) = text(
            &mcp.call("build_start", json!({"repo": "owner/repo", "number": 8}))
                .unwrap(),
        );
        assert!(error);
        assert!(
            message.starts_with("couldn't build owner/repo#8: "),
            "{message}"
        );
        let (message, _) = text(
            &mcp.call("build_status", json!({"run_id": "owner/repo#9"}))
                .unwrap(),
        );
        assert_eq!(message, "no run owner/repo#9");
        let (message, _) = text(&mcp.call("build_status", json!({"run_id": "nope"})).unwrap());
        assert!(message.contains("is not a run"), "{message}");
    }
}
