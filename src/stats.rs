//! Build que muestra la interfaz, construida a partir de los datos de OP.GG.

use crate::opgg::ChampData;

#[derive(Clone, Debug, Default)]
pub struct CoreBuild {
    pub items: Vec<i64>,
    pub games: i64,
    pub winrate: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Build {
    pub position: String,
    pub patch: String,
    pub games: i64,
    pub cores: Vec<CoreBuild>,
    pub boots: Option<i64>,
    pub situational: Vec<i64>,
    pub start: Vec<i64>,
    pub skills: String,
}

pub fn from_opgg(d: &ChampData) -> Build {
    let mut skills = d.skill_max.join(" > ");
    if !d.skill_start.is_empty() {
        skills.push_str(&format!("  ({})", d.skill_start.join(" ")));
    }
    Build {
        position: d.position.clone(),
        patch: d.patch.clone(),
        games: d.games,
        cores: if d.core.is_empty() {
            Vec::new()
        } else {
            vec![CoreBuild {
                items: d.core.clone(),
                games: d.core_play,
                winrate: if d.core_play > 0 { d.core_win as f64 / d.core_play as f64 } else { 0.0 },
            }]
        },
        boots: d.boots,
        situational: d.situational.clone(),
        start: d.starter.clone(),
        skills,
    }
}
