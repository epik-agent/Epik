//! The narrow door between the keystore and the window.
//!
//! Secret bytes cross IPC in exactly two places — [`secret_reveal`]'s answer
//! and [`secret_save`]'s argument. That exposure is accepted, known debt to
//! be closed; keeping the hole this narrow is the module's whole discipline.
//! Nothing else may carry a secret: no events, no logs, no further types.
//!
//! The commands are wiring and nothing more. Everything they do is a
//! keystore call, so everything they do is testable against [`InMemory`]
//! without a keychain in sight — which is what the functions the commands
//! wrap are for.
//!
//! [`InMemory`]: epik::keystore::InMemory

use epik::keystore::{KeyStore, OsKeyring, Resolved};
use serde::Serialize;

/// What resolving a secret came to, in a shape that can cross IPC.
///
/// [`Resolved`]'s three states, with `Found` carrying the bytes as a plain
/// string — the accepted hole. Deliberately not `Debug`: nothing that can
/// hold a secret gets a printable form.
#[derive(Clone, Serialize)]
pub enum RevealOutcome {
    /// The secret, on its way to prefill an input.
    Found(String),
    /// No secret, and nothing wrong.
    Absent,
    /// The store would not answer, and this is why.
    Unreachable(String),
}

/// [`secret_reveal`] against any store, which is what makes it testable.
fn reveal(store: &impl KeyStore, name: &str) -> RevealOutcome {
    match store.resolve(name) {
        Resolved::Found(secret) => RevealOutcome::Found(secret.reveal().to_owned()),
        Resolved::Absent => RevealOutcome::Absent,
        Resolved::Unreachable(reason) => RevealOutcome::Unreachable(reason),
    }
}

/// [`secret_save`] against any store, which is what makes it testable.
fn save(store: &mut impl KeyStore, name: &str, value: &str) -> Result<(), String> {
    store
        .set(name, value.into())
        .map_err(|error| format!("{error:#}"))
}

/// The secret filed under `name`, for the settings page to prefill.
///
/// Async so the keyring's answer — which on macOS can involve a permission
/// dialog — never stalls the window's event loop.
#[tauri::command]
pub async fn secret_reveal(name: String) -> RevealOutcome {
    reveal(&OsKeyring, &name)
}

/// Files `value` under `name` in the OS keyring. The error, when there is
/// one, names the store and the entry — never the value.
#[tauri::command]
pub async fn secret_save(name: String, value: String) -> Result<(), String> {
    save(&mut OsKeyring, &name, &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use epik::keystore::{InMemory, Secret};

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
    fn reveal_hands_over_a_stored_secret() {
        let mut store = InMemory::default();
        store.set("anthropic", "sk-stored".into()).unwrap();
        assert!(matches!(
            reveal(&store, "anthropic"),
            RevealOutcome::Found(value) if value == "sk-stored"
        ));
    }

    #[test]
    fn reveal_reports_a_missing_secret_as_absent() {
        assert!(matches!(
            reveal(&InMemory::default(), "anthropic"),
            RevealOutcome::Absent
        ));
    }

    #[test]
    fn reveal_reports_an_unreachable_store_with_its_reason() {
        assert!(matches!(
            reveal(&Broken, "anthropic"),
            RevealOutcome::Unreachable(reason) if reason.contains("locked")
        ));
    }

    #[test]
    fn save_writes_through_to_the_store() {
        let mut store = InMemory::default();
        save(&mut store, "anthropic", "sk-pasted").unwrap();
        assert_eq!(store.get("anthropic").unwrap(), Some("sk-pasted".into()));
    }

    #[test]
    fn a_failed_save_surfaces_its_reason() {
        let reason = save(&mut Broken, "anthropic", "sk-pasted").unwrap_err();
        assert!(reason.contains("locked"), "{reason}");
    }
}
