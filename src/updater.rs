//! Actualización automática desde la carpeta del proyecto (E:\kite).
//!
//! Kite.exe vive junto al código. Si algún archivo del código es más nuevo que el .exe,
//! se recompila en segundo plano (sin consola), se cambia el .exe y se reinicia.
//! Windows no deja sobrescribir un .exe en marcha, pero sí renombrarlo: el viejo pasa a
//! `Kite.old.exe` y se borra en el siguiente arranque.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

/// Carpeta del proyecto si Kite se ejecuta desde ella (no en `cargo run`, que va en target\).
pub fn project_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    (dir.join("Cargo.toml").exists() && dir.join("src").is_dir()).then_some(dir)
}

fn newest_in(path: &Path, newest: &mut Option<SystemTime>) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.is_dir() {
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            newest_in(&entry.path(), newest);
        }
    } else if let Ok(t) = meta.modified() {
        if newest.map_or(true, |n| t > n) {
            *newest = Some(t);
        }
    }
}

/// ¿Hay código más nuevo que el .exe en marcha?
pub fn needs_update(dir: &Path) -> bool {
    let Some(exe_time) = std::env::current_exe().ok().and_then(|e| fs::metadata(e).ok()?.modified().ok()) else {
        return false;
    };
    let mut newest = None;
    for p in ["Cargo.toml", "Cargo.lock", "build.rs", "src", "ui", "assets"] {
        newest_in(&dir.join(p), &mut newest);
    }
    newest.is_some_and(|n| n > exe_time)
}

/// Borra el .exe viejo de una actualización anterior.
pub fn cleanup(dir: &Path) {
    let _ = fs::remove_file(dir.join("Kite.old.exe"));
}

fn cargo_command() -> Command {
    // Desde un acceso directo el PATH puede no tener cargo: se usa la ruta de rustup si existe
    let home_cargo = std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join(".cargo").join("bin").join("cargo.exe"))
        .filter(|p| p.exists());
    #[allow(unused_mut)]
    let mut cmd = Command::new(home_cargo.unwrap_or_else(|| PathBuf::from("cargo")));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Compila la versión nueva. Si falla, deja los detalles en `update.log`.
pub fn build(dir: &Path) -> Result<(), String> {
    let out = cargo_command()
        .args(["build", "--release"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("no se encontró cargo ({e})"))?;
    let log = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = fs::write(dir.join("update.log"), &log);
    if out.status.success() {
        Ok(())
    } else {
        Err("no se pudo compilar (detalles en update.log)".into())
    }
}

/// Cambia el .exe por el recién compilado y abre la versión nueva. No vuelve si todo va bien.
pub fn swap_and_restart(dir: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = dir.join("target").join("release").join("kite.exe");
    if !new.exists() {
        return Err("no está el .exe nuevo".into());
    }
    let old = dir.join("Kite.old.exe");
    let _ = fs::remove_file(&old);
    fs::rename(&exe, &old).map_err(|e| format!("no se pudo apartar el .exe ({e})"))?;
    if let Err(e) = fs::copy(&new, &exe) {
        let _ = fs::rename(&old, &exe); // deshacer
        return Err(format!("no se pudo copiar el .exe nuevo ({e})"));
    }
    // Marca el .exe como recién actualizado para no volver a compilar en el siguiente arranque
    if let Ok(f) = fs::OpenOptions::new().write(true).open(&exe) {
        let _ = f.set_modified(SystemTime::now());
    }
    Command::new(&exe).current_dir(dir).spawn().map_err(|e| e.to_string())?;
    std::process::exit(0);
}
