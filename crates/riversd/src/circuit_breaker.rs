use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BreakerState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Serialize)]
pub struct BreakerEntry {
    #[serde(rename = "breakerId")]
    pub breaker_id: String,
    pub state: BreakerState,
    pub dataviews: Vec<String>,
}

pub struct BreakerRegistry {
    breakers: RwLock<HashMap<String, BreakerEntry>>,
}

impl BreakerRegistry {
    pub fn new() -> Self {
        Self {
            breakers: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, breaker_id: String, dataview_name: String) {
        let mut map = self.breakers.write().await;
        let entry = map.entry(breaker_id.clone()).or_insert_with(|| BreakerEntry {
            breaker_id,
            state: BreakerState::Closed,
            dataviews: Vec::new(),
        });
        if !entry.dataviews.contains(&dataview_name) {
            entry.dataviews.push(dataview_name);
        }
    }

    pub async fn is_open(&self, breaker_id: &str) -> bool {
        let map = self.breakers.read().await;
        map.get(breaker_id)
            .map(|e| e.state == BreakerState::Open)
            .unwrap_or(false)
    }

    pub async fn trip(&self, breaker_id: &str) -> Option<BreakerEntry> {
        let mut map = self.breakers.write().await;
        if let Some(entry) = map.get_mut(breaker_id) {
            entry.state = BreakerState::Open;
            Some(entry.clone())
        } else {
            None
        }
    }

    pub async fn reset(&self, breaker_id: &str) -> Option<BreakerEntry> {
        let mut map = self.breakers.write().await;
        if let Some(entry) = map.get_mut(breaker_id) {
            entry.state = BreakerState::Closed;
            Some(entry.clone())
        } else {
            None
        }
    }

    pub async fn get(&self, breaker_id: &str) -> Option<BreakerEntry> {
        let map = self.breakers.read().await;
        map.get(breaker_id).cloned()
    }

    pub async fn list(&self) -> Vec<BreakerEntry> {
        let map = self.breakers.read().await;
        let mut entries: Vec<BreakerEntry> = map.values().cloned().collect();
        entries.sort_by(|a, b| a.breaker_id.cmp(&b.breaker_id));
        entries
    }

    pub async fn set_state(&self, breaker_id: &str, state: BreakerState) {
        let mut map = self.breakers.write().await;
        if let Some(entry) = map.get_mut(breaker_id) {
            entry.state = state;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_creates_closed_breaker() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        let entry = reg.get("WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Closed);
        assert_eq!(entry.dataviews, vec!["search_orders"]);
    }

    #[tokio::test]
    async fn register_adds_dataview_to_existing_breaker() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        reg.register("WH_TX".into(), "update_orders".into()).await;
        let entry = reg.get("WH_TX").await.unwrap();
        assert_eq!(entry.dataviews, vec!["search_orders", "update_orders"]);
    }

    #[tokio::test]
    async fn register_deduplicates_dataviews() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        reg.register("WH_TX".into(), "search_orders".into()).await;
        let entry = reg.get("WH_TX").await.unwrap();
        assert_eq!(entry.dataviews.len(), 1);
    }

    #[tokio::test]
    async fn trip_sets_state_to_open() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        let entry = reg.trip("WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Open);
        assert!(reg.is_open("WH_TX").await);
    }

    #[tokio::test]
    async fn reset_sets_state_to_closed() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        reg.trip("WH_TX").await;
        let entry = reg.reset("WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Closed);
        assert!(!reg.is_open("WH_TX").await);
    }

    #[tokio::test]
    async fn trip_idempotent() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        reg.trip("WH_TX").await;
        let entry = reg.trip("WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Open);
    }

    #[tokio::test]
    async fn reset_idempotent() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        let entry = reg.reset("WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Closed);
    }

    #[tokio::test]
    async fn trip_unknown_returns_none() {
        let reg = BreakerRegistry::new();
        assert!(reg.trip("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn is_open_unknown_returns_false() {
        let reg = BreakerRegistry::new();
        assert!(!reg.is_open("nonexistent").await);
    }

    #[tokio::test]
    async fn list_returns_sorted_entries() {
        let reg = BreakerRegistry::new();
        reg.register("Zebra".into(), "dv1".into()).await;
        reg.register("Alpha".into(), "dv2".into()).await;
        let entries = reg.list().await;
        assert_eq!(entries[0].breaker_id, "Alpha");
        assert_eq!(entries[1].breaker_id, "Zebra");
    }
}
