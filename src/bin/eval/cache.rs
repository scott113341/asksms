//! A file cache of answers and judgments, keyed by a hash of everything that
//! produced them. Each entry is `<dir>/<kind>/<sha256>.json` and stores its key
//! next to its value, so it is easy to see why an entry exists.

use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub struct Cache {
    dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Entry<T> {
    key: Value,
    value: T,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The cached value for `key`, or `None` if there isn't one or it no longer parses.
    pub fn get<T: DeserializeOwned>(&self, kind: &str, key: &Value) -> Option<T> {
        let text = fs::read_to_string(self.path(kind, key)).ok()?;
        let entry: Entry<T> = serde_json::from_str(&text).ok()?;
        Some(entry.value)
    }

    pub fn put<T: Serialize>(&self, kind: &str, key: &Value, value: &T) -> anyhow::Result<()> {
        let path = self.path(kind, key);
        let dir = path.parent().expect("cache paths have a parent");
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

        // Write then rename, so an interrupted run never leaves a half-written entry.
        let tmp = path.with_extension("json.tmp");
        let entry = Entry {
            key: key.clone(),
            value,
        };
        fs::write(&tmp, serde_json::to_vec_pretty(&entry)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    fn path(&self, kind: &str, key: &Value) -> PathBuf {
        self.dir.join(kind).join(format!("{}.json", hash(key)))
    }
}

/// The SHA-256 of `key`'s JSON, in hex.
pub fn hash(key: &Value) -> String {
    let json = serde_json::to_vec(key).expect("a JSON value always serializes");
    Sha256::digest(&json)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn round_trips_a_value() {
        let cache = Cache::new(crate::test_dir("cache"));
        let key = json!({ "question": "hi", "sample": 1 });

        assert_eq!(cache.get::<String>("answers", &key), None);
        cache.put("answers", &key, &"hello".to_owned()).unwrap();
        assert_eq!(
            cache.get::<String>("answers", &key).as_deref(),
            Some("hello")
        );
        assert_eq!(cache.get::<String>("judgments", &key), None);
    }

    #[test]
    fn hashes_differ_by_content() {
        assert_eq!(hash(&json!({ "a": 1 })), hash(&json!({ "a": 1 })));
        assert_ne!(hash(&json!({ "a": 1 })), hash(&json!({ "a": 2 })));
        assert_eq!(hash(&json!({})).len(), 64);
    }
}
