//! Cliente mínimo del servidor MCP oficial de OP.GG (https://mcp-api.op.gg/mcp).
//! Solo peticiones HTTP con JSON: no se instala nada de OP.GG en el PC.

use std::time::Duration;

use serde_json::{json, Value};

/// `KITE_OPGG` solo sirve para pruebas con un servidor local.
fn endpoint() -> String {
    std::env::var("KITE_OPGG").unwrap_or_else(|_| "https://mcp-api.op.gg/mcp".into())
}

pub struct Mcp {
    agent: ureq::Agent,
    session: Option<String>,
    next_id: u64,
}

/// Las respuestas pueden llegar como JSON o como Server-Sent Events ("data: {...}").
fn parse_body(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    text.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .last()
}

impl Mcp {
    fn post(&mut self, body: &Value) -> Result<Option<Value>, String> {
        let mut req = self
            .agent
            .post(&endpoint())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-06-18");
        if let Some(s) = &self.session {
            req = req.header("Mcp-Session-Id", s);
        }
        let mut resp = req.send(body.to_string()).map_err(|e| format!("red: {e}"))?;
        if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            self.session = Some(s.to_string());
        }
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .with_config()
            .limit(20 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| format!("lectura: {e}"))?;
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status}: {}", text.chars().take(500).collect::<String>()));
        }
        Ok(parse_body(&text))
    }

    fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let body = json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params });
        let v = self.post(&body)?.ok_or("respuesta vacía")?;
        if let Some(err) = v.get("error") {
            return Err(format!("error MCP: {err}"));
        }
        Ok(v["result"].clone())
    }

    pub fn connect() -> Result<Self, String> {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .build()
            .into();
        let mut mcp = Self { agent, session: None, next_id: 0 };
        mcp.rpc(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "kite", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
        let _ = mcp.post(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        Ok(mcp)
    }

    pub fn list_tools(&mut self) -> Result<Value, String> {
        self.rpc("tools/list", json!({}))
    }

    /// Devuelve el `result` completo de la herramienta (content, structuredContent, isError...).
    pub fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
        self.rpc("tools/call", json!({ "name": tool, "arguments": args }))
    }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// Elige del `enum` del esquema el valor que mejor encaja con alguno de los deseados.
pub fn pick_enum(schema: &Value, wanted: &[&str]) -> Option<String> {
    let options: Vec<&str> = schema["enum"].as_array()?.iter().filter_map(Value::as_str).collect();
    for w in wanted {
        if let Some(o) = options.iter().find(|o| norm(o) == norm(w)) {
            return Some(o.to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Lector del formato de OP.GG:
//   class Nombre: campo1,campo2
//   ...
//   Nombre(valor1,[lista],"texto",Otro(1,2))
// Se convierte a JSON (objetos con los nombres de campo) para leerlo cómodamente.
// ---------------------------------------------------------------------------

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    classes: &'a std::collections::HashMap<String, Vec<String>>,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.skip_ws();
        match *self.s.get(self.i)? {
            b'"' => self.string(),
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_ws();
                    if self.s.get(self.i) == Some(&b']') {
                        self.i += 1;
                        return Some(Value::Array(items));
                    }
                    items.push(self.value()?);
                    self.skip_ws();
                    match self.s.get(self.i)? {
                        b',' => self.i += 1,
                        b']' => {}
                        _ => return None,
                    }
                }
            }
            c if c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                while self.i < self.s.len() && matches!(self.s[self.i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'-' | b'+') {
                    self.i += 1;
                }
                serde_json::from_slice(&self.s[start..self.i]).ok()
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
                    self.i += 1;
                }
                let ident = std::str::from_utf8(&self.s[start..self.i]).ok()?;
                self.skip_ws();
                if self.s.get(self.i) != Some(&b'(') {
                    return Some(match ident {
                        "true" | "True" => Value::Bool(true),
                        "false" | "False" => Value::Bool(false),
                        _ => Value::Null,
                    });
                }
                self.i += 1;
                let fields = self.classes.get(ident).cloned().unwrap_or_default();
                let mut obj = serde_json::Map::new();
                let mut n = 0;
                loop {
                    self.skip_ws();
                    if self.s.get(self.i) == Some(&b')') {
                        self.i += 1;
                        return Some(Value::Object(obj));
                    }
                    let v = self.value()?;
                    let key = fields.get(n).cloned().unwrap_or_else(|| format!("_{n}"));
                    obj.insert(key, v);
                    n += 1;
                    self.skip_ws();
                    match self.s.get(self.i)? {
                        b',' => self.i += 1,
                        b')' => {}
                        _ => return None,
                    }
                }
            }
            _ => None,
        }
    }

    fn string(&mut self) -> Option<Value> {
        let start = self.i;
        self.i += 1;
        while self.i < self.s.len() {
            match self.s[self.i] {
                b'\\' => self.i += 2,
                b'"' => {
                    self.i += 1;
                    return serde_json::from_slice(&self.s[start..self.i]).ok();
                }
                _ => self.i += 1,
            }
        }
        None
    }
}

