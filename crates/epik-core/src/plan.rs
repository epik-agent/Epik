//! A plan: tasks and the order among them, run as the order allows.
//! [`Task`] is the work, [`Implement`] is an agent's run as one,
//! [`Plan::execute`] runs them all at once, and [`ProcessSlots`]
//! bounds the processes they launch.

pub mod implement;
pub mod task;

use std::iter::repeat_with;
use std::num::NonZeroUsize;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Semaphore;
use tokio::task::{JoinSet, spawn_blocking};

use crate::dag::Dag;
pub use implement::Implement;
pub use task::Task;

/// Tasks and the order among them: a task runs once every task before
/// it has.
pub type Plan<T> = Dag<T>;

impl<T: Task + 'static> Plan<T> {
    /// Every task that can run, each as soon as those before it have
    /// succeeded, all of them at once on the runtime. A failure holds
    /// back what comes after it and nothing else: the other branches
    /// run to their ends. Then the first failure in
    /// [`nodes`](Dag::nodes) order is the plan's. A panic in a task is
    /// its error.
    ///
    /// The plan is consumed: each task moves to the runtime to run,
    /// and the runtime owns what it runs.
    pub async fn execute(self) -> Result<()> {
        let tasks = self.map(Arc::new);
        let count = tasks.nodes().len();
        let mut started = vec![false; count];
        let mut outcomes: Vec<Option<Result<()>>> = repeat_with(|| None).take(count).collect();
        let mut running = JoinSet::new();
        loop {
            let ready: Vec<_> = tasks
                .nodes()
                .filter(|node| !started[node.position()])
                .filter(|node| {
                    node.predecessors()
                        .all(|before| matches!(outcomes[before.position()], Some(Ok(()))))
                })
                .map(|node| (node.position(), Arc::clone(node.value())))
                .collect();
            for (at, task) in ready {
                started[at] = true;
                // The task runs as a spawn of its own, so a panic in it
                // comes back here, beside its position, as its error.
                running.spawn(async move {
                    let outcome = tokio::spawn(async move { task.run().await }).await;
                    (at, outcome.unwrap_or_else(|panic| Err(panic.into())))
                });
            }
            let Some(joined) = running.join_next().await else {
                break;
            };
            let (at, outcome) = joined.expect("no task is ever aborted");
            outcomes[at] = Some(outcome);
        }
        outcomes.into_iter().flatten().collect()
    }
}

/// A set of slots in which concurrent processes may be run. A process blocks until a slot becomes
/// available and then takes ownership of that slot for the duration of its lifetime.
#[derive(Clone, Debug)]
pub struct ProcessSlots(Arc<Semaphore>);

impl ProcessSlots {
    /// `concurrency` slots.
    #[must_use]
    pub fn new(concurrency: NonZeroUsize) -> Self {
        Self(Arc::new(Semaphore::new(concurrency.get())))
    }

    /// The one door a process goes through: `work` starts a process
    /// and waits out its life. The wait for a slot parks until a task
    /// anywhere on the host gives one back; then `work` runs on the
    /// blocking pool, holding the slot, and its return gives the slot
    /// back. A panic in the work is its error.
    pub async fn run<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let slot = Arc::clone(&self.0)
            .acquire_owned()
            .await
            .expect("the slots are never closed");
        spawn_blocking(move || {
            let _slot = slot;
            work()
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::task::Task;
    use super::{Plan, ProcessSlots};
    use crate::dag::{Dag, Node};
    use crate::process::Process;
    use crate::temp::Scratch;
    use anyhow::{Result, anyhow, ensure};
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::Barrier;
    use tokio::time::{sleep, timeout};

    /// Long enough for anything here that makes progress at all.
    const PATIENCE: Duration = Duration::from_secs(30);

    fn slots(count: usize) -> ProcessSlots {
        ProcessSlots::new(NonZeroUsize::new(count).unwrap())
    }

    /// Writes its name to a shared log when it runs, then succeeds or
    /// fails, as made.
    struct Log {
        name: &'static str,
        fails: bool,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Task for Log {
        async fn run(&self) -> Result<()> {
            self.log.lock().unwrap().push(self.name);
            ensure!(!self.fails, "{} failed", self.name);
            Ok(())
        }
    }

    #[tokio::test]
    async fn an_empty_plan_succeeds() {
        let plan: Plan<Log> = Dag::new(vec![]).unwrap();
        plan.execute().await.unwrap();
    }

    #[tokio::test]
    async fn the_first_failure_in_node_order_is_the_plans() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let entry = |name, fails| Log {
            name,
            fails,
            log: Arc::clone(&log),
        };
        // c is first in node order and runs last, after a.
        let plan = Dag::new(vec![
            Node::new(entry("c", true), [1]),
            Node::new(entry("a", false), []),
            Node::new(entry("b", true), []),
        ])
        .unwrap();
        let error = plan.execute().await.unwrap_err();
        assert_eq!(error.to_string(), "c failed");
    }

    #[tokio::test]
    async fn a_task_runs_after_every_task_before_it() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let entry = |name| Log {
            name,
            fails: false,
            log: Arc::clone(&log),
        };
        // A diamond: d after b and c, which each come after a.
        let plan = Dag::new(vec![
            Node::new(entry("d"), [1, 2]),
            Node::new(entry("b"), [3]),
            Node::new(entry("c"), [3]),
            Node::new(entry("a"), []),
        ])
        .unwrap();
        plan.execute().await.unwrap();

        let ran = log.lock().unwrap().clone();
        assert_eq!(ran.len(), 4, "every task ran");
        assert_eq!(ran[0], "a");
        assert_eq!(ran[3], "d");
    }

    #[tokio::test]
    async fn a_failure_holds_back_what_comes_after_it_and_nothing_else() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let entry = |name, fails| Log {
            name,
            fails,
            log: Arc::clone(&log),
        };
        // a, then b and d; b fails, so c never runs, but d does.
        let plan = Dag::new(vec![
            Node::new(entry("a", false), []),
            Node::new(entry("b", true), [0]),
            Node::new(entry("c", false), [1]),
            Node::new(entry("d", false), [0]),
        ])
        .unwrap();
        let error = plan.execute().await.unwrap_err();
        assert_eq!(error.to_string(), "b failed");

        let mut ran = log.lock().unwrap().clone();
        ran.sort_unstable();
        assert_eq!(ran, ["a", "b", "d"]);
    }

