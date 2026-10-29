//! `build_start`, `build_status` and `build_list`: the tools that ask the
//! Epik app, over its socket, to build and to say where its runs are.

use std::io;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Args, Context, Reply, integer, string};
use crate::ipc::{self, Request, Response};
use crate::run::RunKey;

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
    let plugin_dir = context
        .plugin_dir
        .clone()
        .ok_or("CLAUDE_PLUGIN_ROOT is not set, so the build cannot load the Epik plugin.")?;
    let build = Request::Build {
        repo: string(args, "repo").unwrap_or_default().to_owned(),
        number: integer(args, "number").unwrap_or_default() as u64,
        branch: string(args, "branch").map(str::to_owned),
        plugin_dir,
    };
    match request(context, &build, true)? {
        Response::Started(started) => Ok(Reply::Json(json!({
            "run_id": started.run,
            "build_id": started.build_id,
            "kind": started.kind,
            "branch": started.branch,
            "build_dir": started.build_dir,
            "events": started.build_dir.join(crate::record::EVENTS_FILE),
        }))),
        reply => Err(unexpected(&reply)),
    }
}

pub fn status(context: &Context, args: &Args) -> Result<Reply, String> {
    let run: RunKey = string(args, "run_id").unwrap_or_default().parse()?;
    match request(context, &Request::Status { run }, false)? {
        Response::Run(run) => Ok(Reply::Json(to_json(&run))),
        reply => Err(unexpected(&reply)),
    }
}

pub fn list(context: &Context, args: &Args) -> Result<Reply, String> {
    let limit = integer(args, "limit").map(|limit| limit as usize);
    match request(context, &Request::List { limit }, false)? {
        Response::Runs { runs } => Ok(Reply::Json(to_json(&runs))),
        reply => Err(unexpected(&reply)),
    }
}

fn to_json(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("a run serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builds::{Builds, HangingAgent, fake_github};
    use crate::ipc::{Address, Server};
    use crate::mcp::gh::Gh;
    use crate::mcp::tests::text;
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
            Self {
                address: address(root.path()),
                builds: Arc::new(Builds::new(
                    root.path(),
                    Arc::new(fake_github()),
                    Arc::new(HangingAgent),
                )),
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
                plugin_dir: Some(PathBuf::from("/plugin")),
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

    #[test]
    fn build_start_needs_the_plugin() {
        let app = FakeApp::new();
        let mut context_less = app.mcp();
        Arc::get_mut(&mut context_less.context).unwrap().plugin_dir = None;

        let (message, error) = text(
            &context_less
                .call("build_start", json!({"repo": "owner/repo", "number": 7}))
                .unwrap(),
        );

        assert!(error);
        assert_eq!(
            message,
            "CLAUDE_PLUGIN_ROOT is not set, so the build cannot load the Epik plugin."
        );
    }
}