/// Convierte la respuesta de texto de OP.GG en JSON. Si ya es JSON, la devuelve tal cual.
pub fn parse_notation(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    let (head, body) = text.split_once("\n\n")?;
    let classes = head
        .lines()
        .filter_map(|l| l.strip_prefix("class "))
        .filter_map(|l| l.split_once(':'))
        .map(|(name, fields)| {
            (name.trim().to_string(), fields.split(',').map(|f| f.trim().to_string()).collect())
        })
        .collect();
    Parser { s: body.trim().as_bytes(), i: 0, classes: &classes }.value()
}

// ---------------------------------------------------------------------------
// Datos de un campeón en una línea
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Runes {
    pub primary_style: i64,
    pub primary_name: String,
    pub sub_style: i64,
    pub sub_name: String,
    pub keystone: String,
    pub perk_ids: Vec<i64>,   // 4 principales + 2 secundarias + 3 fragmentos
    pub perk_names: Vec<String>, // nombres en inglés de las 6 runas (sin fragmentos)
    pub play: i64,
    pub win: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Matchup {
    pub champion_id: i64,
    pub name: String,
    pub my_win_rate: f64,
    pub play: i64,
}

#[derive(Clone, Debug, Default)]
pub struct ChampData {
    pub champion_id: i64,
    pub position: String, // formato LCU: top, jungle, middle, bottom, utility
    pub main_position: String, // la línea donde más se juega este campeón (formato OP.GG)
    pub patch: String,
    pub games: i64,
    pub win_rate: f64,
    pub core: Vec<i64>,
    pub core_play: i64,
    pub core_win: i64,
    pub boots: Option<i64>,
    pub starter: Vec<i64>,
    pub situational: Vec<i64>,
    pub skill_max: Vec<String>,
    pub skill_start: Vec<String>,
    pub spells: Vec<i64>,
    pub runes: Option<Runes>,
    pub strong_against: Vec<Matchup>,
    pub weak_against: Vec<Matchup>,
}

fn ids(v: &Value) -> Vec<i64> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub fn lcu_to_opgg_position(pos: &str) -> Option<&'static str> {
    Some(match pos {
        "top" => "top",
        "jungle" => "jungle",
        "middle" => "mid",
        "bottom" => "adc",
        "utility" => "support",
        _ => return None,
    })
}

fn opgg_to_lcu_position(pos: &str) -> String {
    match pos.to_lowercase().as_str() {
        "top" => "top",
        "jungle" => "jungle",
        "mid" | "middle" => "middle",
        "adc" | "bottom" => "bottom",
        "support" | "utility" => "utility",
        _ => "",
    }
    .to_string()
}

fn matchups(v: &Value) -> Vec<Matchup> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(Matchup {
                champion_id: m["champion_id"].as_i64()?,
                name: m["champion_name"].as_str().unwrap_or("").to_string(),
                my_win_rate: m["my_win_rate"].as_f64().unwrap_or(0.0),
                play: m["play"].as_i64().unwrap_or(0),
            })
        })
        .collect()
}

