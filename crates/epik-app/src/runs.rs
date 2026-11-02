//! The runs of the monitored repositories, this computer's and each remote
//! host's, as the window and the tray see them.
//!
//! The sidebar, the page beside it and the tray icon each ask for the runs
//! every second or two, and reading them means reading every run's record.
//! So the runs read are kept for a second, and everything that asks in
//! that second shares them. Whatever changes the runs forgets them, so the
//! next to ask sees the change.

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use epik_core::hosted::{self, Shown};

/// How long the runs read are kept.
const FRESH: Duration = Duration::from_secs(1);

/// The runs of the repositories monitored under `root`, on every host,
/// read at most once a second.
pub struct Runs {
    root: PathBuf,
    kept: Mutex<Option<(Instant, Arc<Shown>)>>,
}

impl Runs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            kept: Mutex::default(),
        }
    }

    /// The runs, as read in the last second, or read now.
    pub fn monitored(&self) -> io::Result<Arc<Shown>> {
        self.monitored_at(Instant::now())
    }

    fn monitored_at(&self, now: Instant) -> io::Result<Arc<Shown>> {
        let mut kept = self
            .kept
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((read_at, runs)) = kept.as_ref()
            && now.duration_since(*read_at) < FRESH
        {
            return Ok(runs.clone());
        }
        let runs = Arc::new(hosted::monitored(&self.root)?);
        *kept = Some((now, runs.clone()));
        Ok(runs)
    }

    /// Forgets the runs read, after something changed them.
    pub fn forget(&self) {
        *self
            .kept
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runs_read_are_shared_for_a_second() {
        let root = tempfile::tempdir().unwrap();
        let runs = Runs::new(root.path().to_owned());
        let now = Instant::now();
        let first = runs.monitored_at(now).unwrap();
        let soon = runs.monitored_at(now + Duration::from_millis(900)).unwrap();
        assert!(Arc::ptr_eq(&first, &soon));

        let later = runs.monitored_at(now + FRESH).unwrap();
        assert!(!Arc::ptr_eq(&first, &later));
    }

    #[test]
    fn forgetting_the_runs_reads_them_again() {
        let root = tempfile::tempdir().unwrap();
        let runs = Runs::new(root.path().to_owned());
        let now = Instant::now();
        let first = runs.monitored_at(now).unwrap();
        runs.forget();
        let again = runs.monitored_at(now).unwrap();
        assert!(!Arc::ptr_eq(&first, &again));
    }
}
