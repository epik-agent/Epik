//! The first real [`CodingAgent`]: Claude Code behind the one verb.
//!
//! The wrapper spawns `claude -p <prompt> --output-format stream-json
//! --verbose` in the task's worktree, the task's env laid over its own, and
//! folds the JSON lines the process prints into [`AgentEvent`]s. What only
//! Claude understands — the init handshake, tool calls, tool results —
//! passes through [`AgentEvent::Detail`] untranslated.
//!
//! Governance lives here, not around here: a black box mid-`run` cannot be
//! killed from outside, so a private reader thread feeds a channel, the run
//! loop waits in `recv_timeout`, and a budget breach, a stall, or a
//! cancellation kills the whole process group — Claude Code spawns children,
//! and a budget that stops the parent while the children keep billing is no
//! budget — before `Finished` reports the corresponding [`Stop`].
//!
//! Unix only, by decision: the group kill *is* the governance, and a port
//! without it (Windows would need Job Objects) would be silently
//! half-governed — the exact failure this module exists to prevent. CI
//! covers Linux and macOS; elsewhere the type honestly does not exist.

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::agent::{AgentError, AgentEvent, CodingAgent, Stop, Task, finish};
use crate::chat::StopToken;
use crate::event::{Money, Usage};

/// How long a silent wait holds before glancing at the stop token: the
/// ceiling on cancellation latency. Invisible to the stall clock, which
/// measures real elapsed time rather than counting wakes.
const GLANCE: Duration = Duration::from_millis(100);

/// How long [`died`] waits for the stderr gatherer after the group kill.
/// Everything holding the pipe is dead, so EOF is imminent — but a bounded
/// wait cannot hang a run past every budget, and the paranoia is free.
const GRACE: Duration = Duration::from_secs(2);

/// Claude Code, as configuration: which binary, and which tools it may use.
#[derive(Clone, Debug)]
pub struct ClaudeCode {
    binary: PathBuf,
    allowed_tools: Option<Vec<String>>,
}

impl Default for ClaudeCode {
    fn default() -> Self {
        Self::at("claude")
    }
}

impl ClaudeCode {
    /// The `claude` on `PATH`, every tool pre-granted.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The binary config points at. An explicit path is the reliable
    /// spelling — launchd's `PATH` is not a login shell's — and a bare name
    /// falls back to `PATH` resolution, which is what [`new`](Self::new)
    /// does.
    #[must_use]
    pub fn at(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            allowed_tools: None,
        }
    }

    /// The tapering dial: pre-grant exactly these tools and no others.
    /// Headless has nobody to ask, so an unlisted tool is refused, never
    /// prompted for — including `Task`, Claude Code's own subagent spawner,
    /// withheld by omitting it. Without this the run gets everything
    /// (`--dangerously-skip-permissions`): the wide-now posture, tapering
    /// by evidence.
    #[must_use]
    pub fn allowing(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_tools = Some(tools.into_iter().map(Into::into).collect());
        self
    }

    /// The invocation, fully provisioned: cwd the worktree, env injected
    /// from the task on top of the wrapper's own, permissions settled up
    /// front, and every pipe owned so nothing reaches a terminal.
    fn command(&self, task: &Task) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .arg("-p")
            .arg(&task.prompt)
            .args(["--output-format", "stream-json", "--verbose"])
            .current_dir(&task.worktree)
            .envs(task.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match &self.allowed_tools {
            Some(tools) => {
                command.arg("--allowedTools").arg(tools.join(","));
            }
            None => {
                command.arg("--dangerously-skip-permissions");
            }
        }
        // Its own process group, so that ending the run ends the children
        // Claude Code spawned along with it.
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command
    }
}