/// Extrae lo que usa Kite de la respuesta ya convertida a JSON.
pub fn extract(root: &Value) -> Option<ChampData> {
    let d = &root["data"];
    let champion_id = d["summary"]["id"].as_i64()?;
    let position = opgg_to_lcu_position(root["position"].as_str().unwrap_or(""));

    // Partidas y winrate en la línea pedida (si no, el global)
    let pos_stats = d["summary"]["positions"]
        .as_array()
        .and_then(|a| {
            a.iter().find(|p| opgg_to_lcu_position(p["name"].as_str().unwrap_or("")) == position)
        })
        .map(|p| p["stats"].clone())
        .unwrap_or_else(|| d["summary"]["average_stats"].clone());

    let core = ids(&d["core_items"]["ids"]);
    let boots = ids(&d["boots"]["ids"]).first().copied();

    // Situacionales: opciones de 1º (alternativas), 4º, 5º y 6º objeto, sin repetir la build
    let mut situational: Vec<(i64, i64)> = Vec::new(); // (id, partidas)
    for key in ["fourth_items", "fifth_items", "sixth_items", "last_items"] {
        for opt in d[key].as_array().into_iter().flatten() {
            let play = opt["play"].as_i64().unwrap_or(0);
            for id in ids(&opt["ids"]) {
                if core.contains(&id) || Some(id) == boots {
                    continue;
                }
                match situational.iter_mut().find(|(i, _)| *i == id) {
                    Some(e) => e.1 += play,
                    None => situational.push((id, play)),
                }
            }
        }
    }
    situational.sort_by(|a, b| b.1.cmp(&a.1));

    let mut starter = ids(&d["starter_items"]["ids"]);
    starter.dedup();

    let r = &d["runes"];
    let runes = r["primary_page_id"].as_i64().map(|primary| {
        let mut perk_names = strs(&r["primary_rune_names"]);
        perk_names.extend(strs(&r["secondary_rune_names"]));
        let mut perk_ids = ids(&r["primary_rune_ids"]);
        perk_ids.extend(ids(&r["secondary_rune_ids"]));
        perk_ids.extend(ids(&r["stat_mod_ids"]));
        Runes {
            primary_style: primary,
            primary_name: r["primary_page_name"].as_str().unwrap_or("").to_string(),
            sub_style: r["secondary_page_id"].as_i64().unwrap_or(0),
            sub_name: r["secondary_page_name"].as_str().unwrap_or("").to_string(),
            keystone: strs(&r["primary_rune_names"]).first().cloned().unwrap_or_default(),
            perk_ids,
            perk_names,
            play: r["play"].as_i64().unwrap_or(0),
            win: r["win"].as_i64().unwrap_or(0),
        }
    });

    let main_position = d["summary"]["positions"]
        .as_array()
        .and_then(|a| a.iter().max_by_key(|p| p["stats"]["play"].as_i64().unwrap_or(0)))
        .and_then(|p| p["name"].as_str())
        .and_then(|n| lcu_to_opgg_position(&opgg_to_lcu_position(n)))
        .unwrap_or("")
        .to_string();

    Some(ChampData {
        champion_id,
        position,
        main_position,
        patch: d["trends"]["win"]["version"].as_str().unwrap_or("").to_string(),
        games: pos_stats["play"].as_i64().unwrap_or(0),
        win_rate: pos_stats["win_rate"].as_f64().unwrap_or(0.0),
        core_play: d["core_items"]["play"].as_i64().unwrap_or(0),
        core_win: d["core_items"]["win"].as_i64().unwrap_or(0),
        core,
        boots,
        starter,
        situational: situational.into_iter().map(|(id, _)| id).take(6).collect(),
        skill_max: strs(&d["skill_masteries"]["ids"]),
        skill_start: strs(&d["skills"]["order"]).into_iter().take(3).collect(),
        spells: ids(&d["summoner_spells"]["ids"]),
        runes: runes.filter(|r| r.perk_ids.len() == 9),
        strong_against: matchups(&d["strong_counters"]),
        weak_against: matchups(&d["weak_counters"]),
    })
}

// ---------------------------------------------------------------------------
// Consulta con caché (una por campeón y línea; válida 12 h)
// ---------------------------------------------------------------------------

