#![allow(dead_code)]

mod task;

use std::iter::{Sum, repeat_with};
use std::num::NonZeroUsize;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::{JoinSet, spawn_blocking};

use crate::dag::Dag;
use task::Task;

/// Tasks and the order among them: a task runs once every task before
/// it has.
type Plan<T> = Dag<T>;

impl<T> Plan<T>
where
    T: Task + 'static,
    T::Return: Sum + Send + 'static,
{
    /// Every task that can run, each once those before it have
    /// succeeded and as the slots allow. A failure holds back what
    /// comes after it and nothing else: the other branches run to
    /// their ends. Then the first failure in [`nodes`](Dag::nodes)
    /// order is the plan's, or the returns summed in that order.
    ///
    /// The plan is consumed: each task moves to the blocking pool to
    /// run, since a task blocks, and the pool owns what it runs.
    pub async fn execute(self, slots: ProcessSlots) -> Result<T::Return> {
        let tasks = self.map(Arc::new);
        let count = tasks.nodes().len();
        let mut started = vec![false; count];
        let mut outcomes: Vec<Option<Result<T::Return>>> =
            repeat_with(|| None).take(count).collect();
        let mut running = JoinSet::new();
        loop {
            let ready: Vec<_> = tasks
                .nodes()
                .filter(|node| !started[node.position()])
                .filter(|node| {
                    node.predecessors()
                        .all(|before| matches!(outcomes[before.position()], Some(Ok(_))))
                })
                .map(|node| (node.position(), Arc::clone(node.value())))
                .collect();
            for (at, task) in ready {
                started[at] = true;
                let slots = slots.clone();
                running.spawn(async move { (at, run(task, slots).await) });
            }
            let Some(joined) = running.join_next().await else {
                break;
            };
            let (at, outcome) = joined.expect("no task is ever aborted");
            outcomes[at] = Some(outcome);
        }
        outcomes.into_iter().flatten().sum()
    }
}

/// `task` in a slot: the wait for one, then the run on the blocking
/// pool, the slot given back as the run ends. A panic in the task is
/// its error.
async fn run<T: Task + 'static>(task: Arc<T>, slots: ProcessSlots) -> Result<T::Return>
where
    T::Return: Send + 'static,
{
    let slot = slots.take().await;
    spawn_blocking(move || {
        let returned = task.run();
        drop(slot);
        returned
    })
    .await?
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

    /// A slot, as soon as one is free: the wait parks until a task
    /// anywhere on the host gives one back. Dropping the slot is the
    /// giving back.
    async fn take(&self) -> OwnedSemaphorePermit {
        Arc::clone(&self.0)
            .acquire_owned()
            .await
            .expect("the slots are never closed")
    }
}

#[cfg(test)]
mod tests {
    use super::task::Task;
    use super::{Plan, ProcessSlots};
    use crate::dag::{Dag, Node};
    use anyhow::{Result, anyhow};
    use std::num::NonZeroUsize;
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::Duration;

    fn slots(count: usize) -> ProcessSlots {
        ProcessSlots::new(NonZeroUsize::new(count).unwrap())
    }

    /// Divides, or fails on a zero divisor.
    struct Division(f32, f32);

    impl Task for Division {
        type Return = f32;

        fn run(&self) -> Result<f32> {
            if self.1 == 0.0 {
                Err(anyhow!("division by zero"))
            } else {
                Ok(self.0 / self.1)
            }
        }
    }

    /// Writes its name to a shared log when it runs, then counts one
    /// or fails, as made.
    struct Log {
        name: &'static str,
        fails: bool,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Task for Log {
        type Return = usize;

        fn run(&self) -> Result<usize> {
            self.log.lock().unwrap().push(self.name);
            if self.fails {
                Err(anyhow!("{} failed", self.name))
            } else {
                Ok(1)
            }
        }
    }

    #[tokio::test]
    async fn an_empty_plan_sums_to_the_unit() {
        let plan: Plan<Division> = Dag::new(vec![]).unwrap();
        assert_eq!(plan.execute(slots(1)).await.unwrap(), 0.0);
    }

    #[tokio::test]
    async fn a_plan_sums_what_its_tasks_return() {
        let plan = Dag::new(vec![
            Node::new(Division(10.0, 2.0), []),
            Node::new(Division(6.0, 3.0), [0]),
            Node::new(Division(1.0, 1.0), [0]),
        ])
        .unwrap();
        assert_eq!(plan.execute(slots(2)).await.unwrap(), 8.0);
    }

    #[tokio::test]
    async fn the_first_failure_is_the_plans() {
        let plan = Dag::new(vec![
            Node::new(Division(10.0, 2.0), []),
            Node::new(Division(1.0, 0.0), [0]),
            Node::new(Division(1.0, 1.0), [1]),
        ])
        .unwrap();
        let error = plan.execute(slots(2)).await.unwrap_err();
        assert_eq!(error.to_string(), "division by zero");
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
        assert_eq!(plan.execute(slots(4)).await.unwrap(), 4, "every task ran");

        let ran = log.lock().unwrap().clone();
        assert_eq!(ran.len(), 4);
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
        let error = plan.execute(slots(1)).await.unwrap_err();
        assert_eq!(error.to_string(), "b failed");

        let mut ran = log.lock().unwrap().clone();
        ran.sort_unstable();
        assert_eq!(ran, ["a", "b", "d"]);
    }

    /// Holds the slot it runs in for a moment, counting how many run
    /// at once; counts one.
    struct Hold {
        running: Arc<Mutex<(usize, usize)>>,
    }

    impl Task for Hold {
        type Return = usize;

        fn run(&self) -> Result<usize> {
            {
                let mut counts = self.running.lock().unwrap();
                counts.0 += 1;
                counts.1 = counts.1.max(counts.0);
            }
            std::thread::sleep(Duration::from_millis(20));
            self.running.lock().unwrap().0 -= 1;
            Ok(1)
        }
    }

    #[tokio::test]
    async fn one_slot_runs_the_tasks_one_at_a_time() {
        let running = Arc::new(Mutex::new((0, 0)));
        let hold = || Hold {
            running: Arc::clone(&running),
        };
        let plan = Dag::new(vec![
            Node::new(hold(), []),
            Node::new(hold(), []),
            Node::new(hold(), []),
        ])
        .unwrap();
        assert_eq!(plan.execute(slots(1)).await.unwrap(), 3);
        assert_eq!(running.lock().unwrap().1, 1, "never two at once");
    }

    /// Meets its partner at a barrier: both must be running at once.
    /// Counts one.
    struct Meet(Arc<Barrier>);

    impl Task for Meet {
        type Return = usize;

        fn run(&self) -> Result<usize> {
            self.0.wait();
            Ok(1)
        }
    }

    #[tokio::test]
    async fn two_slots_run_two_tasks_at_once() {
        let barrier = Arc::new(Barrier::new(2));
        let plan = Dag::new(vec![
            Node::new(Meet(Arc::clone(&barrier)), []),
            Node::new(Meet(barrier), []),
        ])
        .unwrap();
        assert_eq!(plan.execute(slots(2)).await.unwrap(), 2);
    }
}
