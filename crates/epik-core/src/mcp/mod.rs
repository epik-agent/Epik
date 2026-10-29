//! Epik's MCP server, which `epik-app mcp` serves over stdio.
//!
//! It offers the tools the plugin's Node server offered before it, with
//! the same names, descriptions and arguments, so the plugin's skills
//! didn't change.
//! Each tool's name, description and input schema are in `tools.json`, and
//! its behavior is a function here or in a module beside this one. A tool
//! returns text, which the caller gets as is, or JSON, which the caller
//! gets as JSON text; a tool that fails gives the caller its message.

mod blocked_by;
mod build;
mod epic_status;
pub mod gh;
mod gh_raw;
mod labels;
mod projects;
mod runs;
mod schema;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value};

use crate::ipc::Address;
use gh::Gh;

/// What the server tells every session that has the plugin enabled. It
/// stays a few sentences long; the substance lives in the `assistant`
/// skill.
pub const INSTRUCTIONS: &str = "Epik plans software with the user and has Claude Code build it from GitHub issues. \
When the user addresses Epik by name or asks what Epik is or how it works, and before you design, plan or scope software with the user, write or revise an epic or issue, or start a build, read Epik's `assistant` skill (`epik:assistant`): \
it holds what Epik believes about building software and how Epik writes epics. \
The user always understands what is going on; that principle outranks speed.";

/// Every tool's name, description and input schema.
const TOOLS: &str = include_str!("tools.json");

/// What a tool gives back.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Text(String),
    Json(Value),
}

impl Reply {
    fn into_text(self) -> String {
        match self {
            Reply::Text(text) => text,
            Reply::Json(value) => value.to_string(),
        }
    }
}

/// A tool's arguments, already checked against its schema.
pub type Args = Map<String, Value>;

/// What the tools reach: GitHub through `gh`, and the Epik app.
pub struct Context {
    pub gh: Gh,
    /// Epik's root, `~/.epik`.
    pub root: PathBuf,
    /// The Epik app's socket.
    pub address: Address,
    /// Starts the Epik app, detached, when it isn't running.
    pub start_app: Box<dyn Fn() -> std::io::Result<()> + Send + Sync>,
    /// How long a started app has to answer.
    pub start_timeout: Duration,
    /// Epik's plugin, from `CLAUDE_PLUGIN_ROOT`, which builds load.
    pub plugin_dir: Option<PathBuf>,
}

/// Epik's MCP server.
#[derive(Clone)]
pub struct Server {
    context: Arc<Context>,
    tools: Arc<Vec<Tool>>,
}

impl Server {
    pub fn new(context: Context) -> Self {
        let tools: Vec<Tool> = serde_json::from_str(TOOLS).expect("tools.json is a list of tools");
        Self {
            context: Arc::new(context),
            tools: Arc::new(tools),
        }
    }

    /// The tools, as `tools/list` gives them.
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// Calls tool `name` with `args`. An unknown tool or arguments its
    /// schema refuses are an MCP error; a tool that fails is a result
    /// carrying its message.
    pub fn call(&self, name: &str, args: Value) -> Result<CallToolResult, McpError> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .ok_or_else(|| McpError::invalid_params(format!("Tool {name} not found"), None))?;
        let args = match args {
            Value::Null => Map::new(),
            Value::Object(args) => args,
            _ => return Err(invalid(name, "the arguments are not an object")),
        };
        schema::check(&tool.input_schema, &args).map_err(|why| invalid(name, &why))?;
        Ok(match run(&self.context, name, &args) {
            Ok(reply) => CallToolResult::success(vec![ContentBlock::text(reply.into_text())]),
            Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
        })
    }

    /// Serves MCP over stdin and stdout until the client goes away.
    pub fn serve_stdio(self) -> std::io::Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let service = self
                .serve(rmcp::transport::stdio())
                .await
                .map_err(std::io::Error::other)?;
            service.waiting().await.map_err(std::io::Error::other)?;
            Ok(())
        })
    }
}

fn invalid(tool: &str, why: &str) -> McpError {
    McpError::invalid_params(
        format!("Input validation error: Invalid arguments for tool {tool}: {why}"),
        None,
    )
}

/// Runs tool `name`.
fn run(context: &Context, name: &str, args: &Args) -> Result<Reply, String> {
    match name {
        "build_start" => build::start(context, args),
        "build_status" => build::status(context, args),
        "build_list" => build::list(context, args),
        "epic_status" => epic_status::run(context, args),
        "gh_raw" => gh_raw::run(context, args),
        "label_list" => labels::list(context, args),
        "label_create" => labels::create(context, args),
        "label_delete" => labels::delete(context, args),
        "run_list" => runs::list(context, args),
        "run_get" => runs::get(context, args),
        "run_logs" => runs::logs(context, args),
        "project_list" => projects::list(context, args),
        "project_list_items" => projects::list_items(context, args),
        "project_get_item" => projects::get_item(context, args),
        "project_set_status" => projects::set_status(context, args),
        "issue_set_blocked_by" => blocked_by::set(context, args),
        "issue_remove_blocked_by" => blocked_by::remove(context, args),
        "issue_list_blocked_by" => blocked_by::list(context, args),
        // `Server::call` calls only the tools in `tools.json`.
        _ => Err(format!("Tool {name} not found")),
    }
}

