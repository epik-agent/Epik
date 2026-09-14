//! An agent's run as a task: the issue it implements is in the agent
//! already, and what it said is the task's return.

use std::iter::Sum;

use anyhow::{Result, bail};

use super::task::Task;
use crate::agent::{Agent, Update};
use crate::process::Exit;

/// One [`Agent`], run to its exit when the task runs. The agent is
/// cloned to start, since starting consumes one and a task may only
/// borrow itself.
#[derive(Clone, Debug)]
pub struct Implement<A>(pub A);

/// Everything the agents of a plan said, in the plan's order: a
/// [`Sum`], so a plan of them returns one.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Narration(pub Vec<Update>);

impl Sum for Narration {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        Self(iter.flat_map(|narration| narration.0).collect())
    }
}

impl<A: Agent + Clone + Send + Sync> Task for Implement<A> {
    type Return = Narration;

    /// The agent started, its stdout read as updates to the end, and
    /// its exit waited for: what it said, or an exit other than
    /// success as the error.
    fn run(&self) -> Result<Narration> {
        let mut process = self.0.clone().start()?;
        let updates = process
            .stdout()
            .flat_map(|line| A::interpret(&line))
            .collect();
        match process.wait()? {
            Exit::Code(0) => Ok(Narration(updates)),
            Exit::Code(code) => bail!("the agent exited with code {code}"),
            Exit::Signal(signal) => bail!("the agent was killed by signal {signal}"),
        }
    }
}

#[cfg(all(test, unix, feature = "testing"))]
mod tests {
    use super::*;
    use crate::agent::scripted::Scripted;
    use crate::temp::Scratch;

    #[test]
    fn a_scripted_agent_runs_as_a_task_and_returns_what_it_said() {
        let scratch = Scratch::new("implement");
        let task = Implement(Scripted {
            cwd: scratch.0.clone(),
            files: [("note.txt".into(), "noted\n".to_owned())].into(),
        });
        let Narration(updates) = task.run().unwrap();
        assert!(matches!(updates[0], Update::Session { .. }));
        assert_eq!(
            updates[1],
            Update::ToolUse {
                name: "Write".to_owned()
            }
        );
        assert!(matches!(updates[2], Update::Result { ok: true, .. }));
        assert_eq!(
            std::fs::read_to_string(scratch.0.join("note.txt")).unwrap(),
            "noted\n"
        );
    }

    #[test]
    fn narrations_sum_in_order() {
        let text = |text: &str| Update::Text {
            text: text.to_owned(),
        };
        let summed: Narration = [
            Narration(vec![text("a")]),
            Narration(vec![]),
            Narration(vec![text("b"), text("c")]),
        ]
        .into_iter()
        .sum();
        assert_eq!(summed, Narration(vec![text("a"), text("b"), text("c")]));
    }
}
