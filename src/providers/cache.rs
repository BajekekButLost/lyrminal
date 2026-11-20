use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::Candidate;
use crate::lyrics::Lyrics;

const NEGATIVE_TTL_SECS: u64 = 60 * 60 * 12;
pub const MAX_TTL_DAYS: u32 = 30;

#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
    ttl_secs: u64,
    enabled: bool,
}

#[derive(Serialize, Deserialize)]
struct Entry<T> {
    saved_at: u64,
    value: T,
}

pub type Lookup = Option<(Candidate, Lyrics)>;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

impl Cache {
    pub fn new(dir: PathBuf, ttl_days: u32, enabled: bool) -> Self {
        Self {
            dir,
            ttl_secs: ttl_days.clamp(1, MAX_TTL_DAYS) as u64 * 86_400,
            enabled,
        }
    }

    fn path(&self, kind: &str, provider: &str, key: &str) -> PathBuf {
        self.dir.join(kind).join(provider).join(format!("{}.json", fnv(key)))
    }

    fn read<T: DeserializeOwned>(&self, path: &PathBuf, ttl: u64) -> Option<T> {
        if !self.enabled {
            return None;
        }
        let text = fs::read_to_string(path).ok()?;
        let entry: Entry<T> = serde_json::from_str(&text).ok()?;
        (now().saturating_sub(entry.saved_at) <= ttl).then_some(entry.value)
    }

    fn write<T: Serialize>(&self, path: &PathBuf, value: &T) {
        if !self.enabled {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let entry = Entry {
            saved_at: now(),
            value,
        };
        if let Ok(text) = serde_json::to_string(&entry) {
            let _ = fs::write(path, text);
        }
    }

    pub fn get_lyrics(&self, provider: &str, id: &str) -> Option<Lyrics> {
        self.read(&self.path("lyrics", provider, id), self.ttl_secs)
    }

    pub fn put_lyrics(&self, provider: &str, id: &str, lyrics: &Lyrics) {
        self.write(&self.path("lyrics", provider, id), lyrics);
    }

    pub fn get_lookup(&self, provider: &str, track_key: &str) -> Option<Lookup> {
        let path = self.path("lookup", provider, track_key);
        let found: Lookup = self.read(&path, self.ttl_secs)?;
        match found {
            Some(hit) => Some(Some(hit)),
            None => self.read::<Lookup>(&path, NEGATIVE_TTL_SECS).map(|_| None),
        }
    }

    pub fn put_lookup(&self, provider: &str, track_key: &str, value: &Lookup) {
        self.write(&self.path("lookup", provider, track_key), value);
        if let Some((c, l)) = value {
            self.put_lyrics(provider, &c.id, l);
        }
    }

    pub fn forget_track(&self, providers: &[&str], track_key: &str) {
        for p in providers {
            let _ = fs::remove_file(self.path("lookup", p, track_key));
        }
    }

    pub fn clear(&self) -> std::io::Result<()> {
        match fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::{Credits, SyncKind};

    #[test]
    fn roundtrips_and_negative_caches() {
        let dir = std::env::temp_dir().join(format!("lyrminal-test-{}", fnv(&format!("{:?}", SystemTime::now()))));
        let cache = Cache::new(dir.clone(), 1, true);
        assert!(cache.get_lookup("p", "k").is_none());
        cache.put_lookup("p", "k", &None);
        assert!(matches!(cache.get_lookup("p", "k"), Some(None)));
        let l = Lyrics {
            kind: SyncKind::Static,
            lines: vec![],
            credits: Credits::default(),
            language: None,
            instrumental: true,
        };
        cache.put_lyrics("p", "id", &l);
        assert!(cache.get_lyrics("p", "id").is_some());
        let _ = fs::remove_dir_all(dir);
    }
}
