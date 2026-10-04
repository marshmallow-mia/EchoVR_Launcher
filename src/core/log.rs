//! Logging to a per-user file (the Java app wrote `./log.log` into the working directory,
//! which is unwritable under Program Files, and never rotated it).

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub fn log_file(name: &str) -> PathBuf {
    super::paths::log_dir().join(name)
}

/// Starts logging to `<log dir>/<name>`, keeping one rotated `<name>.1`.
pub fn init(name: &str) {
    let path = log_file(name);
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "info,wgpu=warn,naga=warn,eframe=warn,egui_wgpu=warn,winit=warn",
        )
    });
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(false);
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => {
            let _ = builder.with_writer(Mutex::new(f)).try_init();
        }
        Err(_) => {
            let _ = builder.try_init();
        }
    }
    tracing::info!("---- {} starting ----", crate::version::VERSION_TITLE);
}
