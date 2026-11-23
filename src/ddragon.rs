//! Data Dragon (CDN estático oficial de Riot): versión del parche, objetos en inglés e iconos.
//! Todo se cachea en %APPDATA%\Kite\ddragon\<versión>\ para no descargar dos veces.

use std::{collections::HashMap, fs, path::PathBuf, time::Duration};

use serde_json::Value;

/// CDN oficial. `KITE_CDN` solo sirve para pruebas con un servidor local.
fn cdn() -> String {
    std::env::var("KITE_CDN").unwrap_or_else(|_| "https://ddragon.leagueoflegends.com".into())
}

#[derive(Clone, Debug)]
pub struct ItemInfo {
    pub name: String,
    pub legendary: bool,
    pub boots: bool,
}

pub struct DDragon {
    pub version: String,
    pub items: HashMap<i64, ItemInfo>,
    dir: PathBuf,
}

fn http_get(url: &str) -> Option<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let mut resp = agent.get(url).call().ok()?;
    resp.body_mut().with_config().limit(20 * 1024 * 1024).read_to_vec().ok()
}

/// Descarga cualquier ruta de Data Dragon (por ejemplo, un splash).
pub fn fetch(path: &str) -> Option<Vec<u8>> {
    http_get(&format!("{}{path}", cdn()))
}

fn base_dir() -> PathBuf {
    crate::config::data_dir().join("ddragon")
}

/// Última versión publicada; si no hay internet, la última que tengamos en caché.
fn latest_version() -> Option<String> {
    if let Some(v) = http_get(&format!("{}/api/versions.json", cdn()))
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v[0].as_str().map(str::to_string))
    {
        return Some(v);
    }
    let mut cached: Vec<String> = fs::read_dir(base_dir())
        .ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    cached.sort_by_key(|v| version_key(v));
    cached.pop()
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').filter_map(|p| p.parse().ok()).collect()
}

/// Clasifica los objetos: legendarios (objetos completos) y botas de nivel 2.
pub fn classify(item_json: &Value) -> HashMap<i64, ItemInfo> {
    let mut out = HashMap::new();
    let Some(data) = item_json["data"].as_object() else { return out };
    for (id, it) in data {
        let Ok(id) = id.parse::<i64>() else { continue };
        let name = it["name"].as_str().unwrap_or("").to_string();
        let tags: Vec<&str> = it["tags"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let has_into = it["into"].as_array().is_some_and(|a| !a.is_empty());
        let has_from = it["from"].as_array().is_some_and(|a| !a.is_empty());
        let on_sr = it["maps"]["11"].as_bool().unwrap_or(false);
        let purchasable = it["gold"]["purchasable"].as_bool().unwrap_or(false) && it["inStore"].as_bool() != Some(false);
        let total = it["gold"]["total"].as_i64().unwrap_or(0);
        let champ_only = it["requiredChampion"].is_string();
        let is_boots = tags.contains(&"Boots");
        let usable = on_sr && purchasable && !champ_only && !tags.contains(&"Consumable") && !tags.contains(&"Trinket");
        out.insert(
            id,
            ItemInfo {
                name,
                legendary: usable && !is_boots && !has_into && has_from && total >= 2000,
                boots: usable && is_boots && has_from && !has_into,
            },
        );
    }
    out
}

impl DDragon {
    /// Carga (o descarga) los datos del parche actual en inglés.
    pub fn load() -> Option<Self> {
        let version = latest_version()?;
        let dir = base_dir().join(&version);
        let _ = fs::create_dir_all(dir.join("item"));
        let item_path = dir.join("item_en_US.json");
        let raw = match fs::read(&item_path) {
            Ok(b) => b,
            Err(_) => {
                let b = http_get(&format!("{}/cdn/{version}/data/en_US/item.json", cdn()))?;
                let _ = fs::write(&item_path, &b);
                b
            }
        };
        let json: Value = serde_json::from_slice(&raw).ok()?;
        Some(Self { version, items: classify(&json), dir })
    }

    pub fn name(&self, id: i64) -> String {
        self.items.get(&id).map(|i| i.name.clone()).unwrap_or_else(|| format!("Item {id}"))
    }

    /// Ruta del icono en caché (lo descarga la primera vez).
    pub fn icon_path(&self, id: i64) -> Option<PathBuf> {
        let path = self.dir.join("item").join(format!("{id}.png"));
        if !path.exists() {
            let bytes = http_get(&format!("{}/cdn/{}/img/item/{id}.png", cdn(), self.version))?;
            fs::write(&path, bytes).ok()?;
        }
        Some(path)
    }
}

/// Decodifica un PNG a RGBA (ancho, alto, píxeles).
pub fn decode_png(path: &PathBuf) -> Option<(u32, u32, Vec<u8>)> {
    decode_png_bytes(&fs::read(path).ok()?)
}

pub fn decode_png_bytes(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        _ => return None,
    };
    Some((info.width, info.height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clasificacion_objetos() {
        let j = json!({"data": {
            "3031": {"name": "Infinity Edge", "tags": ["Damage"], "from": ["1038"], "gold": {"total": 3450, "purchasable": true}, "maps": {"11": true}},
            "1038": {"name": "B. F. Sword", "tags": ["Damage"], "into": ["3031"], "gold": {"total": 1300, "purchasable": true}, "maps": {"11": true}},
            "3006": {"name": "Berserker's Greaves", "tags": ["Boots"], "from": ["1001"], "gold": {"total": 1100, "purchasable": true}, "maps": {"11": true}},
            "2003": {"name": "Health Potion", "tags": ["Consumable"], "gold": {"total": 50, "purchasable": true}, "maps": {"11": true}}
        }});
        let m = classify(&j);
        assert!(m[&3031].legendary && !m[&3031].boots);
        assert!(!m[&1038].legendary);
        assert!(m[&3006].boots && !m[&3006].legendary);
        assert!(!m[&2003].legendary && !m[&2003].boots);
    }
}
