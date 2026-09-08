//! The scripted model: a deterministic fake provider speaking just
//! enough OpenAI protocol, on a loopback socket.
//!
//! Test infrastructure with a hard rule behind it: no test in this
//! project ever depends on a real LLM. A test writes the script — text
//! deltas, tool-call fragments cut exactly where the author says, an
//! HTTP error, a malformed stream — points a real [`Client`] at
//! [`Scripted::base_url`], and afterwards reads back every request body
//! the fixture received, in order. A request arriving beyond the
//! script's end answers with an error that names itself, so the test
//! fails saying why.
//!
//! [`Client`]: crate::chat::Client

use std::io::Write;
use std::net::TcpListener;
use std::sync::{Arc, Mutex, PoisonError};

/// One delta's worth of one tool call in a scripted stream: which index
/// it belongs to, and whichever of id, name, and arguments-fragment this
/// delta carries. The test author cuts the arguments wherever they like.
#[derive(Clone, Debug, Default)]
pub struct Fragment {
    index: usize,
    pub(super) id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

impl Fragment {
    /// The opening fragment of a call: id and name announced, arguments
    /// begun with `arguments`.
    #[must_use]
    pub fn open(index: usize, id: &str, name: &str, arguments: &str) -> Self {
        Self {
            index,
            id: Some(id.to_owned()),
            name: Some(name.to_owned()),
            arguments: Some(arguments.to_owned()),
        }
    }

    /// A continuation: more of the arguments string for `index`.
    #[must_use]
    pub fn more(index: usize, arguments: &str) -> Self {
        Self {
            index,
            arguments: Some(arguments.to_owned()),
            ..Self::default()
        }
    }
}

/// What the scripted model does with one request.
#[derive(Clone, Debug)]
pub enum Turn {
    /// Text deltas, then `[DONE]`.
    Text(Vec<String>),
    /// Tool-call fragments — each inner Vec is one SSE chunk's
    /// `tool_calls` array, so the author controls exactly how arguments
    /// fragment and how indexes interleave — then finish_reason
    /// "tool_calls", then `[DONE]`.
    ToolCalls(Vec<Vec<Fragment>>),
    /// An HTTP error status with a canned body.
    Error { status: u16, body: String },
    /// A raw SSE body written verbatim: truncated streams, garbage JSON.
    Malformed(String),
}

impl Turn {
    /// Text as one delta per string slice, for the common case.
    #[must_use]
    pub fn text(deltas: &[&str]) -> Self {
        Self::Text(deltas.iter().map(|&delta| delta.to_owned()).collect())
    }
}

/// The scripted model, listening on loopback until the process ends.
pub struct Scripted {
    base_url: String,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Scripted {
    /// Spawns the model on its own thread, playing `script` one turn per
    /// request.
    #[must_use]
    pub fn spawn(script: Vec<Turn>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        std::thread::spawn(move || {
            let mut turns = script.into_iter();
            loop {
                let Ok((mut socket, _)) = listener.accept() else {
                    return;
                };
                let Some(body) = crate::chat::read_request(&mut socket) else {
                    continue;
                };
                if let Ok(body) = serde_json::from_slice(&body) {
                    recorded
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(body);
                }
                let count = recorded
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .len();
                let response = match turns.next() {
                    Some(turn) => respond(&turn),
                    None => error_response(
                        500,
                        &format!(
                            "the scripted model's script has ended; request {count} is beyond it"
                        ),
                    ),
                };
                let _ = socket.write_all(response.as_bytes());
            }
        });
        Self { base_url, requests }
    }

    /// Where to aim the client: the `/v1` prefix on loopback.
    #[must_use]
    pub fn base_url(&self) -> String {
        self.base_url.clone()
    }

    /// Every request body received so far, in order, for post-hoc
    /// assertion.
    #[must_use]
    pub fn requests(&self) -> Vec<serde_json::Value> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn sse_headers() -> &'static str {
    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
}

fn chunk(delta: &serde_json::Value, finish_reason: Option<&str>) -> String {
    let choice = serde_json::json!({ "delta": delta, "finish_reason": finish_reason });
    format!("data: {}\n\n", serde_json::json!({ "choices": [choice] }))
}

fn respond(turn: &Turn) -> String {
    match turn {
        Turn::Text(deltas) => {
            let mut body = sse_headers().to_owned();
            body.push_str(&chunk(&serde_json::json!({ "role": "assistant" }), None));
            for delta in deltas {
                body.push_str(&chunk(&serde_json::json!({ "content": delta }), None));
            }
            body.push_str(&chunk(&serde_json::json!({}), Some("stop")));
            body.push_str("data: [DONE]\n\n");
            body
        }
        Turn::ToolCalls(chunks) => {
            let mut body = sse_headers().to_owned();
            for fragments in chunks {
                let calls: Vec<_> = fragments.iter().map(fragment_json).collect();
                body.push_str(&chunk(&serde_json::json!({ "tool_calls": calls }), None));
            }
            body.push_str(&chunk(&serde_json::json!({}), Some("tool_calls")));
            body.push_str("data: [DONE]\n\n");
            body
        }
        Turn::Error { status, body } => error_body(*status, body),
        Turn::Malformed(raw) => format!("{}{raw}", sse_headers()),
    }
}

fn fragment_json(fragment: &Fragment) -> serde_json::Value {
    let mut call = serde_json::Map::new();
    call.insert("index".to_owned(), fragment.index.into());
    if let Some(id) = &fragment.id {
        call.insert("id".to_owned(), id.clone().into());
        call.insert("type".to_owned(), "function".into());
    }
    let mut function = serde_json::Map::new();
    if let Some(name) = &fragment.name {
        function.insert("name".to_owned(), name.clone().into());
    }
    if let Some(arguments) = &fragment.arguments {
        function.insert("arguments".to_owned(), arguments.clone().into());
    }
    if !function.is_empty() {
        call.insert("function".to_owned(), function.into());
    }
    call.into()
}

fn error_body(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} Scripted\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len(),
    )
}

fn error_response(status: u16, message: &str) -> String {
    let body = serde_json::json!({ "error": { "message": message } }).to_string();
    error_body(status, &body)
}