/// A string argument.
fn string<'a>(args: &'a Args, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}

/// An integer argument.
fn integer(args: &Args, name: &str) -> Option<i64> {
    args.get(name).and_then(Value::as_i64)
}

/// A boolean argument, false when it's missing.
fn boolean(args: &Args, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// A string argument, `None` when it's missing or empty, as JavaScript's
/// falsy check had it.
fn nonempty<'a>(args: &'a Args, name: &str) -> Option<&'a str> {
    string(args, name).filter(|text| !text.is_empty())
}

impl ServerHandler for Server {
    fn get_info(&self) -> InitializeResult {
        let mut info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("epik", crate::version());
        info.instructions = Some(INSTRUCTIONS.to_owned());
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self.tools.to_vec(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let server = self.clone();
        let name = request.name.to_string();
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        // Tools block on `gh` and the app's socket.
        tokio::task::spawn_blocking(move || server.call(&name, args))
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?
            .map(CallToolResponse::from)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|tool| tool.name == name).cloned()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// A server whose `gh` answers with `replies`, and whose app is at an
    /// address where nothing listens and that can't be started.
    pub(crate) fn server(replies: Vec<(&str, Value)>) -> Server {
        server_with(scripted(replies).0)
    }

    /// The calls a scripted `gh` got, each its arguments.
    pub(crate) type Calls = Arc<Mutex<Vec<Vec<String>>>>;

    /// A `gh` that answers with `replies`, as [`Gh::scripted`] does, and the
    /// calls it gets.
    pub(crate) fn scripted(replies: Vec<(&str, Value)>) -> (Gh, Calls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let replies = replies
            .into_iter()
            .map(|(needle, body)| (needle.to_owned(), body))
            .collect();
        (Gh::scripted(replies, calls.clone()), calls)
    }

    /// A `gh` that prints `stdout` when called with exactly `args`, joined
    /// by spaces, and otherwise fails naming the arguments it got.
    pub(crate) fn expect_gh(args: &str, stdout: &str) -> Gh {
        let (args, stdout) = (args.to_owned(), stdout.to_owned());
        Gh::with(move |got| {
            let got = got.join(" ");
            Ok(if got == args {
                gh::Output {
                    exit_code: 0,
                    stdout: stdout.clone(),
                    stderr: String::new(),
                }
            } else {
                gh::Output {
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("unexpected: gh {got}\n"),
                }
            })
        })
    }

    /// A `gh` that always fails, printing `stderr`.
    pub(crate) fn failing_gh(stderr: &str) -> Gh {
        let stderr = format!("{stderr}\n");
        Gh::with(move |_| {
            Ok(gh::Output {
                exit_code: 1,
                stdout: String::new(),
                stderr: stderr.clone(),
            })
        })
    }

    /// Calls tool `name` on a server with `gh`, giving its result's text, or
    /// its message when it fails.
    pub(crate) fn call(gh: Gh, name: &str, args: Value) -> Result<String, String> {
        let result = server_with(gh).call(name, args).expect("valid arguments");
        let (text, error) = text(&result);
        if error { Err(text) } else { Ok(text) }
    }

    /// Calls tool `name` as [`call`] does, parsing its result as JSON.
    pub(crate) fn call_json(gh: Gh, name: &str, args: Value) -> Result<Value, String> {
        call(gh, name, args).map(|text| serde_json::from_str(&text).expect("JSON"))
    }

    /// A server with `gh`, whose app is at an address where nothing listens
    /// and that can't be started.
    pub(crate) fn server_with(gh: Gh) -> Server {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("epik-mcp-{}-{n}", std::process::id()));
        Server::new(Context {
            gh,
            address: crate::ipc::Address::in_root(&root.join("nowhere")),
            root,
            start_app: Box::new(|| Err(std::io::Error::other("no app in tests"))),
            start_timeout: Duration::from_millis(100),
            plugin_dir: Some(PathBuf::from("/plugin")),
        })
    }

    /// The text of a call's result, and whether it is an error.
    pub(crate) fn text(result: &CallToolResult) -> (String, bool) {
        let content = &result.content[0];
        let text = content.as_text().expect("text content").text.clone();
        (text, result.is_error == Some(true))
    }

    #[test]
    fn every_tool_the_node_server_offered_is_listed() {
        let names: Vec<String> = server(vec![])
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();

        assert_eq!(
            names,
            [
                "gh_raw",
                "label_list",
                "label_create",
                "label_delete",
                "run_list",
                "run_get",
                "run_logs",
                "project_list",
                "project_list_items",
                "project_get_item",
                "project_set_status",
                "issue_set_blocked_by",
                "issue_remove_blocked_by",
                "issue_list_blocked_by",
                "epic_status",
                "build_start",
                "build_status",
                "build_list",
            ]
        );
    }

    #[test]
    fn an_unknown_tool_or_bad_arguments_are_mcp_errors() {
        let server = server(vec![]);

        assert!(server.call("dance", json!({})).is_err());
        let error = server
            .call("epic_status", json!({"repo": "o/r"}))
            .unwrap_err();
        assert!(
            error
                .message
                .starts_with("Input validation error: Invalid arguments for tool epic_status: "),
            "{}",
            error.message
        );
        assert!(
            server
                .call("build_start", json!({"repo": "o/r", "number": 0}))
                .is_err()
        );
    }
}
