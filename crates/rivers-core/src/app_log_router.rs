//! Per-application log file router.
//!
//! Manages a registry of app-name → BufWriter<File> mappings.
//! When an app is loaded, `register()` opens (or creates) `<base_dir>/<app_name>.log`.
//! `write()` routes a formatted log line to the correct file.
//! Server logs are NOT routed here — only `Rivers.log` handler output.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static GLOBAL_ROUTER: OnceLock<Arc<AppLogRouter>> = OnceLock::new();

/// Set the global app log router. Called once during server startup.
pub fn set_global_router(router: Arc<AppLogRouter>) {
    let _ = GLOBAL_ROUTER.set(router);
}

/// Get the global app log router, if configured.
pub fn global_router() -> Option<&'static Arc<AppLogRouter>> {
    GLOBAL_ROUTER.get()
}

/// Registry of per-app log file writers.
pub struct AppLogRouter {
    base_dir: PathBuf,
    writers: Mutex<HashMap<String, BufWriter<File>>>,
}

impl AppLogRouter {
    /// Create a new router that writes `<app_name>.log` files into `base_dir`.
    pub fn new(base_dir: &Path) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
            writers: Mutex::new(HashMap::new()),
        }
    }

    /// Register an app — opens or creates `<base_dir>/<app_name>.log` in append mode.
    pub fn register(&self, app_name: &str) -> Result<(), String> {
        let path = self.base_dir.join(format!("{app_name}.log"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        let new_writer = BufWriter::new(file);
        let mut writers = self.writers.lock().unwrap();
        // Flush existing writer before replacing to prevent data loss on hot reload
        if let Some(old) = writers.get_mut(app_name) {
            let _ = old.flush();
        }
        writers.insert(app_name.to_string(), new_writer);
        Ok(())
    }

    /// Write a log line to the app's log file. Returns false if app not registered.
    pub fn write(&self, app_name: &str, line: &str) -> bool {
        let mut writers = self.writers.lock().unwrap();
        if let Some(writer) = writers.get_mut(app_name) {
            let _ = writeln!(writer, "{}", line);
            true
        } else {
            false
        }
    }

    /// Flush all writers.
    pub fn flush_all(&self) {
        let mut writers = self.writers.lock().unwrap();
        for writer in writers.values_mut() {
            let _ = writer.flush();
        }
    }

    /// Check if an app is registered.
    pub fn is_registered(&self, app_name: &str) -> bool {
        self.writers.lock().unwrap().contains_key(app_name)
    }
}

impl Drop for AppLogRouter {
    fn drop(&mut self) {
        self.flush_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_creates_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let router = AppLogRouter::new(dir.path());
        router.register("my-app").unwrap();
        assert!(dir.path().join("my-app.log").exists());
    }

    #[test]
    fn write_appends_to_correct_file() {
        let dir = tempfile::tempdir().unwrap();
        let router = AppLogRouter::new(dir.path());
        router.register("app-a").unwrap();
        router.register("app-b").unwrap();

        router.write("app-a", "line 1 for A");
        router.write("app-b", "line 1 for B");
        router.write("app-a", "line 2 for A");

        // Flush buffered writes before reading (no per-write flush — BufWriter batches)
        router.flush_all();

        let a_content = std::fs::read_to_string(dir.path().join("app-a.log")).unwrap();
        let b_content = std::fs::read_to_string(dir.path().join("app-b.log")).unwrap();

        assert_eq!(a_content.lines().count(), 2);
        assert_eq!(b_content.lines().count(), 1);
        assert!(a_content.contains("line 1 for A"));
        assert!(a_content.contains("line 2 for A"));
        assert!(b_content.contains("line 1 for B"));
    }

    #[test]
    fn write_returns_false_for_unknown_app() {
        let dir = tempfile::tempdir().unwrap();
        let router = AppLogRouter::new(dir.path());
        assert!(!router.write("unknown", "test"));
    }

    #[test]
    fn is_registered_works() {
        let dir = tempfile::tempdir().unwrap();
        let router = AppLogRouter::new(dir.path());
        assert!(!router.is_registered("app"));
        router.register("app").unwrap();
        assert!(router.is_registered("app"));
    }
}
