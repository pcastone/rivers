//! Service discovery — resolves service names to running endpoints.
//!
//! Per spec §13.5: automatic resolution by appId, health check gating.

use std::collections::HashMap;
use std::sync::RwLock;

/// Registry of running services and their endpoints.
pub struct ServiceRegistry {
    /// Map of service name to endpoint.
    services: RwLock<HashMap<String, ServiceEndpoint>>,
}

/// A discovered service endpoint.
#[derive(Debug, Clone)]
pub struct ServiceEndpoint {
    pub name: String,
    pub url: String,
    pub healthy: bool,
    pub app_id: String,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self {
            services: RwLock::new(HashMap::new()),
        }
    }

    /// Register a service endpoint.
    pub fn register(&self, name: &str, url: &str, app_id: &str) {
        let mut services = self.services.write().unwrap();
        services.insert(
            name.to_string(),
            ServiceEndpoint {
                name: name.to_string(),
                url: url.to_string(),
                healthy: true,
                app_id: app_id.to_string(),
            },
        );
    }

    /// Resolve a service name to its endpoint URL.
    ///
    /// Returns None if the service is not registered or not healthy.
    pub fn resolve(&self, name: &str) -> Option<String> {
        let services = self.services.read().unwrap();
        services
            .get(name)
            .filter(|ep| ep.healthy)
            .map(|ep| ep.url.clone())
    }

    /// Mark a service as unhealthy.
    pub fn mark_unhealthy(&self, name: &str) {
        let mut services = self.services.write().unwrap();
        if let Some(ep) = services.get_mut(name) {
            ep.healthy = false;
        }
    }

    /// Mark a service as healthy.
    pub fn mark_healthy(&self, name: &str) {
        let mut services = self.services.write().unwrap();
        if let Some(ep) = services.get_mut(name) {
            ep.healthy = true;
        }
    }

    /// Unregister a service.
    pub fn unregister(&self, name: &str) {
        let mut services = self.services.write().unwrap();
        services.remove(name);
    }

    /// List all registered services.
    pub fn list(&self) -> Vec<ServiceEndpoint> {
        let services = self.services.read().unwrap();
        services.values().cloned().collect()
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_resolve() {
        let registry = ServiceRegistry::new();
        registry.register("orders", "http://localhost:9100", "app-1");
        assert_eq!(
            registry.resolve("orders"),
            Some("http://localhost:9100".into())
        );
    }

    #[test]
    fn resolve_missing_returns_none() {
        let registry = ServiceRegistry::new();
        assert!(registry.resolve("nonexistent").is_none());
    }

    #[test]
    fn unhealthy_service_not_resolved() {
        let registry = ServiceRegistry::new();
        registry.register("orders", "http://localhost:9100", "app-1");
        registry.mark_unhealthy("orders");
        assert!(registry.resolve("orders").is_none());
    }

    #[test]
    fn re_mark_healthy() {
        let registry = ServiceRegistry::new();
        registry.register("orders", "http://localhost:9100", "app-1");
        registry.mark_unhealthy("orders");
        registry.mark_healthy("orders");
        assert!(registry.resolve("orders").is_some());
    }

    #[test]
    fn unregister_service() {
        let registry = ServiceRegistry::new();
        registry.register("orders", "http://localhost:9100", "app-1");
        registry.unregister("orders");
        assert!(registry.resolve("orders").is_none());
    }

    #[test]
    fn list_services() {
        let registry = ServiceRegistry::new();
        registry.register("orders", "http://localhost:9100", "app-1");
        registry.register("inventory", "http://localhost:9200", "app-2");
        assert_eq!(registry.list().len(), 2);
    }
}
