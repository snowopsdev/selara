//! API keys in the OS credential store (macOS Keychain, Windows Credential
//! Manager, Linux keyutils) via the `keyring` crate. One entry per provider
//! kind under a fixed service name, so the Settings app, `serve`, and the CLI
//! all read the same secret without it living in `config.toml`.

use crate::error::CoreError;
use crate::providers::ProviderKind;

/// Keychain service name shared by every Selara binary.
pub const KEYCHAIN_SERVICE: &str = "dev.snowops.selara";

/// Account name for a provider kind (the snake_case serde spelling).
pub fn keychain_account(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAiCompatible => "open_ai_compatible",
        ProviderKind::OpenRouter => "open_router",
        ProviderKind::Anthropic => "anthropic",
    }
}

fn entry(kind: ProviderKind) -> Result<keyring::Entry, CoreError> {
    keyring::Entry::new(KEYCHAIN_SERVICE, keychain_account(kind))
        .map_err(|e| CoreError::Config(format!("keychain: {e}")))
}

/// The stored key for `kind`, or `None` when there is no entry.
pub fn keychain_get(kind: ProviderKind) -> Result<Option<String>, CoreError> {
    match entry(kind)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(CoreError::Config(format!("keychain read: {e}"))),
    }
}

/// Store (or replace) the key for `kind`.
pub fn keychain_set(kind: ProviderKind, api_key: &str) -> Result<(), CoreError> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(CoreError::Config(
            "refusing to store an empty API key".into(),
        ));
    }
    entry(kind)?
        .set_password(key)
        .map_err(|e| CoreError::Config(format!("keychain write: {e}")))
}

/// Remove the key for `kind`; a missing entry is not an error.
pub fn keychain_delete(kind: ProviderKind) -> Result<(), CoreError> {
    match entry(kind)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(CoreError::Config(format!("keychain delete: {e}"))),
    }
}

/// Route every keychain call in this process to a shared in-memory store. For
/// tests only: it keeps `cargo test` from touching the developer's real
/// Keychain. (`keyring::mock` gives each `Entry` private storage, which does
/// not model a real store where one entry is visible to every lookup.)
pub fn use_mock_store() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        keyring::set_default_credential_builder(Box::new(memory::MemoryStore::default()));
    });
}

mod memory {
    use std::any::Any;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi};

    type Entries = Arc<Mutex<HashMap<String, Vec<u8>>>>;

    #[derive(Default)]
    pub struct MemoryStore {
        entries: Entries,
    }

    impl CredentialBuilderApi for MemoryStore {
        fn build(
            &self,
            target: Option<&str>,
            service: &str,
            user: &str,
        ) -> keyring::Result<Box<Credential>> {
            Ok(Box::new(MemoryCredential {
                key: format!("{}|{service}|{user}", target.unwrap_or("")),
                entries: self.entries.clone(),
            }))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct MemoryCredential {
        key: String,
        entries: Entries,
    }

    impl CredentialApi for MemoryCredential {
        fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
            self.entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(self.key.clone(), secret.to_vec());
            Ok(())
        }

        fn get_secret(&self) -> keyring::Result<Vec<u8>> {
            self.entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&self.key)
                .cloned()
                .ok_or(keyring::Error::NoEntry)
        }

        fn delete_credential(&self) -> keyring::Result<()> {
            self.entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.key)
                .map(|_| ())
                .ok_or(keyring::Error::NoEntry)
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_delete_round_trip() {
        use_mock_store();
        let kind = ProviderKind::OpenRouter;
        keychain_delete(kind).unwrap();
        assert_eq!(keychain_get(kind).unwrap(), None);
        keychain_set(kind, "  sk-or-test  ").unwrap();
        assert_eq!(keychain_get(kind).unwrap().as_deref(), Some("sk-or-test"));
        keychain_set(kind, "sk-or-second").unwrap();
        assert_eq!(keychain_get(kind).unwrap().as_deref(), Some("sk-or-second"));
        keychain_delete(kind).unwrap();
        assert_eq!(keychain_get(kind).unwrap(), None);
        // Deleting twice is fine.
        keychain_delete(kind).unwrap();
    }

    #[test]
    fn empty_key_is_rejected() {
        use_mock_store();
        let err = keychain_set(ProviderKind::Anthropic, "   ").unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn accounts_are_distinct_per_kind() {
        use_mock_store();
        keychain_set(ProviderKind::Anthropic, "a").unwrap();
        keychain_set(ProviderKind::OpenAiCompatible, "b").unwrap();
        assert_eq!(
            keychain_get(ProviderKind::Anthropic).unwrap().as_deref(),
            Some("a")
        );
        assert_eq!(
            keychain_get(ProviderKind::OpenAiCompatible)
                .unwrap()
                .as_deref(),
            Some("b")
        );
        keychain_delete(ProviderKind::Anthropic).unwrap();
        keychain_delete(ProviderKind::OpenAiCompatible).unwrap();
    }
}
