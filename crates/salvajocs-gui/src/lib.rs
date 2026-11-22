//! egui front-end for salvajocs.

pub mod app;
pub mod env;
pub mod first_run;
pub mod instance;
pub mod job;
pub mod prompt;
pub mod startup;
pub mod state;
pub mod status_bar;
pub mod tabs;
pub mod theme;
pub mod title_bar;
pub mod update;
pub mod widgets;

pub use app::App;
pub use env::Env;

use std::path::{Path, PathBuf};

/// The default application ID; Wayland matches it against the `.desktop` file name to find the
/// icon. An AppImage started from an integrator's renamed `.desktop` file uses that name instead.
const APP_ID: &str = "io.github.jmigual.salvajocs";

/// The 256 px app icon.
const ICON_PNG: &[u8] = include_bytes!("../assets/io.github.jmigual.salvajocs.png");

/// Opens the main window and blocks until it is closed.
pub fn run() -> eframe::Result {
    let env = Env::system().ok_or_else(|| {
        eframe::Error::AppCreation("this OS has no config or cache directory".into())
    })?;
    // A cache directory that cannot take the lock file cannot take a backup either, which the
    // app reports itself; the lock is not a reason to refuse to start.
    let lock = match salvajocs_core::instance::lock(&env.cache_dir) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            return eframe::run_native(
                "Salvajocs",
                already_running_options(app_id())?,
                Box::new(|cc| {
                    theme::apply(&cc.egui_ctx);
                    Ok(Box::new(instance::AlreadyRunning))
                }),
            );
        }
        Err(err) => {
            eprintln!("salvajocs: could not take the instance lock: {err}");
            None
        }
    };
    eframe::run_native(
        "Salvajocs",
        native_options(app_id())?,
        Box::new(move |_cc| Ok(Box::new(App::new(env).with_instance_lock(lock)))),
    )
}

/// The app id: on Linux, the stem of the installed `.desktop` file that starts this AppImage
/// (Gear Lever, AppImageLauncher and appimaged rename it), otherwise [`APP_ID`].
fn app_id() -> String {
    if cfg!(target_os = "linux")
        && let Some(appimage) = std::env::var_os("APPIMAGE")
        && let Some(dir) = dirs::data_dir()
        && let Some(id) = installed_app_id(Path::new(&appimage), &dir.join("applications"))
    {
        return id;
    }
    APP_ID.to_owned()
}

/// The stem of the first `.desktop` file in `dir` (by file name) whose `Exec=` or `TryExec=`
/// runs `appimage`, or `None` if there is none or `dir` is unreadable.
fn installed_app_id(appimage: &Path, dir: &Path) -> Option<String> {
    // `$APPIMAGE` is a resolved path, while integrators may write it through a symlink such as
    // `/home -> var/home`; compare resolved paths.
    let appimage = std::fs::canonicalize(appimage).unwrap_or_else(|_| appimage.to_owned());
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "desktop"))
        .collect();
    files.sort();
    files.into_iter().find_map(|path| {
        let text = std::fs::read_to_string(&path).ok()?;
        if !runs_appimage(&text, &appimage) {
            return None;
        }
        path.file_stem()?.to_str().map(str::to_owned)
    })
}

/// Whether the `Exec=` or `TryExec=` line of the `[Desktop Entry]` group runs `appimage`: one of
/// the `Exec` arguments, or the whole `TryExec` value, is that path.
fn runs_appimage(desktop_file: &str, appimage: &Path) -> bool {
    let is_appimage = |path: &str| {
        let path = Path::new(path);
        path == appimage
            || (path.is_absolute() && std::fs::canonicalize(path).is_ok_and(|c| c == appimage))
    };
    let mut in_entry = false;
    desktop_file.lines().any(|line| {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            return false;
        }
        in_entry
            && match (line.strip_prefix("Exec="), line.strip_prefix("TryExec=")) {
                (Some(command), _) => exec_args(command).iter().any(|arg| is_appimage(arg)),
                (_, Some(path)) => is_appimage(path.trim()),
                _ => false,
            }
    })
}

/// Splits an `Exec=` value into arguments as the Desktop Entry spec says: spaces and tabs
/// separate them outside quotes, `"` groups one, and `\` escapes the next character inside quotes.
fn exec_args(command: &str) -> Vec<String> {
    let (mut args, mut arg, mut quoted, mut chars) =
        (Vec::new(), String::new(), false, command.chars());
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '\\' if quoted => arg.extend(chars.next()),
            ' ' | '\t' if !quoted => {
                if !arg.is_empty() {
                    args.push(std::mem::take(&mut arg));
                }
            }
            _ => arg.push(c),
        }
    }
    args.push(arg);
    args
}

/// The size, in points, the window opens at. eframe shrinks it to the largest monitor, and does
/// not leave room for a taskbar, hence the modest height.
const WINDOW_SIZE: [f32; 2] = [1100.0, 640.0];
/// The smallest size the window can be resized to. The width keeps the top bar's tabs clear of
/// the window buttons.
pub const MIN_WINDOW_SIZE: [f32; 2] = [840.0, 480.0];

/// The window options: the default ones plus `app_id`, the embedded 256 px icon, no OS
/// decorations, as [`title_bar`] draws the bar, and a centered window sized for the app.
fn native_options(app_id: String) -> eframe::Result<eframe::NativeOptions> {
    let icon = eframe::icon_data::from_png_bytes(ICON_PNG)
        .map_err(|e| eframe::Error::AppCreation(Box::new(e)))?;
    let mut options = eframe::NativeOptions::default();
    options.viewport = options
        .viewport
        .with_app_id(app_id)
        .with_icon(icon)
        .with_decorations(false)
        .with_inner_size(WINDOW_SIZE)
        .with_min_inner_size(MIN_WINDOW_SIZE);
    options.centered = true;
    Ok(options)
}

