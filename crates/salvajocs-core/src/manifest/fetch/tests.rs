use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Instant,
};

use super::*;

const SAMPLE: &str = include_str!("../../../tests/fixtures/manifest-sample.yaml");
const ETAG_VALUE: &str = "\"v1\"";

/// Serves `connections` requests: 304 when `If-None-Match` equals [`ETAG_VALUE`], otherwise
/// `status` with `body` and, if `with_etag`, the `ETag`. Returns the URL.
fn serve(connections: usize, status: &'static str, body: &'static str, with_etag: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/manifest.yaml", listener.local_addr().unwrap());
    thread::spawn(move || {
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut buf = [0; 1024];
            while !head.ends_with(b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap();
                head.extend_from_slice(&buf[..n]);
            }
            let request = String::from_utf8_lossy(&head).to_lowercase();
            let etag_header = if with_etag {
                format!("ETag: {ETAG_VALUE}\r\n")
            } else {
                String::new()
            };
            let reply = if request.contains(&format!("if-none-match: {ETAG_VALUE}")) {
                "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".to_owned()
            } else {
                format!(
                    "HTTP/1.1 {status}\r\n{etag_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            stream.write_all(reply.as_bytes()).unwrap();
        }
    });
    url
}

#[test]
fn load_cached_is_none_without_cache() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_cached(dir.path()).unwrap().is_none());
}

#[test]
fn load_cached_rejects_corrupt_json() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(JSON_FILE), "{").unwrap();
    assert!(matches!(
        load_cached(dir.path()),
        Err(FetchError::Manifest(ManifestError::Json(_)))
    ));
}

#[test]
fn refresh_caches_then_revalidates_with_etag() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("nested");
    let url = serve(2, "200 OK", SAMPLE, true);

    assert_eq!(
        refresh_from(&url, &cache, false, MAX_MANIFEST).unwrap(),
        Freshness::Updated
    );
    assert_eq!(
        fs::read_to_string(cache.join(ETAG_FILE)).unwrap(),
        ETAG_VALUE
    );
    let cached = load_cached(&cache).unwrap().unwrap();
    assert_eq!(cached, parse_yaml(SAMPLE).unwrap());

    assert_eq!(
        refresh_from(&url, &cache, false, MAX_MANIFEST).unwrap(),
        Freshness::NotModified
    );
    assert_eq!(load_cached(&cache).unwrap().unwrap(), cached);
}

#[test]
fn etag_without_json_is_not_sent() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(ETAG_FILE), ETAG_VALUE).unwrap();
    let url = serve(1, "200 OK", SAMPLE, true);
    assert_eq!(
        refresh_from(&url, dir.path(), false, MAX_MANIFEST).unwrap(),
        Freshness::Updated
    );
    assert!(load_cached(dir.path()).unwrap().is_some());
}

#[test]
fn old_cache_name_forces_full_fetch() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(OLD_JSON_FILE), "{}").unwrap();
    fs::write(dir.path().join(ETAG_FILE), ETAG_VALUE).unwrap();
    // The server answers 304 to this ETag, which would keep a cache without the new fields.
    let url = serve(1, "200 OK", "Game:\n  gog:\n    id: 5\n", true);

    assert_eq!(
        refresh_from(&url, dir.path(), false, MAX_MANIFEST).unwrap(),
        Freshness::Updated
    );
    assert!(!dir.path().join(OLD_JSON_FILE).exists());
    let cached = load_cached(dir.path()).unwrap().unwrap();
    assert_eq!(
        cached.games["Game"].gog,
        Some(crate::manifest::Gog { id: Some(5) })
    );
}

#[test]
fn update_without_etag_drops_stale_etag() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(JSON_FILE), "{\"games\":{}}").unwrap();
    fs::write(dir.path().join(ETAG_FILE), "\"old\"").unwrap();
    let url = serve(1, "200 OK", SAMPLE, false);
    assert_eq!(
        refresh_from(&url, dir.path(), false, MAX_MANIFEST).unwrap(),
        Freshness::Updated
    );
    assert!(!dir.path().join(ETAG_FILE).exists());
}

#[test]
fn error_status_leaves_cache_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let url = serve(1, "500 Internal Server Error", "", true);
    assert!(matches!(
        refresh_from(&url, dir.path(), false, MAX_MANIFEST),
        Err(FetchError::Http(_))
    ));
    assert!(load_cached(dir.path()).unwrap().is_none());
}

#[test]
fn invalid_yaml_leaves_cache_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let url = serve(1, "200 OK", "a: [", true);
    assert!(matches!(
        refresh_from(&url, dir.path(), false, MAX_MANIFEST),
        Err(FetchError::Manifest(ManifestError::Yaml(_)))
    ));
    assert!(!dir.path().join(JSON_FILE).exists());
    assert!(!dir.path().join(ETAG_FILE).exists());
}

#[test]
fn read_capped_refuses_bodies_over_the_limit() {
    assert_eq!(read_capped(Some(5), &b"hello"[..], 5).unwrap(), "hello");
    assert!(matches!(
        read_capped(Some(6), &b"hello!"[..], 5),
        Err(FetchError::TooLarge)
    ));
    assert!(matches!(
        read_capped(None, &b"hello!"[..], 5),
        Err(FetchError::TooLarge)
    ));
}

#[test]
fn a_download_over_the_limit_leaves_the_cache_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let url = serve(1, "200 OK", SAMPLE, true);
    assert!(matches!(
        refresh_from(&url, dir.path(), false, SAMPLE.len() as u64 - 1),
        Err(FetchError::TooLarge)
    ));
    assert!(!dir.path().join(JSON_FILE).exists());
    assert!(!dir.path().join(ETAG_FILE).exists());
}

#[test]
fn plain_http_is_refused_when_https_only() {
    let dir = tempfile::tempdir().unwrap();
    // The request is refused before connecting, so nothing needs to listen.
    let url = "http://127.0.0.1:9/manifest.yaml";
    assert!(matches!(
        refresh_from(url, dir.path(), true, MAX_MANIFEST),
        Err(FetchError::Http(err)) if err.is_builder()
    ));
}

#[test]
#[ignore = "downloads the real manifest"]
fn real_manifest_downloads_then_not_modified() {
    let dir = tempfile::tempdir().unwrap();

    let start = Instant::now();
    assert_eq!(refresh(dir.path()).unwrap(), Freshness::Updated);
    println!("download + parse + cache: {:?}", start.elapsed());

    let start = Instant::now();
    let manifest = load_cached(dir.path()).unwrap().unwrap();
    println!("load_cached: {:?}", start.elapsed());
    assert!(manifest.games.len() > 10_000);

    assert_eq!(refresh(dir.path()).unwrap(), Freshness::NotModified);
}
