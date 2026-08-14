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
//! The commands are wiring and nothing more. Everything they do is a
//! keystore call, so everything they do is testable against
//! [`InMemory`](epik::keystore::InMemory) without a keychain in sight.

use epik::keystore::{KeyStore, OsKeyring, Resolved, Secret};

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
    use anyhow::anyhow;
    use epik::keystore::InMemory;

    /// The backend's own broken store: `Unplugged` is private to the
    /// library's tests, and this crate's coping deserves its own proof.
    struct Broken;

    impl KeyStore for Broken {
        fn get(&self, _: &str) -> anyhow::Result<Option<Secret>> {
            Err(anyhow!("the keychain is locked"))
        }

        fn set(&mut self, _: &str, _: Secret) -> anyhow::Result<()> {
            Err(anyhow!("the keychain is locked"))
        }
    }

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
