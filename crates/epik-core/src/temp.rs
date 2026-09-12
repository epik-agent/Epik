//! Fresh paths under the temp dir: one that no other call answers
//! with, and for tests, a directory there that cleans up after itself.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A path under the temp dir that no other call answers with, in this
/// process or another: `stem`, the pid, the clock, and a count. The
/// clock alone collides when two callers ask in the same tick — a
/// process-wide count settles it.
pub fn unique(stem: &str) -> PathBuf {
    static NTH: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "{stem}-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default(),
        NTH.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A scratch directory that cleans up after itself: a [`unique`] path,
/// made, and removed on drop. For tests — this crate's, and under the
/// `testing` feature its dependents'.
#[cfg(any(test, feature = "testing"))]
pub struct Scratch(pub PathBuf);

#[cfg(any(test, feature = "testing"))]
impl Scratch {
    pub fn new(name: &str) -> Self {
        let path = unique(&format!("epik-test-{name}"));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }

    pub fn join(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }

    pub fn write(&self, name: &str, content: &str) -> &Self {
        std::fs::write(self.0.join(name), content).unwrap();
        self
    }
}

#[cfg(any(test, feature = "testing"))]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
