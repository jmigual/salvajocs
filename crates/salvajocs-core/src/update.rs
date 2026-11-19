//! Finds a newer GitHub release and downloads its Windows installer or AppImage, checked against
//! the release's `SHA256SUMS` (ADR-0013, ADR-0022).

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{self, Command},
    time::Duration,
};

use reqwest::blocking::{Client, RequestBuilder};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The GitHub API endpoint of the latest published release.
const LATEST_URL: &str = "https://api.github.com/repos/jmigual/salvajocs/releases/latest";

/// The release asset that lists the SHA-256 of every other asset.
const SUMS_ASSET: &str = "SHA256SUMS";
/// Inno Setup's uninstaller, which the installer puts next to `salvajocs.exe`.
const UNINSTALLER: &str = "unins000.exe";
/// Installs without questions and starts the app again afterwards.
const SILENT_ARGS: [&str; 4] = [
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/RELAUNCH",
];
/// How long connecting to a server may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
// Blocking reqwest applies its timeout to the response headers and to each read, not to the whole
// body, so it is an idle limit: a 20 MB AppImage on a slow link still finishes.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// The largest response read; the installer and the AppImage are a few tens of MB.
const MAX_DOWNLOAD: u64 = 100 * 1024 * 1024;

/// Failure to check for, download or start an update.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UpdateError {
    /// The request failed or the server answered with an error status.
    #[error("update request failed")]
    Http(#[from] reqwest::Error),
    /// The release JSON is not what the GitHub API returns.
    #[error("invalid release JSON")]
    Json(#[from] serde_json::Error),
    /// A version is not `MAJOR.MINOR.PATCH`.
    #[error("invalid version")]
    Version(#[from] semver::Error),
    /// The release has no asset with this name.
    #[error("the release has no asset `{0}`")]
    MissingAsset(String),
    /// An asset is not downloaded from this repository's release of that version.
    #[error("the release asset URL `{0}` is not this release's")]
    ForeignUrl(String),
    /// A response is larger than any release file should be.
    #[error("the download is larger than {MAX_DOWNLOAD} bytes")]
    TooLarge,
    /// `SHA256SUMS` lists no hash for this file.
    #[error("SHA256SUMS has no entry for `{0}`")]
    MissingChecksum(String),
    /// The downloaded file does not match its hash in `SHA256SUMS`.
    #[error("`{0}` does not match its SHA-256 in SHA256SUMS")]
    ChecksumMismatch(String),
    /// The update could not be written or started.
    #[error("update I/O failed")]
    Io(#[from] io::Error),
}

/// One downloadable file of a release.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct Asset {
    /// The file name.
    pub name: String,
    /// Where to download it.
    pub browser_download_url: String,
}

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Release {
    /// The version, from the tag without its leading `v`.
    pub version: String,
    /// The release page.
    pub html_url: String,
    /// The release's files.
    pub assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    html_url: String,
    assets: Vec<Asset>,
}

impl Release {
    /// A release from its fields.
    pub fn new(version: String, html_url: String, assets: Vec<Asset>) -> Self {
        Self {
            version,
            html_url,
            assets,
        }
    }

    /// The name of the Windows installer of this release.
    fn installer_name(&self) -> String {
        format!("salvajocs-{}-x86_64-setup.exe", self.version)
    }

    /// The name of the Linux AppImage of this release.
    fn appimage_name(&self) -> String {
        format!("salvajocs-{}-x86_64.AppImage", self.version)
    }

    /// The asset called `name`, refused unless its URL is exactly this release's download URL.
    fn asset(&self, name: &str) -> Result<&Asset, UpdateError> {
        let asset = self
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .ok_or_else(|| UpdateError::MissingAsset(name.to_string()))?;
        // Exact, not a prefix: `../` or `%2e%2e` after a prefix would reach another repository.
        let expected = format!(
            "https://github.com/jmigual/salvajocs/releases/download/v{}/{name}",
            self.version
        );
        if asset.browser_download_url == expected {
            Ok(asset)
        } else {
            Err(UpdateError::ForeignUrl(asset.browser_download_url.clone()))
        }
    }
}

/// Reads a GitHub API release object.
fn parse_release(json: &str) -> Result<Release, UpdateError> {
    let release: ReleaseJson = serde_json::from_str(json)?;
    Ok(Release {
        version: release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name)
            .to_string(),
        html_url: release.html_url,
        assets: release.assets,
    })
}

/// Whether `latest` is a plain `MAJOR.MINOR.PATCH` higher than `current`; pre-releases and
/// build metadata never count as newer.
fn is_newer(current: &str, latest: &str) -> Result<bool, UpdateError> {
    let latest = Version::parse(latest)?;
    Ok(latest.pre.is_empty() && latest.build.is_empty() && latest > Version::parse(current)?)
}

/// The lowercase hex SHA-256 that `sums` (`sha256sum` output) lists for `file`.
fn checksum(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim_end().split_once(' ')?;
        // `sha256sum` separates with two spaces, or with " *" in binary mode.
        let name = name.trim_start_matches([' ', '*']);
        (name == file && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// Checks `bytes` against the hash `sums` lists for `file`.
fn verify(bytes: &[u8], sums: &str, file: &str) -> Result<(), UpdateError> {
    let expected =
        checksum(sums, file).ok_or_else(|| UpdateError::MissingChecksum(file.to_string()))?;
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual == expected {
        Ok(())
    } else {
        Err(UpdateError::ChecksumMismatch(file.to_string()))
    }
}

/// Whether the app in `exe_dir` was installed by the Windows installer, which leaves its
/// uninstaller there.
pub fn installed_by_installer(exe_dir: &Path) -> bool {
    exe_dir.join(UNINSTALLER).is_file()
}

/// Asks GitHub for the latest release; `None` when it is not newer than `current`.
pub fn check(current: &str) -> Result<Option<Release>, UpdateError> {
    let json = download(
        client(current)?
            .get(LATEST_URL)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json"),
    )?;
    let release = parse_release(&String::from_utf8_lossy(&json))?;
    Ok(is_newer(current, &release.version)?.then_some(release))
}

/// Empties `dir`, then downloads the installer of `release` into it after checking it against
/// the release's `SHA256SUMS`, and returns its path. Nothing is written when the check fails.
pub fn download_installer(
    release: &Release,
    current: &str,
    dir: &Path,
) -> Result<PathBuf, UpdateError> {
    // Installers of earlier updates would otherwise pile up.
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let name = release.installer_name();
    let bytes = download_verified(release, current, &name)?;
    fs::create_dir_all(dir)?;
    let path = dir.join(name);
    fs::write(&path, &bytes)?;
    Ok(path)
}

/// Starts the installer at `path` silently; it closes the running app and starts it again.
pub fn run_installer(path: &Path) -> Result<(), UpdateError> {
    Command::new(path).args(SILENT_ARGS).spawn()?;
    Ok(())
}

/// The AppImage that `exe` runs from: `appimage` (`$APPIMAGE`) when it is a file and `exe` lies
/// in `appdir` (`$APPDIR`, its mount). An app started by another AppImage inherits its variables.
pub fn running_appimage(exe: &Path, appimage: PathBuf, appdir: &Path) -> Option<PathBuf> {
    let inside = fs::canonicalize(appdir).is_ok_and(|appdir| exe.starts_with(appdir));
    (inside && appimage.is_file()).then_some(appimage)
}

/// Whether [`download_appimage`] can replace the file at `path`: it is a file and its folder
/// takes a new one.
pub fn replaceable(path: &Path) -> bool {
    let probe = temp_path(path);
    path.is_file()
        && OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .and_then(|_| fs::remove_file(&probe))
            .is_ok()
}

/// Downloads the AppImage of `release`, checks it against the release's `SHA256SUMS`, and
/// replaces the file at `appimage` with it in one rename, keeping its name. A failed download
/// or check leaves `appimage` untouched and writes nothing.
pub fn download_appimage(
    release: &Release,
    current: &str,
    appimage: &Path,
) -> Result<(), UpdateError> {
    let bytes = download_verified(release, current, &release.appimage_name())?;
    replace(appimage, &bytes)?;
    Ok(())
}

/// Starts the AppImage at `path` with this process's arguments.
pub fn restart_appimage(path: &Path) -> Result<(), UpdateError> {
    Command::new(path)
        .args(std::env::args_os().skip(1))
        .spawn()?;
    Ok(())
}

/// Downloads the asset `name` of `release` and checks it against the release's `SHA256SUMS`.
fn download_verified(release: &Release, current: &str, name: &str) -> Result<Vec<u8>, UpdateError> {
    let client = client(current)?;
    let asset = release.asset(name)?;
    let sums = release.asset(SUMS_ASSET)?;
    let sums = download(client.get(&sums.browser_download_url))?;
    let bytes = download(client.get(&asset.browser_download_url))?;
    verify(&bytes, &String::from_utf8_lossy(&sums), name)?;
    Ok(bytes)
}

/// A hidden file next to `path`; the same folder keeps the rename atomic.
fn temp_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".update-{}", process::id()));
    path.with_file_name(name)
}

