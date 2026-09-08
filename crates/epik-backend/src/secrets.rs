//! The narrow door between the keystore and the window.
//!
//! Secret bytes cross IPC in exactly two places — [`secret_reveal`]'s answer
//! and [`secret_save`]'s argument. That exposure is accepted, known debt to
//! be closed; keeping the hole this narrow is the module's whole discipline.
//! Nothing else may carry a secret: no events, no logs, no further types.
//! Both crossings speak the library's own types — [`Resolved`] out,
//! [`Secret`] in — so the wire shape is defined once, in `epik`, for both
//! sides of the barrier.
//!
//! The store itself is [`OsKeyring`]: the operating system's own secret
//! holder — Keychain, Credential Manager, secret service — with every
//! Epik secret filed under service [`SERVICE`]. The commands are wiring
//! and nothing more. Everything they do is a keystore call, so
//! everything they do is testable against
//! [`InMemory`](epik::keystore::InMemory) without a keychain in sight.

use epik::keystore::{KeyStore, Resolved, Secret};

/// The keyring service every Epik secret is filed under.
const SERVICE: &str = "Epik";

/// The operating system's own secret holder.
#[derive(Debug, Default)]
pub(crate) struct OsKeyring;

impl OsKeyring {
    fn entry(name: &str) -> anyhow::Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, name)
            .map_err(|error| anyhow::anyhow!("opening the {SERVICE}/{name} keyring entry: {error}"))
    }
}

impl KeyStore for OsKeyring {
    fn get(&self, name: &str) -> anyhow::Result<Option<Secret>> {
        match Self::entry(name)?.get_password() {
            Ok(secret) => Ok(Some(secret.into())),
            // No entry is the ordinary state of a fresh install, not a fault.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(anyhow::anyhow!(
                "reading the {SERVICE}/{name} secret from the keyring: {error}"
            )),
        }
    }

    fn set(&mut self, name: &str, secret: Secret) -> anyhow::Result<()> {
        Self::entry(name)?
            .set_password(secret.reveal())
            .map_err(|error| {
                anyhow::anyhow!("storing the {SERVICE}/{name} secret in the keyring: {error}")
            })
    }
}

/// [`secret_save`] against any store, which is what makes it testable.
fn save(store: &mut impl KeyStore, name: &str, secret: Secret) -> Result<(), String> {
    store
        .set(name, secret)
        .map_err(|error| format!("{error:#}"))
}

/// The secret filed under `name`, for the settings page to prefill.
///
/// Async so the keyring's answer — which on macOS can involve a permission
/// dialog — never stalls the window's event loop.
#[tauri::command]
pub async fn secret_reveal(name: String) -> Resolved {
    OsKeyring.resolve(&name)
}

/// Files `value` under `name` in the OS keyring. The error, when there is
/// one, names the store and the entry — never the value.
#[tauri::command]
pub async fn secret_save(name: String, value: Secret) -> Result<(), String> {
    save(&mut OsKeyring, &name, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use epik::keystore::InMemory;

    use crate::testing::Broken;

    #[test]
    fn save_writes_through_to_the_store() {
        let mut store = InMemory::default();
        save(&mut store, "anthropic", "sk-pasted".into()).unwrap();
        assert_eq!(store.get("anthropic").unwrap(), Some("sk-pasted".into()));
    }

    #[test]
    fn a_failed_save_surfaces_its_reason() {
        let reason = save(&mut Broken, "anthropic", "sk-pasted".into()).unwrap_err();
        assert!(reason.contains("locked"), "{reason}");
    }
}