impl CodingAgent for ClaudeCode {
    fn run(
        &self,
        task: &Task,
        sink: &mut dyn FnMut(AgentEvent),
        stop: &StopToken,
    ) -> Result<Stop, AgentError> {
        // A run canceled before anything was spawned still has its shape:
        // `Started`, then the one `Finished` — and no process to show for
        // either.
        if stop.is_stopped() {
            sink(started());
            return Ok(finish(sink, Stop::Canceled));
        }
        let mut child = self
            .command(task)
            .spawn()
            .map_err(|error| AgentError::Unstartable {
                binary: self.binary.clone(),
                error: error.to_string(),
            })?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            // `Stdio::piped` above makes this unreachable; a typed refusal
            // beats a panic all the same, and an errored run emits nothing.
            reap(child);
            return Err(AgentError::Broken {
                error: "the spawned claude arrived without its pipes".to_owned(),
            });
        };
        // `Started` is emitted only for a process that actually started —
        // an `Unstartable` run streams no events at all — and carries the
        // wrapper's version: the binary's own is unknown until its init
        // line arrives, and the first event cannot wait on a process that
        // may never speak. The init line, Claude's version included, passes
        // through `Detail` when it does.
        sink(started());
        let lines = lines(stdout);
        let noise = gather(stderr);
        let mut fold = Fold::default();
        let mut heartbeat = Instant::now();
        // `None`: stdout closed with no result line — the process is gone,
        // and the account is settled once the reap returns its status.
        let ended = loop {
            if stop.is_stopped() {
                break Some(Stop::Canceled);
            }
            let Some(patience) = task.budget.stall.checked_sub(heartbeat.elapsed()) else {
                break Some(Stop::Stalled);
            };
            match lines.recv_timeout(patience.min(GLANCE)) {
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break None,
                Ok(line) => {
                    heartbeat = Instant::now();
                    let resulted = fold.line(&line, sink);
                    // The budget outranks the result: real claude states
                    // its cost only on the result line, and a final
                    // reading over the ceiling is a breach, not a finish.
                    if task.budget.spent(&fold.total) {
                        break Some(Stop::Spent);
                    }
                    if let Some(stop) = resulted {
                        break Some(stop);
                    }
                }
            }
        };
        // The one reap: `reap` consumes the child, so no exit path can
        // kill a pid the OS may have reissued.
        let status = reap(child);
        let stopped = ended.unwrap_or_else(|| died(status, &noise));
        Ok(finish(sink, stopped))
    }
}

/// The wrapper naming itself; always the run's first event.
fn started() -> AgentEvent {
    AgentEvent::Started {
        agent: "claude-code".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// The private reader: stdout lines into a channel, so the run loop can
/// wait with a timeout — which is the whole stall mechanism. Bytes are
/// decoded lossily, because one mangled byte must not read as the whole
/// process dying; only genuine EOF — or a read error, after which nothing
/// more will ever arrive — ends the stream.
fn lines(stdout: ChildStdout) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buffer).trim_end().to_owned();
                    if sender.send(line).is_err() {
                        return;
                    }
                }
            }
        }
    });
    receiver
}

/// Collects stderr off-thread: an unread pipe would wedge a chatty child,
/// and what a dying claude says there is the best account of why it died.
/// A channel rather than a join handle, so the one reader ([`died`]) can
/// bound its wait.
fn gather(stderr: ChildStderr) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut text);
        let _ = sender.send(text);
    });
    receiver
}

/// The process ended on its own terms without a result line. Composed after
/// the reap, from the status it returned and whatever stderr held.
fn died(status: Option<ExitStatus>, noise: &Receiver<String>) -> Stop {
    let ended = status.map_or_else(
        || "claude ended without a result".to_owned(),
        |status| format!("claude ended without a result ({status})"),
    );
    let noise = noise.recv_timeout(GRACE).unwrap_or_default();
    let noise = noise.trim();
    Stop::Died {
        error: if noise.is_empty() {
            ended
        } else {
            format!("{ended}: {noise}")
        },
    }
}

/// Kills the whole process group and reaps the child — consuming it, so a
/// second reap of the same run cannot typecheck, let alone signal a pid the
/// OS has reissued. Harmless on a process that already exited: the kill
/// misses, the wait returns the stored status.
fn reap(mut child: Child) -> Option<ExitStatus> {
    // The group, not the child — see the module doc. `kill(1)` rather than
    // libc, to stay dependency-free; `--` because a negative pid reads like
    // a flag.
    let _ = Command::new("kill")
        .args(["-9", "--", &format!("-{}", child.id())])
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    child.wait().ok()
}

/// The running translation of Claude Code's stream into the contract's
/// vocabulary.
#[derive(Debug, Default)]
struct Fold {
    /// The wrapper's own cumulative totals — grown, never replaced, so no
    /// claim the feed makes can run them backward.
    total: Usage,
}