/// Writes `bytes` to a new file next to `path`, with the old file's mode plus the owner's execute
/// bit, and renames it over `path`, so the running AppImage keeps reading the old file. The new
/// file is removed when a step fails.
fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    let mode = (fs::metadata(path)?.permissions().mode() & 0o777) | 0o100;
    let temp = temp_path(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        // Set, not created with: the umask would mask the mode passed to `open`.
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        // The step's error is the one worth reporting; a leftover hidden file is harmless.
        let _ = fs::remove_file(&temp);
    }
    result
}

fn client(current: &str) -> Result<Client, UpdateError> {
    Ok(Client::builder()
        .user_agent(format!("salvajocs/{current}"))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(READ_TIMEOUT)
        .https_only(true)
        .build()?)
}

/// Sends `request` and reads the body, up to [`MAX_DOWNLOAD`] bytes.
fn download(request: RequestBuilder) -> Result<Vec<u8>, UpdateError> {
    let response = request.send()?.error_for_status()?;
    read_capped(response.content_length(), response, MAX_DOWNLOAD)
}

/// Reads `body`, refusing it when the declared or the actual length exceeds `limit`.
fn read_capped(declared: Option<u64>, body: impl Read, limit: u64) -> Result<Vec<u8>, UpdateError> {
    if declared.is_some_and(|len| len > limit) {
        return Err(UpdateError::TooLarge);
    }
    let mut bytes = Vec::new();
    body.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(UpdateError::TooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
