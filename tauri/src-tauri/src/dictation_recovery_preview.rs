//! Bounded, ephemeral recovery previews. Staging never writes history.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(super) struct PreviewStore<T> {
    entries: Mutex<HashMap<String, (Instant, T)>>,
    next: AtomicU64,
}
impl<T: Clone> PreviewStore<T> {
    pub(super) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }
    pub(super) fn stage(&self, value: T) -> Result<String, String> {
        self.stage_at(value, Instant::now())
    }
    fn stage_at(&self, value: T, now: Instant) -> Result<String, String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "Recovery previews unavailable.")?;
        entries.retain(|_, (at, _)| now.saturating_duration_since(*at) < Duration::from_secs(600));
        if entries.len() >= 4 {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest);
            }
        }
        let id = format!(
            "recovery-preview-{}",
            self.next.fetch_add(1, Ordering::Relaxed)
        );
        entries.insert(id.clone(), (now, value));
        Ok(id)
    }
    pub(super) fn get(&self, id: &str) -> Result<T, String> {
        self.get_at(id, Instant::now())
    }
    fn get_at(&self, id: &str, now: Instant) -> Result<T, String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "Recovery previews unavailable.")?;
        entries.retain(|_, (at, _)| now.saturating_duration_since(*at) < Duration::from_secs(600));
        entries
            .get(id)
            .map(|(_, item)| item.clone())
            .ok_or_else(|| "This preview expired. Retranscribe the saved audio again.".into())
    }
    pub(super) fn remove(&self, id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(id);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn staging_is_readable_without_consuming_the_preview() {
        let store = PreviewStore::new();
        let id = store.stage("old and new").unwrap();
        assert_eq!(store.get(&id).unwrap(), "old and new");
        assert_eq!(store.get(&id).unwrap(), "old and new");
    }
    #[test]
    fn accepted_preview_is_consumed() {
        let store = PreviewStore::new();
        let id = store.stage(42).unwrap();
        store.remove(&id);
        assert!(store.get(&id).is_err());
    }
    #[test]
    fn expired_preview_cannot_be_accepted() {
        let store = PreviewStore::new();
        let now = Instant::now();
        let id = store.stage_at(42, now).unwrap();
        assert!(store.get_at(&id, now + Duration::from_secs(600)).is_err());
    }
    #[test]
    fn bounded_store_retires_oldest_preview_only() {
        let store = PreviewStore::new();
        let now = Instant::now();
        let first = store.stage_at(0, now).unwrap();
        let mut last = String::new();
        for n in 1..5 {
            last = store.stage_at(n, now + Duration::from_secs(n)).unwrap();
        }
        assert!(store.get_at(&first, now + Duration::from_secs(5)).is_err());
        assert_eq!(
            store.get_at(&last, now + Duration::from_secs(5)).unwrap(),
            4
        );
    }
}
