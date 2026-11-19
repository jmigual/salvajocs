//! Starts `rclone serve restic` for an `rclone:` location, so rustic reaches it as a REST server.
//! salvajocs starts it itself to hide its console window on Windows (ADR-0003). Adapted from
//! rustic_backend 0.7.0 `src/rclone.rs` (Apache-2.0 OR MIT).

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use rustic_core::{CommandInput, Id};
use semver::{BuildMetadata, Prerelease, Version};

use super::StorageError;

/// How long rclone may take to print the address it serves on.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// How often rclone is checked for an exit once its stderr is closed.
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(20);
/// The text before the address in rclone's stderr.
const SERVING: &str = "Serving restic REST API on ";

/// A running `rclone serve restic`, killed when dropped.
pub(super) struct Rclone {
    child: Child,
}

impl Drop for Rclone {
    fn drop(&mut self) {
        // Errors mean it has already exited. The stderr thread is not joined: a grandchild still
        // holding the pipe would block the drop.
        _ = self.child.kill();
        _ = self.child.wait();
    }
}

impl Rclone {
    /// Starts rclone for `remote` (the location after `rclone:`) with rustic's rclone options
    /// (`rclone-command`, `use-password`, `rest-url`). Returns it with the REST URL to give rustic.
    pub(super) fn start(
        remote: &str,
        options: &BTreeMap<String, String>,
    ) -> Result<(Self, String), StorageError> {
        Self::start_with("rclone", remote, options, START_TIMEOUT)
    }

    /// [`Rclone::start`] with the program run when `rclone-command` is not set, and the timeout.
    fn start_with(
        program: &str,
        remote: &str,
        options: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<(Self, String), StorageError> {
        let use_password = match options.get("use-password") {
            None => true,
            Some(value) => value.parse().map_err(|_| {
                StorageError::Backend(format!("use-password must be true or false, not {value}"))
            })?,
        };
        let mut rclone_command = match options.get("rclone-command") {
            Some(command) => command.parse::<CommandInput>().map_err(|err| {
                StorageError::Backend(format!("rclone-command {command} is not valid: {err}"))
            })?,
            None => {
                // Older rclone ignores the user and password variables and serves without a login.
                if use_password {
                    check_version(&run_version(program)?)?;
                }
                CommandInput::from(
                    [program, "serve", "restic", "--addr", "localhost:0"]
                        .map(str::to_string)
                        .to_vec(),
                )
            }
        };
        rclone_command.append_arg(remote.to_string());

        let user = Id::random().to_hex().to_string();
        let password = Id::random().to_hex().to_string();
        let mut command = command(rclone_command.command());
        if use_password {
            _ = command
                .env("RCLONE_USER", &user)
                .env("RCLONE_PASS", &password);
        }
        // No stdin: rclone cannot ask for a config password where nobody sees the prompt.
        let mut child = command
            .args(rclone_command.args())
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(spawn_error)?;
        let lines = read_lines(child.stderr.take().expect("stderr is piped"));
        let mut rclone = Self { child };

        let url = match options.get("rest-url") {
            Some(url) => url.clone(),
            None => rclone.wait_for_url(&lines, timeout)?,
        };
        if !use_password {
            return Ok((rclone, url));
        }
        let address = url.strip_prefix("http://").ok_or_else(|| {
            StorageError::Backend(format!("the rclone URL {url} does not start with http://"))
        })?;
        Ok((rclone, format!("http://{user}:{password}@{address}")))
    }

    /// Waits up to `timeout` for the line with the address rclone serves on.
    fn wait_for_url(
        &mut self,
        lines: &mpsc::Receiver<String>,
        timeout: Duration,
    ) -> Result<String, StorageError> {
        let deadline = Instant::now() + timeout;
        let mut output = String::new();
        loop {
            match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) => match served_url(&line) {
                    Some(url) => return Ok(url.to_string()),
                    None => output.push_str(&line),
                },
                Err(RecvTimeoutError::Timeout) => break,
                // rclone can close stderr and keep running, so its exit is awaited only until the
                // deadline.
                Err(RecvTimeoutError::Disconnected) => {
                    while Instant::now() < deadline {
                        if let Some(status) = self.child.try_wait().map_err(spawn_error)? {
                            return Err(StorageError::RcloneFailed(match output.trim() {
                                "" => format!("rclone {status}"),
                                message => message.to_string(),
                            }));
                        }
                        std::thread::sleep(EXIT_POLL_INTERVAL);
                    }
                    break;
                }
            }
        }
        Err(StorageError::RcloneTimeout(
            timeout,
            output.trim().to_string(),
        ))
    }
}

/// A [`Command`] that opens no console window.
#[cfg(windows)]
fn command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;

    let mut command = Command::new(program);
    _ = command.creation_flags(super::CREATE_NO_WINDOW);
    command
}

/// A [`Command`] that opens no console window.
#[cfg(not(windows))]
fn command(program: &str) -> Command {
    Command::new(program)
}

fn spawn_error(err: io::Error) -> StorageError {
    if err.kind() == io::ErrorKind::NotFound {
        StorageError::RcloneMissing
    } else {
        StorageError::RcloneFailed(err.to_string())
    }
}

/// Sends each line rclone writes to stderr, and keeps reading after nobody listens, so rclone
/// never blocks on a full pipe.
fn read_lines(stderr: impl io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).split(b'\n') {
            let Ok(line) = line else { break };
            _ = send.send(String::from_utf8_lossy(&line).into_owned() + "\n");
        }
    });
    receive
}

/// The address in rclone's "Serving restic REST API on" line. rclone after 1.61 puts it in
/// brackets.
fn served_url(line: &str) -> Option<&str> {
    let start = line.find(SERVING)? + SERVING.len();
    Some(line[start..].trim_end().trim_matches(['[', ']']))
}

/// The stdout of `program version`.
fn run_version(program: &str) -> Result<String, StorageError> {
    let output = command(program)
        .arg("version")
        .stdin(Stdio::null())
        .output()
        .map_err(spawn_error)?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Fails unless the `rclone version` output names 1.52.2 or newer, the first version that reads
/// the user and password from the environment.
fn check_version(output: &str) -> Result<(), StorageError> {
    let text = output
        .lines()
        .next()
        .unwrap_or_default()
        .trim_start_matches(|c: char| !c.is_numeric());
    let mut version = Version::parse(text).map_err(|_| {
        StorageError::RcloneFailed(format!("could not read the rclone version from {output:?}"))
    })?;
    // A pre-release of 1.52.2 counts as 1.52.2, as rustic counts it.
    version.pre = Prerelease::EMPTY;
    version.build = BuildMetadata::EMPTY;
    if version >= Version::new(1, 52, 2) {
        Ok(())
    } else {
        Err(StorageError::RcloneFailed(format!(
            "rclone {version} is too old; 1.52.2 or newer is needed"
        )))
    }
}

#[cfg(test)]
mod tests;