/// Nombre que espera OP.GG: MAYÚSCULAS_CON_GUIONES ("Lee Sin" -> LEE_SIN, "Kai'Sa" -> KAISA).
pub fn name_candidates(name: &str, alias: &str) -> Vec<String> {
    let from_name: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '&')
        .collect::<String>()
        .split(|c: char| c.is_whitespace() || c == '&')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("_")
        .to_uppercase();
    let mut from_alias = String::new();
    let mut prev_lower = false;
    for c in alias.chars() {
        if c.is_uppercase() && prev_lower {
            from_alias.push('_');
        }
        prev_lower = c.is_lowercase();
        from_alias.push(c.to_ascii_uppercase());
    }
    let mut out = Vec::new();
    for c in [from_name, from_alias, alias.to_uppercase()] {
        if !c.is_empty() && !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

const CACHE_HOURS: u64 = 12;

/// Dos sesiones: una para lo que pides tú (prioridad) y otra para la precarga en segundo plano.
pub enum Lane {
    Interactive,
    Prefetch,
}
static SESSION: std::sync::Mutex<Option<Mcp>> = std::sync::Mutex::new(None);
static PREFETCH_SESSION: std::sync::Mutex<Option<Mcp>> = std::sync::Mutex::new(None);
/// Si OP.GG rechaza la lista de campos una vez, no se vuelve a intentar (ahorra una consulta).
static FIELDS_BROKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Campeón que quieres ver ahora. Si cambias, las consultas del anterior se cancelan.
static WANTED: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
/// Campeón de la consulta interactiva en curso (para poder cancelarla entre llamadas)
static CURRENT_FETCH: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn set_wanted(champion_id: i64) {
    WANTED.store(champion_id, std::sync::atomic::Ordering::SeqCst);
}

fn cancelled(lane: &Lane, champion_id: i64) -> bool {
    matches!(lane, Lane::Interactive) && WANTED.load(std::sync::atomic::Ordering::SeqCst) != champion_id
}

fn slot(lane: &Lane) -> &'static std::sync::Mutex<Option<Mcp>> {
    match lane {
        Lane::Interactive => &SESSION,
        Lane::Prefetch => &PREFETCH_SESSION,
    }
}

/// Abre la sesión con OP.GG por adelantado para que la primera build no espere.
pub fn warm_up() {
    let t = std::time::Instant::now();
    let mut guard = SESSION.lock().unwrap();
    if guard.is_none() {
        match Mcp::connect() {
            Ok(m) => {
                *guard = Some(m);
                crate::klog!("OP.GG listo en {:.1} s", t.elapsed().as_secs_f32());
            }
            Err(e) => crate::klog!("OP.GG no disponible: {e}"),
        }
    }
}

fn cache_dir() -> std::path::PathBuf {
    crate::config::data_dir().join("opgg")
}

fn cache_path(champion_id: i64, pos: &str, tier: Option<&str>) -> std::path::PathBuf {
    let pos = if pos.is_empty() { "auto" } else { pos };
    match tier {
        Some(t) => cache_dir().join(format!("{champion_id}_{pos}_{t}.txt")),
        None => cache_dir().join(format!("{champion_id}_{pos}.txt")),
    }
}

