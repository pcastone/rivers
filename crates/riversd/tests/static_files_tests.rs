use std::path::Path;

use riversd::static_files::{
    compute_etag, guess_content_type, is_excluded, resolve_static_file_path, serve_static_file,
};
use rivers_runtime::rivers_core::config::StaticFilesConfig;

// ── Path Resolution ──────────────────────────────────────────────

/// Canonicalize a path the same way the resolver does (so test assertions
/// match the canonical form returned by `resolve_static_file_path`).
fn canonical(p: std::path::PathBuf) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap()
}

#[tokio::test]
async fn resolve_empty_path_returns_index() {
    let dir = tempdir_with_files(&[("index.html", "<html></html>")]);
    let result =
        resolve_static_file_path(dir.path(), "", "index.html", false, false).await;
    assert_eq!(result.unwrap(), canonical(dir.path().join("index.html")));
}

#[tokio::test]
async fn resolve_slash_returns_index() {
    let dir = tempdir_with_files(&[("index.html", "<html></html>")]);
    let result =
        resolve_static_file_path(dir.path(), "/", "index.html", false, false).await;
    assert_eq!(result.unwrap(), canonical(dir.path().join("index.html")));
}

#[tokio::test]
async fn resolve_normal_file() {
    let dir = tempdir_with_files(&[("style.css", "body {}")]);
    let result =
        resolve_static_file_path(dir.path(), "/style.css", "index.html", false, false).await;
    assert_eq!(result.unwrap(), canonical(dir.path().join("style.css")));
}

#[tokio::test]
async fn resolve_nested_file() {
    let dir = tempdir_with_files(&[("assets/app.js", "console.log('hi')")]);
    let result =
        resolve_static_file_path(dir.path(), "/assets/app.js", "index.html", false, false).await;
    assert_eq!(result.unwrap(), canonical(dir.path().join("assets/app.js")));
}

#[tokio::test]
async fn resolve_traversal_rejected() {
    let dir = tempdir_with_files(&[("index.html", "ok")]);
    let result = resolve_static_file_path(
        dir.path(),
        "/../../../etc/passwd",
        "index.html",
        false,
        false,
    )
    .await;
    assert!(result.is_none());
}

#[tokio::test]
async fn resolve_double_dot_rejected() {
    let dir = tempdir_with_files(&[("index.html", "ok")]);
    let result =
        resolve_static_file_path(dir.path(), "/foo/../../bar", "index.html", false, false).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn resolve_absolute_root_rejected() {
    let dir = tempdir_with_files(&[("index.html", "ok")]);
    let result =
        resolve_static_file_path(dir.path(), "//etc/passwd", "index.html", false, false).await;
    // On Unix, `//etc/passwd` after trim_start_matches('/') becomes "etc/passwd"
    // which resolves to root/etc/passwd — a non-existent file → None
    assert!(result.is_none());
}

#[tokio::test]
async fn resolve_nonexistent_no_spa_returns_none() {
    let dir = tempdir_with_files(&[("index.html", "ok")]);
    let result =
        resolve_static_file_path(dir.path(), "/doesnt-exist.js", "index.html", false, false)
            .await;
    assert!(result.is_none());
}

#[tokio::test]
async fn resolve_nonexistent_with_spa_returns_index() {
    let dir = tempdir_with_files(&[("index.html", "<html>spa</html>")]);
    let result = resolve_static_file_path(
        dir.path(),
        "/some/client/route",
        "index.html",
        true, // spa_fallback
        false,
    )
    .await;
    assert_eq!(result.unwrap(), canonical(dir.path().join("index.html")));
}

#[tokio::test]
async fn resolve_existing_file_with_spa_returns_file_not_index() {
    let dir = tempdir_with_files(&[("index.html", "spa"), ("app.js", "real file")]);
    let result =
        resolve_static_file_path(dir.path(), "/app.js", "index.html", true, false).await;
    assert_eq!(result.unwrap(), canonical(dir.path().join("app.js")));
}

#[tokio::test]
async fn resolve_directory_returns_index_inside() {
    let dir = tempdir_with_files(&[("subdir/index.html", "sub index")]);
    let result =
        resolve_static_file_path(dir.path(), "/subdir", "index.html", false, false).await;
    assert_eq!(result.unwrap(), canonical(dir.path().join("subdir/index.html")));
}

// ── Symlink Handling (F1) ────────────────────────────────────────

/// A symlink inside the static root pointing at a file *outside* the root
/// must be rejected even when symlinks are allowed — canonicalization
/// resolves the symlink and the canonical path is no longer under the
/// canonical root.
#[tokio::test]
async fn resolve_symlink_escaping_root_is_rejected() {
    use std::os::unix::fs::symlink;

    // Outside dir holds the secret target.
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "TOP SECRET").unwrap();

    // Static root contains a symlink "leak" → outside/secret.txt
    let root = tempfile::tempdir().unwrap();
    symlink(&secret, root.path().join("leak")).unwrap();

    // With allow_symlinks=false, the symlink itself is rejected.
    let denied =
        resolve_static_file_path(root.path(), "/leak", "index.html", false, false).await;
    assert!(denied.is_none(), "symlink should be rejected when allow_symlinks=false");

    // Even with allow_symlinks=true, the canonical-prefix check rejects it
    // because the resolved target is outside the canonical root.
    let allowed =
        resolve_static_file_path(root.path(), "/leak", "index.html", false, true).await;
    assert!(
        allowed.is_none(),
        "symlink escaping the root must be rejected even when allow_symlinks=true"
    );
}

