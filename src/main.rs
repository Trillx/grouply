#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]

mod command;
mod drag;
mod handoff;
mod layout;
mod model;
mod settings;
mod settings_ui;
mod update;

#[cfg(windows)]
mod platform;

#[cfg(windows)]
fn main() {
    if let Err(error) = platform::run() {
        platform::report_fatal(&error);
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Grouply manages Windows desktop windows and only runs on Windows.");
    std::process::exit(1);
}
