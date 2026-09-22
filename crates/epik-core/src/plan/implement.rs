//! An agent's run as a task: the issue it implements is in the agent
//! already, the run holds an agent slot from start to exit, and what
//! the agent says leaves on the stream as it arrives.

use anyhow::{Result, bail};

use super::AgentSlots;
use super::stream::{Happened, Sink};
use super::task::Task;
use crate::agent::Agent;
use crate::process::Exit;

/// One [`Agent`], run to its exit in one of the `slots` when the task
/// runs, with what it says emitted into the `sink` as it says it. This
/// is the one task that takes a slot: an agent is what a slot bounds.
/// The agent is cloned to start, since starting consumes one and a task
/// may only borrow itself.
#[derive(Clone, Debug)]
pub struct Implement<A> {
    pub agent: A,
    pub slots: AgentSlots,
    pub sink: Sink<Happened>,
}

impl<A: Agent + Clone + Send + Sync + 'static> Task for Implement<A> {
    /// The agent started, heard out, and its exit waited for, all
    /// within a slot: an exit other than success is the error.
    ///
    /// Each line of stdout is emitted as the updates it carries, and
    /// each line of stderr as itself, the two streams followed at once
    /// on a thread apiece. Both are drained to their ends before the
    /// exit is waited for, so the slot is held until the process and
    /// anything holding its pipes open are gone.
    async fn run(&self) -> Result<()> {
        let agent = self.agent.clone();
        let sink = self.sink.clone();
        self.slots
            .run(move || {
                let mut process = agent.start()?;
                std::thread::scope(|scope| {
                    scope.spawn(|| {
                        for line in process.stderr() {
                            sink.emit(Happened::Stderr(line));
                        }
                    });
                    for update in process.stdout().flat_map(|line| A::interpret(&line)) {
                        sink.emit(Happened::Said(update));
                    }
                });
                match process.wait()? {
                    Exit::Code(0) => Ok(()),
                    Exit::Code(code) => bail!("the agent exited with code {code}"),
                    Exit::Signal(signal) => bail!("the agent was killed by signal {signal}"),
                }
            })
            .await
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::Implement;
    use crate::agent::claude::ClaudeCode;
    use crate::agent::{Agent, Update};
    use crate::dag::{Dag, Node};
    use crate::plan::stream::{Event, Happened, Reported, Sink, received};
    use crate::plan::{AgentSlots, Task};
    use crate::process::Process;
    use crate::temp::Scratch;
    use anyhow::Result;
    use rust_decimal::Decimal;
    use std::num::NonZeroUsize;
    use std::time::Duration;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::time::timeout;

    /// Long enough for anything here that makes progress at all.
    const PATIENCE: Duration = Duration::from_secs(30);

    /// A complete captured session of Claude Code's, where it is on
    /// disk.
    const SESSION: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/agent/claude/fixtures/session.jsonl"
    );

    /// The scripted engine: `sh -c` of an inline script, from `/`. A
    /// line that is Claude Code's protocol reads as it does there, and
    /// any other line is the engine's text.
    #[derive(Clone)]
    struct Script(String);

    impl Agent for Script {
        fn start(self) -> Result<Process> {
            Process::new("sh", vec!["-c".to_owned(), self.0], "/", [], None)
        }

        fn interpret(line: &str) -> Vec<Update> {
            let updates = ClaudeCode::interpret(line);
            if updates.is_empty() {
                vec![text(line)]
            } else {
                updates
            }
        }
    }

    fn text(text: &str) -> Update {
        Update::Text {
            text: text.to_owned(),
        }
    }

    fn slots(count: usize) -> AgentSlots {
        AgentSlots::new(NonZeroUsize::new(count).unwrap())
    }

    fn implement(script: &str, slots: &AgentSlots, sink: Sink<Happened>) -> Implement<Script> {
        Implement {
            agent: Script(script.to_owned()),
            slots: slots.clone(),
            sink,
        }
    }

    /// The next thing to arrive, which had better be soon.
    async fn next<E>(receiver: &mut UnboundedReceiver<E>) -> E {
        timeout(PATIENCE, receiver.recv())
            .await
            .expect("something arrives")
            .expect("the sink is still held")
    }

    /// A script that says `one`, runs until `flag` exists, and says
    /// `two`.
    fn paused(flag: &str) -> String {
        format!("echo one; until [ -e '{flag}' ]; do sleep 0.01; done; echo two")
    }

    #[tokio::test]
    async fn a_line_is_received_while_the_process_is_still_running() {
        let scratch = Scratch::new("implement-paused");
        let flag = scratch.join("flag");
        let (sink, mut receiver) = Sink::channel();
        let task = implement(&paused(&flag), &slots(1), sink);
        let run = tokio::spawn(async move { task.run().await });

        // The process cannot end until the flag is raised, and the
        // flag is raised only once its first line is here.
        assert_eq!(next(&mut receiver).await, Happened::Said(text("one")));
        std::fs::write(&flag, "").unwrap();
        assert_eq!(next(&mut receiver).await, Happened::Said(text("two")));
        run.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_run_holds_one_slot_while_its_process_is_alive_and_none_after() {
        let scratch = Scratch::new("implement-slot");
        let flag = scratch.join("flag");
        let slots = slots(2);
        let (sink, mut receiver) = Sink::channel();
        let task = implement(&paused(&flag), &slots, sink);
        let run = tokio::spawn(async move { task.run().await });

        next(&mut receiver).await;
        assert_eq!(slots.0.available_permits(), 1, "one slot while it lives");
        std::fs::write(&flag, "").unwrap();
        run.await.unwrap().unwrap();
        assert_eq!(slots.0.available_permits(), 2, "and none after");
    }

    #[tokio::test]
    async fn two_concurrent_runs_are_told_apart_by_the_node_each_names() {
        let slots = slots(2);
        let (sink, mut receiver) = Sink::channel();
        let plan = Dag::new(
            [203, 205]
                .map(|issue| {
                    let script = format!("echo 'implementing {issue}'; echo 'done with {issue}'");
                    Node::new(implement(&script, &slots, sink.node(issue)), [])
                })
                .into(),
        )
        .unwrap();
        plan.execute().await.unwrap();

        let events = received(&mut receiver);
        assert_eq!(events.len(), 4, "{events:?}");
        for issue in [203, 205] {
            let said: Vec<_> = events.iter().filter(|event| event.node == issue).collect();
            let expected = [
                format!("implementing {issue}"),
                format!("done with {issue}"),
            ]
            .map(|line| Event {
                node: issue,
                happened: Happened::Said(text(&line)),
            });
            assert_eq!(said, expected.iter().collect::<Vec<_>>());
        }
    }

    #[tokio::test]
    async fn stderr_arrives_alongside_the_failure() {
        let (sink, mut receiver) = Sink::channel();
        let script = "echo 'error: unknown option --bogus' >&2; exit 2";
        let task = Reported::with(sink, |sink| implement(script, &slots(1), sink));
        task.run().await.unwrap_err();
        assert_eq!(
            received(&mut receiver),
            [
                Happened::Started,
                Happened::Stderr("error: unknown option --bogus".to_owned()),
                Happened::Failed("the agent exited with code 2".to_owned()),
            ]
        );
    }

    #[tokio::test]
    async fn a_missing_binary_is_the_failure() {
        let (sink, mut receiver) = Sink::channel();
        let agent = ClaudeCode {
            binary: "/nonexistent/claude".into(),
            cwd: "/".into(),
            prompt: String::new(),
            model: None,
            api_key: None,
        };
        let task = Reported::with(sink, |sink| Implement {
            agent,
            slots: slots(1),
            sink,
        });
        task.run().await.unwrap_err();
        let events = received(&mut receiver);
        let [Happened::Started, Happened::Failed(words)] = events.as_slice() else {
            panic!("started, then failed: {events:?}");
        };
        assert!(
            words.contains("could not start /nonexistent/claude"),
            "{words}"
        );
    }

    #[tokio::test]
    async fn the_bill_is_recoverable_from_the_stream_alone() {
        let (sink, mut receiver) = Sink::channel();
        implement(&format!("cat '{SESSION}'"), &slots(1), sink.node(203))
            .run()
            .await
            .unwrap();

        // As a log would hold it: a line of JSON for each event.
        let log: Vec<String> = received(&mut receiver)
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect();
        let bills: Vec<_> = log
            .iter()
            .map(|line| serde_json::from_str::<Event<u64>>(line).unwrap())
            .filter_map(|event| match event.happened {
                Happened::Said(Update::Result {
                    cost_usd,
                    input_tokens,
                    output_tokens,
                    ..
                }) => Some((event.node, cost_usd, input_tokens, output_tokens)),
                _ => None,
            })
            .collect();
        assert_eq!(
            bills,
            [(203, Some(Decimal::new(316_251, 6)), Some(4), Some(181))]
        );
    }
}
