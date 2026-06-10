//! Vanilla + mod game-data loading for extraction.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use csv::ReaderBuilder;
use serde_json::Value;

use crate::extract::Result;

#[derive(Debug, Clone)]
pub struct ConditionSpec {
    pub id: String,
    pub group: String,
    pub rank: Option<i32>,
    pub hazard: f64,
    pub source: String,
    pub feature_key: String,
}

#[derive(Debug, Clone)]
pub struct PlanetTypeSpec {
    pub source: String,
    pub is_star_like: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GameData {
    pub conditions: HashMap<String, ConditionSpec>,
    pub planet_types: HashMap<String, PlanetTypeSpec>,
    /// Condition ids flagged `planetary` in market_conditions.csv (vanilla + mods).
    /// Used to separate planetary conditions from colony-management conditions
    /// (population_*, pirate_activity, ...) on colonized markets.
    pub planetary_conditions: HashSet<String>,
}

impl GameData {
    pub fn condition_feature_key(&self, condition_id: &str) -> Option<&str> {
        self.conditions.get(condition_id).map(|spec| spec.feature_key.as_str())
    }

    pub fn condition_hazard(&self, condition_id: &str) -> Option<f64> {
        self.conditions.get(condition_id).map(|spec| spec.hazard)
    }

    pub fn planet_type_source(&self, type_id: &str) -> Option<&str> {
        self.planet_types.get(type_id).map(|spec| spec.source.as_str())
    }

    /// A condition counts as planetary when procgen defines it (condition_gen_data)
    /// or market_conditions.csv flags it planetary.
    pub fn is_planetary_condition(&self, condition_id: &str) -> bool {
        self.conditions.contains_key(condition_id)
            || self.planetary_conditions.contains(condition_id)
    }

    pub fn is_star_type(&self, type_id: &str) -> bool {
        self.planet_types
            .get(type_id)
            .map(|spec| spec.is_star_like)
            .unwrap_or_else(|| is_star_like(type_id, None))
    }
}

pub fn load_game_data(starsector_dir: impl AsRef<Path>) -> Result<GameData> {
    let starsector_dir = starsector_dir.as_ref();
    let core_data = starsector_dir.join("starsector-core").join("data");

    let mut game_data = GameData::default();

    load_condition_file(
        &core_data.join("campaign").join("procgen").join("condition_gen_data.csv"),
        "vanilla",
        &mut game_data.conditions,
        false,
    )?;
    load_planet_types_file(
        &core_data.join("config").join("planets.json"),
        "vanilla",
        &mut game_data.planet_types,
        false,
    )?;
    let market_conditions_path = core_data.join("campaign").join("market_conditions.csv");
    if market_conditions_path.exists() {
        load_market_conditions_file(&market_conditions_path, &mut game_data.planetary_conditions)?;
    }

    let mods_dir = starsector_dir.join("mods");
    if mods_dir.is_dir() {
        for entry in fs::read_dir(&mods_dir)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    eprintln!("warning: failed to read mod directory entry: {err}");
                    continue;
                }
            };
            let mod_path = entry.path();
            if !mod_path.is_dir() {
                continue;
            }
            let mod_name = entry.file_name().to_string_lossy().to_string();

            let condition_path = mod_path
                .join("data")
                .join("campaign")
                .join("procgen")
                .join("condition_gen_data.csv");
            if condition_path.exists() {
                if let Err(err) = load_condition_file(
                    &condition_path,
                    &mod_name,
                    &mut game_data.conditions,
                    true,
                ) {
                    eprintln!("warning: skipping bad mod condition file {:?}: {err}", condition_path);
                }
            }

            let market_conditions_path = mod_path
                .join("data")
                .join("campaign")
                .join("market_conditions.csv");
            if market_conditions_path.exists() {
                if let Err(err) = load_market_conditions_file(
                    &market_conditions_path,
                    &mut game_data.planetary_conditions,
                ) {
                    eprintln!(
                        "warning: skipping bad mod market_conditions file {:?}: {err}",
                        market_conditions_path
                    );
                }
            }

            let planet_types_path = mod_path.join("data").join("config").join("planets.json");
            if planet_types_path.exists() {
                if let Err(err) = load_planet_types_file(
                    &planet_types_path,
                    &mod_name,
                    &mut game_data.planet_types,
                    true,
                ) {
                    eprintln!("warning: skipping bad mod planets file {:?}: {err}", planet_types_path);
                }
            }
        }
    }

    Ok(game_data)
}

/// Mod CSVs are not always valid UTF-8 (e.g. Windows-1252 description text), so
/// read lossily instead of failing the whole file.
fn read_lossy(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn load_condition_file(
    path: &Path,
    source: &str,
    out: &mut HashMap<String, ConditionSpec>,
    merge_only: bool,
) -> Result<()> {
    let text = read_lossy(path)?;
    let mut rdr = ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());

    for record in rdr.records() {
        let record = record?;
        let id = record.get(0).unwrap_or("").trim();
        if id.is_empty() {
            continue;
        }

        if merge_only && out.contains_key(id) {
            continue;
        }

        let group = record.get(1).unwrap_or("").trim().to_string();
        let rank = record.get(2).unwrap_or("").trim().parse::<i32>().ok();
        let hazard = record
            .get(4)
            .unwrap_or("")
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0);
        let feature_key = if !group.is_empty() && rank.is_some() {
            format!("{}:{}", group, rank.unwrap())
        } else {
            id.to_string()
        };

        out.insert(
            id.to_string(),
            ConditionSpec {
                id: id.to_string(),
                group,
                rank,
                hazard,
                source: source.to_string(),
                feature_key,
            },
        );
    }

    Ok(())
}

