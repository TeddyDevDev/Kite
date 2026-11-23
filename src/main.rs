// Sin consola en la versión release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::HashMap,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

use kite::{config, ddragon, lcu, stats};
use serde_json::Value;
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

slint::include_modules!();

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

fn position_es(pos: &str) -> &'static str {
    match pos {
        "top" => "TOP",
        "jungle" => "JUNGLA",
        "middle" => "MID",
        "bottom" => "ADC",
        "utility" => "SUPPORT",
        _ => "",
    }
}

type Pixels = Option<(u32, u32, Vec<u8>)>;
/// Icono listo para enviar al hilo de la interfaz: (nombre, píxeles)
type Icon = (String, Pixels);

fn to_icon((name, px): &Icon) -> ItemIcon {
    let icon = px
        .as_ref()
        .map(|(w, h, rgba)| Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, *w, *h)))
        .unwrap_or_default();
    ItemIcon { icon, name: name.clone().into() }
}

fn icons(v: &[Icon]) -> ModelRc<ItemIcon> {
    model(v.iter().map(to_icon).collect())
}

// ---------------------------------------------------------------------------
// Iconos de objetos (Data Dragon, nombres en inglés)
// ---------------------------------------------------------------------------

static DDRAGON: OnceLock<Option<ddragon::DDragon>> = OnceLock::new();
static ITEM_ICONS: OnceLock<Mutex<HashMap<i64, Pixels>>> = OnceLock::new();

