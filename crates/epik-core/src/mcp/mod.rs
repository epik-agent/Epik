//! Epik's MCP server, which `epik-app mcp` serves over stdio.
//!
//! It offers the tools the plugin's Node server offered, with the same
//! names, descriptions and arguments, so the plugin's skills don't change.
//! Each tool's name, description and input schema are in `tools.json`, and
//! its behavior is a function here or in a module beside this one. A tool
//! returns text, which the caller gets as is, or JSON, which the caller
//! gets as JSON text; a tool that fails gives the caller its message.

mod build;
mod epic_status;
pub mod gh;
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
        _ => Err(format!("{name} isn't served by epik-app mcp yet")),
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
        let replies = replies
            .into_iter()
            .map(|(needle, body)| (needle.to_owned(), body))
            .collect();
        let root = std::env::temp_dir().join(format!("epik-mcp-{}", std::process::id()));
        Server::new(Context {
            gh: Gh::scripted(replies, Arc::new(Mutex::new(Vec::new()))),
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
