//! Visual Diff — compare two images or videos.
//!
//!   visual-diff                         GUI
//!   visual-diff A B [original]          GUI with files opened (3rd file = original/reference)
//!   visual-diff --lang en|ru|uk ...     interface language for this run (not saved)
//!   visual-diff --x11 ...               force X11/XWayland for this run (not saved)
//!   visual-diff --wayland ...           force Wayland for this run (not saved)
//!   visual-diff --cli A B [out.png]     console mode

mod cli;
mod config;
mod diff;
mod gui;
mod i18n;
mod worker;

use config::{Backend, Config};
use i18n::Lang;

fn main() {
    let mut cfg = Config::load();

    // Pull our own flags out of the argument list; everything else is a file or the --cli switch.
    let mut args: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--lang" => {
                if let Some(l) = it.next().and_then(|c| Lang::from_code(&c)) {
                    cfg.lang = l;
                }
            }
            "--x11" => cfg.backend = Backend::X11,
            "--wayland" => cfg.backend = Backend::Wayland,
            _ => args.push(a),
        }
    }

    i18n::set_lang(cfg.lang);

    if args.first().map(String::as_str) == Some("--cli") {
        cli::run(&args[1..]);
        return;
    }

    // Must run before the window is created and before any threads are started.
    config::apply_backend(cfg.backend);

    gui::run(args, cfg);
}
