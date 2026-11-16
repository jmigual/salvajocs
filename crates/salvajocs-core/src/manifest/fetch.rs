//! Downloads the ludusavi manifest with `ETag` revalidation and keeps a JSON cache (ADR-0008).

use std::{
    fs::{self, File},
    io::{self, Write},
    path::Path,
    time::Duration,
};

use reqwest::{
    StatusCode,
    blocking::Client,
    header::{ETAG, IF_NONE_MATCH},
};

use super::{Manifest, ManifestError, from_json, parse_yaml, to_json};

/// Ludusavi's own manifest endpoint.
pub const MANIFEST_URL: &str = "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/refs/heads/data/data/manifest.yaml";

const JSON_FILE: &str = "manifest.json";
const ETAG_FILE: &str = "manifest.etag";
// The default 30 s total timeout is too short for 2.4 MB on a slow link.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Failure to refresh or read the manifest cache.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// The request failed or the server answered with an error status.
    #[error("manifest download failed")]
    Http(#[from] reqwest::Error),
    /// The cache directory could not be read or written.
    #[error("manifest cache I/O failed")]
    Io(#[from] io::Error),
    /// The downloaded YAML or the cached JSON is invalid.
    #[error("invalid manifest")]
    Manifest(#[from] ManifestError),
}

/// Outcome of a successful [`refresh`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// The server confirmed the cached copy is current.
    NotModified,
    /// A new manifest was downloaded, parsed and cached.
    Updated,
}

/// Revalidates the cache in `cache_dir` against [`MANIFEST_URL`], replacing it when it changed.
/// `cache_dir` holds `manifest.json` and `manifest.etag`; it is created if missing.
pub fn refresh(cache_dir: &Path) -> Result<Freshness, FetchError> {
    refresh_from(MANIFEST_URL, cache_dir)
}

/// Reads the cached manifest, or `None` when nothing was cached yet.
pub fn load_cached(cache_dir: &Path) -> Result<Option<Manifest>, FetchError> {
    match fs::read_to_string(cache_dir.join(JSON_FILE)) {
        Ok(json) => Ok(Some(from_json(&json)?)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn refresh_from(url: &str, cache_dir: &Path) -> Result<Freshness, FetchError> {
    let client = Client::builder().timeout(REQUEST_TIMEOUT).build()?;
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
    let json = to_json(&parse_yaml(&response.text()?)?)?;

    fs::create_dir_all(cache_dir)?;
    // The old ETag must not outlive a JSON it no longer describes.
    remove_if_exists(&cache_dir.join(ETAG_FILE))?;
    write_atomic(&cache_dir.join(JSON_FILE), json.as_bytes())?;
    if let Some(etag) = etag {
        write_atomic(&cache_dir.join(ETAG_FILE), etag.as_bytes())?;
    }
    Ok(Freshness::Updated)
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Writes via a synced sibling temp file so a crash never leaves a truncated cache.
fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let mut file = File::create(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests;
