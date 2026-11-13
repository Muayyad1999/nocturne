//! The web UI: compiled into the binary, or read from a directory while the
//! UI itself is being developed (`--ui-dir`).
//!
//! `build.rs` sets `cfg(nocturne_ui_built)` when `ui/dist` existed at compile
//! time. Without it the binary carries only the small page in `fallback-ui/`,
//! which says how to build the real thing.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH, LOCATION};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use percent_encoding::percent_decode_str;
use rust_embed::{EmbeddedFile, RustEmbed};
use sha2::{Digest, Sha256};

use crate::envelope::ApiError;
use crate::text::printable;

const INDEX: &str = "index.html";

/// Where the bundler puts files whose names contain a hash of their content.
const HASHED_ASSETS: &str = "assets/";

/// The page is tiny and its name never changes: always ask before reusing it.
const REVALIDATE: &str = "no-cache";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// A UI file is a few megabytes at most; a directory given by mistake may hold anything.
const MAX_DIRECTORY_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(RustEmbed)]
#[folder = "fallback-ui"]
struct FallbackUi;

// `allow_missing`: if `ui/dist` vanishes between the build script and this
// derive (the UI is being rebuilt), the build still succeeds and `embedded()`
// falls back at run time because there is no `index.html`.
#[cfg(nocturne_ui_built)]
#[derive(RustEmbed)]
#[folder = "../../ui/dist"]
#[allow_missing = true]
struct BuiltUi;

type Lookup = fn(&str) -> Option<EmbeddedFile>;

fn built_ui() -> Option<Lookup> {
    #[cfg(nocturne_ui_built)]
    if BuiltUi::get(INDEX).is_some() {
        return Some(BuiltUi::get);
    }
    None
}

/// Whether the real web UI was compiled into this binary. When it was not,
/// the server answers with a page that explains how to build it.
pub fn ui_is_embedded() -> bool {
    built_ui().is_some()
}

/// Where UI files come from.
#[derive(Clone)]
pub(crate) enum UiSource {
    Embedded(Lookup),
    /// A canonical directory path.
    Directory(PathBuf),
}

impl UiSource {
    /// The UI inside the binary: the real one if it was built, else the fallback page.
    pub(crate) fn embedded() -> UiSource {
        UiSource::Embedded(built_ui().unwrap_or(FallbackUi::get))
    }

    #[cfg(test)]
    pub(crate) fn fallback() -> UiSource {
        UiSource::Embedded(FallbackUi::get)
    }

    /// A UI build on disk.
    pub(crate) fn directory(dir: &Path) -> anyhow::Result<UiSource> {
        let canonical = dir
            .canonicalize()
            .with_context(|| format!("the UI directory `{}` cannot be opened", dir.display()))?;
        if !canonical.is_dir() {
            bail!("the UI directory `{}` is not a directory", dir.display());
        }
        if !canonical.join(INDEX).is_file() {
            tracing::warn!(
                directory = %canonical.display(),
                "the UI directory has no index.html yet; build the UI with `pnpm --dir ui build`"
            );
        }
        Ok(UiSource::Directory(canonical))
    }

    /// The file at `relative` (a path produced by [`relative_path`]).
    async fn file(&self, relative: &str) -> Option<Asset> {
        match self {
            UiSource::Embedded(lookup) => {
                let file = lookup(relative)?;
                let body = match file.data {
                    std::borrow::Cow::Borrowed(bytes) => Bytes::from_static(bytes),
                    std::borrow::Cow::Owned(bytes) => Bytes::from(bytes),
                };
                Some(Asset::new(relative, body, &file.metadata.sha256_hash()))
            }
            UiSource::Directory(base) => {
                let base = base.clone();
                let relative = relative.to_string();
                tokio::task::spawn_blocking(move || read_from_directory(&base, &relative))
                    .await
                    .ok()?
            }
        }
    }
}

fn read_from_directory(base: &Path, relative: &str) -> Option<Asset> {
    let mut path = base.to_path_buf();
    path.extend(relative.split('/'));
    // Resolving links (and, on Windows, device names) must not lead out of the directory.
    let path = path.canonicalize().ok()?;
    if !path.starts_with(base) {
        return None;
    }
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_DIRECTORY_FILE_BYTES {
        return None;
    }
    let body = std::fs::read(&path).ok()?;
    let digest = Sha256::digest(&body);
    Some(Asset::new(relative, Bytes::from(body), &digest))
}

/// A file ready to be sent.
struct Asset {
    body: Bytes,
    content_type: HeaderValue,
    etag: HeaderValue,
}

