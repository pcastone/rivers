use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// State of a circuit breaker (Open or Closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BreakerState {
    /// Circuit is open, blocking traffic.
    Open,
    /// Circuit is closed, allowing traffic.
    Closed,
}

/// Entry describing a circuit breaker and its associated DataViews.
#[derive(Debug, Clone, Serialize)]
pub struct BreakerEntry {
    /// Unique identifier for the circuit breaker (user-facing, without app_id prefix).
    #[serde(rename = "breakerId")]
    pub breaker_id: String,
    /// Current state of the circuit breaker.
    pub state: BreakerState,
    /// List of DataView names controlled by this breaker.
    pub dataviews: Vec<String>,
}

/// App-scoped circuit breaker registry.
///
/// Internal keys are `"{app_id}:{breaker_id}"` to prevent collisions
/// between apps that use the same breaker name.
pub struct BreakerRegistry {
    breakers: RwLock<HashMap<String, BreakerEntry>>,
}

impl BreakerRegistry {
    /// Create a new circuit breaker registry.
    pub fn new() -> Self {
        Self {
            breakers: RwLock::new(HashMap::new()),
        }
    }

    /// Register a DataView with a circuit breaker scoped to `app_id`.
    pub async fn register(&self, app_id: &str, breaker_id: String, dataview_name: String) {
        let key = format!("{}:{}", app_id, breaker_id);
        let mut map = self.breakers.write().await;
        let entry = map.entry(key).or_insert_with(|| BreakerEntry {
            breaker_id,
            state: BreakerState::Closed,
            dataviews: Vec::new(),
        });
        if !entry.dataviews.contains(&dataview_name) {
            entry.dataviews.push(dataview_name);
        }
    }

    /// Check if a circuit breaker is open.
    pub async fn is_open(&self, app_id: &str, breaker_id: &str) -> bool {
        let key = format!("{}:{}", app_id, breaker_id);
        let map = self.breakers.read().await;
        map.get(&key)
            .map(|e| e.state == BreakerState::Open)
            .unwrap_or(false)
    }

    /// Open a circuit breaker, blocking traffic.
    pub async fn trip(&self, app_id: &str, breaker_id: &str) -> Option<BreakerEntry> {
        let key = format!("{}:{}", app_id, breaker_id);
        let mut map = self.breakers.write().await;
        if let Some(entry) = map.get_mut(&key) {
            entry.state = BreakerState::Open;
            Some(entry.clone())
        } else {
            None
        }
    }

    /// Close a circuit breaker, allowing traffic.
    pub async fn reset(&self, app_id: &str, breaker_id: &str) -> Option<BreakerEntry> {
        let key = format!("{}:{}", app_id, breaker_id);
        let mut map = self.breakers.write().await;
        if let Some(entry) = map.get_mut(&key) {
            entry.state = BreakerState::Closed;
            Some(entry.clone())
        } else {
            None
        }
    }

    /// Get a circuit breaker by app and breaker ID.
    pub async fn get(&self, app_id: &str, breaker_id: &str) -> Option<BreakerEntry> {
        let key = format!("{}:{}", app_id, breaker_id);
        let map = self.breakers.read().await;
        map.get(&key).cloned()
    }

