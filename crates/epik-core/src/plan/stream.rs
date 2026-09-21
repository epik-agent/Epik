//! The stream: what a plan's tasks say while they run, and the one
//! account of what a build did and what it cost.
//!
//! An [`Event`] is something that [`Happened`] at a node. A [`Sink`]
//! is where events go: every node of a plan holds the one sink,
//! narrowed to its own name by [`Sink::node`], so many tasks running
//! at once are told apart at the far end. [`Reported`] is any task
//! with its lifecycle on the stream. Everything that crosses is serde:
//! a log is a sink that writes each event as a line of JSON.

use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use super::task::Task;
use crate::agent::Update;

/// Something that happened at `node`, which is whatever the plan names
/// its nodes by.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Event<N> {
    pub node: N,
    pub happened: Happened,
}

/// What a task has to tell: its lifecycle, and between the start and
/// the end, what its agent said.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Happened {
    /// The task began to run.
    Started,
    /// The agent said this on stdout, as its engine's protocol reads.
    Said(Update),
    /// A line of the agent's stderr: where a CLI's last words land.
    Stderr(String),
    /// The task succeeded.
    Finished,
    /// The task failed, in the error's own words, causes and all.
    Failed(String),
}

/// Where `E`s go as they happen, from any thread. Clones share the one
/// receiver, which is called as each `E` is emitted and keeps its own
/// failures.
pub struct Sink<E>(Arc<dyn Fn(E) + Send + Sync>);

impl<E> Sink<E> {
    pub fn new(receive: impl Fn(E) + Send + Sync + 'static) -> Self {
        Self(Arc::new(receive))
    }

    pub fn emit(&self, event: E) {
        (self.0)(event);
    }
}

impl<E: Send + 'static> Sink<E> {
    /// A sink and the receiver of everything emitted into it, in
    /// order. The receiver ends once every clone of the sink is gone.
    #[must_use]
    pub fn channel() -> (Self, UnboundedReceiver<E>) {
        let (sender, receiver) = unbounded_channel();
        // A receiver that has hung up has heard all it wanted.
        let sink = Self::new(move |event| drop(sender.send(event)));
        (sink, receiver)
    }
}

impl<N: Clone + Send + Sync + 'static> Sink<Event<N>> {
    /// This sink as one node sees it: what happens there, emitted
    /// under the node's name.
    #[must_use]
    pub fn node(&self, node: N) -> Sink<Happened> {
        let events = self.clone();
        Sink::new(move |happened| {
            events.emit(Event {
                node: node.clone(),
                happened,
            });
        })
    }
}

impl<E> Clone for Sink<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<E> Debug for Sink<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sink").finish_non_exhaustive()
    }
}

/// A task with its lifecycle on the stream: started, then finished or
/// failed. The outcome is the task's own either way.
#[derive(Clone, Debug)]
pub struct Reported<T> {
    pub task: T,
    pub sink: Sink<Happened>,
}

impl<T: Task> Task for Reported<T> {
    async fn run(&self) -> Result<()> {
        self.sink.emit(Happened::Started);
        let outcome = self.task.run().await;
        self.sink.emit(match &outcome {
            Ok(()) => Happened::Finished,
            Err(error) => Happened::Failed(format!("{error:#}")),
        });
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, Happened, Reported, Sink};
    use crate::agent::Update;
    use crate::plan::task::Task;
    use anyhow::{Context, Result, bail};
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::task::spawn_blocking;

    /// Everything the receiver has been sent so far.
    fn received<E>(receiver: &mut UnboundedReceiver<E>) -> Vec<E> {
        std::iter::from_fn(|| receiver.try_recv().ok()).collect()
    }

    /// A blocking call that is no process, as a request to GitHub is:
    /// it answers or it refuses, as made.
    struct Call(bool);

    impl Task for Call {
        async fn run(&self) -> Result<()> {
            let answers = self.0;
            spawn_blocking(move || {
                if !answers {
                    bail!("422 Unprocessable Entity");
                }
                Ok(())
            })
            .await?
            .context("could not open the pull request")
        }
    }

    #[tokio::test]
    async fn a_task_that_runs_no_agent_is_visible_through_its_lifecycle() {
        let (sink, mut receiver) = Sink::channel();
        let task = Reported {
            task: Call(true),
            sink: sink.node("open the pull request"),
        };
        task.run().await.unwrap();
        assert_eq!(
            received(&mut receiver),
            [Happened::Started, Happened::Finished].map(|happened| Event {
                node: "open the pull request",
                happened,
            })
        );
    }

    #[tokio::test]
    async fn a_failure_is_emitted_in_its_own_words_and_is_still_the_tasks() {
        let (sink, mut receiver) = Sink::channel();
        let task = Reported {
            task: Call(false),
            sink: sink.node(7),
        };
        let error = task.run().await.unwrap_err();
        assert_eq!(error.to_string(), "could not open the pull request");
        let words = "could not open the pull request: 422 Unprocessable Entity";
        assert_eq!(
            received(&mut receiver),
            [Happened::Started, Happened::Failed(words.to_owned())]
                .map(|happened| Event { node: 7, happened })
        );
    }

    #[test]
    fn an_event_crosses_as_json_and_comes_back_the_same() {
        let event = Event {
            node: 203,
            happened: Happened::Said(Update::ToolUse {
                name: "Write".to_owned(),
            }),
        };
        let line = serde_json::to_string(&event).unwrap();
        assert_eq!(
            line,
            r#"{"node":203,"happened":{"Said":{"ToolUse":{"name":"Write"}}}}"#
        );
        assert_eq!(serde_json::from_str::<Event<u64>>(&line).unwrap(), event);
    }

    #[test]
    fn a_sink_outlives_its_receiver() {
        let (sink, receiver) = Sink::channel();
        drop(receiver);
        sink.emit(Happened::Started);
    }
}
