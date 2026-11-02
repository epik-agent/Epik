//! Keeping the machine from idle sleep while builds run.
//!
//! [`KeepAwake`] follows the number of active builds: it holds an
//! idle-sleep assertion while there is at least one, and releases it when
//! there are none. The assertion itself comes from a [`Waker`], so tests
//! can follow the hold without holding a real one. The operating system's,
//! [`System`], comes with the feature `awake`.

use std::sync::{Mutex, PoisonError};

/// An idle-sleep assertion, released when dropped.
pub type Assertion = Box<dyn Send>;

/// Takes idle-sleep assertions.
pub trait Waker: Send + Sync {
    /// Keeps the machine from idle sleep until the returned assertion is
    /// dropped.
    fn hold(&self) -> Result<Assertion, String>;
}

/// The operating system's idle-sleep assertion, through the `keepawake`
/// crate. The display may still sleep.
#[cfg(feature = "awake")]
pub struct System;

#[cfg(feature = "awake")]
impl Waker for System {
    fn hold(&self) -> Result<Assertion, String> {
        keepawake::Builder::default()
            .idle(true)
            .reason("Epik is running a build")
            .app_name("Epik")
            .app_reverse_domain("io.github.epik-agent.epik")
            .create()
            .map(|assertion| Box::new(assertion) as Assertion)
            .map_err(|error| error.to_string())
    }
}

/// What following the active builds changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The assertion is now held.
    Held,
    /// The assertion has been released.
    Released,
}

/// Holds an idle-sleep assertion while any build is active.
pub struct KeepAwake {
    waker: Box<dyn Waker>,
    held: Mutex<Option<Assertion>>,
}

impl KeepAwake {
    pub fn new(waker: impl Waker + 'static) -> Self {
        Self {
            waker: Box::new(waker),
            held: Mutex::default(),
        }
    }

    /// Holds the assertion if `active` builds are more than none, and
    /// releases it if they are none. Returns what changed, if anything, or
    /// why the assertion couldn't be held; the next call tries again.
    pub fn follow(&self, active: usize) -> Result<Option<Change>, String> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        match (active > 0, held.is_some()) {
            (true, false) => {
                *held = Some(self.waker.hold()?);
                Ok(Some(Change::Held))
            }
            (false, true) => {
                *held = None;
                Ok(Some(Change::Released))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A [`Waker`] that counts the assertions it has handed out and how
    /// many are still held, and fails while `failing` is set.
    #[derive(Clone, Default)]
    struct Counting {
        taken: Arc<AtomicUsize>,
        held: Arc<AtomicUsize>,
        failing: Arc<AtomicBool>,
    }

    struct Held(Arc<AtomicUsize>);

    impl Drop for Held {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl Waker for Counting {
        fn hold(&self) -> Result<Assertion, String> {
            if self.failing.load(Ordering::SeqCst) {
                return Err("no power management here".to_owned());
            }
            self.taken.fetch_add(1, Ordering::SeqCst);
            self.held.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Held(self.held.clone())))
        }
    }

    #[test]
    fn the_assertion_is_held_while_any_build_is_active() {
        let waker = Counting::default();
        let awake = KeepAwake::new(waker.clone());
        assert_eq!(waker.held.load(Ordering::SeqCst), 0);

        assert_eq!(awake.follow(1), Ok(Some(Change::Held)));
        assert_eq!(awake.follow(2), Ok(None));
        assert_eq!(awake.follow(1), Ok(None));
        assert_eq!(waker.held.load(Ordering::SeqCst), 1);

        assert_eq!(awake.follow(0), Ok(Some(Change::Released)));
        assert_eq!(awake.follow(0), Ok(None));
        assert_eq!(waker.held.load(Ordering::SeqCst), 0);

        assert_eq!(awake.follow(1), Ok(Some(Change::Held)));
        assert_eq!(waker.taken.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn an_assertion_that_cannot_be_held_is_tried_again() {
        let waker = Counting::default();
        waker.failing.store(true, Ordering::SeqCst);
        let awake = KeepAwake::new(waker.clone());

        assert_eq!(awake.follow(1), Err("no power management here".to_owned()));
        assert_eq!(waker.held.load(Ordering::SeqCst), 0);

        waker.failing.store(false, Ordering::SeqCst);
        assert_eq!(awake.follow(2), Ok(Some(Change::Held)));
        assert_eq!(waker.held.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_it_releases_the_assertion() {
        let waker = Counting::default();
        let awake = KeepAwake::new(waker.clone());
        awake.follow(1).unwrap();

        drop(awake);

        assert_eq!(waker.held.load(Ordering::SeqCst), 0);
    }
}
