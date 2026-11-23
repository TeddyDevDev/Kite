//! Conexión con el cliente de LoL (LCU) usando SOLO sus APIs oficiales locales.
//!
//! - Credenciales: se leen del `lockfile` que el propio cliente escribe en su carpeta.
//! - Eventos: WebSocket de la LCU (sin sondeo; el hilo duerme mientras no pase nada).
//! - Nunca se lee memoria, no se inyecta nada y no se tocan archivos del juego.
//! - En champ select solo se lee TU celda. Nunca nombres/PUUID de otros jugadores.

use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::PathBuf,
    sync::{atomic::AtomicBool, atomic::Ordering, Arc, Mutex},
    time::Duration,
};

use base64::Engine;
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use serde_json::{json, Value};
use tungstenite::{client::IntoClientRequest, Message};

// ---------------------------------------------------------------------------
// Tipos públicos
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Credentials {
    pub port: u16,
    pub password: String,
}

impl Credentials {
    fn auth_header(&self) -> String {
        let raw = format!("riot:{}", self.password);
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
    }
}

/// Una página de runas recomendada (lista para mostrar e importar).
#[derive(Clone, Debug)]
pub struct RunePage {
    pub source: String,
    pub keystone: String,
    pub styles: String,
    pub spells: String,
    pub primary_style: i64,
    pub sub_style: i64,
    pub perk_ids: Vec<i64>,
    pub perk_names: Vec<String>,
    pub spell_ids: Vec<i64>,
}

/// Lo que la interfaz necesita saber.
#[derive(Debug)]
pub enum Event {
    Connected,
    Disconnected,
    Phase(String),
    MyPick { champion_id: i64, champion: String, position: String },
    Runes(Vec<RunePage>),
    /// Pedir la build de OP.GG para este campeón (None = limpiar)
    BuildFor(Option<BuildRequest>),
    Notice(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuildRequest {
    pub champion_id: i64,
    pub name: String,
    pub alias: String,
    pub position: String, // formato LCU
}

/// Nombre en inglés de los hechizos de invocador.
pub fn spell_name(id: i64) -> String {
    match id {
        1 => "Cleanse",
        3 => "Exhaust",
        4 => "Flash",
        6 => "Ghost",
        7 => "Heal",
        11 => "Smite",
        12 => "Teleport",
        13 => "Clarity",
        14 => "Ignite",
        21 => "Barrier",
        32 => "Mark",
        _ => return format!("#{id}"),
    }
    .to_string()
}

impl RunePage {
    /// Página de runas a partir de los datos de OP.GG (nombres en inglés).
    /// `label`: "Todas las ligas", "Challenger"...
    pub fn from_opgg(r: &crate::opgg::Runes, spells: &[i64], label: &str) -> Self {
        let mut spell_ids = spells.to_vec();
        flash_on_right(&mut spell_ids);
        let wr = if r.play > 0 { format!(" · {:.0}% WR", r.win as f64 / r.play as f64 * 100.0) } else { String::new() };
        RunePage {
            source: format!("OP.GG {label}{wr}"),
            keystone: r.keystone.clone(),
            styles: format!("{} + {}", r.primary_name, r.sub_name),
            spells: spell_ids.iter().map(|s| spell_name(*s)).collect::<Vec<_>>().join(" + "),
            primary_style: r.primary_style,
            sub_style: r.sub_style,
            perk_ids: r.perk_ids.clone(),
            perk_names: r.perk_names.clone(),
            spell_ids,
        }
    }

    pub fn is_opgg(&self) -> bool {
        self.source.starts_with("OP.GG")
    }
}

/// Estado compartido entre el hilo de la LCU y la interfaz.
pub struct Shared {
    pub auto_import: AtomicBool,
    /// Solo se importa durante la selección de campeones
    pub in_champ_select: AtomicBool,
    pub client: Mutex<Option<Client>>,
    pub pages: Mutex<Vec<RunePage>>,
    pub champion: Mutex<(String, String)>, // (nombre, posición)
    /// Campeón y línea actuales (para no mezclar datos si cambias rápido)
    pub current: Mutex<Option<(i64, String)>>,
    /// Campeón bloqueado y campeón para el que ya se importaron runas
    pub locked: Mutex<Option<i64>>,
    pub imported_for: Mutex<Option<i64>>,
    /// Última build pedida (para poder repetirla con otro rol desde la ventana)
    pub last_request: Mutex<Option<BuildRequest>>,
}

impl Shared {
    pub fn new(auto_import: bool) -> Arc<Self> {
        Arc::new(Self {
            auto_import: AtomicBool::new(auto_import),
            in_champ_select: AtomicBool::new(false),
            client: Mutex::new(None),
            pages: Mutex::new(Vec::new()),
            champion: Mutex::new((String::new(), String::new())),
            current: Mutex::new(None),
            locked: Mutex::new(None),
            imported_for: Mutex::new(None),
            last_request: Mutex::new(None),
        })
    }
}

// ---------------------------------------------------------------------------
// Localizar el lockfile
// ---------------------------------------------------------------------------

const RIOT_METADATA: &str = r"C:\ProgramData\Riot Games\Metadata\league_of_legends.live\league_of_legends.live.product_settings.yaml";

fn install_dir_from_metadata() -> Option<PathBuf> {
    let text = fs::read_to_string(RIOT_METADATA).ok()?;
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("product_install_full_path:"))?;
    let value = line.split_once(':')?.1.trim().trim_matches('"');
    Some(PathBuf::from(value))
}

fn lockfile_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    // Ruta manual (instalación en otra carpeta, o pruebas)
    if let Ok(custom) = std::env::var("KITE_LOCKFILE") {
        candidates.push(PathBuf::from(custom));
    }
    if let Some(dir) = install_dir_from_metadata() {
        candidates.push(dir.join("lockfile"));
    }
    for drive in ['C', 'D', 'E', 'F'] {
        candidates.push(PathBuf::from(format!(r"{drive}:\Riot Games\League of Legends\lockfile")));
    }
    candidates
}

