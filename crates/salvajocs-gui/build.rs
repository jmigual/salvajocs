use std::path::Path;
use std::process::Command;

#[path = "build/vcs_info.rs"]
mod vcs_info;

fn main() {
    // Gate on the target, not the host: a build script always compiles for the host. The
    // dependency is not host-gated so that a cross build to Windows embeds the icon too.
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        // Version info is taken from the package metadata, whose name is lowercase; Task Manager
        // and the taskbar show FileDescription.
        winresource::WindowsResource::new()
            .set("FileDescription", "Salvajocs")
            .set("ProductName", "Salvajocs")
            .set_icon("assets/salvajocs.ico")
            .compile()
            .expect("failed to embed the Windows icon");
    }
    // Any rerun-if-changed turns off cargo's scan of the whole package, so list the icon too.
    println!("cargo:rerun-if-changed=assets/salvajocs.ico");
    emit_build_info();
}

/// Passes the commit, rustc and rustic_core version to the crate as `SALVAJOCS_*` variables. A
/// value that cannot be found is left unset. None of them names the machine or its paths.
fn emit_build_info() {
    if let Some((commit, date)) = commit() {
        println!("cargo:rustc-env=SALVAJOCS_COMMIT={commit}");
        if let Some(date) = date {
            println!("cargo:rustc-env=SALVAJOCS_COMMIT_DATE={date}");
        }
    }
    if let Some(rustc) = rustc_version() {
        println!("cargo:rustc-env=SALVAJOCS_RUSTC={rustc}");
    }
    if let Some(version) = rustic_core_version() {
        println!("cargo:rustc-env=SALVAJOCS_RUSTIC_CORE={version}");
    }
}

/// The short commit, with " (modified)" when tracked files differ from it, and the commit date
/// when git gives it. A crates.io package carries `.cargo_vcs_info.json` and is read first, and
/// then git is not asked: the package can sit under an unrelated git checkout, such as a home
/// directory kept in git. "(modified)" can go stale in a dev build, as only HEAD is watched.
fn commit() -> Option<(String, Option<String>)> {
    let (sha, dirty, date) = match std::fs::read_to_string(".cargo_vcs_info.json") {
        Ok(json) => {
            let (sha, dirty) = vcs_info::parse(&json)?;
            (sha.chars().take(12).collect(), dirty, None)
        }
        Err(_) => {
            let sha = git(&["rev-parse", "--short=12", "HEAD"])?;
            watch_git_head();
            // Untracked files are left out: `cargo install --git` leaves a `.cargo-ok` marker.
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
            let date = git(&["log", "-1", "--format=%cs", "HEAD"]).filter(|d| is_date(d));
            (sha, dirty, date)
        }
    };
    let commit = if dirty {
        format!("{sha} (modified)")
    } else {
        sha
    };
    Some((commit, date))
}

/// Runs `git` with `args`; `None` when git is missing, fails or prints nothing. The options keep
/// the user's git config (signature display, colors, path quoting) out of the output, and the
/// build from rewriting the index.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(["--no-optional-locks", "-c", "log.showSignature=false"])
        .args(["-c", "color.ui=never", "-c", "core.quotepath=false"])
        .args(args)
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (output.status.success() && !text.is_empty()).then(|| text.to_owned())
}

/// Whether `text` has the form `YYYY-MM-DD`.
fn is_date(text: &str) -> bool {
    text.len() == 10
        && text.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        })
}

/// Reruns the script when HEAD moves, but only for the files that exist: a missing path would
/// make cargo rerun the script on every build.
fn watch_git_head() {
    let head_ref = git(&["symbolic-ref", "-q", "HEAD"]);
    let mut names = vec!["HEAD", "packed-refs"];
    names.extend(head_ref.as_deref());
    for name in names {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            && Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

/// The version line of the compiler cargo builds with, without the leading "rustc ".
fn rustc_version() -> Option<String> {
    let rustc = std::env::var_os("RUSTC")?;
    let output = Command::new(rustc).arg("--version").output().ok()?;
    let line = String::from_utf8(output.stdout).ok()?;
    let line = line.trim().strip_prefix("rustc ")?;
    output.status.success().then(|| line.to_owned())
}

/// The locked rustic_core version, best effort: a `cargo install` without `--locked` may resolve
/// differently. The root manifest pins it only in the workspace, and a packaged crate has no
/// workspace, so this reads the lock file `cargo package` puts next to this crate's manifest, then
/// the workspace's, which counts only when this crate is one of its members.
fn rustic_core_version() -> Option<String> {
    let in_workspace = Path::new("../../crates/salvajocs-gui/Cargo.toml").exists();
    let workspace = in_workspace.then_some("../../Cargo.lock");
    ["Cargo.lock"]
        .into_iter()
        .chain(workspace)
        .find_map(|path| {
            let lock = std::fs::read_to_string(path).ok()?;
            println!("cargo:rerun-if-changed={path}");
            let mut lines = lock.lines().map(str::trim);
            lines.find(|line| *line == "name = \"rustic_core\"")?;
            let version = lines.next()?.strip_prefix("version = \"")?;
            version.strip_suffix('"').map(str::to_owned)
        })
}
