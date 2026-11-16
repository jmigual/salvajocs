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
/// `status` with `body` and the `ETag`. Returns the URL.
fn serve(connections: usize, status: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/manifest.yaml", listener.local_addr().unwrap());
    thread::spawn(move || {
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0; 4096];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            let reply = if request.contains(&format!("if-none-match: {ETAG_VALUE}")) {
                "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".to_owned()
            } else {
                format!(
                    "HTTP/1.1 {status}\r\nETag: {ETAG_VALUE}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    let url = serve(2, "200 OK", SAMPLE);

    assert_eq!(refresh_from(&url, &cache).unwrap(), Freshness::Updated);
    assert_eq!(
        fs::read_to_string(cache.join(ETAG_FILE)).unwrap(),
        ETAG_VALUE
    );
    let cached = load_cached(&cache).unwrap().unwrap();
    assert_eq!(cached, parse_yaml(SAMPLE).unwrap());

    assert_eq!(refresh_from(&url, &cache).unwrap(), Freshness::NotModified);
    assert_eq!(load_cached(&cache).unwrap().unwrap(), cached);
}

#[test]
fn etag_without_json_is_not_sent() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(ETAG_FILE), ETAG_VALUE).unwrap();
    let url = serve(1, "200 OK", SAMPLE);
    assert_eq!(refresh_from(&url, dir.path()).unwrap(), Freshness::Updated);
    assert!(load_cached(dir.path()).unwrap().is_some());
}

#[test]
fn error_status_leaves_cache_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let url = serve(1, "500 Internal Server Error", "");
    assert!(matches!(
        refresh_from(&url, dir.path()),
        Err(FetchError::Http(_))
    ));
    assert!(load_cached(dir.path()).unwrap().is_none());
}

#[test]
fn invalid_yaml_leaves_cache_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let url = serve(1, "200 OK", "a: [");
    assert!(matches!(
        refresh_from(&url, dir.path()),
        Err(FetchError::Manifest(ManifestError::Yaml(_)))
    ));
    assert!(!dir.path().join(JSON_FILE).exists());
    assert!(!dir.path().join(ETAG_FILE).exists());
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
