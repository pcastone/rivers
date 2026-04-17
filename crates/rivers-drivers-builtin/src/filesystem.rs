//! Filesystem driver — chroot-sandboxed direct-I/O driver.
//!
//! Spec: docs/arch/rivers-filesystem-driver-spec.md

use async_trait::async_trait;
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, DriverError, OpKind, OperationDescriptor,
    Param, ParamType, Query, QueryResult, QueryValue,
};
use std::path::PathBuf;

pub struct FilesystemDriver;

pub struct FilesystemConnection {
    pub root: PathBuf,
}

impl FilesystemDriver {
    pub fn resolve_root(database: &str) -> Result<PathBuf, DriverError> {
        let path = PathBuf::from(database);

        if !path.is_absolute() {
            return Err(DriverError::Connection(format!(
                "filesystem root must be absolute path, got: {database}"
            )));
        }

        let canonical = std::fs::canonicalize(&path).map_err(|e| {
            DriverError::Connection(format!(
                "filesystem root does not exist or is not accessible: {database} — {e}"
            ))
        })?;

        if !canonical.is_dir() {
            return Err(DriverError::Connection(format!(
                "filesystem root is not a directory: {}",
                canonical.display()
            )));
        }

        Ok(canonical)
    }
}

impl FilesystemConnection {
    pub fn resolve_path(&self, relative: &str) -> Result<PathBuf, DriverError> {
        let normalized = relative.replace('\\', "/");

        let bytes = normalized.as_bytes();
        let is_windows_drive =
            bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic();
        if normalized.starts_with('/') || is_windows_drive {
            return Err(DriverError::Query(
                "absolute paths not permitted — all paths relative to datasource root".into(),
            ));
        }

        let joined = self.root.join(&normalized);
        reject_symlinks_within(&self.root, &joined)?;

        let canonical = canonicalize_for_op(&joined)?;

        if !canonical.starts_with(&self.root) {
            return Err(DriverError::Forbidden(
                "path escapes datasource root".into(),
            ));
        }

        Ok(canonical)
    }
}

fn canonicalize_for_op(path: &std::path::Path) -> Result<PathBuf, DriverError> {
    // For nonexistent paths (writeFile, mkdir), canonicalize the deepest existing
    // ancestor, then append the remaining segments. This preserves chroot checks
    // while letting write ops target paths that do not yet exist.
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let base = std::fs::canonicalize(&existing).map_err(|e| {
        DriverError::Query(format!("could not canonicalize ancestor of path: {e}"))
    })?;
    let mut out = base;
    for piece in tail.into_iter().rev() {
        out.push(piece);
    }
    Ok(out)
}

fn reject_symlinks_within(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<(), DriverError> {
    // Walk from root forward, checking every intermediate component.
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut current = root.to_path_buf();
    for comp in rel.components() {
        current.push(comp);
        if !current.exists() {
            break;
        }
        let is_symlink = current
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            return Err(DriverError::Forbidden(format!(
                "symlink detected in path: {}",
                current.display()
            )));
        }
    }
    Ok(())
}

#[async_trait]
impl DatabaseDriver for FilesystemDriver {
    fn name(&self) -> &str {
        "filesystem"
    }

    fn operations(&self) -> &[OperationDescriptor] {
        FILESYSTEM_OPERATIONS
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let root = Self::resolve_root(&params.database)?;
        Ok(Box::new(FilesystemConnection { root }))
    }
}

#[async_trait]
impl Connection for FilesystemConnection {
    async fn execute(&mut self, q: &Query) -> Result<QueryResult, DriverError> {
        match q.operation.as_str() {
            // Reads (Tasks 14–19)
            "readFile" => ops::read_file(self, q).await,
            "readDir" => ops::read_dir(self, q).await,
            "stat" => ops::stat(self, q).await,
            "exists" => ops::exists(self, q).await,
            "find" => Err(DriverError::NotImplemented("find — Task 18".into())),
            "grep" => Err(DriverError::NotImplemented("grep — Task 19".into())),
            // Writes (Tasks 20–24)
            "writeFile" => Err(DriverError::NotImplemented("writeFile — Task 20".into())),
            "mkdir" => Err(DriverError::NotImplemented("mkdir — Task 21".into())),
            "delete" => Err(DriverError::NotImplemented("delete — Task 22".into())),
            "rename" => Err(DriverError::NotImplemented("rename — Task 23".into())),
            "copy" => Err(DriverError::NotImplemented("copy — Task 24".into())),
            other => Err(DriverError::Unsupported(format!(
                "unknown filesystem operation: {other}"
            ))),
        }
    }

