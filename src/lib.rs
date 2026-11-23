//! Kite: módulos de la app.

/// Escribe una línea en la consola y en %APPDATA%\\Kite\\kite.log.
#[macro_export]
macro_rules! klog {
    ($($t:tt)*) => {{ $crate::config::log_line(&format!($($t)*)); }};
}

pub mod config;
pub mod ddragon;
pub mod lcu;
pub mod opgg;
pub mod stats;
pub mod updater;
