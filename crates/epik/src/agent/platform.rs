//! Everything that depends on the operating system.
//!
//! [`Agent`](super::Agent) needs two things the standard library does not
//! provide portably: starting a process so that its descendants can be found
//! again later, and killing all of them at once. This module supplies each as
//! one function. The Unix implementation uses process groups. The fallback
//! for other platforms handles the process alone and leaves descendants that
//! outlive it running.
//!
//! Porting to another platform means adding a module here with the same two
//! functions. On Windows, for example, a job object with the kill-on-close
//! limit takes the place of the process group.

use std::process::{Child, Command};

#[cfg(not(unix))]
pub use fallback::*;
#[cfg(unix)]
pub use unix::*;

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::process::CommandExt;

    /// Arrange for the process to start as the leader of a new process
    /// group, so that it and every descendant can be signalled together.
    pub fn prepare(command: &mut Command) {
        // A group id of 0 means a new group whose id is the child's own pid.
        command.process_group(0);
    }

    /// Kill `process` and every process in its group.
    ///
    /// The kernel keeps a pid from being reused while it still names a live
    /// process group, so this is safe to call after the process itself has
    /// been reaped, when it reaches any descendants that are still running.
    pub fn kill_tree(process: &mut Child) {
        let group = -(process.id() as libc::pid_t);
        // SAFETY: `kill` takes two integers and touches no memory. A
        // negative pid addresses the process group with that id.
        unsafe {
            libc::kill(group, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
mod fallback {
    use super::*;

    /// Nothing to arrange: there is no way to group descendants here.
    pub fn prepare(_command: &mut Command) {}

    /// Kill `process` alone. Descendants that outlive it are left running.
    pub fn kill_tree(process: &mut Child) {
        let _ = process.kill();
    }
}
