//! The local game's files: `~/.snaketui/config` and `~/.snaketui/records`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use snake_core::config::GameConfig;
use snake_core::records::Records;

fn data_dir() -> PathBuf {
    let dir = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".snaketui");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn load_config() -> GameConfig {
    fs::read_to_string(data_dir().join("config"))
        .map(|text| GameConfig::from_saved_text(&text))
        .unwrap_or_default()
}

pub fn save_config(cfg: &GameConfig) {
    let _ = write_atomic(&data_dir().join("config"), &cfg.to_saved_text());
}

pub fn load_records() -> Records {
    fs::read_to_string(data_dir().join("records"))
        .map(|text| Records::from_text(&text))
        .unwrap_or_default()
}

pub fn save_records(records: &Records) {
    let _ = write_atomic(&data_dir().join("records"), &records.to_text());
}

/// Writes to a temporary file and renames it over `path`, so a crash
/// mid-write can't leave `path` truncated.
fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)
}