    /// List all breakers for a specific app, sorted by breaker ID.
    pub async fn list_for_app(&self, app_id: &str) -> Vec<BreakerEntry> {
        let prefix = format!("{}:", app_id);
        let map = self.breakers.read().await;
        let mut entries: Vec<BreakerEntry> = map.iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| v.clone())
            .collect();
        entries.sort_by(|a, b| a.breaker_id.cmp(&b.breaker_id));
        entries
    }

    /// List ALL breakers across all apps, sorted by breaker ID.
    pub async fn list(&self) -> Vec<BreakerEntry> {
        let map = self.breakers.read().await;
        let mut entries: Vec<BreakerEntry> = map.values().cloned().collect();
        entries.sort_by(|a, b| a.breaker_id.cmp(&b.breaker_id));
        entries
    }

    /// Set the state of a circuit breaker.
    pub async fn set_state(&self, app_id: &str, breaker_id: &str, state: BreakerState) {
        let key = format!("{}:{}", app_id, breaker_id);
        let mut map = self.breakers.write().await;
        if let Some(entry) = map.get_mut(&key) {
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
        reg.register("test-app", "WH_TX".into(), "search_orders".into()).await;
        let entry = reg.get("test-app", "WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Closed);
        assert_eq!(entry.dataviews, vec!["search_orders"]);
    }

    #[tokio::test]
    async fn register_adds_dataview_to_existing_breaker() {
        let reg = BreakerRegistry::new();
        reg.register("test-app", "WH_TX".into(), "search_orders".into()).await;
        reg.register("test-app", "WH_TX".into(), "update_orders".into()).await;
        let entry = reg.get("test-app", "WH_TX").await.unwrap();
        assert_eq!(entry.dataviews, vec!["search_orders", "update_orders"]);
    }

    #[tokio::test]
    async fn register_deduplicates_dataviews() {
        let reg = BreakerRegistry::new();
        reg.register("test-app", "WH_TX".into(), "search_orders".into()).await;
        reg.register("test-app", "WH_TX".into(), "search_orders".into()).await;
        let entry = reg.get("test-app", "WH_TX").await.unwrap();
        assert_eq!(entry.dataviews.len(), 1);
    }

    #[tokio::test]
    async fn trip_sets_state_to_open() {
        let reg = BreakerRegistry::new();
        reg.register("test-app", "WH_TX".into(), "search_orders".into()).await;
        let entry = reg.trip("test-app", "WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Open);
        assert!(reg.is_open("test-app", "WH_TX").await);
    }

    #[tokio::test]
    async fn reset_sets_state_to_closed() {
        let reg = BreakerRegistry::new();
        reg.register("test-app", "WH_TX".into(), "search_orders".into()).await;
        reg.trip("test-app", "WH_TX").await;
        let entry = reg.reset("test-app", "WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Closed);
        assert!(!reg.is_open("test-app", "WH_TX").await);
    }

    #[tokio::test]
    async fn trip_idempotent() {
        let reg = BreakerRegistry::new();
        reg.register("test-app", "WH_TX".into(), "search_orders".into()).await;
        reg.trip("test-app", "WH_TX").await;
        let entry = reg.trip("test-app", "WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Open);
    }

    #[tokio::test]
    async fn reset_idempotent() {
        let reg = BreakerRegistry::new();
        reg.register("test-app", "WH_TX".into(), "search_orders".into()).await;
        let entry = reg.reset("test-app", "WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Closed);
    }

    #[tokio::test]
    async fn trip_unknown_returns_none() {
        let reg = BreakerRegistry::new();
        assert!(reg.trip("test-app", "nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn is_open_unknown_returns_false() {
        let reg = BreakerRegistry::new();
        assert!(!reg.is_open("test-app", "nonexistent").await);
    }

    #[tokio::test]
    async fn list_returns_sorted_entries() {
        let reg = BreakerRegistry::new();
        reg.register("test-app", "Zebra".into(), "dv1".into()).await;
        reg.register("test-app", "Alpha".into(), "dv2".into()).await;
        let entries = reg.list().await;
        assert_eq!(entries[0].breaker_id, "Alpha");
        assert_eq!(entries[1].breaker_id, "Zebra");
    }

    #[tokio::test]
    async fn separate_apps_dont_collide() {
        let reg = BreakerRegistry::new();
        reg.register("app1", "cache".into(), "dv1".into()).await;
        reg.register("app2", "cache".into(), "dv2".into()).await;

        // Trip app1's cache breaker
        reg.trip("app1", "cache").await;

        // app1 is open, app2 is still closed
        assert!(reg.is_open("app1", "cache").await);
        assert!(!reg.is_open("app2", "cache").await);

        // list_for_app returns only that app's breakers
        assert_eq!(reg.list_for_app("app1").await.len(), 1);
        assert_eq!(reg.list_for_app("app2").await.len(), 1);
    }
}
