//! Configuración persistente en %APPDATA%\Kite\config.json (JSON plano, sin dependencias extra).

use std::{fs, path::PathBuf};

use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Config {
    pub auto_import: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { auto_import: true }
    }
}

/// Carpeta de datos de Kite: %APPDATA%\Kite
pub fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Kite")
}

fn path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn load() -> Config {
    let mut cfg = Config::default();
    let Some(v) = fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return cfg;
    };
    if let Some(b) = v["auto_import"].as_bool() {
        cfg.auto_import = b;
    }
    cfg
}

pub fn save(cfg: &Config) {
    let _ = fs::create_dir_all(data_dir());
    let v = json!({ "auto_import": cfg.auto_import });
    let _ = fs::write(path(), serde_json::to_string_pretty(&v).unwrap_or_default());
}

// ---------------------------------------------------------------------------
// Registro: %APPDATA%\Kite\kite.log (se vacía al arrancar si pasa de 1 MB)
// ---------------------------------------------------------------------------

static LOG: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);

fn now_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let s = secs % 86_400;
    format!("{:02}:{:02}:{:02} UTC", s / 3600, (s / 60) % 60, s % 60)
}

pub fn log_line(msg: &str) {
    eprintln!("[kite] {msg}");
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let _ = fs::create_dir_all(data_dir());
        let path = data_dir().join("kite.log");
        let too_big = fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false);
        *guard = fs::OpenOptions::new()
            .create(true)
            .append(!too_big)
            .write(true)
            .truncate(too_big)
            .open(path)
            .ok();
    }
    if let Some(f) = guard.as_mut() {
        use std::io::Write;
        let _ = writeln!(f, "{} {msg}", now_hms());
    }
}