impl Asset {
    fn new(path: &str, body: Bytes, sha256: &[u8]) -> Asset {
        // 128 bits of the content hash are plenty to tell two versions of a file apart.
        let etag = format!("\"{}\"", hex::encode(&sha256[..16]));
        Asset {
            body,
            content_type: content_type(path),
            etag: HeaderValue::from_str(&etag).expect("hex is a valid header value"),
        }
    }
}

/// The `Content-Type` for a file name. Text types are declared as UTF-8,
/// which is what the bundler writes.
fn content_type(path: &str) -> HeaderValue {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let essence = mime.essence_str();
    let value = if mime.type_() == mime_guess::mime::TEXT || essence == "application/javascript" {
        format!("{essence}; charset=utf-8")
    } else {
        essence.to_string()
    };
    HeaderValue::from_str(&value)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
}

/// The file a request path names, relative to the UI root and with `/`
/// separators; empty for the root itself. `None` if the path cannot name a
/// file the UI is allowed to serve.
fn relative_path(uri_path: &str) -> Option<String> {
    let decoded = percent_decode_str(uri_path).decode_utf8().ok()?;
    let mut segments = Vec::new();
    for segment in decoded.split('/') {
        if segment.is_empty() {
            continue;
        }
        // No `.`/`..`, no hidden files, and nothing that is a separator, a
        // drive or a stream on some platform.
        if segment.starts_with('.') || segment.contains(['\\', ':', '\0']) {
            return None;
        }
        segments.push(segment);
    }
    Some(segments.join("/"))
}

enum Resolved {
    Asset(Asset, &'static str),
    /// Send the browser to `/`.
    Home,
    NotFound,
}

async fn resolve(ui: &UiSource, uri_path: &str) -> Resolved {
    let Some(relative) = relative_path(uri_path) else {
        return Resolved::NotFound;
    };
    if !relative.is_empty() {
        if let Some(asset) = ui.file(&relative).await {
            let cache = if relative.starts_with(HASHED_ASSETS) {
                IMMUTABLE
            } else {
                REVALIDATE
            };
            return Resolved::Asset(asset, cache);
        }
        // A missing script or image must be a 404, not the page: a browser
        // that asked for JavaScript cannot use HTML.
        let names_a_file = relative.starts_with(HASHED_ASSETS)
            || relative
                .rsplit('/')
                .next()
                .is_some_and(|name| name.contains('.'));
        if names_a_file {
            return Resolved::NotFound;
        }
        // Anything else is a route of the single-page app. Its asset and API
        // URLs are relative, so the page only works when loaded from the top
        // level: `/devices` is served in place, `/devices/42` is sent to `/`.
        let top_level = uri_path
            .strip_prefix('/')
            .is_some_and(|rest| !rest.contains('/'));
        if !top_level {
            return Resolved::Home;
        }
    }
    match ui.file(INDEX).await {
        Some(index) => Resolved::Asset(index, REVALIDATE),
        None => Resolved::NotFound,
    }
}

/// Serve the web UI for a request that matched no API route.
pub(crate) async fn serve(ui: &UiSource, request: Request) -> Response {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return ApiError::method_not_allowed(
            "GET, HEAD",
            format!(
                "the web interface is only read with GET; the API lives under /api (got {})",
                request.method()
            ),
        )
        .into_response();
    }
    let path = request.uri().path();
    match resolve(ui, path).await {
        Resolved::Asset(asset, cache) => respond(asset, cache, request.headers()),
        Resolved::Home => (StatusCode::FOUND, [(LOCATION, "/")]).into_response(),
        Resolved::NotFound => ApiError::not_found(format!(
            "the web interface has no file at `{}`",
            printable(path, 100)
        ))
        .into_response(),
    }
}

fn respond(asset: Asset, cache: &'static str, request_headers: &HeaderMap) -> Response {
    let unchanged = request_headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .any(|candidates| etag_matches(candidates, &asset.etag));
    let mut response = if unchanged {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(CONTENT_TYPE, asset.content_type)], asset.body).into_response()
    };
    let headers = response.headers_mut();
    headers.insert(ETAG, asset.etag);
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

