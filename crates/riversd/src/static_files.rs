//! Static file serving with ETag, Cache-Control, and SPA fallback.
//!
//! Per `rivers-httpd-spec.md` §7-8.

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use rivers_runtime::rivers_core::config::StaticFilesConfig;

/// Resolve and normalize a static file path.
///
/// Per spec §7.2:
/// 1. Empty path → root/index_file
/// 2. Normalize path components — reject `..` and absolute roots
/// 3. root/normalized exists → return it
/// 4. Does not exist + spa_fallback → root/index_file
/// 5. Does not exist + no spa_fallback → None → 404
///
/// After step 3/4, the candidate path is canonicalized and verified to be
/// inside the canonical root (defense against symlink escape — F1). When
/// `allow_symlinks` is false, any candidate whose final component is a
/// symlink is also rejected (treated as 404).
pub async fn resolve_static_file_path(
    root: &Path,
    requested: &str,
    index_file: &str,
    spa_fallback: bool,
    allow_symlinks: bool,
) -> Option<PathBuf> {
    // 1. Empty path → index file
    let cleaned = requested.trim_start_matches('/');
    if cleaned.is_empty() {
        let index_path = root.join(index_file);
        if tokio::fs::metadata(&index_path).await.ok()?.is_file() {
            return verify_inside_root(root, index_path, allow_symlinks).await;
        }
        return None;
    }

    // 2. Normalize path components, reject traversal
    let mut resolved = root.to_path_buf();
    for component in Path::new(cleaned).components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}       // "." is harmless
            Component::ParentDir => return None, // ".." rejected
            Component::RootDir => return None,   // absolute path rejected
            Component::Prefix(_) => return None, // Windows prefix rejected
        }
    }

    // Verify resolved path is still under root (syntactic prefix check)
    if !resolved.starts_with(root) {
        return None;
    }

    // 3. File exists and is a regular file → return it
    if let Ok(meta) = tokio::fs::metadata(&resolved).await {
        if meta.is_file() {
            return verify_inside_root(root, resolved, allow_symlinks).await;
        }
        // If it's a directory, try index_file inside it
        if meta.is_dir() {
            let dir_index = resolved.join(index_file);
            if let Ok(m) = tokio::fs::metadata(&dir_index).await {
                if m.is_file() {
                    return verify_inside_root(root, dir_index, allow_symlinks).await;
                }
            }
        }
    }

    // 4. Does not exist + spa_fallback → root/index_file
    if spa_fallback {
        let index_path = root.join(index_file);
        if let Ok(m) = tokio::fs::metadata(&index_path).await {
            if m.is_file() {
                return verify_inside_root(root, index_path, allow_symlinks).await;
            }
        }
    }

    // 5. Does not exist + no spa_fallback → None
    None
}

/// Canonicalize the candidate file path and confirm it lives inside the
/// canonical root. When `allow_symlinks` is false, also reject any candidate
/// whose final component is a symlink.
///
/// Returns `Some(canonical_path)` on success, `None` (→ 404) on any failure
/// (path missing, canonicalize error, escape, or symlink when disallowed).
/// Fails closed on every error path — the goal is symlink-escape defense in
/// depth on top of the existing syntactic guard, so we never serve a file
/// whose true on-disk identity we couldn't verify.
async fn verify_inside_root(
    root: &Path,
    candidate: PathBuf,
    allow_symlinks: bool,
) -> Option<PathBuf> {
    // Reject final-component symlink up front when not allowed. This is a
    // best-effort check: a symlink anywhere in the path will also be caught
    // by the canonicalization comparison below (because the canonical path
    // will resolve out of root, or to a path that isn't under canonical root).
    if !allow_symlinks {
        match tokio::fs::symlink_metadata(&candidate).await {
            Ok(meta) if meta.file_type().is_symlink() => return None,
            Ok(_) => {}
            Err(_) => return None,
        }
    }

    let canonical_root = tokio::fs::canonicalize(root).await.ok()?;
    let canonical_path = tokio::fs::canonicalize(&candidate).await.ok()?;

    if !canonical_path.starts_with(&canonical_root) {
        return None;
    }

    Some(canonical_path)
}