/// Formato del lockfile: `nombre:pid:puerto:contraseña:protocolo`
pub fn read_credentials(path: &PathBuf) -> Option<Credentials> {
    let text = fs::read_to_string(path).ok()?;
    let parts: Vec<&str> = text.trim().split(':').collect();
    if parts.len() < 5 {
        return None;
    }
    Some(Credentials {
        port: parts[2].parse().ok()?,
        password: parts[3].to_string(),
    })
}

fn port_alive(port: u16) -> bool {
    TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    )
    .is_ok()
}

/// Primer lockfile cuyo puerto responde (ignora lockfiles viejos).
fn find_live_credentials() -> Option<Credentials> {
    lockfile_candidates()
        .iter()
        .filter(|p| p.exists())
        .filter_map(read_credentials)
        .find(|c| port_alive(c.port))
}

/// Imprime qué ve Kite: instalación detectada, lockfiles y si el puerto responde.
pub fn diagnose() {
    println!("[diag] carpeta según Riot: {:?}", install_dir_from_metadata());
    for path in lockfile_candidates() {
        if !path.exists() {
            continue;
        }
        match read_credentials(&path) {
            Some(c) => println!(
                "[diag] {} -> puerto {} {}",
                path.display(),
                c.port,
                if port_alive(c.port) { "RESPONDE" } else { "no responde (viejo)" }
            ),
            None => println!("[diag] {} -> no se pudo leer", path.display()),
        }
    }
    println!("[diag] fin. Esperando eventos...");
}

// ---------------------------------------------------------------------------
// TLS: la LCU usa un certificado autofirmado en 127.0.0.1.
// Este verificador SOLO se usa para conexiones a 127.0.0.1 (nunca a internet).
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct LocalhostVerifier(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for LocalhostVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("versiones TLS")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(LocalhostVerifier(provider)))
        .with_no_client_auth();
    Arc::new(config)
}