/// The window options of the notice that another instance runs: a small, fixed-size window with
/// the OS decorations, and the same app id and icon.
fn already_running_options(app_id: String) -> eframe::Result<eframe::NativeOptions> {
    let icon = eframe::icon_data::from_png_bytes(ICON_PNG)
        .map_err(|e| eframe::Error::AppCreation(Box::new(e)))?;
    let mut options = eframe::NativeOptions::default();
    options.viewport = options
        .viewport
        .with_app_id(app_id)
        .with_icon(icon)
        .with_inner_size([440.0, 150.0])
        .with_resizable(false);
    options.centered = true;
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_options_set_the_app_id_icon_size_and_no_decorations() {
        let options = native_options(APP_ID.to_owned()).expect("the embedded icon decodes");
        assert_eq!(options.viewport.app_id.as_deref(), Some(APP_ID));
        let icon = options.viewport.icon.expect("an icon is set");
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(options.viewport.decorations, Some(false));
        assert_eq!(options.viewport.inner_size, Some(WINDOW_SIZE.into()));
        assert_eq!(
            options.viewport.min_inner_size,
            Some(MIN_WINDOW_SIZE.into())
        );
        assert!(options.centered);
    }

    const APPIMAGE: &str = "/home/u/AppImages/salvajocs.appimage";

    fn install(dir: &Path, name: &str, exec: &str) {
        let body = format!("[Desktop Entry]\nName=Salvajocs\nExec={exec}\n");
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn installed_app_id_matches_a_gear_lever_exec_line() {
        let dir = tempfile::tempdir().unwrap();
        install(
            dir.path(),
            "salvajocs.desktop",
            &format!("env DESKTOPINTEGRATION=1 {APPIMAGE}"),
        );
        assert_eq!(
            installed_app_id(Path::new(APPIMAGE), dir.path()).as_deref(),
            Some("salvajocs")
        );
    }

    #[test]
    fn installed_app_id_matches_a_quoted_path_with_field_codes() {
        let dir = tempfile::tempdir().unwrap();
        install(
            dir.path(),
            "appimagekit_abc-Salvajocs.desktop",
            &format!("\"{APPIMAGE}\" %U"),
        );
        assert_eq!(
            installed_app_id(Path::new(APPIMAGE), dir.path()).as_deref(),
            Some("appimagekit_abc-Salvajocs")
        );
    }

    #[test]
    fn installed_app_id_matches_a_quoted_path_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let spaced = "/home/u/My Apps/salvajocs.AppImage";
        install(dir.path(), "a.desktop", &format!("\"{spaced}\" %U"));
        install(
            dir.path(),
            "b.desktop",
            &format!("env DESKTOPINTEGRATION=1 \"{spaced}\""),
        );
        let only_try = "[Desktop Entry]\nTryExec=/home/u/My Apps/other.AppImage\n";
        std::fs::write(dir.path().join("c.desktop"), only_try).unwrap();
        assert_eq!(
            installed_app_id(Path::new(spaced), dir.path()).as_deref(),
            Some("a")
        );
        std::fs::remove_file(dir.path().join("a.desktop")).unwrap();
        assert_eq!(
            installed_app_id(Path::new(spaced), dir.path()).as_deref(),
            Some("b")
        );
        std::fs::remove_file(dir.path().join("b.desktop")).unwrap();
        let other = "/home/u/My Apps/other.AppImage";
        assert_eq!(
            installed_app_id(Path::new(other), dir.path()).as_deref(),
            Some("c")
        );
    }

    #[test]
    fn installed_app_id_ignores_exec_lines_of_other_groups() {
        let dir = tempfile::tempdir().unwrap();
        let body =
            format!("[Desktop Entry]\nExec=other\n\n[Desktop Action Remove]\nExec=rm {APPIMAGE}\n");
        std::fs::write(dir.path().join("a.desktop"), body).unwrap();
        assert_eq!(installed_app_id(Path::new(APPIMAGE), dir.path()), None);
    }

    #[cfg(unix)]
    #[test]
    fn installed_app_id_matches_a_path_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("real");
        std::fs::create_dir(&real_dir).unwrap();
        let real = real_dir.join("salvajocs.appimage");
        std::fs::write(&real, "").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();
        let apps = dir.path().join("apps");
        std::fs::create_dir(&apps).unwrap();
        install(
            &apps,
            "salvajocs.desktop",
            link.join("salvajocs.appimage").to_str().unwrap(),
        );
        let real = std::fs::canonicalize(real).unwrap();
        assert_eq!(installed_app_id(&real, &apps).as_deref(), Some("salvajocs"));
    }

    #[test]
    fn installed_app_id_ignores_other_and_prefix_paths() {
        let dir = tempfile::tempdir().unwrap();
        install(
            dir.path(),
            "other.desktop",
            "/home/u/AppImages/other.appimage",
        );
        install(dir.path(), "old.desktop", &format!("{APPIMAGE}.old"));
        assert_eq!(installed_app_id(Path::new(APPIMAGE), dir.path()), None);
    }

    #[test]
    fn installed_app_id_picks_the_first_file_by_name() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), "b.desktop", APPIMAGE);
        install(dir.path(), "a.desktop", APPIMAGE);
        assert_eq!(
            installed_app_id(Path::new(APPIMAGE), dir.path()).as_deref(),
            Some("a")
        );
    }

    #[test]
    fn installed_app_id_is_none_without_the_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            installed_app_id(Path::new(APPIMAGE), &dir.path().join("missing")),
            None
        );
    }
}