/// Línea principal de cada campeón (formato LCU), recordada entre sesiones.
fn main_positions() -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(cache_dir().join("main_positions.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn remember_main_position(champion_id: i64, lcu_pos: &str) {
    if lcu_pos.is_empty() {
        return;
    }
    let mut map = main_positions();
    if map.get(&champion_id.to_string()).map(String::as_str) != Some(lcu_pos) {
        map.insert(champion_id.to_string(), lcu_pos.to_string());
        let _ = std::fs::create_dir_all(cache_dir());
        let _ = std::fs::write(cache_dir().join("main_positions.json"), serde_json::to_string(&map).unwrap_or_default());
    }
}

/// Solo los campos que usa Kite (respuesta más pequeña y rápida). OP.GG exige esta lista.
const FIELDS: [&str; 20] = [
    "position",
    "data.summary.id",
    "data.summary.average_stats.{play,win_rate}",
    "data.summary.positions[].name",
    "data.summary.positions[].stats.{play,win_rate,role_rate}",
    "data.core_items.{ids[],play,win}",
    "data.boots.{ids[]}",
    "data.starter_items.{ids[]}",
    "data.last_items[].{ids[],play}",
    "data.fourth_items[].{ids[],play}",
    "data.fifth_items[].{ids[],play}",
    "data.sixth_items[].{ids[],play}",
    "data.summoner_spells.{ids[]}",
    "data.runes.{primary_page_id,primary_page_name,primary_rune_ids[],primary_rune_names[],secondary_page_id,secondary_page_name,secondary_rune_ids[],secondary_rune_names[],stat_mod_ids[],play,win}",
    "data.skills.{order[]}",
    "data.skill_masteries.{ids[]}",
    "data.strong_counters[].{champion_id,champion_name,my_win_rate,play}",
    "data.weak_counters[].{champion_id,champion_name,my_win_rate,play}",
    "data.trends.win.{version}",
    "champion",
];

fn call_analysis(lane: &Lane, champion: &str, pos: &str, tier: Option<&str>, with_fields: bool) -> Result<String, String> {
    let mut args = json!({
        "champion": champion,
        "position": pos,
        "game_mode": "ranked",
        "lang": "en_US",
        "client_app": concat!("Kite/", env!("CARGO_PKG_VERSION")),
    });
    if with_fields {
        args["desired_output_fields"] = json!(FIELDS);
    }
    if let Some(t) = tier {
        args["tier"] = json!(t);
    }
    let mut guard = slot(lane).lock().unwrap();
    for attempt in 0..2 {
        if guard.is_none() {
            *guard = Some(Mcp::connect()?);
        }
        match guard.as_mut().unwrap().call("lol_get_champion_analysis", args.clone()) {
            Ok(r) => {
                let text = r["content"][0]["text"].as_str().unwrap_or("").to_string();
                if r["isError"].as_bool() == Some(true) {
                    return Err(text);
                }
                return Ok(text);
            }
            // Sesión caducada: se reconecta una vez
            // Sesión caducada o red: se reconecta una vez (un error de OP.GG no lo arregla reconectar)
            Err(e) if attempt == 0 && !e.starts_with("error MCP") => {
                *guard = None;
                crate::klog!("OP.GG: {e}, reconectando");
            }
            Err(e) => return Err(e),
        }
    }
    Err("sin respuesta".into())
}

/// Una consulta: con la lista de campos y, si falla, sin ella.
fn fetch(lane: &Lane, champion: &str, pos: &str, tier: Option<&str>) -> Result<(String, ChampData), String> {
    use std::sync::atomic::Ordering;
    let t = std::time::Instant::now();
    let mut last = String::new();
    let modes: &[bool] = if FIELDS_BROKEN.load(Ordering::Relaxed) { &[false] } else { &[true, false] };
    for &with_fields in modes {
        if matches!(lane, Lane::Interactive) && !with_fields && last.contains("cancelado") {
            break;
        }
        match call_analysis(lane, champion, pos, tier, with_fields) {
            Ok(text) => match parse_notation(&text).and_then(|v| extract(&v)) {
                Some(d) => {
                    // Solo si falló CON campos y funciona SIN ellos, la culpa es de la lista de campos
                    if !with_fields && modes.len() == 2 {
                        crate::klog!("OP.GG no acepta la lista de campos; se piden completos a partir de ahora");
                        FIELDS_BROKEN.store(true, Ordering::Relaxed);
                    }
                    crate::klog!("OP.GG {champion} {pos}{}: {:.1} s ({} KB{})",
                        tier.map(|t| format!(" {t}")).unwrap_or_default(),
                        t.elapsed().as_secs_f32(),
                        text.len() / 1024,
                        if with_fields { ", campos" } else { "" }
                    );
                    return Ok((text, d));
                }
                None => last = format!("respuesta no reconocida: {}", text.chars().take(160).collect::<String>()),
            },
            Err(e) => last = e,
        }
        if matches!(lane, Lane::Interactive) && WANTED.load(std::sync::atomic::Ordering::SeqCst) != CURRENT_FETCH.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("cancelado (has cambiado de campeón)".into());
        }
    }
    Err(last)
}

fn read_cache(path: &std::path::Path, max_age: Option<Duration>) -> Option<ChampData> {
    if let Some(max) = max_age {
        let age = std::fs::metadata(path).and_then(|m| m.modified()).ok()?.elapsed().ok()?;
        if age > max {
            return None;
        }
    }
    std::fs::read_to_string(path).ok().and_then(|t| parse_notation(&t)).and_then(|v| extract(&v))
}

/// Lo que haya en caché para este campeón, aunque sea antiguo: (datos, ¿está al día?).
/// Sirve para enseñar algo al instante mientras se descarga lo nuevo.
pub fn cached(champion_id: i64, lcu_pos: &str, tier: Option<&str>) -> Option<(ChampData, bool)> {
    let known_main = if lcu_pos.is_empty() { main_positions().get(&champion_id.to_string()).cloned() } else { None };
    let pos = known_main.as_deref().unwrap_or(lcu_pos);
    let path = cache_path(champion_id, pos, tier);
    let data = read_cache(&path, None)?;
    let fresh = read_cache(&path, Some(Duration::from_secs(CACHE_HOURS * 3600))).is_some();
    Some((data, fresh))
}

