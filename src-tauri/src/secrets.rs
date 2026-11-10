//! Passwords and API keys live in the macOS Keychain, never on disk.
//!
//! They're kept together in a single Keychain item (service `identity::keychain_service()`, which
//! is "com.nemecj.sparkbox" unless a dev build has its own identity; account "vault", a JSON map),
//! so macOS asks for access at most once per app signature, and they're read once per session.
//! Earlier SparkBox versions used one item per secret; those move into the vault the first time
//! they're read.
//!
//! Secret names: "ssh" is the shared node password (also used for sudo);
//! "routeros:<switch id>" a MikroTik REST password; "vpn:private-key" and
//! "vpn:preshared-key" come from the imported VPN profile; "llm:<provider id>"
//! are API keys; "server:clients" holds the local server's app keys; "firewalla:msp" is the
//! token for Firewalla's cloud.
//!
//! The copy in memory is dropped after 10 minutes without use (`forget_idle`, from the admin
//! ticker) and wiped as it goes (zeroize); the next use reads the Keychain again.
//!
//! A Keychain call can block for as long as a macOS access prompt stays open. On one of tokio's
//! worker threads it runs inside `block_in_place`, so the worker's other tasks move to another
//! thread meanwhile and no caller has to change.
//!
//! The release app and a debug build share the vault while they share an identifier. So `set` and
//! `clear` read the item afresh, change only their own entry and write it back: a secret the other
//! instance saved since this one loaded its copy is kept.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use zeroize::Zeroize;

/// How long the vault stays in memory after its last use.
pub const IDLE: Duration = Duration::from_secs(10 * 60);

/// The Keychain account holding the vault.
const VAULT: &str = "vault";

/// The secrets read from the vault, wiped from memory when dropped.
#[derive(Default)]
pub struct Cache {
    pub values: HashMap<String, String>,
    /// Names already looked up as separate (pre-vault) items.
    pub migrated: std::collections::HashSet<String>,
    used: Option<Instant>,
}

impl Cache {
    pub fn new(values: HashMap<String, String>) -> Cache {
        Cache { values, migrated: Default::default(), used: Some(Instant::now()) }
    }

    pub fn touch(&mut self) {
        self.used = Some(Instant::now());
    }

    /// Unused for `idle` (as of `now`).
    pub fn idle(&self, now: Instant, idle: Duration) -> bool {
        self.used.is_none_or(|u| now.saturating_duration_since(u) >= idle)
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        for v in self.values.values_mut() {
            v.zeroize();
        }
    }
}

/// Runs blocking Keychain work. On a multi-thread runtime's worker it goes through
/// `block_in_place`; anywhere else (the main thread, a blocking thread, a current-thread runtime,
/// where `block_in_place` would panic) it runs directly.
fn off_worker<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(f),
        _ => f(),
    }
}

/// Where the vault (and any pre-vault item) is kept: the Keychain, or memory in tests.
trait Backend {
    /// The item for `account`, or None when there isn't one.
    fn read(&self, account: &str) -> Result<Option<String>, String>;
    fn write(&self, account: &str, value: &str) -> Result<(), String>;
    /// Removes the item; one that isn't there is fine.
    fn delete(&self, account: &str) -> Result<(), String>;
}

#[cfg(not(test))]
struct Keychain;

#[cfg(not(test))]
impl Keychain {
    fn entry(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(crate::identity::keychain_service(), account).map_err(|e| e.to_string())
    }
}

