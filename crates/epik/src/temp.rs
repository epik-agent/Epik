//! A fresh path under the temp dir, for whatever must not collide.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A path under the temp dir that no other call answers with, in this
/// process or another: `stem`, the pid, the clock, and a count. The
/// clock alone collides when two callers ask in the same tick — a
/// process-wide count settles it.
pub(crate) fn unique(stem: &str) -> PathBuf {
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