fn ddragon_ready() -> Option<&'static ddragon::DDragon> {
    // Espera como mucho 20 s a que termine de cargar (solo la primera vez)
    for _ in 0..100 {
        if let Some(d) = DDRAGON.get() {
            return d.as_ref();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

fn item_icon(id: i64) -> Icon {
    let Some(dd) = ddragon_ready() else { return (format!("Item {id}"), None) };
    let cache = ITEM_ICONS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(px) = cache.lock().unwrap().get(&id) {
        return (dd.name(id), px.clone());
    }
    let px = dd.icon_path(id).and_then(|p| ddragon::decode_png(&p));
    cache.lock().unwrap().insert(id, px.clone());
    (dd.name(id), px)
}

// ---------------------------------------------------------------------------
// Iconos de runas y hechizos: salen del propio cliente de LoL (sin internet)
// ---------------------------------------------------------------------------

static LCU_INDEX: Mutex<Option<HashMap<i64, (String, String)>>> = Mutex::new(None);
static LCU_ICONS: OnceLock<Mutex<HashMap<i64, Pixels>>> = OnceLock::new();

fn lcu_index(client: &lcu::Client) -> HashMap<i64, (String, String)> {
    let mut guard = LCU_INDEX.lock().unwrap();
    if let Some(idx) = guard.as_ref() {
        return idx.clone();
    }
    let mut idx = HashMap::new();
    let mut add = |v: Option<Value>| {
        for e in v.as_ref().and_then(Value::as_array).into_iter().flatten() {
            if let (Some(id), Some(name), Some(icon)) = (e["id"].as_i64(), e["name"].as_str(), e["iconPath"].as_str()) {
                idx.insert(id, (name.to_string(), icon.to_string()));
            }
        }
    };
    add(client.get("/lol-game-data/assets/v1/perks.json"));
    add(client.get("/lol-game-data/assets/v1/perkstyles.json").map(|v| v["styles"].clone()));
    add(client.get("/lol-game-data/assets/v1/summoner-spells.json"));
    if !idx.is_empty() {
        *guard = Some(idx.clone());
    }
    idx
}

fn lcu_icon(client: &lcu::Client, idx: &HashMap<i64, (String, String)>, id: i64, name: Option<&str>) -> Icon {
    let (lcu_name, path) = idx.get(&id).cloned().unwrap_or_default();
    let name = name.map(str::to_string).filter(|n| !n.is_empty()).unwrap_or(lcu_name);
    let cache = LCU_ICONS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(px) = cache.lock().unwrap().get(&id) {
        return (name, px.clone());
    }
    let px = (!path.is_empty())
        .then(|| client.get_bytes(&path))
        .flatten()
        .and_then(|b| ddragon::decode_png_bytes(&b));
    if px.is_some() {
        cache.lock().unwrap().insert(id, px.clone());
    }
    (name, px)
}

struct RunesData {
    tab: String,
    source: String,
    keystone: Icon,
    primary_style: Icon,
    primary: Vec<Icon>,
    secondary_style: Icon,
    secondary: Vec<Icon>,
    shards: Vec<Icon>,
    spells: Vec<Icon>,
}

fn resolve_runes(client: &lcu::Client, page: &lcu::RunePage, tab: &str) -> RunesData {
    let idx = lcu_index(client);
    let names: Vec<&str> = page.perk_names.iter().map(String::as_str).collect();
    let rune = |i: usize| lcu_icon(client, &idx, page.perk_ids.get(i).copied().unwrap_or(0), names.get(i).copied());
    let style_name = |n: &str| n.split(" + ").map(str::to_string).collect::<Vec<_>>();
    let styles = style_name(&page.styles);
    RunesData {
        tab: tab.to_string(),
        source: page.source.clone(),
        keystone: rune(0),
        primary_style: lcu_icon(client, &idx, page.primary_style, styles.first().map(String::as_str)),
        primary: (1..4).map(rune).collect(),
        secondary_style: lcu_icon(client, &idx, page.sub_style, styles.get(1).map(String::as_str)),
        secondary: (4..6).map(rune).collect(),
        shards: (6..9).map(rune).collect(),
        spells: page
            .spell_ids
            .iter()
            .map(|s| lcu_icon(client, &idx, *s, Some(&lcu::spell_name(*s))))
            .collect(),
    }
}

fn apply_runes(app: &AppWindow, options: &[RunesData]) {
    let views: Vec<RunesView> = options
        .iter()
        .map(|r| RunesView {
            tab: r.tab.clone().into(),
            source: r.source.clone().into(),
            keystone: to_icon(&r.keystone),
            primary_style: to_icon(&r.primary_style),
            primary: icons(&r.primary),
            secondary_style: to_icon(&r.secondary_style),
            secondary: icons(&r.secondary),
            shards: icons(&r.shards),
            spells: icons(&r.spells),
        })
        .collect();
    if (app.get_selected_rune() as usize) >= views.len() {
        app.set_selected_rune(0);
    }
    app.set_rune_options(model(views));
}

fn clear_runes(app: &AppWindow) {
    app.set_rune_options(model(Vec::new()));
    app.set_selected_rune(0);
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

struct BuildView {
    info: String,
    cores: Vec<(Vec<Icon>, String)>,
    boots: Vec<Icon>,
    situational: Vec<Icon>,
    start: Vec<Icon>,
    skills: String,
}

fn resolve_build(b: &stats::Build) -> BuildView {
    let items = |ids: &[i64]| ids.iter().map(|id| item_icon(*id)).collect::<Vec<_>>();
    let pos = position_es(&b.position);
    BuildView {
        info: format!(
            "{}{} partidas{}",
            if b.patch.is_empty() { String::new() } else { format!("Parche {} · ", b.patch) },
            b.games,
            if pos.is_empty() { String::new() } else { format!(" · {pos}") }
        ),
        cores: b
            .cores
            .iter()
            .map(|c| (items(&c.items), format!("{:.0}% · {}", c.winrate * 100.0, c.games)))
            .collect(),
        boots: items(&b.boots.into_iter().collect::<Vec<_>>()),
        situational: items(&b.situational),
        start: items(&b.start),
        skills: b.skills.clone(),
    }
}

fn apply_build(app: &AppWindow, view: BuildView) {
    app.set_build_info(view.info.into());
    app.set_cores(model(
        view.cores
            .iter()
            .map(|(items, label)| CoreRow { items: icons(items), label: label.clone().into() })
            .collect(),
    ));
    app.set_boots(icons(&view.boots));
    app.set_situational(icons(&view.situational));
    app.set_start_items(icons(&view.start));
    app.set_skills(view.skills.into());
    app.set_has_build(true);
    app.set_loading(false);
}

/// Pinta una build y sus runas (de la caché o recién descargadas).
fn present(
    d: &kite::opgg::ChampData,
    high: Option<&kite::opgg::ChampData>,
    req: &lcu::BuildRequest,
    shared: &Arc<lcu::Shared>,
    weak: &slint::Weak<AppWindow>,
    build_gen: &Arc<AtomicU64>,
    my_gen: u64,
) {
    let still_same = shared.current.lock().unwrap().as_ref() == Some(&(req.champion_id, req.position.clone()));
    if !still_same || build_gen.load(Ordering::SeqCst) != my_gen {
        return;
    }
    let view = resolve_build(&stats::from_opgg(d));
    let role = if req.position.is_empty() { d.position.clone() } else { req.position.clone() };

    // Runas: todas las ligas + Challenger (si es distinta)
    let mut pages: Vec<(lcu::RunePage, &str)> = Vec::new();
    if let Some(r) = &d.runes {
        pages.push((lcu::RunePage::from_opgg(r, &d.spells, "Todas las ligas"), "TODAS LAS LIGAS"));
    }
    if let Some(h) = high {
        if let Some(r) = &h.runes {
            let p = lcu::RunePage::from_opgg(r, &h.spells, "Challenger");
            if !pages.iter().any(|(q, _)| q.perk_ids == p.perk_ids && q.spell_ids == p.spell_ids) {
                pages.push((p, "CHALLENGER"));
            }
        }
    }

    // Si las runas han cambiado y ya se habían importado, se vuelven a importar
    // Cada candado por separado (nunca dos a la vez: evita bloqueos)
    let changed = {
        let mut current = shared.pages.lock().unwrap();
        let changed = current.first().map(|p| &p.perk_ids) != pages.first().map(|(p, _)| &p.perk_ids);
        *current = pages.iter().map(|(p, _)| p.clone()).collect();
        changed
    };
    if changed {
        *shared.imported_for.lock().unwrap() = None;
    }
    // El candado se suelta en esta línea (dentro de un `match` duraría hasta el final)
    let client = shared.client.lock().unwrap().clone();
    let runes_view: Vec<RunesData> = match client {
        Some(c) => pages.iter().map(|(p, tab)| resolve_runes(&c, p, tab)).collect(),
        None => Vec::new(),
    };
    let notice = lcu::try_auto_import(shared);
    if let Some(n) = &notice {
        kite::klog!("importación: {n}");
    }
    kite::klog!("build de {} pintada ({} opciones de runas)", req.name, pages.len());

    let gen = build_gen.clone();
    let _ = weak.upgrade_in_event_loop(move |app| {
        if gen.load(Ordering::SeqCst) != my_gen {
            return;
        }
        app.set_role(role.into());
        if !runes_view.is_empty() {
            apply_runes(&app, &runes_view);
        }
        if let Some(n) = &notice {
            set_notice(&app, n);
        }
        apply_build(&app, view);
    });
}

/// Pide la build (y las runas) en un hilo aparte:
/// 1) enseña al instante lo que haya en caché, aunque sea antiguo;
/// 2) si no está al día, lo descarga de OP.GG y actualiza la ventana.
fn spawn_build(
    request: Option<lcu::BuildRequest>,
    shared: Arc<lcu::Shared>,
    weak: slint::Weak<AppWindow>,
    build_gen: Arc<AtomicU64>,
) {
    use kite::opgg::{cached, champion_data, Lane, HIGH_ELO};
    let my_gen = build_gen.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let Some(req) = request else {
            let _ = weak.upgrade_in_event_loop(move |app| {
                if build_gen.load(Ordering::SeqCst) == my_gen {
                    app.set_has_build(false);
                    app.set_loading(false);
                    app.set_refreshing(false);
                    clear_runes(&app);
                    app.set_build_hint("".into());
                    app.set_role("".into());
                }
            });
            return;
        };

        kite::opgg::set_wanted(req.champion_id);
        // 1) Caché (instantáneo)
        let old = cached(req.champion_id, &req.position, None);
        kite::klog!(
            "build de {} ({:?}): caché {}",
            req.name,
            req.position,
            match &old { Some((_, true)) => "al día", Some((_, false)) => "antigua", None => "no hay" }
        );
        let old_high = old.as_ref().and_then(|(d, _)| cached(req.champion_id, &d.position, Some(HIGH_ELO)));
        let pos = req.position.clone();
        let has_old = old.is_some();
        let _ = weak.upgrade_in_event_loop(move |app| {
            app.set_role(pos.into());
            if !has_old {
                app.set_has_build(false);
                clear_runes(&app);
                app.set_build_hint("".into());
                app.set_loading(true);
            }
        });
        if let Some((d, _)) = &old {
            present(d, old_high.as_ref().map(|(h, _)| h), &req, &shared, &weak, &build_gen, my_gen);
        }
        let main_fresh = old.as_ref().is_some_and(|(_, fresh)| *fresh);
        let high_fresh = old_high.as_ref().is_some_and(|(_, fresh)| *fresh);
        if main_fresh && high_fresh {
            return;
        }
        // Se está enseñando una build guardada antigua: avisar de que se actualiza
        if has_old && !main_fresh {
            let gen = build_gen.clone();
            let _ = weak.upgrade_in_event_loop(move |app| {
                if gen.load(Ordering::SeqCst) == my_gen {
                    app.set_refreshing(true);
                }
            });
        }

        // 2) Descarga lo que no esté al día y vuelve a pintar
        let data = if main_fresh {
            Ok(old.as_ref().unwrap().0.clone())
        } else {
            champion_data(Lane::Interactive, req.champion_id, &req.name, &req.alias, &req.position, None)
        };
        let stop_refreshing = |weak: &slint::Weak<AppWindow>| {
            let gen = build_gen.clone();
            let _ = weak.upgrade_in_event_loop(move |app| {
                if gen.load(Ordering::SeqCst) == my_gen {
                    app.set_refreshing(false);
                }
            });
        };
        let d = match data {
            Ok(d) => d,
            Err(e) => {
                kite::klog!("build de {}: error de OP.GG: {e}", req.name);
                stop_refreshing(&weak);
                if !has_old {
                    let _ = weak.upgrade_in_event_loop(move |app| {
                        if build_gen.load(Ordering::SeqCst) == my_gen {
                            app.set_has_build(false);
                            app.set_loading(false);
                            app.set_build_hint(format!("OP.GG no respondió: {e}").into());
                        }
                    });
                }
                return;
            }
        };
        if !main_fresh {
            present(&d, old_high.as_ref().map(|(h, _)| h), &req, &shared, &weak, &build_gen, my_gen);
        }
        stop_refreshing(&weak);
        if !high_fresh {
            if let Ok(h) = champion_data(Lane::Interactive, req.champion_id, &req.name, &req.alias, &d.position, Some(HIGH_ELO)) {
                present(&d, Some(&h), &req, &shared, &weak, &build_gen, my_gen);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Arte del campeón (del propio cliente): retrato PNG y splash JPG reducido
// ---------------------------------------------------------------------------

static ART_FOR: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

fn rgba_image(px: &(u32, u32, Vec<u8>)) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&px.2, px.0, px.1))
}

/// JPG -> RGBA, reducido a la mitad de ancho máximo 640 px (menos memoria, igual de nítido en la ventana).
fn decode_splash(bytes: &[u8]) -> Pixels {
    let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    let pixels = dec.decode().ok()?;
    let info = dec.info()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => 3,
        jpeg_decoder::PixelFormat::L8 => 1,
        _ => return None,
    };
    let step = (w / 640).max(1);
    let (nw, nh) = (w / step, h / step);
    let mut out = Vec::with_capacity(nw * nh * 4);
    for y in 0..nh {
        for x in 0..nw {
            let i = ((y * step) * w + x * step) * channels;
            if channels == 3 {
                out.extend_from_slice(&[pixels[i], pixels[i + 1], pixels[i + 2], 255]);
            } else {
                out.extend_from_slice(&[pixels[i], pixels[i], pixels[i], 255]);
            }
        }
    }
    Some((nw as u32, nh as u32, out))
}

fn load_champion_art(id: i64, shared: &Arc<lcu::Shared>, weak: &slint::Weak<AppWindow>) {
    if ART_FOR.swap(id, Ordering::SeqCst) == id {
        return;
    }
    let Some(client) = shared.client.lock().unwrap().clone() else { return };
    let weak = weak.clone();
    std::thread::spawn(move || {
        let icon = client
            .get_bytes(&format!("/lol-game-data/assets/v1/champion-icons/{id}.png"))
            .and_then(|b| ddragon::decode_png_bytes(&b));
        // La ruta exacta del splash la da el propio cliente; si falla, Data Dragon (oficial de Riot)
        let info = client.get(&format!("/lol-game-data/assets/v1/champions/{id}.json"));
        let skin = info.as_ref().map(|v| v["skins"][0].clone()).unwrap_or_default();
        let mut splash = None;
        for key in ["uncenteredSplashPath", "splashPath", "loadScreenPath"] {
            if let Some(path) = skin[key].as_str() {
                splash = client.get_bytes(path).and_then(|b| decode_splash(&b));
                if splash.is_some() {
                    break;
                }
            }
        }
        if splash.is_none() {
            let alias = info.as_ref().and_then(|v| v["alias"].as_str().map(String::from)).unwrap_or_default();
            if !alias.is_empty() {
                splash = ddragon::fetch(&format!("/cdn/img/champion/splash/{alias}_0.jpg")).and_then(|b| decode_splash(&b));
            }
        }
        if icon.is_none() || splash.is_none() {
            kite::klog!("arte del campeón {id}: retrato {}, splash {}", icon.is_some(), splash.is_some());
        }
        let _ = weak.upgrade_in_event_loop(move |app| {
            if ART_FOR.load(Ordering::SeqCst) != id {
                return;
            }
            app.set_champ_icon(icon.as_ref().map(rgba_image).unwrap_or_default());
            app.set_splash(splash.as_ref().map(rgba_image).unwrap_or_default());
        });
    });
}

fn clear_art(app: &AppWindow) {
    ART_FOR.store(0, Ordering::SeqCst);
    app.set_champ_icon(Image::default());
    app.set_splash(Image::default());
}

fn set_notice(app: &AppWindow, msg: &str) {
    app.set_notice_error(msg.starts_with("No se pudieron"));
    app.set_notice(msg.into());
}

// ---------------------------------------------------------------------------

fn main() -> Result<(), slint::PlatformError> {
    let cfg = config::load();
    let shared = lcu::Shared::new(cfg.auto_import);
    kite::klog!("Kite {} iniciado", env!("CARGO_PKG_VERSION"));

    // Datos del parche (nombres de objetos en inglés + iconos) en segundo plano
    std::thread::spawn(|| {
        let _ = DDRAGON.set(ddragon::DDragon::load());
    });
    // Sesión con OP.GG abierta desde ya: la primera build no espera
    std::thread::spawn(kite::opgg::warm_up);

    // Diagnóstico de OP.GG: `cargo run -- --probar-opgg` (guarda respuestas en .\probe)
    if std::env::args().any(|a| a == "--probar-opgg") {
        kite::opgg::probe();
        return Ok(());
    }

    // Modo diagnóstico: `kite --debug` imprime los eventos en la consola, sin ventana
    if std::env::args().any(|a| a == "--debug") {
        lcu::diagnose();
        lcu::run(shared, |event| println!("{event:?}"));
    }

    let app = AppWindow::new()?;
    app.set_auto_import(cfg.auto_import);
    if let Some(px) = ddragon::decode_png_bytes(include_bytes!("../assets/kite-256.png")) {
        app.set_app_icon(rgba_image(&px));
    }

    // Interruptor de importación automática (se guarda al momento)
    {
        let shared = shared.clone();
        app.on_auto_import_toggled(move |on| {
            shared.auto_import.store(on, Ordering::Relaxed);
            let mut cfg = config::load();
            cfg.auto_import = on;
            config::save(&cfg);
        });
    }

    // Botón "Importar"
    {
        let shared = shared.clone();
        let weak = app.as_weak();
        app.on_import_runes(move |index| {
            let client = shared.client.lock().unwrap().clone();
            let page = shared.pages.lock().unwrap().get(index as usize).cloned();
            let (champ, pos) = shared.champion.lock().unwrap().clone();
            let (Some(client), Some(page)) = (client, page) else { return };
            let weak = weak.clone();
            // En otro hilo para que la ventana nunca se congele
            std::thread::spawn(move || {
                let msg = lcu::import_page(&client, &page, format!("{champ} {pos}").trim());
                let _ = weak.upgrade_in_event_loop(move |app| set_notice(&app, &msg));
            });
        });
    }

    // Actualización automática desde la carpeta del proyecto
    let update_timer = slint::Timer::default();
    if let Some(dir) = kite::updater::project_dir() {
        kite::updater::cleanup(&dir);
        let start_update = {
            let weak = app.as_weak();
            let dir = dir.clone();
            move || {
                let weak = weak.clone();
                let dir = dir.clone();
                let _ = weak.upgrade_in_event_loop(|app| {
                    app.set_update_error(false);
                    app.set_update_busy(true);
                    app.set_update_text("Actualizando Kite… compilando la versión nueva (puede tardar unos minutos)".into());
                });
                std::thread::spawn(move || {
                    let result = kite::updater::build(&dir).and_then(|_| kite::updater::swap_and_restart(&dir));
                    // Si llega aquí es que algo ha fallado (si va bien, Kite se reinicia)
                    if let Err(e) = result {
                        let _ = weak.upgrade_in_event_loop(move |app| {
                            app.set_update_busy(false);
                            app.set_update_error(true);
                            app.set_update_text(format!("No se pudo actualizar: {e}").into());
                        });
                    }
                });
            }
        };
        // Al abrir: si hay cambios, se actualiza solo
        if kite::updater::needs_update(&dir) {
            start_update();
        }
        app.on_apply_update({
            let start_update = start_update.clone();
            move || start_update()
        });
        // Con Kite abierto: cada 2 min se mira si hay cambios y se ofrece el botón (sin cortarte la partida)
        let weak = app.as_weak();
        update_timer.start(slint::TimerMode::Repeated, Duration::from_secs(120), move || {
            let Some(app) = weak.upgrade() else { return };
            if !app.get_update_busy() && app.get_update_text().is_empty() && kite::updater::needs_update(&dir) {
                app.set_update_text("Hay una versión nueva de Kite".into());
            }
        });
    }

    // Progreso de la precarga: se mira 2 veces por segundo (solo repinta si cambia)
    let sync_timer = slint::Timer::default();
    {
        let weak = app.as_weak();
        sync_timer.start(slint::TimerMode::Repeated, Duration::from_millis(500), move || {
            let Some(app) = weak.upgrade() else { return };
            let (done, total) = kite::opgg::prefetch_progress();
            let text: slint::SharedString = if total > 0 {
                format!("Descargando archivos · {done}/{total}").into()
            } else {
                "".into()
            };
            if app.get_sync_text() != text {
                app.set_sync_text(text);
            }
            let p = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            if (app.get_sync_progress() - p).abs() > f32::EPSILON {
                app.set_sync_progress(p);
            }
        });
    }

    // Contador para descartar respuestas viejas si cambias de campeón o rol rápido
    let build_gen = Arc::new(AtomicU64::new(0));

    // Selector de rol: repite la consulta con otra línea
    {
        let shared = shared.clone();
        let weak = app.as_weak();
        let build_gen = build_gen.clone();
        app.on_select_role(move |role| {
            let Some(mut req) = shared.last_request.lock().unwrap().clone() else { return };
            req.position = role.to_string();
            *shared.last_request.lock().unwrap() = Some(req.clone());
            *shared.current.lock().unwrap() = Some((req.champion_id, req.position.clone()));
            // Si ya habías bloqueado, se reimportan las runas del nuevo rol
            *shared.imported_for.lock().unwrap() = None;
            spawn_build(Some(req), shared.clone(), weak.clone(), build_gen.clone());
        });
    }

    // Hilo de la LCU: duerme mientras no hay eventos
    let weak = app.as_weak();
    let shared_ui = shared.clone();
    std::thread::spawn(move || {
        lcu::run(shared, move |event| {
            if let lcu::Event::BuildFor(request) = event {
                spawn_build(request, shared_ui.clone(), weak.clone(), build_gen.clone());
                return;
            }
            if let lcu::Event::MyPick { champion_id, .. } = &event {
                load_champion_art(*champion_id, &shared_ui, &weak);
            }

            let _ = weak.upgrade_in_event_loop(move |app| match event {
                lcu::Event::Connected => app.set_connected(true),
                lcu::Event::Disconnected => {
                    app.set_connected(false);
                    app.set_phase("Esperando a League of Legends…".into());
                    app.set_champion("".into());
                    app.set_position("".into());
                    clear_runes(&app);
                    app.set_notice("".into());
                    app.set_has_build(false);
                    app.set_loading(false);
                    app.set_build_hint("".into());
                    clear_art(&app);
                }
                lcu::Event::Phase(phase) => {
                    // Antes de la partida se limpia; en partida se mantiene tu campeón y tu build
                    let keep = ["Selección de campeones", "En partida", "Reconectando a la partida", "Fin de partida"];
                    if !keep.contains(&phase.as_str()) {
                        clear_art(&app);
                        app.set_champion("".into());
                        app.set_position("".into());
                        app.set_notice("".into());
                        app.set_build_hint("".into());
                    }
                    app.set_phase(phase.into());
                }
                lcu::Event::MyPick { champion, position, .. } => {
                    app.set_champion(champion.into());
                    app.set_position(position.into());
                }
                lcu::Event::Runes(pages) => {
                    if pages.is_empty() {
                        clear_runes(&app);
                    }
                }
                lcu::Event::Notice(msg) => set_notice(&app, &msg),
                lcu::Event::BuildFor(_) => {}
            });
        })
    });

    app.run()
}