/// Datos de OP.GG para un campeón (id de Riot) en una línea (formato LCU; vacío = la principal).
/// En caso de error devuelve el motivo para enseñarlo en la ventana.
pub fn champion_data(
    lane: Lane,
    champion_id: i64,
    name: &str,
    alias: &str,
    lcu_pos: &str,
    tier: Option<&str>,
) -> Result<ChampData, String> {
    // Sin línea: si ya sabemos cuál es la principal de este campeón, se usa directamente
    let known_main = if lcu_pos.is_empty() { main_positions().get(&champion_id.to_string()).cloned() } else { None };
    let effective_pos = known_main.as_deref().unwrap_or(lcu_pos);

    let fresh = Some(Duration::from_secs(CACHE_HOURS * 3600));
    if let Some(d) = read_cache(&cache_path(champion_id, effective_pos, tier), fresh) {
        return Ok(d);
    }

    let save = |pos: &str, text: &str| {
        let _ = std::fs::create_dir_all(cache_dir());
        let _ = std::fs::write(cache_path(champion_id, pos, tier), text);
    };

    // OP.GG exige línea. Sin línea ni principal conocida se prueba MID y, si el campeón
    // es de otra línea, se repite con su principal (y se recuerda para la próxima vez).
    // Combinaciones (nombre, línea) a probar, de la más probable a la menos:
    // primero cada forma del nombre con la línea pedida (o MID) y, solo si ninguna sirve
    // y no había línea, las demás líneas con el nombre principal.
    let names = name_candidates(name, alias);
    let first_pos = lcu_to_opgg_position(effective_pos).unwrap_or("mid");
    let mut attempts: Vec<(String, &str)> = names.iter().map(|n| (n.clone(), first_pos)).collect();
    if effective_pos.is_empty() {
        if let Some(n) = names.first() {
            attempts.extend(["top", "jungle", "adc", "support"].map(|p| (n.clone(), p)));
        }
    }
    let mut last_err = String::from("sin respuesta");
    for (candidate, pos) in &attempts {
        if cancelled(&lane, champion_id) {
            return Err("cancelado (has cambiado de campeón)".into());
        }
        if matches!(lane, Lane::Interactive) {
            CURRENT_FETCH.store(champion_id, std::sync::atomic::Ordering::SeqCst);
        }
        {
            match fetch(&lane, &candidate, pos, tier) {
                // Se comprueba que OP.GG ha devuelto el campeón correcto
                Ok((text, d)) if d.champion_id == champion_id => {
                    let main_lcu = opgg_to_lcu_position(&d.main_position);
                    remember_main_position(champion_id, &main_lcu);
                    if effective_pos.is_empty() && !d.main_position.is_empty() && d.main_position != *pos {
                        if let Ok((t2, d2)) = fetch(&lane, &candidate, &d.main_position, tier) {
                            save(&d2.position, &t2);
                            return Ok(d2);
                        }
                    }
                    save(&d.position, &text);
                    if !effective_pos.is_empty() {
                        save(effective_pos, &text);
                    }
                    return Ok(d);
                }
                Ok(_) => {
                    last_err = format!("{candidate}: OP.GG devolvió otro campeón");
                    continue;
                }
                Err(e) => {
                    crate::klog!("OP.GG {candidate} {pos}: {}", e.chars().take(200).collect::<String>());
                    last_err = e.chars().take(160).collect();
                }
            }
        }
    }
    // Sin conexión: mejor datos de ayer que nada
    read_cache(&cache_path(champion_id, effective_pos, tier), None).ok_or(last_err)
}

pub const HIGH_ELO: &str = "challenger";
static PREFETCH_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Progreso de la precarga, para enseñarlo en la ventana: (hechas, total).
static PREFETCH_DONE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PREFETCH_TOTAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// (hechas, total) de la precarga en curso; (0, 0) si no hay ninguna.
pub fn prefetch_progress() -> (usize, usize) {
    use std::sync::atomic::Ordering;
    (PREFETCH_DONE.load(Ordering::Relaxed), PREFETCH_TOTAL.load(Ordering::Relaxed))
}

