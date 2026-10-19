//! Настройки приложения и выбор графического сервера (Wayland / X11 через XWayland).
//!
//! Файл настроек: $XDG_CONFIG_HOME/compare-media/config.txt (или ~/.config/compare-media/config.txt),
//! простой формат "ключ=значение".

use crate::i18n::{tr, trf, Lang};
use std::{
    ffi::OsString,
    path::PathBuf,
    process::Command,
    sync::OnceLock,
};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Backend {
    Auto,
    X11,
    Wayland,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Auto => tr("Auto (default)"),
            Backend::X11 => tr("X11 (via XWayland)"),
            Backend::Wayland => "Wayland",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Backend::Auto => "auto",
            Backend::X11 => "x11",
            Backend::Wayland => "wayland",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub backend: Backend,
    pub theme: Theme,
    pub lang: Lang,
}

impl Default for Config {
    fn default() -> Self {
        Self { backend: Backend::Auto, theme: Theme::Dark, lang: Lang::En }
    }
}

fn config_path_in(dir: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join(dir).join("config.txt"))
}

fn config_path() -> Option<PathBuf> {
    config_path_in("compare-media")
}

/// Папка настроек из прежних версий (программа раньше называлась visual-diff).
fn legacy_config_path() -> Option<PathBuf> {
    config_path_in("visual-diff")
}

impl Config {
    pub fn load() -> Self {
        let mut cfg = Config::default();
        let Some(path) = config_path() else { return cfg };
        // Если новых настроек ещё нет, подхватываем старые (из папки visual-diff).
        let text = std::fs::read_to_string(&path).or_else(|e| match legacy_config_path() {
            Some(old) => std::fs::read_to_string(old),
            None => Err(e),
        });
        let Ok(text) = text else { return cfg };

        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match (k.trim(), v.trim()) {
                ("backend", "x11") => cfg.backend = Backend::X11,
                ("backend", "wayland") => cfg.backend = Backend::Wayland,
                ("backend", "auto") => cfg.backend = Backend::Auto,
                ("theme", "light") => cfg.theme = Theme::Light,
                ("theme", "dark") => cfg.theme = Theme::Dark,
                ("lang", code) => {
                    if let Some(l) = Lang::from_code(code) {
                        cfg.lang = l;
                    }
                }
                _ => {}
            }
        }
        cfg
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path().ok_or_else(|| tr("could not determine the settings folder").to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let theme = match self.theme {
            Theme::Dark => "dark",
            Theme::Light => "light",
        };
        let text = format!("backend={}\ntheme={}\nlang={}\n", self.backend.key(), theme, self.lang.code());
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

// Исходные значения переменных окружения (до наших изменений) — нужны при перезапуске.
static ORIGINAL_ENV: OnceLock<(Option<OsString>, Option<OsString>)> = OnceLock::new();

/// Применяет выбор графического сервера. Вызывать в начале main(), до создания окна.
///
/// winit (на котором работает eframe) выбирает Wayland, если задан WAYLAND_DISPLAY.
/// Чтобы принудительно использовать XWayland, мы скрываем Wayland-сокет от процесса.
pub fn apply_backend(backend: Backend) {
    let _ = ORIGINAL_ENV.set((
        std::env::var_os("WAYLAND_DISPLAY"),
        std::env::var_os("XDG_SESSION_TYPE"),
    ));

    match backend {
        Backend::Auto => {}
        Backend::X11 => {
            if std::env::var_os("DISPLAY").is_some() {
                std::env::remove_var("WAYLAND_DISPLAY");
                std::env::set_var("XDG_SESSION_TYPE", "x11");
                std::env::set_var("WINIT_UNIX_BACKEND", "x11");
            } else {
                eprintln!(
                    "{}",
                    tr("X11 selected but DISPLAY is not set (XWayland unavailable). Falling back to auto-detection.")
                );
            }
        }
        Backend::Wayland => {
            std::env::set_var("WINIT_UNIX_BACKEND", "wayland");
        }
    }
}

/// Краткое описание того, как сейчас запущено приложение (для окна настроек).
pub fn session_description() -> String {
    let wayland_session = ORIGINAL_ENV
        .get()
        .map(|(w, _)| w.is_some())
        .unwrap_or_else(|| std::env::var_os("WAYLAND_DISPLAY").is_some());
    let has_display = std::env::var_os("DISPLAY").is_some();

    let running = if std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("WINIT_UNIX_BACKEND").as_deref() == Ok("wayland")
    {
        "Wayland"
    } else if has_display {
        "X11 / XWayland"
    } else {
        tr("unknown")
    };

    trf(
        "running via: {0}\nWayland session: {1}\nDISPLAY (X11/XWayland): {2}",
        &[
            &running,
            &if wayland_session { tr("yes") } else { tr("no") },
            &if has_display { tr("available") } else { tr("not set") },
        ],
    )
}

/// Запускает новый экземпляр приложения с теми же файлами (чтобы применить смену бэкенда).
pub fn restart(files: &[String]) -> Result<(), String> {
    // Внутри AppImage current_exe() указывает на временную точку монтирования,
    // которая исчезнет при выходе родителя, поэтому берём путь к самому .AppImage.
    let exe = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .ok_or_else(|| tr("could not determine the program path").to_string())?;

    let mut cmd = Command::new(exe);
    cmd.args(files).env_remove("WINIT_UNIX_BACKEND");

    if let Some((wayland, session)) = ORIGINAL_ENV.get() {
        match wayland {
            Some(v) => {
                cmd.env("WAYLAND_DISPLAY", v);
            }
            None => {
                cmd.env_remove("WAYLAND_DISPLAY");
            }
        }
        match session {
            Some(v) => {
                cmd.env("XDG_SESSION_TYPE", v);
            }
            None => {
                cmd.env_remove("XDG_SESSION_TYPE");
            }
        }
    }

    cmd.spawn().map(|_| ()).map_err(|e| trf("failed to restart: {0}", &[&e]))
}
