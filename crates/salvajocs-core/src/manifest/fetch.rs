//! Downloads the ludusavi manifest with `ETag` revalidation and keeps a JSON cache (ADR-0008).

use std::{fs, io, path::Path, time::Duration};

use reqwest::{
    StatusCode,
    blocking::Client,
    header::{ETAG, IF_NONE_MATCH},
};

use super::{Manifest, ManifestError, from_json, parse_yaml, to_json};
use crate::fsutil::{read_capped, remove_if_exists, write_atomic};

/// Ludusavi's own manifest endpoint.
pub const MANIFEST_URL: &str = "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/refs/heads/data/data/manifest.yaml";

// v2 added the GOG and cross-store ids; an older cache lacks them, so it is not read.
/// Name of the JSON cache file inside the cache dir.
pub const JSON_FILE: &str = "manifest-v2.json";
const OLD_JSON_FILE: &str = "manifest.json";
const ETAG_FILE: &str = "manifest.etag";
// The default 30 s total timeout is too short for 2.4 MB on a slow link.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
// The real manifest is a few MB; the cap only stops a hostile or broken server filling memory.
const MAX_MANIFEST: u64 = 64 * 1024 * 1024;

/// Failure to refresh or read the manifest cache.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FetchError {
    /// The request failed or the server answered with an error status.
    #[error("manifest download failed")]
    Http(#[from] reqwest::Error),
    /// The cache directory could not be read or written.
    #[error("manifest cache I/O failed")]
    Io(#[from] io::Error),
    /// The download is larger than the cap.
    #[error("the manifest is larger than {MAX_MANIFEST} bytes")]
    TooLarge,
    /// The downloaded YAML or the cached JSON is invalid.
    #[error("invalid manifest")]
    Manifest(#[from] ManifestError),
}

/// Outcome of a successful [`refresh`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Freshness {
    /// The server confirmed the cached copy is current.
    NotModified,
    /// A new manifest was downloaded, parsed and cached.
    Updated,
}

/// Revalidates the cache in `cache_dir` against [`MANIFEST_URL`], replacing it when it changed.
/// `cache_dir` holds `manifest-v2.json` and `manifest.etag`; it is created if missing.
pub fn refresh(cache_dir: &Path) -> Result<Freshness, FetchError> {
    refresh_from(MANIFEST_URL, cache_dir, true, MAX_MANIFEST)
}

/// Reads the cached manifest, or `None` when nothing was cached yet.
pub fn load_cached(cache_dir: &Path) -> Result<Option<Manifest>, FetchError> {
    match fs::read_to_string(cache_dir.join(JSON_FILE)) {
        Ok(json) => Ok(Some(from_json(&json)?)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// [`refresh`] of `url`, reading at most `limit` bytes; `https_only` and a small `limit` are
/// relaxed only by tests, which serve plain HTTP.
fn refresh_from(
    url: &str,
    cache_dir: &Path,
    https_only: bool,
    limit: u64,
) -> Result<Freshness, FetchError> {
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .https_only(https_only)
        .build()?;
    let mut request = client.get(url);
    // An ETag without its JSON would make a 304 leave us with no manifest.
    if cache_dir.join(JSON_FILE).is_file()
        && let Ok(etag) = fs::read_to_string(cache_dir.join(ETAG_FILE))
    {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let response = request.send()?;
    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(Freshness::NotModified);
    }
    let response = response.error_for_status()?;
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body =
        read_capped(response.content_length(), response, limit)?.ok_or(FetchError::TooLarge)?;
    let yaml = String::from_utf8_lossy(&body);
    let json = to_json(&parse_yaml(&yaml)?)?;

    fs::create_dir_all(cache_dir)?;
    // The old ETag must not outlive a JSON it no longer describes.
    remove_if_exists(&cache_dir.join(ETAG_FILE))?;
    write_atomic(&cache_dir.join(JSON_FILE), json.as_bytes())?;
    if let Some(etag) = etag {
        write_atomic(&cache_dir.join(ETAG_FILE), etag.as_bytes())?;
    }
    // A leftover old cache is harmless, so its removal must not fail the refresh.
    let _ = remove_if_exists(&cache_dir.join(OLD_JSON_FILE));
    Ok(Freshness::Updated)
}

#[cfg(test)]
mod tests;