/// Precarga en segundo plano (sesión aparte, no retrasa lo que pidas tú), de uno en uno:
/// 1) tus campeones más jugados, 2) el resto en su línea principal, 3) runas de alto elo.
/// Lo que ya está en caché (menos de 12 h) no se vuelve a pedir.
pub fn prefetch(champions: Vec<(i64, String, String)>) {
    use std::sync::atomic::Ordering;
    if PREFETCH_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let t = std::time::Instant::now();
    // Solo lo que falta o tiene más de 12 h
    let fresh = Some(Duration::from_secs(CACHE_HOURS * 3600));
    let mut todo = Vec::new();
    for tier in [None, Some(HIGH_ELO)] {
        for (id, name, alias) in &champions {
            let pos = main_positions().get(&id.to_string()).cloned().unwrap_or_default();
            if read_cache(&cache_path(*id, &pos, tier), fresh).is_none() {
                todo.push((*id, name.clone(), alias.clone(), tier));
            }
        }
    }
    PREFETCH_DONE.store(0, Ordering::Relaxed);
    PREFETCH_TOTAL.store(todo.len(), Ordering::Relaxed);
    let mut n = 0;
    for (id, name, alias, tier) in &todo {
        if champion_data(Lane::Prefetch, *id, name, alias, "", *tier).is_ok() {
            n += 1;
        }
        PREFETCH_DONE.fetch_add(1, Ordering::Relaxed);
        // Pausa corta para no saturar a OP.GG
        std::thread::sleep(Duration::from_millis(300));
    }
    PREFETCH_DONE.store(0, Ordering::Relaxed);
    PREFETCH_TOTAL.store(0, Ordering::Relaxed);
    crate::klog!("precarga terminada: {n} consultas nuevas en {:.0} s", t.elapsed().as_secs_f32());
}

// ---------------------------------------------------------------------------
// Diagnóstico: `kite --probar-opgg`
// ---------------------------------------------------------------------------

/// Construye los argumentos de prueba a partir del esquema real de la herramienta.
fn probe_args(schema: &Value, fields: Option<Value>) -> Value {
    let mut args = serde_json::Map::new();
    let props = schema["properties"].as_object().cloned().unwrap_or_default();
    for (name, prop) in &props {
        let n = name.to_lowercase();
        let guess: Option<Value> = if n.contains("champion") {
            Some(pick_enum(prop, &["AHRI", "Ahri", "ahri"]).unwrap_or_else(|| "AHRI".into()).into())
        } else if n.contains("position") || n.contains("lane") {
            Some(pick_enum(prop, &["MID", "mid", "middle"]).unwrap_or_else(|| "mid".into()).into())
        } else if n.contains("mode") || n.contains("queue") {
            Some(pick_enum(prop, &["RANKED", "ranked", "solo"]).unwrap_or_else(|| "ranked".into()).into())
        } else if n.contains("lang") {
            Some(pick_enum(prop, &["en_US", "en-US", "en"]).unwrap_or_else(|| "en_US".into()).into())
        } else if n.contains("region") {
            Some(pick_enum(prop, &["global", "euw", "EUW"]).unwrap_or_else(|| "global".into()).into())
        } else if n.contains("desired_output_fields") {
            fields.clone()
        } else {
            None
        };
        if let Some(g) = guess {
            args.insert(name.clone(), g);
        }
    }
    Value::Object(args)
}

