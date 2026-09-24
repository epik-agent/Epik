//! A state machine of tasks, run as a task. A machine acts in the
//! state it is in, then decides from that state and what the acting
//! produced where to go next; the deciding is a pure function over
//! values, so a transition table can be read and tested as a table.
//! Every back-edge a build loops around lives inside one machine, so
//! the [`Plan`](super::Plan) holding it sees a node that has either
//! finished or not, and stays a [`Dag`](crate::dag::Dag).

use anyhow::Result;

use super::task::Task;

/// A machine's states and what acting in one produces. The machine
/// itself holds what every state draws on; a [`State`](Self::State)
/// holds where the machine is and what it carries there.
pub trait TaskMachine: Send + Sync {
    /// Where the machine is, and what it carries there: the commit,
    /// the pull request, the worktree, the attempt. A state is moved
    /// through [`next`](Self::next), so it may own what it must drop.
    type State: Send;

    /// What acting in a state produced, as a value. Every outcome a
    /// transition tells apart is a variant of this, because
    /// [`next`](Self::next) reads values and nothing else.
    type Outcome: Send;

    /// The state the machine begins in.
    fn start(&self) -> Self::State;

    /// The state's work. An error is a failure the machine cannot
    /// branch on, and is the whole machine's.
    fn act(&self, state: &Self::State) -> impl Future<Output = Result<Self::Outcome>> + Send;

    /// Where to go from `state` given `outcome`: no I/O, no clock, no
    /// git. The state is this function's to keep, to hand on in
    /// [`Next::Go`], or to drop.
    fn next(&self, state: Self::State, outcome: Self::Outcome) -> Next<Self::State>;
}

/// Where a machine goes from the state it has just acted in.
pub enum Next<S> {
    /// On, into `S`, where the machine acts again.
    Go(S),
    /// To the end, having done what it set out to do.
    Done,
    /// To the end, with this error as the machine's.
    Fail(anyhow::Error),
}

/// A [`TaskMachine`] run as a task: act, decide, repeat, until
/// [`Done`](Next::Done) or [`Fail`](Next::Fail). It stands wherever a
/// task stands, so a plan holding one is a plan of tasks like any
/// other.
#[derive(Clone, Copy, Debug)]
pub struct Machine<M>(pub M);