// ---------------------------------------------------------------------------
// Cliente REST mínimo contra la LCU
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Client {
    creds: Credentials,
    tls: Arc<rustls::ClientConfig>,
}

impl Client {
    /// Devuelve (código HTTP, cuerpo JSON o Null).
    pub fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Option<(u16, Value)> {
        let (status, bytes) = self.raw(method, path, body)?;
        let value = if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Some((status, value))
    }

    /// Descarga un archivo del cliente (por ejemplo, un icono de runa).
    pub fn get_bytes(&self, path: &str) -> Option<Vec<u8>> {
        match self.raw("GET", path, None)? {
            (200..=299, b) => Some(b),
            _ => None,
        }
    }

    fn raw(&self, method: &str, path: &str, body: Option<&Value>) -> Option<(u16, Vec<u8>)> {
        // Las imágenes se piden con Accept */* (con application/json el cliente las rechaza)
        let accept = if body.is_none() && (path.ends_with(".png") || path.ends_with(".jpg")) { "*/*" } else { "application/json" };
        let tcp = TcpStream::connect(("127.0.0.1", self.creds.port)).ok()?;
        tcp.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let name = ServerName::try_from("127.0.0.1").ok()?;
        let conn = rustls::ClientConnection::new(self.tls.clone(), name).ok()?;
        let mut stream = rustls::StreamOwned::new(conn, tcp);

        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: {}\r\nAccept: {accept}\r\nConnection: close\r\n",
            self.creds.port,
            self.creds.auth_header()
        );
        if body.is_some() {
            request.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                payload.len()
            ));
        }
        request.push_str("\r\n");
        request.push_str(&payload);
        stream.write_all(request.as_bytes()).ok()?;

        // Se lee solo hasta completar la respuesta: el cliente de LoL a veces deja la
        // conexión abierta y esperar a que la cierre costaba 5 s por petición.
        let mut buf = Vec::new();
        let mut chunk = [0u8; 32 * 1024];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
            if response_complete(&buf) {
                break;
            }
        }
        split_http(&buf)
    }

    pub fn get(&self, path: &str) -> Option<Value> {
        match self.request("GET", path, None) {
            Some((200..=299, v)) => Some(v),
            _ => None,
        }
    }
}

/// ¿Ha llegado ya la respuesta entera? (por Content-Length o por el final de chunked)
fn response_complete(buf: &[u8]) -> bool {
    let Some(split) = buf.windows(4).position(|w| w == b"\r\n\r\n") else { return false };
    let head = String::from_utf8_lossy(&buf[..split]).to_ascii_lowercase();
    let body_len = buf.len() - (split + 4);
    if let Some(len) = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        return body_len >= len;
    }
    if head.contains("transfer-encoding: chunked") {
        return buf.ends_with(b"0\r\n\r\n");
    }
    // 204/304 no llevan cuerpo
    head.starts_with("http/1.1 204") || head.starts_with("http/1.1 304")
}

/// Separa cabeceras y cuerpo; soporta `Transfer-Encoding: chunked`.
fn split_http(raw: &[u8]) -> Option<(u16, Vec<u8>)> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let status: u16 = head.split_whitespace().nth(1)?.parse().ok()?;
    let body = &raw[split + 4..];
    let body = if head.contains("transfer-encoding: chunked") {
        dechunk(body)?
    } else {
        body.to_vec()
    };
    Some((status, body))
}

#[cfg(test)]
fn parse_http(raw: &[u8]) -> Option<(u16, Value)> {
    let (status, body) = split_http(raw)?;
    let value = if body.iter().all(|b| b.is_ascii_whitespace()) { Value::Null } else { serde_json::from_slice(&body).ok()? };
    Some((status, value))
}

fn dechunk(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|w| w == b"\r\n")?;
        let size_str = std::str::from_utf8(&data[..line_end]).ok()?;
        let size = usize::from_str_radix(size_str.split(';').next()?.trim(), 16).ok()?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(data.get(..size)?);
        data = data.get(size + 2..)?;
    }
}