    async fn ddl_execute(&mut self, _q: &Query) -> Result<QueryResult, DriverError> {
        Err(DriverError::Forbidden(
            "filesystem driver does not support ddl_execute".into(),
        ))
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        Err(DriverError::NotImplemented("FilesystemConnection::ping — Task 11".into()))
    }

    fn driver_name(&self) -> &str {
        "filesystem"
    }
}

static FILESYSTEM_OPERATIONS: &[OperationDescriptor] = &[
    // Reads
    OperationDescriptor::read(
        "readFile",
        &[
            Param::required("path", ParamType::String),
            Param::optional("encoding", ParamType::String, "utf-8"),
        ],
        "Read file contents — utf-8 returns string, base64 returns base64-encoded string",
    ),
    OperationDescriptor::read(
        "readDir",
        &[Param::required("path", ParamType::String)],
        "List directory entries — filenames only",
    ),
    OperationDescriptor::read(
        "stat",
        &[Param::required("path", ParamType::String)],
        "File/directory metadata",
    ),
    OperationDescriptor::read(
        "exists",
        &[Param::required("path", ParamType::String)],
        "Returns boolean existence",
    ),
    OperationDescriptor::read(
        "find",
        &[
            Param::required("pattern", ParamType::String),
            Param::optional("max_results", ParamType::Integer, "1000"),
        ],
        "Recursive glob search",
    ),
    OperationDescriptor::read(
        "grep",
        &[
            Param::required("pattern", ParamType::String),
            Param::optional("path", ParamType::String, "."),
            Param::optional("max_results", ParamType::Integer, "1000"),
        ],
        "Regex search across files",
    ),
    // Writes
    OperationDescriptor::write(
        "writeFile",
        &[
            Param::required("path", ParamType::String),
            Param::required("content", ParamType::String),
            Param::optional("encoding", ParamType::String, "utf-8"),
        ],
        "Write file — creates parent dirs, overwrites if exists",
    ),
    OperationDescriptor::write(
        "mkdir",
        &[Param::required("path", ParamType::String)],
        "Create directory recursively",
    ),
    OperationDescriptor::write(
        "delete",
        &[Param::required("path", ParamType::String)],
        "Delete file or recursively delete directory",
    ),
    OperationDescriptor::write(
        "rename",
        &[
            Param::required("oldPath", ParamType::String),
            Param::required("newPath", ParamType::String),
        ],
        "Rename/move within root",
    ),
    OperationDescriptor::write(
        "copy",
        &[
            Param::required("src", ParamType::String),
            Param::required("dest", ParamType::String),
        ],
        "Copy file or recursively copy directory",
    ),
];

mod ops {
    use super::*;
    use base64::Engine;
    use rivers_driver_sdk::{Query, QueryResult, QueryValue};
    use std::collections::HashMap;