impl Fold {
    /// Folds one stdout line, emitting whatever it translates to. Returns
    /// the run's stop when the line was the result — the only line that
    /// ends a run. A line that is not JSON translates to nothing; reaching
    /// the loop at all already reset the stall clock, which is all a stray
    /// line is good for.
    fn line(&mut self, line: &str, sink: &mut dyn FnMut(AgentEvent)) -> Option<Stop> {
        let value: Value = serde_json::from_str(line).ok()?;
        match value.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                self.spoke(&value, sink);
                None
            }
            Some("result") => Some(self.resulted(&value, sink)),
            // The init handshake, tool results, message types not yet
            // invented: Claude-specific richness passes through whole.
            _ => {
                sink(AgentEvent::Detail(value));
                None
            }
        }
    }

    /// An assistant message: text becomes narrative, anything else in the
    /// content passes through, and the message's usage joins the bill.
    fn spoke(&mut self, value: &Value, sink: &mut dyn FnMut(AgentEvent)) {
        let message = value.get("message");
        let content = message
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array);
        let mut richness = false;
        for block in content.into_iter().flatten() {
            if block.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    sink(AgentEvent::Progress(text.to_owned()));
                }
            } else {
                richness = true;
            }
        }
        if richness {
            sink(AgentEvent::Detail(value.clone()));
        }
        // Per-message usage is one API call's bill — an increment, summed —
        // while a stated cost is by name a total: clamped in, never added.
        let increment = message
            .and_then(|message| message.get("usage"))
            .map_or_else(Usage::default, tokens);
        let mut claim = self.total.plus(increment);
        if let Some(stated) = cost(value) {
            claim.cost = Some(claim.cost.map_or(stated, |cost| cost.max(stated)));
        }
        self.credit(claim, sink);
    }

    /// The result line: totals that include Claude Code's own subagents
    /// land as the authoritative final reading — clamped against the
    /// running bill, because the final reading may not run backward either
    /// — and the subtype says how the run ended.
    fn resulted(&mut self, value: &Value, sink: &mut dyn FnMut(AgentEvent)) -> Stop {
        let mut claim = value.get("usage").map_or_else(Usage::default, tokens);
        claim.cost = cost(value);
        self.credit(self.total.max(claim), sink);
        let errored = value.get("is_error").and_then(Value::as_bool) == Some(true)
            || value
                .get("subtype")
                .and_then(Value::as_str)
                .is_some_and(|subtype| subtype != "success");
        if errored {
            let error = value
                .get("result")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .or_else(|| value.get("subtype").and_then(Value::as_str))
                .unwrap_or("claude reported an error")
                .to_owned();
            Stop::Died { error }
        } else {
            // A belief, not a fact: done is the harness's judgment,
            // made out-of-band.
            Stop::Completed
        }
    }

    /// Takes a grown reading, emitting it only when it says something new:
    /// suppressing a byte-identical repeat conforms, and keeps the stream
    /// legible.
    fn credit(&mut self, grown: Usage, sink: &mut dyn FnMut(AgentEvent)) {
        if grown != self.total {
            self.total = grown;
            sink(AgentEvent::Usage(grown));
        }
    }
}

/// The token counts of a stream-json `usage` object, in the contract's
/// denominations: cache creation and cache reads are both cache tokens,
/// distinguished from the prompt exactly as far as Claude distinguishes
/// them. Counts saturate rather than vanish when they exceed the wire type.
fn tokens(usage: &Value) -> Usage {
    let count = |key: &str| {
        usage
            .get(key)
            .and_then(Value::as_u64)
            .map(|count| u32::try_from(count).unwrap_or(u32::MAX))
    };
    let cache = match (
        count("cache_read_input_tokens"),
        count("cache_creation_input_tokens"),
    ) {
        (None, None) => None,
        (read, creation) => Some(read.unwrap_or(0).saturating_add(creation.unwrap_or(0))),
    };
    Usage {
        prompt_tokens: count("input_tokens").unwrap_or(0),
        completion_tokens: count("output_tokens").unwrap_or(0),
        cache_tokens: cache,
        cost: None,
    }
}

