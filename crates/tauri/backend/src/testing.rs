//! Fixtures shared by this crate's unit tests.

use anyhow::{Result, anyhow};
use epik_core::keystore::Secret;

use crate::secrets::KeyStore;

/// A store that has broken down rather than one that is merely empty: the
/// machine with no secret service — a container, a headless Linux box, a
/// CI runner — or a keychain that stays locked.
pub(crate) struct Broken;

impl KeyStore for Broken {
    fn get(&self, _: &str) -> Result<Option<Secret>> {
        Err(anyhow!("the keychain is locked"))
    }

    fn set(&mut self, _: &str, _: Secret) -> Result<()> {
        Err(anyhow!("the keychain is locked"))
    }
}