// ---------------------------------------------------------------------------
// Datos del juego (salen del propio cliente, sin internet)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct GameData {
    champions: HashMap<i64, String>,
    aliases: HashMap<i64, String>,
}

fn id_name_map(v: Option<Value>) -> HashMap<i64, String> {
    v.and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| Some((c["id"].as_i64()?, c["name"].as_str()?.to_string())))
        .collect()
}

impl GameData {
    fn load(client: &Client) -> Self {
        let data = Self {
            champions: id_name_map(client.get("/lol-game-data/assets/v1/champion-summary.json")),
            aliases: client
                .get("/lol-game-data/assets/v1/champion-summary.json")
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|c| Some((c["id"].as_i64()?, c["alias"].as_str()?.to_string())))
                .collect(),
        };
        if data.champions.is_empty() {
            crate::klog!("no se pudieron leer los nombres de campeones");
        }
        data
    }

    fn name<'a>(map: &'a HashMap<i64, String>, id: i64) -> String {
        map.get(&id).cloned().unwrap_or_else(|| format!("#{id}"))
    }
}

// ---------------------------------------------------------------------------
// Importación de runas y hechizos
// ---------------------------------------------------------------------------

/// Destello siempre en la F (segundo hechizo), nunca en la D.
pub const FLASH: i64 = 4;
fn flash_on_right(spells: &mut [i64]) {
    if spells.len() == 2 && spells[0] == FLASH {
        spells.swap(0, 1);
    }
}

fn page_editable(p: &Value) -> bool {
    p["isEditable"].as_bool().unwrap_or(false) || p["isDeletable"].as_bool().unwrap_or(false)
}

fn lcu_message(r: &Option<(u16, Value)>) -> String {
    match r {
        Some((code, v)) => v["message"].as_str().map(|m| format!("{code}: {m}")).unwrap_or_else(|| code.to_string()),
        None => "sin respuesta del cliente".into(),
    }
}

/// Sobrescribe una página existente (y la deja como actual).
fn overwrite_page(client: &Client, id: i64, body: &Value) -> bool {
    let mut b = body.clone();
    b["id"] = json!(id);
    if matches!(client.request("PUT", &format!("/lol-perks/v1/pages/{id}"), Some(&b)), Some((200..=299, _))) {
        client.request("PUT", "/lol-perks/v1/currentpage", Some(&json!(id)));
        return true;
    }
    // Plan B: borrar esa página y crearla de nuevo en su hueco
    client.request("DELETE", &format!("/lol-perks/v1/pages/{id}"), None);
    matches!(client.request("POST", "/lol-perks/v1/pages", Some(body)), Some((200..=299, _)))
}

/// Pone las runas y los hechizos. Nunca crea páginas de más:
/// 1) si ya existe la página "Kite", la modifica;
/// 2) si no, intenta crear una;
/// 3) si no hay hueco, modifica tu página actual.
pub fn import_page(client: &Client, page: &RunePage, title: &str) -> String {
    let body = json!({
        "name": format!("Kite: {title}"),
        "primaryStyleId": page.primary_style,
        "subStyleId": page.sub_style,
        "selectedPerkIds": page.perk_ids,
        "current": true,
    });
    let pages: Vec<Value> = client.get("/lol-perks/v1/pages").and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let kite_page = pages
        .iter()
        .find(|p| p["name"].as_str().is_some_and(|n| n.starts_with("Kite")) && page_editable(p))
        .and_then(|p| p["id"].as_i64());

    let mut runes_ok = kite_page.is_some_and(|id| overwrite_page(client, id, &body));
    let mut detail = String::new();
    if !runes_ok {
        let r = client.request("POST", "/lol-perks/v1/pages", Some(&body));
        runes_ok = matches!(r, Some((200..=299, _)));
        if !runes_ok {
            detail = lcu_message(&r);
            // Sin hueco: se modifica la página actual (o la primera editable)
            let target = client
                .get("/lol-perks/v1/currentpage")
                .filter(page_editable)
                .and_then(|p| p["id"].as_i64())
                .or_else(|| pages.iter().find(|p| page_editable(p)).and_then(|p| p["id"].as_i64()));
            if let Some(id) = target {
                runes_ok = overwrite_page(client, id, &body);
            }
        }
    }

    let spells_ok = page.spell_ids.len() == 2
        && matches!(
            client.request(
                "PATCH",
                "/lol-champ-select/v1/session/my-selection",
                Some(&json!({ "spell1Id": page.spell_ids[0], "spell2Id": page.spell_ids[1] })),
            ),
            Some((200..=299, _))
        );

    match (runes_ok, spells_ok) {
        (true, true) => "Runas y hechizos importados ✓".into(),
        (true, false) => "Runas importadas ✓ (los hechizos solo se pueden cambiar en la selección)".into(),
        (false, _) => format!("No se pudieron importar las runas ({detail})"),
    }
}