/// A symlink inside the root pointing to another file *inside* the root
/// is rejected when `allow_symlinks=false` and served when `true`.
#[tokio::test]
async fn resolve_symlink_inside_root_respects_allow_symlinks() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real.txt");
    std::fs::write(&real, "hello").unwrap();
    symlink(&real, root.path().join("alias")).unwrap();

    // Default-deny: symlink rejected.
    let denied =
        resolve_static_file_path(root.path(), "/alias", "index.html", false, false).await;
    assert!(denied.is_none(), "in-root symlink rejected when allow_symlinks=false");

    // Opt-in allow: symlink served, returned path is the canonical target.
    let allowed =
        resolve_static_file_path(root.path(), "/alias", "index.html", false, true).await;
    assert_eq!(allowed.unwrap(), canonical(real));
}

/// A regular (non-symlink) file inside the root is served regardless of
/// the `allow_symlinks` flag.
#[tokio::test]
async fn resolve_regular_file_unaffected_by_allow_symlinks_flag() {
    let dir = tempdir_with_files(&[("plain.txt", "plain")]);

    let a =
        resolve_static_file_path(dir.path(), "/plain.txt", "index.html", false, false).await;
    let b =
        resolve_static_file_path(dir.path(), "/plain.txt", "index.html", false, true).await;
    assert_eq!(a.unwrap(), canonical(dir.path().join("plain.txt")));
    assert_eq!(b.unwrap(), canonical(dir.path().join("plain.txt")));
}

/// End-to-end: serve_static_file returns 404 for a symlink that escapes
/// the static root.
#[tokio::test]
async fn serve_returns_404_for_symlink_escaping_root() {
    use std::os::unix::fs::symlink;

    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("passwd");
    std::fs::write(&secret, "root:x:0:0").unwrap();

    let root = tempfile::tempdir().unwrap();
    symlink(&secret, root.path().join("leak")).unwrap();

    // allow_symlinks=true so the symlink isn't filtered by the
    // is_symlink check — proves the canonical-prefix check catches the
    // escape on its own.
    let mut config = config_for_dir(root.path(), false);
    config.allow_symlinks = true;

    let response = serve_static_file(&config, "/leak", None).await;
    assert_eq!(response.status(), 404);
}

// ── Exclude Paths ────────────────────────────────────────────────

#[test]
fn exclude_exact_match() {
    assert!(is_excluded("/.env", &[".env".to_string()]));
    assert!(is_excluded(".env", &[".env".to_string()]));
}

#[test]
fn exclude_prefix_match() {
    assert!(is_excluded("/config/secrets.toml", &["config".to_string()]));
}

#[test]
fn exclude_no_match() {
    assert!(!is_excluded("/app.js", &[".env".to_string()]));
}

// ── ETag ─────────────────────────────────────────────────────────

#[test]
fn etag_is_sha256_hex_quoted() {
    let content = b"hello world";
    let etag = compute_etag(content);
    // SHA-256 of "hello world" = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
    assert_eq!(
        etag,
        "\"b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9\""
    );
}

#[test]
fn etag_deterministic() {
    let a = compute_etag(b"test");
    let b = compute_etag(b"test");
    assert_eq!(a, b);
}

#[test]
fn etag_different_content_different_hash() {
    let a = compute_etag(b"foo");
    let b = compute_etag(b"bar");
    assert_ne!(a, b);
}

// ── Content Type ─────────────────────────────────────────────────

#[test]
fn content_type_html() {
    assert_eq!(
        guess_content_type(Path::new("index.html")),
        "text/html; charset=utf-8"
    );
}