impl<M: TaskMachine> Task for Machine<M> {
    /// The machine walked from [`start`](TaskMachine::start) to its
    /// end. The state lives in this future, so it goes when the
    /// machine ends however the machine ends — by `Done`, by `Fail`,
    /// or by this future being dropped where it stands.
    ///
    /// # Errors
    ///
    /// A [`Fail`](Next::Fail) in its own words, and an error from
    /// [`act`](TaskMachine::act), which ends the machine where it
    /// stands with [`next`](TaskMachine::next) unasked.
    async fn run(&self) -> Result<()> {
        let mut state = self.0.start();
        loop {
            let outcome = self.0.act(&state).await?;
            match self.0.next(state, outcome) {
                Next::Go(going) => state = going,
                Next::Done => return Ok(()),
                Next::Fail(error) => return Err(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Machine, Next, TaskMachine};
    use crate::dag::{Dag, Node};
    use crate::plan::Plan;
    use crate::plan::task::Task;
    use anyhow::{Result, bail};
    use std::fmt::{self, Display, Formatter};
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::time::{sleep, timeout};

    /// Long enough for anything here that makes progress at all.
    const PATIENCE: Duration = Duration::from_secs(30);

    /// Counts down: each act takes one off the count, and the machine
    /// is done once it reaches zero. What it acted on is written to a
    /// shared log, so a test can read the states it passed through.
    struct Countdown {
        from: usize,
        log: Arc<Mutex<Vec<usize>>>,
    }

    impl TaskMachine for Countdown {
        type State = usize;
        type Outcome = usize;

        fn start(&self) -> usize {
            self.from
        }

        async fn act(&self, state: &usize) -> Result<usize> {
            self.log.lock().unwrap().push(*state);
            Ok(state.saturating_sub(1))
        }

        fn next(&self, _state: usize, outcome: usize) -> Next<usize> {
            if outcome == 0 {
                Next::Done
            } else {
                Next::Go(outcome)
            }
        }
    }

    /// Writes its name to a shared log when it runs.
    struct Log {
        name: &'static str,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Task for Log {
        async fn run(&self) -> Result<()> {
            self.log.lock().unwrap().push(self.name);
            Ok(())
        }
    }

    /// Either of the two tasks a plan below holds, so that the plan is
    /// a `Dag` of one type as `Plan` asks.
    enum Either {
        Counting(Machine<Countdown>),
        Logging(Log),
    }

    impl Task for Either {
        async fn run(&self) -> Result<()> {
            match self {
                Self::Counting(machine) => machine.run().await,
                Self::Logging(log) => log.run().await,
            }
        }
    }

    #[tokio::test]
    async fn a_machine_stands_in_a_plan_and_what_follows_runs_once_it_is_done() {
        let counted = Arc::new(Mutex::new(Vec::new()));
        let ran = Arc::new(Mutex::new(Vec::new()));
        let machine = Either::Counting(Machine(Countdown {
            from: 3,
            log: Arc::clone(&counted),
        }));
        let after = Either::Logging(Log {
            name: "after",
            log: Arc::clone(&ran),
        });
        let plan: Plan<Either> = Dag::new(vec![Node::new(after, [1]), Node::new(machine, [])])
            .expect("the plan is acyclic");
        plan.execute().await.unwrap();

        assert_eq!(*counted.lock().unwrap(), [3, 2, 1], "it looped to its end");
        assert_eq!(*ran.lock().unwrap(), ["after"], "and what follows ran");
    }

    #[tokio::test]
    async fn a_machine_acts_again_in_the_state_next_sends_it_back_to() {
        let log = Arc::new(Mutex::new(Vec::new()));
        Machine(Countdown {
            from: 5,
            log: Arc::clone(&log),
        })
        .run()
        .await
        .unwrap();
        assert_eq!(*log.lock().unwrap(), [5, 4, 3, 2, 1]);
    }

    /// A machine that gives up at once, in words of its own.
    #[derive(Debug)]
    struct GaveUp;

    impl Display for GaveUp {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            f.write_str("the machine gave up")
        }
    }

    impl std::error::Error for GaveUp {}

    struct Failing;

    impl TaskMachine for Failing {
        type State = ();
        type Outcome = ();

        fn start(&self) {}

        async fn act(&self, (): &()) -> Result<()> {
            Ok(())
        }

        fn next(&self, (): (), (): ()) -> Next<()> {
            Next::Fail(GaveUp.into())
        }
    }

    #[tokio::test]
    async fn a_fail_is_the_tasks_error_in_its_own_words() {
        let error = Machine(Failing).run().await.unwrap_err();
        assert!(error.is::<GaveUp>(), "{error}");
        assert_eq!(error.to_string(), "the machine gave up");
    }

    /// A machine whose acting fails, and whose deciding would panic if
    /// it were ever asked.
    struct ActFails(Arc<Mutex<bool>>);

    impl TaskMachine for ActFails {
        type State = ();
        type Outcome = ();

        fn start(&self) {}

        async fn act(&self, (): &()) -> Result<()> {
            bail!("acting went wrong")
        }

        fn next(&self, (): (), (): ()) -> Next<()> {
            *self.0.lock().unwrap() = true;
            Next::Done
        }
    }

    #[tokio::test]
    async fn an_error_from_act_is_the_tasks_error_and_next_is_not_consulted() {
        let decided = Arc::new(Mutex::new(false));
        let error = Machine(ActFails(Arc::clone(&decided)))
            .run()
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "acting went wrong");
        assert!(!*decided.lock().unwrap(), "next was never asked");
    }

    /// Marks a shared flag when it is dropped: a state holding one
    /// says, after the fact, that the machine let go of what it
    /// carried.
    struct Sentinel(Arc<Mutex<bool>>);

    impl Drop for Sentinel {
        fn drop(&mut self) {
            *self.0.lock().unwrap() = true;
        }
    }

    /// How a machine carrying a sentinel ends.
    #[derive(Clone, Copy)]
    enum Ending {
        Done,
        Fail,
        /// Acts forever, for a caller that means to drop the future.
        Never,
    }

    /// Carries a [`Sentinel`] in its state and ends as `ending` says,
    /// with the sentinel's flag the test's own to read afterwards.
    struct Ends {
        ending: Ending,
        dropped: Arc<Mutex<bool>>,
    }

    impl TaskMachine for Ends {
        type State = Sentinel;
        type Outcome = ();

        fn start(&self) -> Sentinel {
            Sentinel(Arc::clone(&self.dropped))
        }

        async fn act(&self, _state: &Sentinel) -> Result<()> {
            if matches!(self.ending, Ending::Never) {
                loop {
                    sleep(Duration::from_millis(5)).await;
                }
            }
            Ok(())
        }

        fn next(&self, state: Sentinel, (): ()) -> Next<Sentinel> {
            match self.ending {
                Ending::Done => Next::Done,
                Ending::Fail => Next::Fail(GaveUp.into()),
                Ending::Never => Next::Go(state),
            }
        }
    }

    #[tokio::test]
    async fn a_state_is_dropped_when_the_machine_is_done() {
        let dropped = Arc::new(Mutex::new(false));
        Machine(Ends {
            ending: Ending::Done,
            dropped: Arc::clone(&dropped),
        })
        .run()
        .await
        .unwrap();
        assert!(*dropped.lock().unwrap(), "the state was let go of");
    }

    #[tokio::test]
    async fn a_state_is_dropped_when_the_machine_fails() {
        let dropped = Arc::new(Mutex::new(false));
        Machine(Ends {
            ending: Ending::Fail,
            dropped: Arc::clone(&dropped),
        })
        .run()
        .await
        .unwrap_err();
        assert!(*dropped.lock().unwrap(), "the state was let go of");
    }

    #[tokio::test]
    async fn a_state_is_dropped_when_the_machines_future_is_dropped() {
        let dropped = Arc::new(Mutex::new(false));
        let machine = Machine(Ends {
            ending: Ending::Never,
            dropped: Arc::clone(&dropped),
        });
        // The machine runs until it is acting, and is then dropped
        // where it stands. The future is boxed so that dropping it
        // drops the state the machine holds.
        let mut running: Pin<Box<dyn Future<Output = Result<()>>>> = Box::pin(machine.run());
        timeout(PATIENCE, async {
            tokio::select! {
                _ = &mut running => panic!("the machine ended on its own"),
                () = sleep(Duration::from_millis(20)) => {}
            }
        })
        .await
        .unwrap();
        assert!(!*dropped.lock().unwrap(), "it still carries the state");

        drop(running);
        assert!(*dropped.lock().unwrap(), "the state went with the future");
    }
}