#[cfg(not(test))]
impl Backend for Keychain {
    fn read(&self, account: &str) -> Result<Option<String>, String> {
        match Self::entry(account)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write(&self, account: &str, value: &str) -> Result<(), String> {
        Self::entry(account)?.set_password(value).map_err(|e| e.to_string())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

// Tests keep secrets in memory so they never read or write the real Keychain.
#[cfg(test)]
#[derive(Default)]
struct Memory {
    items: Mutex<std::collections::BTreeMap<String, String>>,
}

#[cfg(test)]
impl Memory {
    const fn new() -> Memory {
        Memory { items: Mutex::new(std::collections::BTreeMap::new()) }
    }
}

#[cfg(test)]
impl Backend for Memory {
    fn read(&self, account: &str) -> Result<Option<String>, String> {
        Ok(self.items.lock().unwrap().get(account).cloned())
    }

    fn write(&self, account: &str, value: &str) -> Result<(), String> {
        self.items.lock().unwrap().insert(account.into(), value.into());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.items.lock().unwrap().remove(account);
        Ok(())
    }
}

/// The vault item and the copy of it in memory.
struct Vault<B> {
    backend: B,
    store: Mutex<Option<Cache>>,
}

impl<B: Backend> Vault<B> {
    const fn new(backend: B) -> Vault<B> {
        Vault { backend, store: Mutex::new(None) }
    }

    fn read_vault(&self) -> Result<HashMap<String, String>, String> {
        match self.backend.read(VAULT)? {
            Some(mut json) => {
                let parsed = serde_json::from_str(&json).map_err(|e| format!("SparkBox's Keychain item is unreadable: {e}"));
                json.zeroize();
                parsed
            }
            None => Ok(HashMap::new()),
        }
    }

    fn write_vault(&self, values: &HashMap<String, String>) -> Result<(), String> {
        let mut json = serde_json::to_string(values).map_err(|e| e.to_string())?;
        let result = self.backend.write(VAULT, &json);
        json.zeroize();
        result
    }

    /// Runs `f` on the copy in memory (None until loaded), holding the lock, off the async workers.
    fn locked<T>(&self, f: impl FnOnce(&mut Option<Cache>) -> Result<T, String>) -> Result<T, String> {
        off_worker(|| f(&mut self.store.lock().unwrap()))
    }

    /// The copy in memory, read from the Keychain on first use.
    fn loaded<'a>(&self, slot: &'a mut Option<Cache>) -> Result<&'a mut Cache, String> {
        if slot.is_none() {
            *slot = Some(Cache::new(self.read_vault()?));
        }
        let cache = slot.as_mut().unwrap();
        cache.touch();
        Ok(cache)
    }

    /// Reads the vault afresh, applies `change` to it alone, writes it back, and keeps the result
    /// as the copy in memory. Whatever the other instance saved meanwhile survives.
    fn save(&self, slot: &mut Option<Cache>, change: impl FnOnce(&mut HashMap<String, String>)) -> Result<(), String> {
        let mut fresh = Cache::new(self.read_vault()?);
        change(&mut fresh.values);
        self.write_vault(&fresh.values)?;
        if let Some(old) = slot.as_mut() {
            fresh.migrated = std::mem::take(&mut old.migrated);
        }
        // The old copy is wiped as it drops.
        *slot = Some(fresh);
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
        self.locked(|slot| {
            let cache = self.loaded(slot)?;
            if let Some(v) = cache.values.get(account) {
                return Ok(Some(v.clone()));
            }
            if !cache.migrated.insert(account.to_string()) {
                return Ok(None);
            }
            // A secret saved by an earlier version as its own item: move it into the vault.
            let Some(v) = self.backend.read(account)? else { return Ok(None) };
            self.save(slot, |m| {
                m.insert(account.to_string(), v.clone());
            })?;
            let _ = self.backend.delete(account);
            Ok(Some(v))
        })
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        self.locked(|slot| {
            self.save(slot, |m| {
                if let Some(mut old) = m.insert(account.to_string(), secret.to_string()) {
                    old.zeroize();
                }
            })
        })
    }

    fn clear(&self, account: &str) -> Result<(), String> {
        self.locked(|slot| {
            self.save(slot, |m| {
                if let Some(mut old) = m.remove(account) {
                    old.zeroize();
                }
            })?;
            if let Some(cache) = slot.as_mut() {
                cache.migrated.insert(account.to_string());
            }
            self.backend.delete(account)
        })
    }

    /// Drops (and wipes) the copy in memory once it's been unused for `IDLE`. Busy means in use.
    fn forget_idle(&self) {
        let Ok(mut guard) = self.store.try_lock() else { return };
        if guard.as_ref().is_some_and(|s| s.idle(Instant::now(), IDLE)) {
            *guard = None;
        }
    }
}

#[cfg(not(test))]
static VAULT_ITEM: Vault<Keychain> = Vault::new(Keychain);
#[cfg(test)]
static VAULT_ITEM: Vault<Memory> = Vault::new(Memory::new());

pub fn get(account: &str) -> Result<Option<String>, String> {
    VAULT_ITEM.get(account)
}

pub fn set(account: &str, secret: &str) -> Result<(), String> {
    VAULT_ITEM.set(account, secret)
}

pub fn clear(account: &str) -> Result<(), String> {
    VAULT_ITEM.clear(account)
}

pub fn forget_idle() {
    VAULT_ITEM.forget_idle()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_goes_idle_and_wipes_what_it_held() {
        let mut c = Cache::new([("ssh".to_string(), "hunter2".to_string())].into());
        let t0 = Instant::now();
        assert!(!c.idle(t0, IDLE));
        assert!(c.idle(t0 + IDLE, IDLE), "unused for 10 minutes");
        c.touch();
        assert!(!c.idle(Instant::now(), IDLE), "used again");
        // Dropping wipes the strings in place (zeroize), then frees them.
        let v = c.values.get_mut("ssh").unwrap();
        v.zeroize();
        assert!(v.is_empty());
    }

    fn stored(v: &Vault<Memory>) -> HashMap<String, String> {
        serde_json::from_str(&v.backend.read(VAULT).unwrap().unwrap()).unwrap()
    }

    /// The other SparkBox (release or dev) saves `account` into the shared item.
    fn written_elsewhere(v: &Vault<Memory>, account: &str, secret: &str) {
        let mut outside = stored(v);
        outside.insert(account.into(), secret.into());
        v.backend.write(VAULT, &serde_json::to_string(&outside).unwrap()).unwrap();
    }

    #[test]
    fn set_keeps_what_the_other_instance_saved_meanwhile() {
        let v = Vault::new(Memory::new());
        v.set("ssh", "cluster-pw").unwrap();
        // This instance's copy is loaded.
        assert_eq!(v.get("ssh").unwrap().as_deref(), Some("cluster-pw"));
        written_elsewhere(&v, "llm:openai", "sk-from-the-other-instance");
        v.set("routeros:crs804", "switch-pw").unwrap();
        let s = stored(&v);
        assert_eq!(s.get("ssh").map(String::as_str), Some("cluster-pw"));
        assert_eq!(s.get("llm:openai").map(String::as_str), Some("sk-from-the-other-instance"), "the other instance's secret survives");
        assert_eq!(s.get("routeros:crs804").map(String::as_str), Some("switch-pw"));
        // The copy in memory is the merged vault.
        assert_eq!(v.get("llm:openai").unwrap().as_deref(), Some("sk-from-the-other-instance"));
    }

    #[test]
    fn clear_removes_only_its_own_entry() {
        let v = Vault::new(Memory::new());
        v.set("ssh", "cluster-pw").unwrap();
        v.set("llm:anthropic", "sk-ant-1").unwrap();
        written_elsewhere(&v, "hf", "hf_other_instance_token");
        v.clear("llm:anthropic").unwrap();
        let s = stored(&v);
        assert!(!s.contains_key("llm:anthropic"));
        assert_eq!(s.get("hf").map(String::as_str), Some("hf_other_instance_token"));
        assert_eq!(s.get("ssh").map(String::as_str), Some("cluster-pw"));
        assert_eq!(v.get("llm:anthropic").unwrap(), None);
    }

    #[test]
    fn a_separate_item_from_an_earlier_version_moves_into_the_vault() {
        let v = Vault::new(Memory::new());
        v.set("ssh", "cluster-pw").unwrap();
        v.backend.write("routeros:crs804", "old-switch-pw").unwrap();
        written_elsewhere(&v, "llm:openai", "sk-2");
        assert_eq!(v.get("routeros:crs804").unwrap().as_deref(), Some("old-switch-pw"));
        assert_eq!(v.backend.read("routeros:crs804").unwrap(), None, "the separate item is gone");
        let s = stored(&v);
        assert_eq!(s.get("routeros:crs804").map(String::as_str), Some("old-switch-pw"));
        assert_eq!(s.get("llm:openai").map(String::as_str), Some("sk-2"), "the move merges too");
        // Looked up once: a later miss doesn't read the Keychain again.
        v.backend.write("routeros:crs804", "planted").unwrap();
        v.clear("routeros:crs804").unwrap();
        assert_eq!(v.get("routeros:crs804").unwrap(), None);
    }

    #[test]
    fn forgetting_wipes_only_an_idle_copy() {
        let v = Vault::new(Memory::new());
        v.set("ssh", "cluster-pw").unwrap();
        v.forget_idle();
        assert!(v.store.lock().unwrap().is_some(), "just used");
        v.store.lock().unwrap().as_mut().unwrap().used = Some(Instant::now() - IDLE);
        v.forget_idle();
        assert!(v.store.lock().unwrap().is_none());
        assert_eq!(v.get("ssh").unwrap().as_deref(), Some("cluster-pw"), "read again from the item");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_current_thread_runtime_calls_directly() {
        // block_in_place would panic here.
        set("test:current-thread", "x").unwrap();
        assert_eq!(get("test:current-thread").unwrap().as_deref(), Some("x"));
        clear("test:current-thread").unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_worker_thread_blocks_in_place() {
        set("test:multi-thread", "y").unwrap();
        assert_eq!(get("test:multi-thread").unwrap().as_deref(), Some("y"));
        // From a blocking thread too.
        assert_eq!(tokio::task::spawn_blocking(|| get("test:multi-thread")).await.unwrap().unwrap().as_deref(), Some("y"));
        clear("test:multi-thread").unwrap();
    }
}
