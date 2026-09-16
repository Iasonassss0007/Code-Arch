//! Incremental cache.
//!
//! Job    Remember stage results across runs so a re-run after a small
//!        change skips the slow stages instead of redoing them.
//! In     `.codearch/cache/store.json` from the previous run (if any)
//! Out    Updated store, written atomically (tmp + rename)
//! Fails  Missing, corrupt, or version-mismatched store -> empty cache.
//!        The run continues uncached; a cache can only save time, never
//!        change output, because hits reproduce stored values bit-for-bit.
//!
//! What is keyed on what, and why it is sound:
//!
//! ```text
//! files   (mtime_ns, size) match -> reuse stored hash without reading;
//!         read on miss, then (hash) hit -> reuse stored parse without
//!         parsing. mtime+size trust is the standard make-class tradeoff,
//!         documented in the M5 spec.
//! git     HEAD sha match -> reuse stored pairs/churn mapped through the
//!         current inventory (missing paths dropped). History is a pure
//!         function of HEAD, so same HEAD means same signal.
//! labels  LLM path only, keyed by evidence hash. Derived labels recompute
//!         (milliseconds post-split) so there is exactly one labeling path.
//! ```

use crate::parse::FileParse;
use crate::types::{FileClass, Language};
use std::collections::HashMap;
use std::path::Path;

/// Bump when the stored shapes change. A mismatch ignores the store
/// wholesale rather than migrating it: caches are expendable, maps are not.
/// v2: the M6-lite summary guard — labels stored under the name-only guard
/// must regenerate, never serve.
/// v3: `FileParse` gains `urls` — parses stored without contract evidence
/// must re-parse, never serve stale.
/// v4: `FileParse` gains route evidence (`routes`, `url_segments`, `http`,
/// `extends`) for `contract::route_join`.
/// v5: `FileParse` gains `url_segment_seqs` (per-literal segment sequences
/// for most-specific-wins in `route_join`).
pub const FORMAT: u32 = 5;

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredFile {
    pub mtime_ns: u64,
    pub size: u64,
    pub hash: String,
    pub loc: usize,
    pub class: FileClass,
    pub language: Language,
    pub parse: FileParse,
    /// Content hash `parse` was built from. A crash between inventory (which
    /// refreshes `hash`) and parse must not validate a stale parse: only a
    /// matching pair is a hit.
    pub parse_hash: String,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredGit {
    pub head: String,
    pub pairs: Vec<(String, String, f64)>,
    pub churn: Vec<(String, usize)>,
    pub commits_read: usize,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredLabel {
    pub name: String,
    pub summary: String,
}

/// Memoized split verdict: same fingerprint, same decision, without
/// rebuilding the flat map to measure it again. The fingerprint covers
/// everything the verdict can depend on — file set and contents, git HEAD,
/// budget, domain cap, seed, labeler — so reuse is exact, not heuristic.
/// Any change re-probes; determinism makes the re-probe agree with history.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct SplitDecision {
    pub fingerprint: String,
    pub split: bool,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Store {
    pub version: u32,
    pub files: HashMap<String, StoredFile>,
    pub git: Option<StoredGit>,
    pub labels_llm: HashMap<String, StoredLabel>,
    pub split_decision: Option<SplitDecision>,
}

impl Store {
    pub fn load(dir: &Path) -> Store {
        let path = dir.join("cache").join("store.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Store::empty();
        };
        let Ok(store) = serde_json::from_str::<Store>(&text) else {
            return Store::empty();
        };
        if store.version != FORMAT {
            return Store::empty();
        }
        store
    }

    /// An empty store that still saves as the current format. `Default`
    /// would write version 0, which the next load would reject — the cache
    /// would never engage across runs. Separate constructor, one invariant.
    pub fn empty() -> Store {
        Store {
            version: FORMAT,
            ..Default::default()
        }
    }

    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        let cache_dir = dir.join("cache");
        std::fs::create_dir_all(&cache_dir)?;
        let text = serde_json::to_string(self)?;
        let tmp = cache_dir.join("store.json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, cache_dir.join("store.json"))?;
        // The cache must never be committed: it is machine-local derived
        // data. Created if absent, never clobbered if the user owns it.
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            let _ = std::fs::write(&ignore, "cache/\n");
        }
        Ok(())
    }
}

/// Hex SHA-256 of bytes. One hash function for file content and evidence
/// strings alike; hex because JSON carries it opaquely either way.
pub fn sha_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("codearch-cache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_preserves_everything() {
        let dir = tmp("round-trip");
        let mut store = Store {
            version: FORMAT,
            ..Default::default()
        };
        store.files.insert(
            "a.ts".to_string(),
            StoredFile {
                mtime_ns: 7,
                size: 3,
                hash: "abc".to_string(),
                loc: 1,
                class: FileClass::Source,
                language: Language::Ts,
                parse: FileParse::default(),
                parse_hash: String::new(),
            },
        );
        store.git = Some(StoredGit {
            head: "deadbeef".to_string(),
            pairs: vec![("a.ts".to_string(), "b.ts".to_string(), 0.5)],
            churn: vec![("a.ts".to_string(), 2)],
            commits_read: 9,
        });
        store.save(&dir).unwrap();

        let back = Store::load(&dir);
        assert_eq!(back.version, FORMAT);
        assert_eq!(back.files["a.ts"].hash, "abc");
        assert_eq!(back.git.as_ref().unwrap().commits_read, 9);
        // The gitignore is created, not assumed.
        assert!(dir.join(".gitignore").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_corrupt_and_stale_stores_all_read_empty() {
        let dir = tmp("bad-stores");
        assert!(Store::load(&dir).files.is_empty());

        std::fs::create_dir_all(dir.join("cache")).unwrap();
        std::fs::write(dir.join("cache").join("store.json"), "{nope").unwrap();
        assert!(Store::load(&dir).files.is_empty());

        let stale = Store {
            version: FORMAT + 1,
            ..Default::default()
        };
        let text = serde_json::to_string(&stale).unwrap();
        std::fs::write(dir.join("cache").join("store.json"), text).unwrap();
        assert!(Store::load(&dir).files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_gitignore_is_never_clobbered() {
        let dir = tmp("gitignore");
        std::fs::write(dir.join(".gitignore"), "dist/\n").unwrap();
        Store::default().save(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "dist/\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha_is_stable_and_sized() {
        assert_eq!(sha_hex(b"a"), sha_hex(b"a"));
        assert_ne!(sha_hex(b"a"), sha_hex(b"b"));
        assert_eq!(sha_hex(b"a").len(), 64);
    }
}