    fn get_string<'a>(q: &'a Query, key: &str) -> Option<&'a str> {
        match q.parameters.get(key) {
            Some(QueryValue::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    pub async fn read_file(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("readFile: required parameter 'path' missing".into())
        })?;
        let encoding = get_string(q, "encoding").unwrap_or("utf-8");
        let path = conn.resolve_path(rel)?;
        let bytes = tokio::task::spawn_blocking({
            let path = path.clone();
            move || std::fs::read(&path)
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        let content = match encoding {
            "utf-8" => String::from_utf8(bytes).map_err(|e| {
                DriverError::Query(format!("file is not valid utf-8: {e}"))
            })?,
            "base64" => base64::engine::general_purpose::STANDARD.encode(&bytes),
            other => {
                return Err(DriverError::Query(format!(
                    "unsupported encoding: {other}"
                )));
            }
        };

        let mut row = HashMap::new();
        row.insert("content".to_string(), QueryValue::String(content));

        Ok(QueryResult {
            rows: vec![row],
            affected_rows: 1,
            last_insert_id: None,
            column_names: Some(vec!["content".to_string()]),
        })
    }

    pub async fn read_dir(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("readDir: required parameter 'path' missing".into())
        })?;
        let path = conn.resolve_path(rel)?;
        let entries: Vec<String> = tokio::task::spawn_blocking({
            let path = path.clone();
            move || -> Result<Vec<String>, std::io::Error> {
                let mut out = Vec::new();
                for entry in std::fs::read_dir(&path)? {
                    out.push(entry?.file_name().to_string_lossy().to_string());
                }
                Ok(out)
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        let rows = entries
            .into_iter()
            .map(|name| {
                let mut row = HashMap::new();
                row.insert("name".to_string(), QueryValue::String(name));
                row
            })
            .collect::<Vec<_>>();
        let affected_rows = rows.len() as u64;

        Ok(QueryResult {
            rows,
            affected_rows,
            last_insert_id: None,
            column_names: Some(vec!["name".to_string()]),
        })
    }

    pub async fn stat(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("stat: required parameter 'path' missing".into())
        })?;
        let path = conn.resolve_path(rel)?;
        let md = tokio::task::spawn_blocking({
            let p = path.clone();
            move || std::fs::metadata(&p)
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        fn epoch_secs(t: std::time::SystemTime) -> String {
            let secs = t
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            secs.to_string()
        }

        #[cfg(unix)]
        let mode_val = {
            use std::os::unix::fs::PermissionsExt;
            md.permissions().mode() as i64
        };
        #[cfg(not(unix))]
        let mode_val: i64 = 0;

        let mut row = HashMap::new();
        row.insert("size".into(), QueryValue::Integer(md.len() as i64));
        row.insert(
            "mtime".into(),
            QueryValue::String(epoch_secs(md.modified().unwrap_or(std::time::UNIX_EPOCH))),
        );
        row.insert(
            "atime".into(),
            QueryValue::String(epoch_secs(md.accessed().unwrap_or(std::time::UNIX_EPOCH))),
        );
        row.insert(
            "ctime".into(),
            QueryValue::String(epoch_secs(md.created().unwrap_or(std::time::UNIX_EPOCH))),
        );
        row.insert("isFile".into(), QueryValue::Boolean(md.is_file()));
        row.insert("isDirectory".into(), QueryValue::Boolean(md.is_dir()));
        row.insert("mode".into(), QueryValue::Integer(mode_val));

        Ok(QueryResult {
            rows: vec![row],
            affected_rows: 1,
            last_insert_id: None,
            column_names: Some(vec![
                "size".into(),
                "mtime".into(),
                "atime".into(),
                "ctime".into(),
                "isFile".into(),
                "isDirectory".into(),
                "mode".into(),
            ]),
        })
    }

    pub async fn exists(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("exists: required parameter 'path' missing".into())
        })?;
        let ok = match conn.resolve_path(rel) {
            Ok(p) => tokio::task::spawn_blocking(move || p.exists())
                .await
                .unwrap_or(false),
            Err(DriverError::Forbidden(_)) => false,
            Err(e) => return Err(e),
        };
        let mut row = HashMap::new();
        row.insert("exists".to_string(), QueryValue::Boolean(ok));
        Ok(QueryResult {
            rows: vec![row],
            affected_rows: 1,
            last_insert_id: None,
            column_names: Some(vec!["exists".to_string()]),
        })
    }

    pub fn map_io_error(e: std::io::Error) -> DriverError {
        use std::io::ErrorKind::*;
        match e.kind() {
            NotFound => DriverError::Query(format!("not found: {e}")),
            PermissionDenied => DriverError::Query(format!("permission denied: {e}")),
            _ => DriverError::Internal(format!("I/O error: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_connection() -> (TempDir, FilesystemConnection) {
        let dir = TempDir::new().unwrap();
        let root = FilesystemDriver::resolve_root(dir.path().to_str().unwrap()).unwrap();
        (dir, FilesystemConnection { root })
    }

    #[test]
    fn driver_name_is_filesystem() {
        assert_eq!(FilesystemDriver.name(), "filesystem");
    }

    #[test]
    fn catalog_has_eleven_operations() {
        assert_eq!(FilesystemDriver.operations().len(), 11);
    }

    #[test]
    fn catalog_contains_all_expected_names() {
        let names: Vec<&str> = FilesystemDriver
            .operations()
            .iter()
            .map(|o| o.name)
            .collect();
        for expected in [
            "readFile", "readDir", "stat", "exists", "find", "grep", "writeFile", "mkdir",
            "delete", "rename", "copy",
        ] {
            assert!(
                names.contains(&expected),
                "missing op: {expected}"
            );
        }
    }

    #[test]
    fn read_ops_have_opkind_read() {
        for op in FilesystemDriver.operations() {
            let is_read = matches!(
                op.name,
                "readFile" | "readDir" | "stat" | "exists" | "find" | "grep"
            );
            let is_write = matches!(
                op.name,
                "writeFile" | "mkdir" | "delete" | "rename" | "copy"
            );
            match (is_read, is_write) {
                (true, false) => assert_eq!(op.kind, OpKind::Read, "{}", op.name),
                (false, true) => assert_eq!(op.kind, OpKind::Write, "{}", op.name),
                _ => panic!("unclassified op: {}", op.name),
            }
        }
    }

    #[test]
    fn resolve_root_rejects_relative_path() {
        let err = FilesystemDriver::resolve_root("./relative").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("absolute"),
            "expected 'absolute' in error, got: {msg}"
        );
    }

    #[test]
    fn resolve_root_rejects_nonexistent_path() {
        let err = FilesystemDriver::resolve_root("/does/not/exist/for/real").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("does not exist") || msg.contains("not accessible"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn resolve_root_rejects_file_path() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("not_a_dir.txt");
        std::fs::write(&file_path, b"hi").unwrap();
        let err = FilesystemDriver::resolve_root(file_path.to_str().unwrap()).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("not a directory"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn resolve_root_canonicalizes_valid_directory() {
        let dir = TempDir::new().unwrap();
        let resolved = FilesystemDriver::resolve_root(dir.path().to_str().unwrap()).unwrap();
        assert!(resolved.is_absolute());
        assert!(resolved.is_dir());
    }

    #[test]
    fn resolve_path_rejects_absolute_unix() {
        let (_dir, conn) = test_connection();
        let err = conn.resolve_path("/etc/passwd").unwrap_err();
        assert!(format!("{err}").contains("absolute paths not permitted"));
    }

    #[test]
    fn resolve_path_rejects_parent_escape() {
        let (_dir, conn) = test_connection();
        let err = conn.resolve_path("../../../etc/passwd").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("escapes datasource root") || msg.contains("does not exist"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn resolve_path_accepts_valid_relative() {
        let (dir, conn) = test_connection();
        std::fs::write(dir.path().join("hello.txt"), b"hi").unwrap();
        let resolved = conn.resolve_path("hello.txt").unwrap();
        assert!(resolved.starts_with(&conn.root));
    }

    #[test]
    fn resolve_path_normalizes_backslashes() {
        // On Unix this behaves like a literal; purpose is documentation — real
        // Windows coverage comes via CI.
        let (dir, conn) = test_connection();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a").join("b.txt"), b"x").unwrap();
        let resolved = conn.resolve_path("a\\b.txt").unwrap();
        assert!(resolved.starts_with(&conn.root));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_path_rejects_symlink_inside_root() {
        use std::os::unix::fs::symlink;
        let (dir, conn) = test_connection();
        let target = dir.path().join("real");
        std::fs::create_dir(&target).unwrap();
        symlink(&target, dir.path().join("link")).unwrap();

        let err = conn.resolve_path("link").unwrap_err();
        assert!(
            format!("{err}").contains("symlink detected"),
            "expected 'symlink detected' in error, got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_path_rejects_symlink_pointing_outside_root() {
        use std::os::unix::fs::symlink;
        let (dir, conn) = test_connection();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), dir.path().join("escape")).unwrap();

        let err = conn.resolve_path("escape/file.txt").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("symlink detected") || msg.contains("escapes datasource root"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn connect_returns_connection_with_resolved_root() {
        use std::collections::HashMap;
        let dir = TempDir::new().unwrap();
        let params = ConnectionParams {
            host: String::new(),
            port: 0,
            database: dir.path().to_str().unwrap().to_string(),
            username: String::new(),
            password: String::new(),
            options: HashMap::new(),
        };
        let driver = FilesystemDriver;
        let conn = driver.connect(&params).await.unwrap();
        // Dry-probe: we don't yet have execute(), but we should at least compile + connect.
        drop(conn);
    }

    #[tokio::test]
    async fn connect_fails_on_nonexistent_root() {
        use std::collections::HashMap;
        let params = ConnectionParams {
            host: String::new(),
            port: 0,
            database: "/does/not/exist/nowhere".into(),
            username: String::new(),
            password: String::new(),
            options: HashMap::new(),
        };
        let result = FilesystemDriver.connect(&params).await;
        assert!(result.is_err());
        match result {
            Err(err) => {
                let msg = format!("{err}");
                assert!(msg.contains("does not exist") || msg.contains("not accessible"));
            }
            Ok(_) => panic!("expected error for nonexistent root"),
        }
    }

    #[tokio::test]
    async fn execute_unknown_operation_returns_notimpl() {
        let (_dir, mut conn) = test_connection();
        let q = Query {
            operation: "nope".into(),
            target: String::new(),
            parameters: std::collections::HashMap::new(),
            statement: String::new(),
        };
        let err = conn.execute(&q).await.unwrap_err();
        assert!(
            matches!(err, DriverError::NotImplemented(_) | DriverError::Unsupported(_)),
            "unexpected variant: {err:?}"
        );
    }

    fn mkq(op: &str, params: &[(&str, QueryValue)]) -> Query {
        let mut parameters = std::collections::HashMap::new();
        for (k, v) in params {
            parameters.insert(k.to_string(), v.clone());
        }
        Query {
            operation: op.into(),
            target: String::new(),
            parameters,
            statement: String::new(),
        }
    }

    fn extract_scalar_string(r: &QueryResult) -> String {
        let row = r.rows.first().expect("expected one row");
        let val = row
            .get("content")
            .expect("expected 'content' column");
        match val {
            QueryValue::String(s) => s.clone(),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_file_utf8_returns_string() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let q = mkq("readFile", &[("path", QueryValue::String("a.txt".into()))]);
        let result = conn.execute(&q).await.unwrap();
        let content = extract_scalar_string(&result);
        assert_eq!(content, "hello");
    }

    #[tokio::test]
    async fn read_file_base64_returns_b64_string() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("b.bin"), &[0xff, 0x00, 0xfe]).unwrap();
        let q = mkq(
            "readFile",
            &[
                ("path", QueryValue::String("b.bin".into())),
                ("encoding", QueryValue::String("base64".into())),
            ],
        );
        let result = conn.execute(&q).await.unwrap();
        let content = extract_scalar_string(&result);
        assert_eq!(content, "/wD+"); // base64 of 0xff 0x00 0xfe
    }

    #[tokio::test]
    async fn read_file_unknown_encoding_errors() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        let q = mkq(
            "readFile",
            &[
                ("path", QueryValue::String("a.txt".into())),
                ("encoding", QueryValue::String("ebcdic".into())),
            ],
        );
        let err = conn.execute(&q).await.unwrap_err();
        assert!(format!("{err}").contains("unsupported encoding"));
    }

    #[tokio::test]
    async fn exists_returns_true_for_present_false_for_absent() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("yes.txt"), "").unwrap();
        let q = mkq("exists", &[("path", QueryValue::String("yes.txt".into()))]);
        let r = conn.execute(&q).await.unwrap();
        assert!(matches!(r.rows[0].get("exists"), Some(QueryValue::Boolean(true))));
        let q2 = mkq("exists", &[("path", QueryValue::String("nope.txt".into()))]);
        let r2 = conn.execute(&q2).await.unwrap();
        assert!(matches!(r2.rows[0].get("exists"), Some(QueryValue::Boolean(false))));
    }

    #[tokio::test]
    async fn exists_returns_false_for_chroot_escape() {
        let (_dir, mut conn) = test_connection();
        let q = mkq("exists", &[("path", QueryValue::String("../../etc/passwd".into()))]);
        let r = conn.execute(&q).await.unwrap();
        assert!(matches!(r.rows[0].get("exists"), Some(QueryValue::Boolean(false))));
    }

    #[tokio::test]
    async fn stat_file_returns_metadata() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("f.txt"), b"hello").unwrap();
        let q = mkq("stat", &[("path", QueryValue::String("f.txt".into()))]);
        let result = conn.execute(&q).await.unwrap();
        assert_eq!(result.rows.len(), 1);
        let cols = result.column_names.as_ref().expect("column_names set");
        for expected in ["size", "mtime", "atime", "ctime", "isFile", "isDirectory", "mode"] {
            assert!(cols.iter().any(|c| c == expected), "missing col: {expected}");
        }
        let row = &result.rows[0];
        assert!(matches!(row.get("size"), Some(QueryValue::Integer(5))));
        assert!(matches!(row.get("isFile"), Some(QueryValue::Boolean(true))));
        assert!(matches!(row.get("isDirectory"), Some(QueryValue::Boolean(false))));
    }

    #[tokio::test]
    async fn read_dir_returns_entry_names() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("b")).unwrap();
        let q = mkq("readDir", &[("path", QueryValue::String(".".into()))]);
        let result = conn.execute(&q).await.unwrap();
        let mut names: Vec<String> = result
            .rows
            .iter()
            .map(|row| match row.get("name") {
                Some(QueryValue::String(s)) => s.clone(),
                other => panic!("expected String name, got {other:?}"),
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.txt".to_string(), "b".to_string()]);
    }
}
