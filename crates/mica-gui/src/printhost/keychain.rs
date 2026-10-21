// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Where printer secrets live. `printers.toml` only ever holds a *reference*; the API key or
//! password itself goes to the macOS Keychain (a generic password item of the service
//! "Mica Slicer", the reference as its account). Other platforms get a clear "not supported
//! yet" answer, tests and headless screenshots an in-memory store that never touches disk.

use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// Keychain service name of every Mica item.
#[cfg(target_os = "macos")]
pub const SERVICE: &str = "Mica Slicer";

pub trait SecretStore: Send + Sync {
    /// Where secrets go, for the UI ("macOS Keychain").
    fn label(&self) -> &'static str;
    /// False when secrets cannot be kept on this platform; the UI then says so instead of
    /// pretending to save.
    fn available(&self) -> bool {
        true
    }
    /// `Ok(None)` when nothing is stored under `reference`.
    fn get(&self, reference: &str) -> Result<Option<String>, String>;
    fn set(&self, reference: &str, secret: &str) -> Result<(), String>;
    /// Removing a missing item is not an error.
    fn delete(&self, reference: &str) -> Result<(), String>;
}

/// A fresh, unique reference for one printer's secret (`mica-printer-<n>-<time>`); the
/// reference is what `printers.toml` stores.
pub fn new_reference() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!(
        "mica-printer-{stamp:x}-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Secrets held in memory only: tests, headless screenshots.
#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<String, String>>);
impl SecretStore for MemoryStore {
    fn label(&self) -> &'static str {
        "memory (not saved)"
    }
    fn get(&self, reference: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(reference).cloned())
    }
    fn set(&self, reference: &str, secret: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert(reference.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, reference: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(reference);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub struct Keychain;
#[cfg(target_os = "macos")]
impl SecretStore for Keychain {
    fn label(&self) -> &'static str {
        "macOS Keychain"
    }
    fn get(&self, reference: &str) -> Result<Option<String>, String> {
        use security_framework::passwords::get_generic_password;
        match get_generic_password(SERVICE, reference) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| "the Keychain item is not text".to_string()),
            // errSecItemNotFound
            Err(e) if e.code() == -25300 => Ok(None),
            Err(e) => Err(format!("Keychain: {e}")),
        }
    }
    fn set(&self, reference: &str, secret: &str) -> Result<(), String> {
        security_framework::passwords::set_generic_password(SERVICE, reference, secret.as_bytes())
            .map_err(|e| format!("Keychain: {e}"))
    }
    fn delete(&self, reference: &str) -> Result<(), String> {
        match security_framework::passwords::delete_generic_password(SERVICE, reference) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == -25300 => Ok(()),
            Err(e) => Err(format!("Keychain: {e}")),
        }
    }
}

/// Platforms without a secret store yet: nothing is saved, and the UI says so.
#[cfg(not(target_os = "macos"))]
pub struct Unsupported;
#[cfg(not(target_os = "macos"))]
impl SecretStore for Unsupported {
    fn label(&self) -> &'static str {
        "not supported yet on this platform"
    }
    fn available(&self) -> bool {
        false
    }
    fn get(&self, _: &str) -> Result<Option<String>, String> {
        Err(UNSUPPORTED.into())
    }
    fn set(&self, _: &str, _: &str) -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }
    fn delete(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}
#[cfg(not(target_os = "macos"))]
const UNSUPPORTED: &str = "Saving printer secrets is not supported yet on this platform; use an api_key_env entry in printers.toml";

/// The platform's store: the Keychain on macOS, the unsupported stub elsewhere.
pub fn system() -> Box<dyn SecretStore> {
    assert!(
        !mica_core::user_data::isolated(),
        "a test (or test tool) opened the macOS Keychain: use MemoryStore"
    );
    #[cfg(target_os = "macos")]
    {
        Box::new(Keychain)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests use `MemoryStore`: opening the real Keychain panics.
    #[test]
    fn tests_cannot_open_the_real_keychain() {
        assert!(std::panic::catch_unwind(system).is_err());
    }

    #[test]
    fn memory_store_round_trips_and_forgets() {
        let store = MemoryStore::default();
        assert_eq!(store.get("a").unwrap(), None);
        store.set("a", "s3cret").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("s3cret"));
        store.set("a", "changed").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("changed"));
        store.delete("a").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.get("a").unwrap(), None);
    }

    #[test]
    fn references_are_unique_and_opaque() {
        let a = new_reference();
        let b = new_reference();
        assert_ne!(a, b);
        assert!(a.starts_with("mica-printer-"));
        assert!(!a.contains(' '));
    }
}
