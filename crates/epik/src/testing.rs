//! Fixtures shared by this crate's unit tests — the scratch directory
//! every git-adjacent test module used to carry a copy of.

use std::path::PathBuf;

/// A scratch directory that cleans up after itself.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new(name: &str) -> Self {
        // The clock alone can collide when two tests in this binary ask
        // for the same name in the same tick — a process-wide count
        // settles it.
        static NTH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "epik-test-{name}-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NTH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(crate) fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }

    pub(crate) fn join(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }

    pub(crate) fn write(&self, name: &str, content: &str) -> &Self {
        std::fs::write(self.0.join(name), content).unwrap();
        self
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