/// Importación automática: cuando tu campeón está bloqueado y ya hay runas de OP.GG.
/// Se llama desde el hilo de la LCU (al bloquear) y desde el de OP.GG (al llegar las runas).
pub fn try_auto_import(shared: &Shared) -> Option<String> {
    if !shared.auto_import.load(Ordering::Relaxed) || !shared.in_champ_select.load(Ordering::Relaxed) {
        return None;
    }
    let locked = (*shared.locked.lock().unwrap())?;
    let current = shared.current.lock().unwrap().clone()?;
    if current.0 != locked {
        return None;
    }
    // Cada candado se coge y se suelta por separado (nunca dos a la vez: evita bloqueos)
    let page = shared.pages.lock().unwrap().first().cloned()?;
    let client = shared.client.lock().unwrap().clone()?;
    {
        let mut imported = shared.imported_for.lock().unwrap();
        if *imported == Some(locked) {
            return None;
        }
        *imported = Some(locked);
    }
    let (champ, pos) = shared.champion.lock().unwrap().clone();
    crate::klog!("importando runas de {champ} ({})", page.source);
    Some(import_page(&client, &page, format!("{champ} {pos}").trim()))
}

// ---------------------------------------------------------------------------
// Bucle principal: buscar cliente -> conectar -> escuchar -> si se cae, volver a empezar
// ---------------------------------------------------------------------------

const SUBSCRIPTIONS: [&str; 2] = [
    "OnJsonApiEvent_lol-gameflow_v1_gameflow-phase",
    "OnJsonApiEvent_lol-champ-select_v1_session",
];