fn load_market_conditions_file(path: &Path, out: &mut HashSet<String>) -> Result<()> {
    let text = read_lossy(path)?;
    let mut rdr = ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());

    let headers = rdr.headers()?.clone();
    let id_idx = headers.iter().position(|h| h.eq_ignore_ascii_case("id"));
    let planetary_idx = headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("planetary"));
    let (Some(id_idx), Some(planetary_idx)) = (id_idx, planetary_idx) else {
        return Ok(()); // file without the expected columns: nothing to learn
    };

    for record in rdr.records() {
        let record = record?;
        let id = record.get(id_idx).unwrap_or("").trim();
        let planetary = record.get(planetary_idx).unwrap_or("").trim();
        if !id.is_empty() && planetary.eq_ignore_ascii_case("true") {
            out.insert(id.to_string());
        }
    }

    Ok(())
}

fn load_planet_types_file(
    path: &Path,
    source: &str,
    out: &mut HashMap<String, PlanetTypeSpec>,
    merge_only: bool,
) -> Result<()> {
    let raw = fs::read_to_string(path)?;
    let stripped = strip_json_comments(&raw);
    let stripped = strip_trailing_commas(&stripped);
    let parsed: Value = serde_json::from_str(&stripped)?;
    let Some(map) = parsed.as_object() else {
        return Err(crate::extract::ExtractError::Json(format!(
            "expected top-level object in {:?}",
            path
        )));
    };

    for (type_id, value) in map {
        if merge_only && out.contains_key(type_id) {
            continue;
        }
        let is_star_like = is_star_like(type_id, Some(value));
        out.insert(
            type_id.clone(),
            PlanetTypeSpec {
                source: source.to_string(),
                is_star_like,
            },
        );
    }

    Ok(())
}

fn is_star_like(type_id: &str, value: Option<&Value>) -> bool {
    type_id.starts_with("star_")
        || type_id == "black_hole"
        || type_id.starts_with("nebula_center_")
        || value
            .and_then(|v| v.get("isStar"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

fn strip_json_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '#' => {
                while let Some(next) = chars.next() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            _ => out.push(ch),
        }
    }

    out
}

fn strip_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < chars.len() {
        let ch = chars[i];
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if ch == '"' {
            in_string = true;
            out.push(ch);
            i += 1;
            continue;
        }

        if ch == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1;
                continue;
            }
        }

        out.push(ch);
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, write};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("system_solver_extract_{name}_{unique}"))
    }

    #[test]
    fn load_game_data_merges_vanilla_and_mods_and_skips_bad_mods() {
        let root = temp_path("gamedata");
        let core_procgen = root.join("starsector-core/data/campaign/procgen");
        let core_config = root.join("starsector-core/data/config");
        let mod_good = root.join("mods/Good Mod/data");
        let mod_bad = root.join("mods/Bad Mod/data");

        create_dir_all(&core_procgen).unwrap();
        create_dir_all(&core_config).unwrap();
        create_dir_all(mod_good.join("campaign/procgen")).unwrap();
        create_dir_all(mod_good.join("config")).unwrap();
        create_dir_all(mod_bad.join("campaign/procgen")).unwrap();
        create_dir_all(mod_bad.join("config")).unwrap();

        write(
            core_procgen.join("condition_gen_data.csv"),
            "id,group,rank,order,hazard\nore_moderate,ore,2,5,0\n,ore,3,5,0.5\n",
        )
        .unwrap();
        write(
            root.join("starsector-core/data/campaign/market_conditions.csv"),
            "name,id,tags,planetary,decivRemove\nJungle World,jungle,,TRUE,\nFree Port,free_market,,,\n",
        )
        .unwrap();
        write(
            core_config.join("planets.json"),
            r#"{
                "water": {"isStar": false,},
                "star_yellow": {"isStar": true,},
            }"#,
        )
        .unwrap();

        write(
            mod_good.join("campaign/procgen/condition_gen_data.csv"),
            "id,group,rank,order,hazard\nmod_cond,mod,1,0,0.25\nore_moderate,ore,2,5,1.0\n",
        )
        .unwrap();
        write(
            mod_good.join("config/planets.json"),
            r#"{
                "mod_type": {"isStar": false,},
                "water": {"isStar": true,}
            }"#,
        )
        .unwrap();
        write(mod_bad.join("config/planets.json"), "{ invalid json ").unwrap();

        let gd = load_game_data(&root).unwrap();
        assert_eq!(gd.conditions["ore_moderate"].hazard, 0.0);
        assert_eq!(gd.conditions["ore_moderate"].source, "vanilla");
        assert!(gd.conditions.contains_key("mod_cond"));
        assert_eq!(gd.conditions["mod_cond"].feature_key, "mod:1");
        assert_eq!(gd.planet_types["water"].source, "vanilla");
        assert!(gd.planet_types["water"].is_star_like == false);
        assert_eq!(gd.planet_types["mod_type"].source, "Good Mod");
        assert!(gd.planet_types["star_yellow"].is_star_like);
        assert!(gd.is_planetary_condition("jungle"));
        assert!(gd.is_planetary_condition("ore_moderate"));
        assert!(!gd.is_planetary_condition("free_market"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn comment_and_trailing_comma_stripping_preserves_strings() {
        let raw = r##"{
            "text": "value # not a comment",
            # whole-line comment
            "nested": [1,2,3,],
        }"##;
        let stripped = strip_json_comments(raw);
        assert!(stripped.contains("value # not a comment"));
        assert!(!stripped.contains("whole-line comment"));
        let stripped = strip_trailing_commas(&stripped);
        let parsed: Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(parsed["nested"].as_array().unwrap().len(), 3);
    }
}
