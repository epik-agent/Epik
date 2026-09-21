//! An agent's run as a task: the issue it implements is in the agent
//! already, and the run holds a process slot from start to exit.

use anyhow::{Result, bail};

use super::ProcessSlots;
use super::task::Task;
use crate::agent::Agent;
use crate::process::Exit;

/// One [`Agent`], run to its exit in one of the `slots` when the task
/// runs. The agent is cloned to start, since starting consumes one and
/// a task may only borrow itself.
#[derive(Clone, Debug)]
pub struct Implement<A> {
    pub agent: A,
    pub slots: ProcessSlots,
}

impl<A: Agent + Clone + Send + Sync + 'static> Task for Implement<A> {
    /// The agent started and its exit waited for, all within a slot:
    /// an exit other than success is the error.
    async fn run(&self) -> Result<()> {
        let agent = self.agent.clone();
        self.slots
            .run(move || match agent.start()?.wait()? {
                Exit::Code(0) => Ok(()),
                Exit::Code(code) => bail!("the agent exited with code {code}"),
                Exit::Signal(signal) => bail!("the agent was killed by signal {signal}"),
            })
            .await
    }
}