    /// Panics when run.
    struct Panic;

    impl Task for Panic {
        async fn run(&self) -> Result<()> {
            panic!("the task panicked")
        }
    }

    #[tokio::test]
    async fn a_panic_in_a_task_is_its_error() {
        let plan = Dag::new(vec![Node::new(Panic, [])]).unwrap();
        let error = plan.execute().await.unwrap_err();
        assert!(error.to_string().contains("panicked"), "{error}");
    }

    /// Meets the others at a barrier: all must be running at once.
    struct Meet(Arc<Barrier>);

    impl Task for Meet {
        async fn run(&self) -> Result<()> {
            self.0.wait().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn nodes_that_launch_no_process_all_run_at_once() {
        let barrier = Arc::new(Barrier::new(5));
        let plan = Dag::new(
            (0..5)
                .map(|_| Node::new(Meet(Arc::clone(&barrier)), []))
                .collect(),
        )
        .unwrap();
        timeout(PATIENCE, plan.execute()).await.unwrap().unwrap();
    }

    /// `script` under `sh -c`, from `/`, to a successful exit.
    fn sh(script: &str) -> Result<()> {
        let arguments = vec!["-c".to_owned(), script.to_owned()];
        let exit = Process::new("sh", arguments, "/", [], Some(PATIENCE))?.wait()?;
        ensure!(exit.success(), "sh ended with {exit:?}");
        Ok(())
    }

    /// A process that runs until `flag` exists, or a wait for a moment
    /// and then the making of it.
    enum Flag {
        Awaited(PathBuf, ProcessSlots),
        Raised(PathBuf),
    }

    impl Task for Flag {
        async fn run(&self) -> Result<()> {
            match self {
                Self::Awaited(flag, slots) => {
                    let script = format!("until [ -e '{}' ]; do sleep 0.01; done", flag.display());
                    slots.run(move || sh(&script)).await
                }
                Self::Raised(flag) => {
                    sleep(Duration::from_millis(50)).await;
                    Ok(std::fs::write(flag, "")?)
                }
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn with_one_slot_a_sleeping_task_and_a_process_make_progress_at_once() {
        let scratch = Scratch::new("plan-flag");
        let flag = scratch.0.join("flag");
        // The process holds the one slot until the sleeper has woken.
        let plan = Dag::new(vec![
            Node::new(Flag::Awaited(flag.clone(), slots(1)), []),
            Node::new(Flag::Raised(flag), []),
        ])
        .unwrap();
        timeout(PATIENCE, plan.execute()).await.unwrap().unwrap();
    }

    /// Runs a process through the door, counting how many run at once.
    struct Hold {
        slots: ProcessSlots,
        running: Arc<Mutex<(usize, usize)>>,
    }

    impl Task for Hold {
        async fn run(&self) -> Result<()> {
            let running = Arc::clone(&self.running);
            self.slots
                .run(move || {
                    {
                        let mut counts = running.lock().unwrap();
                        counts.0 += 1;
                        counts.1 = counts.1.max(counts.0);
                    }
                    let slept = sh("sleep 0.02");
                    running.lock().unwrap().0 -= 1;
                    slept
                })
                .await
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_slots_bound_the_processes_however_many_nodes_run() {
        let slots = slots(2);
        let running = Arc::new(Mutex::new((0, 0)));
        let plan = Dag::new(
            (0..6)
                .map(|_| {
                    let hold = Hold {
                        slots: slots.clone(),
                        running: Arc::clone(&running),
                    };
                    Node::new(hold, [])
                })
                .collect(),
        )
        .unwrap();
        plan.execute().await.unwrap();
        assert!(running.lock().unwrap().1 <= 2, "never three at once");
    }

    #[tokio::test]
    async fn two_slots_let_two_processes_run_at_once() {
        let slots = slots(2);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let meet = || {
            let barrier = Arc::clone(&barrier);
            slots.run(move || {
                barrier.wait();
                Ok(())
            })
        };
        timeout(PATIENCE, async { tokio::try_join!(meet(), meet()) })
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn the_door_returns_what_the_work_does() {
        assert_eq!(slots(1).run(|| Ok(7)).await.unwrap(), 7);
        let error = slots(1).run(|| Err::<(), _>(anyhow!("no"))).await;
        assert_eq!(error.unwrap_err().to_string(), "no");
    }

    #[tokio::test]
    async fn a_slot_comes_back_when_the_work_panics() {
        let slots = slots(1);
        assert!(
            slots
                .run(|| -> Result<()> { panic!("oops") })
                .await
                .is_err()
        );
        slots.run(|| Ok(())).await.unwrap();
    }
}
