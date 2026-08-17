//! Daily activity digest — high-frequency telemetry summarized locally,
//! shipped as one `digest` event per product per day.
//!
//! Counters accumulate in `<data_dir>/telemetry/<product>-digest.json` keyed
//! by UTC day. Nothing leaves the machine during the day; the first activity
//! after a day completes ships that day's counts as a single row (backlog
//! capped at the newest 14 days — a machine parked longer drops the tail).
//! Keys are dotted enum identifiers only (`chat.turn_ok`,
//! `error.model.auth_required`); the receiver rejects anything else, so a
//! content-shaped string cannot reach the database even by accident.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Completed days older than this many entries are dropped, newest kept.
const MAX_BACKLOG_DAYS: usize = 14;

type DayCounts = BTreeMap<String, u64>;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct DigestFile {
    #[serde(default)]
    days: BTreeMap<String, DayCounts>,
}

pub(crate) struct Digest {
    path: PathBuf,
    days: Mutex<BTreeMap<String, DayCounts>>,
}

impl Digest {
    pub fn load(data_dir: &Path, product: &str) -> Self {
        let path = data_dir.join("telemetry").join(format!("{product}-digest.json"));
        let days = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DigestFile>(&bytes).ok())
            .map(|f| f.days)
            .unwrap_or_default();
        Self { path, days: Mutex::new(days) }
    }

    /// Increment `key` for today and persist. Cheap enough for per-call use;
    /// the file is a few hundred bytes.
    pub fn bump(&self, key: &str) {
        let mut days = match self.days.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        *days.entry(today_utc_date()).or_default().entry(key.to_string()).or_insert(0) += 1;
        save(&self.path, &days);
    }

    /// Remove and return every completed (strictly before today) day, newest
    /// `MAX_BACKLOG_DAYS` only. Removal before the POST is the double-send
    /// guard: a day in flight is no longer in the file.
    pub fn take_completed(&self) -> Vec<(String, DayCounts)> {
        let today = today_utc_date();
        let mut days = match self.days.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let completed: Vec<String> = days.keys().filter(|d| **d < today).cloned().collect();
        if completed.is_empty() {
            return Vec::new();
        }
        let keep_from = completed.len().saturating_sub(MAX_BACKLOG_DAYS);
        let mut taken = Vec::new();
        for (i, day) in completed.iter().enumerate() {
            let counts = days.remove(day).unwrap_or_default();
            if i >= keep_from && !counts.is_empty() {
                taken.push((day.clone(), counts));
            }
        }
        save(&self.path, &days);
        taken
    }

    /// Merge a failed shipment back so the next trigger retries it. Addition
    /// is safe against bumps that landed meanwhile — counts are additive.
    pub fn restore(&self, day: &str, counts: DayCounts) {
        let mut days = match self.days.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let slot = days.entry(day.to_string()).or_default();
        for (key, n) in counts {
            *slot.entry(key).or_insert(0) += n;
        }
        save(&self.path, &days);
    }
}

fn save(path: &Path, days: &BTreeMap<String, DayCounts>) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let file = DigestFile { days: days.clone() };
    if let Ok(bytes) = serde_json::to_vec(&file) {
        let _ = std::fs::write(path, bytes);
    }
}

/// Today as `YYYY-MM-DD` UTC without pulling in chrono: days-since-epoch to
/// civil date (Howard Hinnant's `civil_from_days`).
fn today_utc_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_is_sane() {
        // 2026-08-17 is day 20 682 since epoch.
        let s = today_utc_date();
        assert_eq!(s.len(), 10);
        assert!(s.starts_with("20"));
    }

    #[test]
    fn bump_take_restore_roundtrip() {
        let dir = std::env::temp_dir().join(format!("digest-test-{}", std::process::id()));
        let digest = Digest::load(&dir, "testprod");
        digest.bump("chat.turn_ok");
        digest.bump("chat.turn_ok");
        digest.bump("error.model.network");
        // Today is not completed, so nothing ships yet.
        assert!(digest.take_completed().is_empty());
        // Simulate a past day.
        digest.restore("2020-01-01", BTreeMap::from([("chat.turn_ok".to_string(), 5)]));
        let taken = digest.take_completed();
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].0, "2020-01-01");
        assert_eq!(taken[0].1.get("chat.turn_ok"), Some(&5));
        // Taken means gone until restored.
        assert!(digest.take_completed().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
