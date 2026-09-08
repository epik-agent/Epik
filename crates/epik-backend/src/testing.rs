//! Fixtures shared by this crate's unit tests.

use anyhow::anyhow;
use epik::keystore::{KeyStore, Secret};

/// The backend's own broken store: the library's `Unplugged` is private
/// to its tests, and this crate's coping deserves its own proof.
pub(crate) struct Broken;

impl KeyStore for Broken {
    fn get(&self, _: &str) -> anyhow::Result<Option<Secret>> {
        Err(anyhow!("the keychain is locked"))
    }

    fn set(&mut self, _: &str, _: Secret) -> anyhow::Result<()> {
        Err(anyhow!("the keychain is locked"))
    }
}