/// A stated cost, wherever the feed states one. Claude Code names
/// `total_cost_usd` on its result line today; the fold believes the name
/// wherever it appears. Reported, never synthesized — and a reading that is
/// negative, infinite, or absurd is no reading.
fn cost(value: &Value) -> Option<Money> {
    let usd = value.get("total_cost_usd").and_then(Value::as_f64)?;
    let micro = (usd * 1_000_000.0).round();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )] // guarded on the line below
    (micro.is_finite() && micro >= 0.0 && micro < u64::MAX as f64).then_some(Money {
        micro_usd: micro as u64,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Runs lines through a fresh fold, collecting what comes out.
    fn folded(lines: &[Value]) -> (Vec<AgentEvent>, Option<Stop>) {
        let mut fold = Fold::default();
        let mut events = Vec::new();
        let mut sink = |event: AgentEvent| events.push(event);
        let mut ended = None;
        for line in lines {
            ended = fold.line(&line.to_string(), &mut sink);
            if ended.is_some() {
                break;
            }
        }
        (events, ended)
    }

    #[test]
    fn assistant_text_is_narrative_and_tool_use_is_detail() {
        let talking = json!({"type": "assistant", "message": {"content": [
            {"type": "text", "text": "reading the issue"},
        ]}});
        let reaching = json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "Bash", "input": {"command": "cargo test"}},
        ]}});
        let (events, ended) = folded(&[talking, reaching.clone()]);
        assert_eq!(ended, None);
        assert_eq!(
            events,
            [
                AgentEvent::Progress("reading the issue".to_owned()),
                AgentEvent::Detail(reaching),
            ]
        );
    }

    #[test]
    fn the_handshake_and_the_tool_results_pass_through_whole() {
        let init = json!({"type": "system", "subtype": "init", "model": "claude-sonnet-5"});
        let answer = json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "content": "ok"},
        ]}});
        let (events, _) = folded(&[init.clone(), answer.clone()]);
        assert_eq!(
            events,
            [AgentEvent::Detail(init), AgentEvent::Detail(answer)],
            "richness the contract has no word for is passed through, not dropped"
        );
    }

    #[test]
    fn per_message_usage_accumulates_and_repeats_are_suppressed() {
        let billed = |input: u32, output: u32| {
            json!({"type": "assistant", "message": {
                "content": [],
                "usage": {"input_tokens": input, "output_tokens": output},
            }})
        };
        let (events, _) = folded(&[billed(100, 10), billed(0, 0), billed(20, 2)]);
        assert_eq!(
            events,
            [
                AgentEvent::Usage(Usage::tokens(100, 10)),
                AgentEvent::Usage(Usage::tokens(120, 12)),
            ],
            "increments sum; a bill of nothing says nothing"
        );
    }

    #[test]
    fn cache_tokens_are_distinguished_exactly_as_far_as_claude_distinguishes() {
        let line = json!({"type": "assistant", "message": {"content": [], "usage": {
            "input_tokens": 7,
            "output_tokens": 3,
            "cache_read_input_tokens": 40,
            "cache_creation_input_tokens": 2,
        }}});
        let (events, _) = folded(&[line]);
        assert_eq!(
            events,
            [AgentEvent::Usage(Usage {
                cache_tokens: Some(42),
                ..Usage::tokens(7, 3)
            })]
        );
    }

    #[test]
    fn a_successful_result_lands_its_totals_and_completes() {
        let mid = json!({"type": "assistant", "message": {"content": [], "usage": {
            "input_tokens": 10, "output_tokens": 5,
        }}});
        // The result's totals include subagents the per-message bills never
        // saw, and its cost is the first cost the stream ever states.
        let result = json!({"type": "result", "subtype": "success", "is_error": false,
            "result": "done",
            "usage": {"input_tokens": 200, "output_tokens": 50},
            "total_cost_usd": 0.0035,
        });
        let (events, ended) = folded(&[mid, result]);
        assert_eq!(ended, Some(Stop::Completed));
        assert_eq!(
            events.last(),
            Some(&AgentEvent::Usage(Usage {
                cost: Some(Money { micro_usd: 3_500 }),
                ..Usage::tokens(200, 50)
            })),
            "the final reading is authoritative, and money is fixed-point"
        );
    }

    #[test]
    fn a_result_that_reads_lower_than_the_bill_cannot_run_it_backward() {
        let mid = json!({"type": "assistant", "message": {"content": [], "usage": {
            "input_tokens": 100, "output_tokens": 50,
        }}});
        let result = json!({"type": "result", "subtype": "success",
            "usage": {"input_tokens": 90, "output_tokens": 50},
        });
        let (events, ended) = folded(&[mid, result]);
        assert_eq!(ended, Some(Stop::Completed));
        assert_eq!(
            events,
            [AgentEvent::Usage(Usage::tokens(100, 50))],
            "the clamped final reading is the reading already emitted, said once"
        );
    }

    #[test]
    fn an_error_result_is_a_death_that_says_why() {
        let result = json!({"type": "result", "subtype": "error_during_execution",
            "is_error": true, "result": "the model refused"});
        let (_, ended) = folded(&[result]);
        assert_eq!(
            ended,
            Some(Stop::Died {
                error: "the model refused".to_owned()
            })
        );
    }

    #[test]
    fn a_line_that_is_not_json_translates_to_nothing() {
        let mut fold = Fold::default();
        let mut events = Vec::new();
        let mut sink = |event: AgentEvent| events.push(event);
        assert_eq!(
            fold.line("warning: not part of the protocol", &mut sink),
            None
        );
        assert!(events.is_empty());
    }

    #[test]
    fn an_absurd_cost_is_no_reading() {
        assert_eq!(cost(&json!({"total_cost_usd": -1.0})), None);
        assert_eq!(
            cost(&json!({"total_cost_usd": 0.0003})),
            Some(Money { micro_usd: 300 }),
            "micro-dollar precision survives the float"
        );
    }
}