pub fn probe() {
    let dir = std::path::PathBuf::from("probe");
    let _ = std::fs::create_dir_all(&dir);
    let save = |name: &str, v: &Value| {
        let path = dir.join(name);
        let _ = std::fs::write(&path, serde_json::to_string_pretty(v).unwrap_or_default());
        println!("  guardado {}", path.display());
    };

    println!("== Probando OP.GG ({}) ==", endpoint());
    let mut mcp = match Mcp::connect() {
        Ok(m) => {
            println!("Conectado (sesión: {})", m.session.as_deref().unwrap_or("sin sesión"));
            m
        }
        Err(e) => {
            println!("No se pudo conectar: {e}");
            return;
        }
    };

    let tools = match mcp.list_tools() {
        Ok(t) => t,
        Err(e) => {
            println!("tools/list falló: {e}");
            return;
        }
    };
    save("tools.json", &tools);
    let find = |name: &str| -> Value {
        tools["tools"]
            .as_array()
            .and_then(|a| a.iter().find(|t| t["name"].as_str() == Some(name)).cloned())
            .unwrap_or(Value::Null)
    };
    let analysis = find("lol_get_champion_analysis");
    if analysis.is_null() {
        println!("No existe lol_get_champion_analysis. Herramientas: {}",
            tools["tools"].as_array().map(|a| a.iter().filter_map(|t| t["name"].as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default());
        return;
    }
    let schema = analysis["inputSchema"].clone();
    println!("\nParámetros de lol_get_champion_analysis:");
    for (k, v) in schema["properties"].as_object().into_iter().flatten() {
        let en = v["enum"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("  - {k}: {} ({} opciones)", v["type"], en);
    }

    // Intento 1: campos amplios. Intento 2: "data" entero. Así vemos la estructura real.
    let attempts = [
        ("ahri_data.json", Some(json!(["data"]))),
        ("ahri_all.json", Some(json!(["data.*"]))),
        ("ahri_nofields.json", None),
    ];
    for (file, fields) in attempts {
        let args = probe_args(&schema, fields);
        println!("\nLlamando con {args}");
        match mcp.call("lol_get_champion_analysis", args.clone()) {
            Ok(r) => {
                let is_err = r["isError"].as_bool().unwrap_or(false);
                let preview: String = r.to_string().chars().take(300).collect();
                println!("  {} {preview}…", if is_err { "ERROR:" } else { "OK:" });
                save(file, &json!({ "args": args, "result": r }));
            }
            Err(e) => {
                println!("  falló: {e}");
                save(file, &json!({ "args": args, "error": e }));
            }
        }
    }

    // Objetos (nombres e ids) para cruzar con los iconos
    let items = find("lol_list_items");
    if !items.is_null() {
        let args = probe_args(&items["inputSchema"], Some(json!(["data"])));
        if let Ok(r) = mcp.call("lol_list_items", args.clone()) {
            save("items.json", &json!({ "args": args, "result": r }));
        }
    }
    println!("\nListo. Avisa a Claude: ya puede leer la carpeta probe.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_respuesta_real_de_opgg() {
        let text = include_str!("../testdata/opgg_ahri_mid.txt");
        let v = parse_notation(text).expect("se puede leer");
        let d = extract(&v).expect("datos");
        assert_eq!(d.champion_id, 103);
        assert_eq!(d.position, "middle");
        assert_eq!(d.patch, "16.23");
        assert_eq!(d.games, 109669);
        assert_eq!(d.core, vec![3118, 4645, 3157]);
        assert_eq!(d.boots, Some(3020));
        assert_eq!(d.starter, vec![1056, 2003]);
        assert_eq!(d.spells, vec![4, 14]);
        assert_eq!(d.skill_max, vec!["Q", "W", "E"]);
        assert_eq!(d.skill_start, vec!["W", "Q", "E"]);
        assert!(d.situational.contains(&3089) && !d.situational.contains(&3157));
        let r = d.runes.expect("runas");
        assert_eq!(r.keystone, "Electrocute");
        assert_eq!(r.perk_ids, vec![8112, 8139, 8140, 8106, 8210, 8226, 5005, 5008, 5001]);
        assert_eq!(d.weak_against[0].name, "Naafiri");
        assert_eq!(d.strong_against[0].name, "Malphite");
    }

    #[test]
    fn nombres_para_opgg() {
        assert_eq!(name_candidates("Lee Sin", "LeeSin")[0], "LEE_SIN");
        assert_eq!(name_candidates("Kai'Sa", "Kaisa")[0], "KAISA");
        assert_eq!(name_candidates("Dr. Mundo", "DrMundo")[0], "DR_MUNDO");
        assert_eq!(name_candidates("Nunu & Willump", "Nunu")[0], "NUNU_WILLUMP");
        assert!(name_candidates("Wukong", "MonkeyKing").contains(&"MONKEY_KING".to_string()));
        assert_eq!(name_candidates("Ahri", "Ahri"), vec!["AHRI"]);
    }

    #[test]
    fn sse_y_enums() {
        assert_eq!(parse_body("event: message\ndata: {\"a\":1}\n\n"), Some(json!({"a": 1})));
        assert_eq!(parse_body("{\"b\":2}"), Some(json!({"b": 2})));
        let s = json!({"enum": ["TOP", "MID", "ADC"]});
        assert_eq!(pick_enum(&s, &["middle", "mid"]), Some("MID".into()));
    }
}