#[test]
fn content_type_css() {
    assert_eq!(
        guess_content_type(Path::new("style.css")),
        "text/css; charset=utf-8"
    );
}

#[test]
fn content_type_js() {
    assert_eq!(
        guess_content_type(Path::new("app.js")),
        "application/javascript; charset=utf-8"
    );
}

#[test]
fn content_type_json() {
    assert_eq!(
        guess_content_type(Path::new("data.json")),
        "application/json; charset=utf-8"
    );
}

#[test]
fn content_type_png() {
    assert_eq!(guess_content_type(Path::new("logo.png")), "image/png");
}

#[test]
fn content_type_wasm() {
    assert_eq!(
        guess_content_type(Path::new("module.wasm")),
        "application/wasm"
    );
}

#[test]
fn content_type_unknown() {
    assert_eq!(
        guess_content_type(Path::new("file.xyz")),
        "application/octet-stream"
    );
}

// ── serve_static_file Integration ────────────────────────────────

#[tokio::test]
async fn serve_returns_200_with_content() {
    let dir = tempdir_with_files(&[("hello.txt", "hello world")]);
    let config = config_for_dir(dir.path(), false);

    let response = serve_static_file(&config, "/hello.txt", None).await;
    assert_eq!(response.status(), 200);

    let headers = response.headers();
    assert_eq!(
        headers.get("content-type").unwrap(),
        "text/plain; charset=utf-8"
    );
    assert!(headers.get("etag").is_some());
    assert!(headers
        .get("cache-control")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("public, max-age="));
}

#[tokio::test]
async fn serve_returns_304_on_matching_etag() {
    let dir = tempdir_with_files(&[("hello.txt", "hello world")]);
    let config = config_for_dir(dir.path(), false);

    // First request to get etag
    let response = serve_static_file(&config, "/hello.txt", None).await;
    let etag = response
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Second request with If-None-Match
    let response = serve_static_file(&config, "/hello.txt", Some(&etag)).await;
    assert_eq!(response.status(), 304);
}

#[tokio::test]
async fn serve_returns_404_for_nonexistent() {
    let dir = tempdir_with_files(&[("index.html", "ok")]);
    let config = config_for_dir(dir.path(), false);

    let response = serve_static_file(&config, "/nope.js", None).await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn serve_returns_404_for_excluded_path() {
    let dir = tempdir_with_files(&[(".env", "SECRET=foo")]);
    let mut config = config_for_dir(dir.path(), false);
    config.exclude_paths = vec![".env".to_string()];

    let response = serve_static_file(&config, "/.env", None).await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn serve_spa_fallback_returns_index_for_unknown_route() {
    let dir = tempdir_with_files(&[("index.html", "<html>spa</html>")]);
    let config = config_for_dir(dir.path(), true);

    let response = serve_static_file(&config, "/dashboard/settings", None).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/html; charset=utf-8"
    );
}

#[tokio::test]
async fn serve_returns_404_when_disabled() {
    let config = StaticFilesConfig {
        enabled: false,
        root_path: None,
        ..Default::default()
    };
    let response = serve_static_file(&config, "/anything", None).await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn serve_custom_max_age() {
    let dir = tempdir_with_files(&[("app.js", "code()")]);
    let mut config = config_for_dir(dir.path(), false);
    config.max_age = Some(86400);

    let response = serve_static_file(&config, "/app.js", None).await;
    assert_eq!(
        response.headers().get("cache-control").unwrap(),
        "public, max-age=86400"
    );
}

#[tokio::test]
async fn serve_default_max_age_3600() {
    let dir = tempdir_with_files(&[("app.js", "code()")]);
    let mut config = config_for_dir(dir.path(), false);
    config.max_age = None; // should default to 3600

    let response = serve_static_file(&config, "/app.js", None).await;
    assert_eq!(
        response.headers().get("cache-control").unwrap(),
        "public, max-age=3600"
    );
}

// ── Helpers ──────────────────────────────────────────────────────

/// Create a temp directory with the given files.
fn tempdir_with_files(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let full = dir.path().join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, content).unwrap();
    }
    dir
}

/// Create a StaticFilesConfig pointing at the given directory.
fn config_for_dir(dir: &Path, spa_fallback: bool) -> StaticFilesConfig {
    StaticFilesConfig {
        enabled: true,
        root_path: Some(dir.to_str().unwrap().to_string()),
        index_file: "index.html".to_string(),
        spa_fallback,
        max_age: None,
        exclude_paths: vec![],
        allow_symlinks: false,
    }
}