pub fn run(shared: Arc<Shared>, mut emit: impl FnMut(Event)) -> ! {
    let tls = tls_config();
    let mut last_error = String::new();
    loop {
        // Sin cliente abierto: comprobar cada 3 s (coste despreciable)
        let Some(creds) = find_live_credentials() else {
            std::thread::sleep(Duration::from_secs(3));
            continue;
        };
        let client = Client { creds, tls: tls.clone() };
        *shared.client.lock().unwrap() = Some(client.clone());

        if let Err(e) = session(&client, &shared, &mut emit) {
            let msg = e.to_string();
            // Solo se registra si el error es distinto al anterior (nada de spam)
            if msg != last_error {
                crate::klog!("conexión perdida: {msg}");
                last_error = msg;
            }
        }
        *shared.client.lock().unwrap() = None;
        shared.pages.lock().unwrap().clear();
        emit(Event::Disconnected);
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// Estado de la selección de campeones actual.
#[derive(Default)]
struct DraftState {
    shown_for: Option<(i64, String)>, // build pedida para (campeón, línea)
}

fn session(
    client: &Client,
    shared: &Arc<Shared>,
    emit: &mut impl FnMut(Event),
) -> Result<(), Box<dyn std::error::Error>> {
    let data = GameData::load(client);

    // Precarga de las builds de todos los campeones (en segundo plano), tus 10 más jugados primero
    {
        let mastery = client.get("/lol-champion-mastery/v1/local-player/champion-mastery");
        let mut top: Vec<(i64, i64)> = mastery
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| Some((m["championId"].as_i64()?, m["championPoints"].as_i64().unwrap_or(0))))
            .collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        // Orden: tus 10 más jugados primero y después todos los demás
        let mut order: Vec<i64> = top.into_iter().take(10).map(|(id, _)| id).collect();
        let mut rest: Vec<i64> = data
            .champions
            .keys()
            .copied()
            .filter(|id| (1..2000).contains(id) && !order.contains(id))
            .collect();
        rest.sort();
        order.extend(rest);
        let list: Vec<(i64, String, String)> = order
            .into_iter()
            .filter_map(|id| {
                Some((id, data.champions.get(&id)?.clone(), data.aliases.get(&id).cloned().unwrap_or_default()))
            })
            .collect();
        std::thread::spawn(move || crate::opgg::prefetch(list));
    }

    let mut request = format!("wss://127.0.0.1:{}/", client.creds.port).into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", client.creds.auth_header().parse()?);

    let tcp = TcpStream::connect(("127.0.0.1", client.creds.port))?;
    let (mut ws, _) = tungstenite::client_tls_with_config(
        request,
        tcp,
        None,
        Some(tungstenite::Connector::Rustls(client.tls.clone())),
    )?;

    for event in SUBSCRIPTIONS {
        ws.send(Message::text(format!(r#"[5, "{event}"]"#)))?;
    }
    emit(Event::Connected);

    let mut draft = DraftState::default();

    // Estado inicial (por si abrimos Kite con el cliente ya en una fase)
    match client.get("/lol-gameflow/v1/gameflow-phase") {
        Some(Value::String(phase)) => {
            on_phase(&phase, client, &data, shared, &mut draft, emit);
            if phase == "ChampSelect" {
                if let Some(s) = client.get("/lol-champ-select/v1/session") {
                    on_champ_select(&s, client, &data, shared, &mut draft, emit);
                }
            }
        }
        other => crate::klog!("no se pudo leer la fase inicial: {other:?}"),
    }

    loop {
        let msg = ws.read()?; // bloquea sin gastar CPU hasta que llega algo
        let Message::Text(text) = msg else { continue };
        let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&text) else { continue };
        // Formato WAMP: [8, "NombreEvento", { uri, eventType, data }]
        let (Some(name), Some(payload)) = (arr.get(1).and_then(Value::as_str), arr.get(2)) else { continue };
        let data_json = &payload["data"];

        match name {
            "OnJsonApiEvent_lol-gameflow_v1_gameflow-phase" => {
                if let Some(phase) = data_json.as_str() {
                    on_phase(phase, client, &data, shared, &mut draft, emit);
                }
            }
            "OnJsonApiEvent_lol-champ-select_v1_session" => {
                if payload["eventType"].as_str() != Some("Delete") {
                    on_champ_select(data_json, client, &data, shared, &mut draft, emit);
                }
            }
            _ => {}
        }
    }
}

fn request_build(shared: &Shared, emit: &mut impl FnMut(Event), req: BuildRequest) {
    *shared.last_request.lock().unwrap() = Some(req.clone());
    emit(Event::BuildFor(Some(req)));
}

/// Fases "antes de la partida": se limpia todo. En partida se mantiene tu campeón y tu build.
fn on_phase(
    phase: &str,
    client: &Client,
    data: &GameData,
    shared: &Arc<Shared>,
    draft: &mut DraftState,
    emit: &mut impl FnMut(Event),
) {
    crate::klog!("fase: {phase}");
    shared.in_champ_select.store(phase == "ChampSelect", Ordering::Relaxed);
    match phase {
        "ChampSelect" => {}
        "GameStart" | "InProgress" | "Reconnect" => {
            shared.pages.lock().unwrap().clear();
            emit(Event::Runes(Vec::new()));
            // Kite abierto con la partida ya empezada: buscar SOLO tu campeón
            if draft.shown_for.is_none() {
                if let Some(id) = my_champion_in_game(client) {
                    draft.shown_for = Some((id, String::new()));
                    let champion = GameData::name(&data.champions, id);
                    *shared.champion.lock().unwrap() = (champion.clone(), String::new());
                    emit(Event::MyPick { champion_id: id, champion: champion.clone(), position: String::new() });
                    *shared.current.lock().unwrap() = Some((id, String::new()));
                    request_build(shared, emit, BuildRequest {
                        champion_id: id,
                        name: champion,
                        alias: data.aliases.get(&id).cloned().unwrap_or_default(),
                        position: String::new(),
                    });
                }
            }
        }
        "WaitingForStats" | "PreEndOfGame" | "EndOfGame" => {
            shared.pages.lock().unwrap().clear();
            emit(Event::Runes(Vec::new()));
        }
        _ => {
            *draft = DraftState::default();
            shared.pages.lock().unwrap().clear();
            *shared.current.lock().unwrap() = None;
            *shared.locked.lock().unwrap() = None;
            *shared.imported_for.lock().unwrap() = None;
            *shared.last_request.lock().unwrap() = None;
            emit(Event::Runes(Vec::new()));
            emit(Event::BuildFor(None));
        }
    }
    emit(Event::Phase(phase_es(phase)));
}

/// Tu campeón en la partida actual. Solo se busca TU entrada por tu PUUID; nada de otros jugadores.
fn my_champion_in_game(client: &Client) -> Option<i64> {
    let me = client.get("/lol-summoner/v1/current-summoner")?;
    let puuid = me["puuid"].as_str()?.to_string();
    let session = client.get("/lol-gameflow/v1/session")?;
    let game = &session["gameData"];
    let candidates = ["playerChampionSelections", "teamOne", "teamTwo"];
    candidates
        .iter()
        .filter_map(|k| game[*k].as_array())
        .flatten()
        .find(|p| p["puuid"].as_str() == Some(puuid.as_str()))
        .and_then(|p| p["championId"].as_i64())
        .filter(|id| *id > 0)
}

fn on_champ_select(
    session: &Value,
    client: &Client,
    data: &GameData,
    shared: &Arc<Shared>,
    draft: &mut DraftState,
    emit: &mut impl FnMut(Event),
) {
    let Some(me) = my_selection(session) else { return };
    let champion = GameData::name(&data.champions, me.champion_id);
    let position_label = position_es(&me.position);
    *shared.champion.lock().unwrap() = (champion.clone(), position_label.clone());
    emit(Event::MyPick { champion_id: me.champion_id, champion: champion.clone(), position: position_label.clone() });

    // Build y runas de OP.GG: solo se piden cuando cambia el campeón o la línea
    let key = (me.champion_id, me.position.clone());
    if draft.shown_for.as_ref() != Some(&key) {
        draft.shown_for = Some(key);
        crate::klog!("campeón: {champion} (línea: {:?}, bloqueado: {})", me.position, me.locked);
        *shared.current.lock().unwrap() = Some((me.champion_id, me.position.clone()));
        shared.pages.lock().unwrap().clear();
        emit(Event::Runes(Vec::new()));
        // La build (y la página de runas de OP.GG) llegan aparte, sin bloquear este hilo
        request_build(shared, emit, BuildRequest {
            champion_id: me.champion_id,
            name: champion.clone(),
            alias: data.aliases.get(&me.champion_id).cloned().unwrap_or_default(),
            position: me.position.clone(),
        });
    }

    // Importación automática al bloquear (una vez por campeón). Si las runas de OP.GG
    // aún no han llegado, se importan en cuanto lleguen.
    if me.locked && *shared.locked.lock().unwrap() != Some(me.champion_id) {
        crate::klog!("bloqueado: {champion}");
    }
    if me.locked {
        *shared.locked.lock().unwrap() = Some(me.champion_id);
        if let Some(msg) = try_auto_import(shared) {
            crate::klog!("importación: {msg}");
            emit(Event::Notice(msg));
        }
    }
    let _ = client;
}

struct MySelection {
    champion_id: i64,
    position: String,
    locked: bool,
}

/// Solo lee TU propia celda del equipo. No se consulta nada de otros jugadores
/// (ni nombres, ni PUUID) => nada de "lobby reveal".
fn my_selection(session: &Value) -> Option<MySelection> {
    let my_cell = session["localPlayerCellId"].as_i64()?;
    let me = session["myTeam"]
        .as_array()?
        .iter()
        .find(|p| p["cellId"].as_i64() == Some(my_cell))?;

    let picked = me["championId"].as_i64().filter(|id| *id > 0);
    let champion_id = picked.or_else(|| me["championPickIntent"].as_i64().filter(|id| *id > 0))?;

    // Bloqueado = tu acción de pick está completada (si no hay acciones, basta con championId)
    let actions: Vec<&Value> = session["actions"]
        .as_array()
        .map(|groups| groups.iter().filter_map(Value::as_array).flatten().collect())
        .unwrap_or_default();
    let my_picks: Vec<&&Value> = actions
        .iter()
        .filter(|a| a["actorCellId"].as_i64() == Some(my_cell) && a["type"].as_str() == Some("pick"))
        .collect();
    let locked = if my_picks.is_empty() {
        picked.is_some()
    } else {
        my_picks.iter().any(|a| a["completed"].as_bool() == Some(true))
    };

    Some(MySelection {
        champion_id,
        position: me["assignedPosition"].as_str().unwrap_or("").to_string(),
        locked,
    })
}

fn phase_es(phase: &str) -> String {
    match phase {
        "None" => "En el cliente",
        "Lobby" => "En la sala",
        "Matchmaking" => "Buscando partida…",
        "ReadyCheck" => "¡Partida encontrada!",
        "ChampSelect" => "Selección de campeones",
        "GameStart" | "InProgress" => "En partida",
        "Reconnect" => "Reconectando a la partida",
        "WaitingForStats" | "PreEndOfGame" | "EndOfGame" => "Fin de partida",
        other => other,
    }
    .to_string()
}

fn position_es(pos: &str) -> String {
    match pos {
        "top" => "TOP",
        "jungle" => "JUNGLA",
        "middle" => "MID",
        "bottom" => "ADC",
        "utility" => "SUPPORT",
        _ => "",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_normal_y_chunked() {
        let normal = b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\n\"None\"";
        assert_eq!(parse_http(normal), Some((200, Value::String("None".into()))));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n\"No\r\n3\r\nne\"\r\n0\r\n\r\n";
        assert_eq!(parse_http(chunked), Some((200, Value::String("None".into()))));
        assert_eq!(parse_http(b"HTTP/1.1 204 No Content\r\n\r\n"), Some((204, Value::Null)));
    }

    #[test]
    fn respuesta_completa() {
        assert!(!response_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab"));
        assert!(response_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabcde"));
        assert!(!response_complete(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n"));
        assert!(response_complete(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n"));
        assert!(response_complete(b"HTTP/1.1 204 No Content\r\n\r\n"));
    }

    #[test]
    fn destello_a_la_derecha() {
        let mut s = vec![4, 14];
        flash_on_right(&mut s);
        assert_eq!(s, vec![14, 4]);
        let mut s = vec![11, 4];
        flash_on_right(&mut s);
        assert_eq!(s, vec![11, 4]);
    }

    #[test]
    fn bloqueo_por_acciones() {
        let s = json!({
            "localPlayerCellId": 1,
            "myTeam": [{"cellId": 1, "championId": 103, "assignedPosition": "middle"}],
            "actions": [[{"actorCellId": 1, "type": "pick", "completed": false}]]
        });
        let me = my_selection(&s).unwrap();
        assert_eq!(me.champion_id, 103);
        assert!(!me.locked);
        let s2 = json!({
            "localPlayerCellId": 1,
            "myTeam": [{"cellId": 1, "championId": 103}],
            "actions": [[{"actorCellId": 1, "type": "pick", "completed": true}]]
        });
        assert!(my_selection(&s2).unwrap().locked);
    }
}