/// Check if a path is in the exclude list.
///
/// Per spec §7.1: 404 for paths in exclude_paths even if file exists.
pub fn is_excluded(request_path: &str, exclude_paths: &[String]) -> bool {
    let cleaned = request_path.trim_start_matches('/');
    exclude_paths.iter().any(|p| {
        let p_cleaned = p.trim_start_matches('/');
        cleaned == p_cleaned || cleaned.starts_with(&format!("{}/", p_cleaned))
    })
}

/// Compute a SHA-256 ETag for file contents.
///
/// Per spec §7.3: ETag is `"{sha256_hex}"`.
pub fn compute_etag(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let hash = hasher.finalize();
    format!("\"{}\"", hex::encode(hash))
}

/// Guess the MIME type from a file extension.
pub fn guess_content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("xml") => "application/xml; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("map") => "application/json; charset=utf-8",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Build a static file response.
///
/// Reads from disk, applies ETag/If-None-Match, Cache-Control.
/// Returns 404 if file not found, 304 if not modified.
pub async fn serve_static_file(
    config: &StaticFilesConfig,
    request_path: &str,
    if_none_match: Option<&str>,
) -> Response {
    let root = match &config.root_path {
        Some(dir) => Path::new(dir.as_str()),
        None => return crate::error_response::not_found("not found").into_axum_response(),
    };

    // Check exclude list first
    if is_excluded(request_path, &config.exclude_paths) {
        return crate::error_response::not_found("not found").into_axum_response();
    }

    // Resolve path per spec §7.2
    let file_path = match resolve_static_file_path(
        root,
        request_path,
        &config.index_file,
        config.spa_fallback,
        config.allow_symlinks,
    )
    .await
    {
        Some(p) => p,
        None => return crate::error_response::not_found("not found").into_axum_response(),
    };

    // Check file size before reading into memory (default: 50 MiB)
    const MAX_STATIC_FILE_SIZE: u64 = 50 * 1024 * 1024;
    if let Ok(meta) = tokio::fs::metadata(&file_path).await {
        if meta.len() > MAX_STATIC_FILE_SIZE {
            return crate::error_response::ErrorResponse::new(413, "file too large")
                .into_axum_response();
        }
    }

    // Read file
    let content = match tokio::fs::read(&file_path).await {
        Ok(bytes) => bytes,
        Err(_) => return crate::error_response::not_found("not found").into_axum_response(),
    };

    let max_age = config.max_age.unwrap_or(3600);
    build_file_response(&content, &file_path, max_age, if_none_match)
}

/// Build a response with ETag, Cache-Control, and Content-Type.
fn build_file_response(
    content: &[u8],
    path: &Path,
    cache_max_age: u64,
    if_none_match: Option<&str>,
) -> Response {
    let etag = compute_etag(content);

    // Check If-None-Match → 304
    if let Some(inm) = if_none_match {
        if inm == etag || inm.trim_matches('"') == etag.trim_matches('"') {
            let mut response = StatusCode::NOT_MODIFIED.into_response();
            if let Ok(val) = HeaderValue::from_str(&etag) {
                response.headers_mut().insert("etag", val);
            }
            return response;
        }
    }

    let content_type = guess_content_type(path);
    let cache_control = format!("public, max-age={}", cache_max_age);

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(content.to_vec()))
        .unwrap();

    let headers = response.headers_mut();
    if let Ok(val) = HeaderValue::from_str(content_type) {
        headers.insert("content-type", val);
    }
    if let Ok(val) = HeaderValue::from_str(&etag) {
        headers.insert("etag", val);
    }
    if let Ok(val) = HeaderValue::from_str(&cache_control) {
        headers.insert("cache-control", val);
    }

    response
}