/// Whether an `If-None-Match` value lists `etag` (weak comparison) or is `*`.
fn etag_matches(candidates: &HeaderValue, etag: &HeaderValue) -> bool {
    let Ok(candidates) = candidates.to_str() else {
        return false;
    };
    candidates.split(',').map(str::trim).any(|candidate| {
        candidate == "*"
            || candidate.strip_prefix("W/").unwrap_or(candidate).as_bytes() == etag.as_bytes()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn request_paths_map_to_relative_files() {
        let cases = [
            ("/", Some("")),
            ("/index.html", Some("index.html")),
            ("/assets/app-1a2b.js", Some("assets/app-1a2b.js")),
            ("//assets///app.js", Some("assets/app.js")),
            ("/assets/", Some("assets")),
            ("/with%20space.png", Some("with space.png")),
            ("/gr%C3%BC%C3%9Fe.txt", Some("grüße.txt")),
            ("/..", None),
            ("/../secret", None),
            ("/assets/../../secret", None),
            ("/%2e%2e/secret", None),
            ("/%2E%2E%2Fsecret", None),
            ("/assets/..%2f..%2fsecret", None),
            ("/assets/..%5c..%5csecret", None),
            ("/a%5cb", None),
            ("/./index.html", None),
            ("/.env", None),
            ("/.git/config", None),
            ("/assets/.hidden", None),
            ("/C:/Windows/win.ini", None),
            ("/file.txt::$DATA", None),
            ("/a%00b", None),
            ("/%ff", None),
        ];
        for (path, expected) in cases {
            assert_eq!(relative_path(path).as_deref(), expected, "{path}");
        }
    }

    #[test]
    fn content_types_cover_what_a_bundle_contains() {
        let cases = [
            ("index.html", "text/html; charset=utf-8"),
            ("assets/app-1a2b.js", "text/javascript; charset=utf-8"),
            ("assets/worker.mjs", "application/javascript; charset=utf-8"),
            ("assets/app.css", "text/css; charset=utf-8"),
            ("assets/inter.woff2", "font/woff2"),
            ("favicon.svg", "image/svg+xml"),
            ("favicon.ico", "image/x-icon"),
            ("logo.png", "image/png"),
            ("manifest.webmanifest", "application/manifest+json"),
            ("data.json", "application/json"),
            ("module.wasm", "application/wasm"),
            ("notes.txt", "text/plain; charset=utf-8"),
            ("unknown.zzz", "application/octet-stream"),
            ("no-extension", "application/octet-stream"),
        ];
        for (path, expected) in cases {
            assert_eq!(content_type(path), expected, "{path}");
        }
    }

    #[test]
    fn if_none_match_comparison() {
        let etag = HeaderValue::from_static("\"abc\"");
        for matching in [
            "\"abc\"",
            "W/\"abc\"",
            "\"x\", \"abc\"",
            "\"x\" ,W/\"abc\"",
            "*",
        ] {
            assert!(
                etag_matches(&HeaderValue::from_static(matching), &etag),
                "{matching}"
            );
        }
        for different in ["\"abd\"", "abc", "\"x\", \"y\"", ""] {
            assert!(
                !etag_matches(&HeaderValue::from_static(different), &etag),
                "{different}"
            );
        }
    }

    fn directory() -> (tempfile::TempDir, UiSource) {
        let root = tempfile::tempdir().unwrap();
        let ui = root.path().join("ui");
        fs::create_dir_all(ui.join("assets")).unwrap();
        fs::write(
            ui.join("index.html"),
            "<!doctype html><title>test ui</title>",
        )
        .unwrap();
        fs::write(ui.join("assets/app-1a2b.js"), "console.log('app')").unwrap();
        fs::write(ui.join("favicon.svg"), "<svg/>").unwrap();
        fs::write(root.path().join("secret.txt"), "outside the ui directory").unwrap();
        let source = UiSource::directory(&ui).unwrap();
        (root, source)
    }

    async fn body_of(resolved: Resolved) -> (String, &'static str) {
        match resolved {
            Resolved::Asset(asset, cache) => {
                (String::from_utf8(asset.body.to_vec()).unwrap(), cache)
            }
            Resolved::Home => panic!("unexpected redirect"),
            Resolved::NotFound => panic!("unexpected 404"),
        }
    }

    #[tokio::test]
    async fn files_routes_and_missing_files_resolve_differently() {
        let (_root, ui) = directory();
        let index = "<!doctype html><title>test ui</title>";

        assert_eq!(
            body_of(resolve(&ui, "/").await).await,
            (index.to_string(), "no-cache")
        );
        assert_eq!(
            body_of(resolve(&ui, "/index.html").await).await,
            (index.to_string(), "no-cache")
        );
        assert_eq!(
            body_of(resolve(&ui, "/assets/app-1a2b.js").await).await,
            (
                "console.log('app')".to_string(),
                "public, max-age=31536000, immutable"
            )
        );
        assert_eq!(
            body_of(resolve(&ui, "/favicon.svg").await).await,
            ("<svg/>".to_string(), "no-cache")
        );

        // A top-level route of the app is the page itself.
        for route in ["/devices", "/settings", "/assets"] {
            assert_eq!(
                body_of(resolve(&ui, route).await).await,
                (index.to_string(), "no-cache"),
                "{route}"
            );
        }
        // A nested one cannot work in place (relative URLs) and goes home.
        for route in ["/devices/42", "/devices/", "/a/b/c", "//devices"] {
            assert!(
                matches!(resolve(&ui, route).await, Resolved::Home),
                "{route}"
            );
        }
        // Things that name files are never answered with the page.
        for missing in [
            "/assets/missing-9f8e.js",
            "/assets/nested/missing",
            "/favicon.ico",
            "/robots.txt",
            "/devices/logo.png",
            "/../secret.txt",
            "/%2e%2e/secret.txt",
            "/.env",
        ] {
            assert!(
                matches!(resolve(&ui, missing).await, Resolved::NotFound),
                "{missing}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_out_of_the_directory_is_not_followed() {
        let (root, ui) = directory();
        std::os::unix::fs::symlink(
            root.path().join("secret.txt"),
            root.path().join("ui/leak.txt"),
        )
        .unwrap();
        assert!(matches!(
            resolve(&ui, "/leak.txt").await,
            Resolved::NotFound
        ));
    }

    #[tokio::test]
    async fn a_directory_without_an_index_serves_nothing_but_does_not_fail() {
        let empty = tempfile::tempdir().unwrap();
        let ui = UiSource::directory(empty.path()).unwrap();
        assert!(matches!(resolve(&ui, "/").await, Resolved::NotFound));
        assert!(matches!(resolve(&ui, "/devices").await, Resolved::NotFound));

        // It appears later (a build finished): it is picked up without a restart.
        fs::write(empty.path().join("index.html"), "now built").unwrap();
        assert_eq!(body_of(resolve(&ui, "/").await).await.0, "now built");
    }

    #[test]
    fn a_missing_or_wrong_ui_directory_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let missing = UiSource::directory(&root.path().join("nope"))
            .err()
            .unwrap();
        assert!(format!("{missing:#}").contains("nope"));
        let file = root.path().join("file");
        fs::write(&file, "x").unwrap();
        let not_a_directory = UiSource::directory(&file).err().unwrap();
        assert!(format!("{not_a_directory:#}").contains("not a directory"));
    }

    #[tokio::test]
    async fn the_fallback_page_explains_how_to_build_the_ui() {
        let ui = UiSource::fallback();
        let (page, cache) = body_of(resolve(&ui, "/").await).await;
        assert_eq!(cache, "no-cache");
        assert!(page.contains("<!doctype html>"), "{page}");
        assert!(page.contains("was not built into this binary"), "{page}");
        assert!(page.contains("pnpm --dir ui build"), "{page}");
        // The content security policy forbids inline scripts, so the page has none.
        assert!(!page.contains("<script"), "{page}");
        // It is the whole "UI": every route shows it, and there are no assets.
        assert_eq!(body_of(resolve(&ui, "/devices").await).await.0, page);
        assert!(matches!(
            resolve(&ui, "/assets/app.js").await,
            Resolved::NotFound
        ));
    }

    #[tokio::test]
    async fn the_embedded_ui_always_has_an_index() {
        let (page, _) = body_of(resolve(&UiSource::embedded(), "/").await).await;
        assert_eq!(
            page.contains("was not built into this binary"),
            !ui_is_embedded(),
            "the fallback page is served exactly when the real UI is not embedded"
        );
    }

    #[tokio::test]
    async fn etags_follow_the_content() {
        let (root, ui) = directory();
        let etag = |resolved| match resolved {
            Resolved::Asset(asset, _) => asset.etag,
            _ => panic!("expected a file"),
        };
        let first = etag(resolve(&ui, "/favicon.svg").await);
        assert_eq!(first, etag(resolve(&ui, "/favicon.svg").await));
        // The first 16 bytes of `printf '<svg/>' | sha256sum`, quoted.
        assert_eq!(first, "\"d4dc56669143034f31aa309635d4113d\"");
        fs::write(root.path().join("ui/favicon.svg"), "<svg></svg>").unwrap();
        assert_ne!(first, etag(resolve(&ui, "/favicon.svg").await));
    }
}
