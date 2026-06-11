//! SQLite persistence, search, and CSV export for extracted save data.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use csv::WriterBuilder;
use rusqlite::{params, Connection, OptionalExtension};

use crate::extract::gamedata::GameData;
use crate::extract::model::{MappedOutput, MappedSystem, SaveInfo};
use crate::extract::{ExtractError, Result};

pub struct Db {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct SaveRow {
    pub id: i64,
    pub dir_name: String,
    pub path: String,
    pub character_name: String,
    pub save_date: String,
    pub game_version: String,
    pub character_level: i64,
    pub extracted_at: String,
}

#[derive(Debug, Clone)]
pub struct SearchSystemRow {
    pub save_dir_name: String,
    pub name: String,
    pub display_name: String,
    pub internal_id: String,
    pub x_ly: Option<f64>,
    pub y_ly: Option<f64>,
    pub dist_from_com_ly: Option<f64>,
    pub stable_points: u32,
    pub has_gate: bool,
    pub has_remnants: bool,
    pub remnant_damaged: bool,
    pub star_types: Vec<String>,
    pub planet_count: u32,
}

#[derive(Debug, Clone)]
pub struct SystemDiscovery {
    pub system_name: String,
    pub planet_count: u32,
    /// Planets with survey_level = 'FULL'.
    pub surveyed_full: u32,
    /// Planets with survey_level IN ('PRELIMINARY','FULL').
    pub surveyed_any: u32,
    /// System has the `theme_core_populated` tag.
    pub is_core: bool,
    /// Planets with owner_faction NOT IN ('neutral','player') AND NOT NULL.
    pub npc_colonized_planets: u32,
}

#[derive(Debug, Clone)]
pub struct WriteSummary {
    pub save_id: i64,
    pub systems: usize,
    pub planets: usize,
    pub infrastructure: usize,
    pub unknown_conditions: usize,
    pub type_mappings: usize,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS saves(
                id INTEGER PRIMARY KEY,
                dir_name TEXT UNIQUE,
                path TEXT,
                character_name TEXT,
                save_date TEXT,
                game_version TEXT,
                character_level INTEGER,
                extracted_at TEXT
            );
            CREATE TABLE IF NOT EXISTS systems(
                id INTEGER PRIMARY KEY,
                save_id INTEGER REFERENCES saves(id) ON DELETE CASCADE,
                name TEXT,
                display_name TEXT,
                internal_id TEXT,
                x_ly REAL,
                y_ly REAL,
                dist_from_com_ly REAL,
                stable_points INTEGER,
                has_gate INTEGER,
                has_remnants INTEGER,
                remnant_damaged INTEGER,
                star_types TEXT,
                UNIQUE(save_id, name, internal_id)
            );
            CREATE TABLE IF NOT EXISTS planets(
                id INTEGER PRIMARY KEY,
                system_id INTEGER REFERENCES systems(id) ON DELETE CASCADE,
                name TEXT,
                internal_id TEXT,
                planet_type TEXT,
                mapped_vanilla_type TEXT,
                is_moon INTEGER,
                survey_level TEXT,
                owner_faction TEXT,
                radius REAL,
                ruins REAL,
                farmland REAL,
                rare_ores REAL,
                ores REAL,
                volatiles REAL,
                organics REAL,
                accessibility_percent REAL,
                hazard_percent REAL,
                hazard_incomplete INTEGER,
                no_atmosphere INTEGER,
                very_hot INTEGER,
                gas_giant INTEGER,
                habitable INTEGER,
                extreme_activity INTEGER,
                water INTEGER
            );
            CREATE TABLE IF NOT EXISTS planet_conditions(
                planet_id INTEGER REFERENCES planets(id) ON DELETE CASCADE,
                condition_id TEXT
            );
            CREATE TABLE IF NOT EXISTS system_tags(
                system_id INTEGER NOT NULL REFERENCES systems(id) ON DELETE CASCADE,
                tag TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS infrastructure(
                system_id INTEGER REFERENCES systems(id) ON DELETE CASCADE,
                infrastructure_type TEXT,
                is_domain INTEGER,
                is_damaged INTEGER
            );
            CREATE TABLE IF NOT EXISTS conditions(
                condition_id TEXT PRIMARY KEY,
                hazard REAL,
                cond_group TEXT,
                rank INTEGER,
                source TEXT
            );
            CREATE TABLE IF NOT EXISTS unknown_conditions(
                condition_id TEXT PRIMARY KEY,
                occurrences INTEGER,
                example_planet TEXT
            );
            CREATE TABLE IF NOT EXISTS planet_types(
                type_id TEXT PRIMARY KEY,
                source TEXT
            );
            CREATE TABLE IF NOT EXISTS planet_type_mappings(
                modded_type TEXT,
                vanilla_type TEXT,
                similarity REAL,
                modded_samples INTEGER,
                vanilla_samples INTEGER,
                PRIMARY KEY(modded_type, vanilla_type)
            );
            CREATE INDEX IF NOT EXISTS idx_systems_name ON systems(name);
            CREATE INDEX IF NOT EXISTS idx_systems_display_name ON systems(display_name);
            "#,
        )?;
        Ok(Self { conn })
    }

    pub fn write_extraction(
        &mut self,
        save: &SaveInfo,
        game_data: &GameData,
        mapped: &MappedOutput,
    ) -> Result<WriteSummary> {
        let tx = self.conn.transaction()?;
        let extracted_at = unix_seconds_string();

        tx.execute(
            r#"
            INSERT INTO saves(dir_name, path, character_name, save_date, game_version, character_level, extracted_at)
            VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(dir_name) DO UPDATE SET
                path=excluded.path,
                character_name=excluded.character_name,
                save_date=excluded.save_date,
                game_version=excluded.game_version,
                character_level=excluded.character_level,
                extracted_at=excluded.extracted_at
            "#,
            params![
                &save.dir_name,
                save.path.to_string_lossy().to_string(),
                &save.character_name,
                &save.save_date,
                &save.game_version,
                save.character_level as i64,
                extracted_at,
            ],
        )?;
        let save_id: i64 = tx.query_row(
            "SELECT id FROM saves WHERE dir_name = ?1",
            params![&save.dir_name],
            |row| row.get(0),
        )?;

        tx.execute("DELETE FROM systems WHERE save_id = ?1", params![save_id])?;
        tx.execute("DELETE FROM unknown_conditions", [])?;
        tx.execute("DELETE FROM conditions", [])?;
        tx.execute("DELETE FROM planet_types", [])?;
        tx.execute("DELETE FROM planet_type_mappings", [])?;

        for condition in game_data.conditions.values() {
            tx.execute(
                "INSERT INTO conditions(condition_id, hazard, cond_group, rank, source) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![
                    &condition.id,
                    condition.hazard,
                    &condition.group,
                    condition.rank.map(|v| v as i64),
                    &condition.source,
                ],
            )?;
        }
        for (type_id, spec) in &game_data.planet_types {
            tx.execute(
                "INSERT INTO planet_types(type_id, source) VALUES(?1, ?2)",
                params![type_id, &spec.source],
            )?;
        }
        for type_id in &mapped.unknown_types {
            tx.execute(
                "INSERT INTO planet_types(type_id, source) VALUES(?1, 'unknown')",
                params![type_id],
            )?;
        }
        for mapping in &mapped.type_mappings {
            tx.execute(
                "INSERT INTO planet_type_mappings(modded_type, vanilla_type, similarity, modded_samples, vanilla_samples) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![
                    &mapping.modded_type,
                    &mapping.vanilla_type,
                    mapping.similarity,
                    mapping.modded_samples as i64,
                    mapping.vanilla_samples as i64,
                ],
            )?;
        }
        for unknown in &mapped.unknown_conditions {
            tx.execute(
                "INSERT INTO unknown_conditions(condition_id, occurrences, example_planet) VALUES(?1, ?2, ?3)",
                params![&unknown.condition_id, unknown.occurrences as i64, &unknown.example_planet],
            )?;
        }

        let mut system_count = 0usize;
        let mut planet_count = 0usize;
        let mut infra_count = 0usize;

        for mapped_system in &mapped.systems {
            let system_id = insert_system(&tx, save_id, mapped_system)?;
            system_count += 1;
            for tag in &mapped_system.system.tags {
                tx.execute(
                    "INSERT INTO system_tags(system_id, tag) VALUES(?1, ?2)",
                    params![system_id, tag],
                )?;
            }
            for infra in &mapped_system.infrastructure {
                tx.execute(
                    "INSERT INTO infrastructure(system_id, infrastructure_type, is_domain, is_damaged) VALUES(?1, ?2, ?3, ?4)",
                    params![
                        system_id,
                        infra.infrastructure_type,
                        bool_to_i64(infra.is_domain),
                        bool_to_i64(infra.is_damaged),
                    ],
                )?;
                infra_count += 1;
            }

            for planet in &mapped_system.planets {
                let planet_id = insert_planet(&tx, system_id, planet)?;
                planet_count += 1;
                for condition_id in &planet.conditions {
                    tx.execute(
                        "INSERT INTO planet_conditions(planet_id, condition_id) VALUES(?1, ?2)",
                        params![planet_id, condition_id],
                    )?;
                }
            }
        }

        tx.commit()?;

        Ok(WriteSummary {
            save_id,
            systems: system_count,
            planets: planet_count,
            infrastructure: infra_count,
            unknown_conditions: mapped.unknown_conditions.len(),
            type_mappings: mapped.type_mappings.len(),
        })
    }

    pub fn list_saves(&self) -> Result<Vec<SaveRow>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, dir_name, path, character_name, save_date, game_version, character_level, extracted_at
            FROM saves
            ORDER BY save_date DESC, extracted_at DESC, dir_name ASC
            "#,
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SaveRow {
                    id: row.get(0)?,
                    dir_name: row.get(1)?,
                    path: row.get(2)?,
                    character_name: row.get(3)?,
                    save_date: row.get(4)?,
                    game_version: row.get(5)?,
                    character_level: row.get::<_, i64>(6)?,
                    extracted_at: row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn search_systems(
        &self,
        query: &str,
        save_dir_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchSystemRow>> {
        let query = query.to_lowercase();
        let save_dir_name = save_dir_name.map(|s| s.to_lowercase());

        let mut stmt = self.conn.prepare(
            r#"
            SELECT s.name, s.display_name, s.internal_id, s.x_ly, s.y_ly, s.dist_from_com_ly,
                   s.stable_points, s.has_gate, s.has_remnants, s.remnant_damaged,
                   s.star_types, sv.dir_name,
                   COUNT(p.id) AS planet_count
            FROM systems s
            JOIN saves sv ON sv.id = s.save_id
            LEFT JOIN planets p ON p.system_id = s.id
            GROUP BY s.id
            "#,
        )?;

        let mut rows = Vec::new();
        for row in stmt.query_map([], |row| {
            let save_dir_name: String = row.get(11)?;
            Ok(SearchCandidate {
                name: row.get(0)?,
                display_name: row.get(1)?,
                internal_id: row.get(2)?,
                x_ly: row.get(3)?,
                y_ly: row.get(4)?,
                dist_from_com_ly: row.get(5)?,
                stable_points: row.get::<_, i64>(6)? as u32,
                has_gate: row.get::<_, i64>(7)? != 0,
                has_remnants: row.get::<_, i64>(8)? != 0,
                remnant_damaged: row.get::<_, i64>(9)? != 0,
                star_types: {
                    let star_types: Option<String> = row.get(10)?;
                    split_csv(star_types.as_deref().unwrap_or(""))
                },
                save_dir_name,
                planet_count: row.get::<_, i64>(12)? as u32,
            })
        })? {
            let candidate = row?;
            if let Some(filter) = &save_dir_name {
                if candidate.save_dir_name.to_lowercase() != *filter {
                    continue;
                }
            }
            if let Some(rank) = search_rank(&candidate.name, &query)
                .or_else(|| search_rank(&candidate.display_name, &query))
            {
                rows.push((rank, candidate));
            }
        }

        rows.sort_by(|(a_rank, a), (b_rank, b)| {
            a_rank
                .cmp(b_rank)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| {
                    a.display_name
                        .to_lowercase()
                        .cmp(&b.display_name.to_lowercase())
                })
        });

        Ok(rows
            .into_iter()
            .take(limit)
            .map(|(_, candidate)| SearchSystemRow {
                save_dir_name: candidate.save_dir_name,
                name: candidate.name,
                display_name: candidate.display_name,
                internal_id: candidate.internal_id,
                x_ly: candidate.x_ly,
                y_ly: candidate.y_ly,
                dist_from_com_ly: candidate.dist_from_com_ly,
                stable_points: candidate.stable_points,
                has_gate: candidate.has_gate,
                has_remnants: candidate.has_remnants,
                remnant_damaged: candidate.remnant_damaged,
                star_types: candidate.star_types,
                planet_count: candidate.planet_count,
            })
            .collect())
    }

    pub fn system_discovery(&self, save: Option<&str>) -> Result<Vec<SystemDiscovery>> {
        let save_id = self.select_save_id(save)?;
        let mut stmt = self.conn.prepare(
            r#"
            SELECT s.name,
                   COUNT(p.id) AS planet_count,
                   SUM(CASE WHEN p.survey_level = 'FULL' THEN 1 ELSE 0 END) AS surveyed_full,
                   SUM(CASE WHEN p.survey_level IN ('PRELIMINARY', 'FULL') THEN 1 ELSE 0 END) AS surveyed_any,
                   EXISTS(
                       SELECT 1 FROM system_tags st
                       WHERE st.system_id = s.id AND st.tag = 'theme_core_populated'
                   ) AS is_core,
                   SUM(CASE
                       WHEN p.owner_faction IS NOT NULL
                            AND p.owner_faction NOT IN ('neutral', 'player')
                       THEN 1 ELSE 0
                   END) AS npc_colonized_planets
            FROM systems s
            LEFT JOIN planets p ON p.system_id = s.id
            WHERE s.save_id = ?1
            GROUP BY s.id
            ORDER BY s.name ASC
            "#,
        )?;
        let rows = stmt
            .query_map(params![save_id], |row| {
                Ok(SystemDiscovery {
                    system_name: row.get(0)?,
                    planet_count: row.get::<_, i64>(1)? as u32,
                    surveyed_full: row.get::<_, i64>(2)? as u32,
                    surveyed_any: row.get::<_, i64>(3)? as u32,
                    is_core: row.get::<_, i64>(4)? != 0,
                    npc_colonized_planets: row.get::<_, i64>(5)? as u32,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn export_csvs(
        &self,
        out_dir: impl AsRef<Path>,
        save_dir_name: Option<&str>,
        system_names: &[String],
    ) -> Result<()> {
        let out_dir = out_dir.as_ref();
        fs::create_dir_all(out_dir)?;
        let system_filter: Option<HashSet<_>> = if system_names.is_empty() {
            None
        } else {
            Some(
                system_names
                    .iter()
                    .map(|name| name.to_lowercase())
                    .collect(),
            )
        };
        let save_filter = save_dir_name.map(|s| s.to_lowercase());

        let systems = self.fetch_systems(save_filter.as_deref(), system_filter.as_ref())?;
        let mut planet_rows = Vec::new();
        let mut infra_rows = Vec::new();
        for system in &systems {
            planet_rows.extend(self.fetch_planets(system.id, &system.name)?);
            infra_rows.extend(self.fetch_infrastructure(system.id, &system.name)?);
        }

        write_planets_csv(out_dir.join("Planets.csv"), &planet_rows)?;
        write_systems_csv(out_dir.join("Systems.csv"), &systems)?;
        write_infrastructure_csv(out_dir.join("Infrastructure.csv"), &infra_rows)?;
        Ok(())
    }

    pub(crate) fn fetch_systems(
        &self,
        save_filter: Option<&str>,
        system_filter: Option<&HashSet<String>>,
    ) -> Result<Vec<SystemRowDb>> {
        let mut sql = String::from(
            r#"
            SELECT s.id, sv.dir_name, s.name, s.display_name, s.internal_id,
                   s.x_ly, s.y_ly, s.dist_from_com_ly, s.stable_points,
                   s.has_gate, s.has_remnants, s.remnant_damaged, s.star_types
            FROM systems s
            JOIN saves sv ON sv.id = s.save_id
            "#,
        );
        let mut conditions = Vec::new();
        if save_filter.is_some() {
            conditions.push("LOWER(sv.dir_name) = ?1");
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY s.name ASC, s.display_name ASC");

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = Vec::new();
        if let Some(filter) = save_filter {
            for row in stmt.query_map(params![filter], system_row_from_row)? {
                let row = row?;
                if let Some(filter) = system_filter {
                    if !filter.contains(&row.name.to_lowercase())
                        && !filter.contains(&row.display_name.to_lowercase())
                    {
                        continue;
                    }
                }
                rows.push(row);
            }
        } else {
            for row in stmt.query_map([], system_row_from_row)? {
                let row = row?;
                if let Some(filter) = system_filter {
                    if !filter.contains(&row.name.to_lowercase())
                        && !filter.contains(&row.display_name.to_lowercase())
                    {
                        continue;
                    }
                }
                rows.push(row);
            }
        }
        Ok(rows)
    }

    pub(crate) fn fetch_planets(
        &self,
        system_id: i64,
        system_name: &str,
    ) -> Result<Vec<PlanetRowDb>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, name, internal_id, planet_type, mapped_vanilla_type, is_moon,
                   survey_level, owner_faction, radius, ruins, farmland, rare_ores,
                   ores, volatiles, organics, accessibility_percent, hazard_percent,
                   hazard_incomplete, no_atmosphere, very_hot, gas_giant, habitable,
                   extreme_activity, water
            FROM planets
            WHERE system_id = ?1
            ORDER BY name ASC
            "#,
        )?;
        let rows = stmt
            .query_map(params![system_id], |row| {
                Ok(PlanetRowDb {
                    id: row.get(0)?,
                    system_name: system_name.to_string(),
                    name: row.get(1)?,
                    internal_id: row.get(2)?,
                    planet_type: row.get(3)?,
                    mapped_vanilla_type: row.get(4)?,
                    is_moon: row.get::<_, i64>(5)? != 0,
                    survey_level: row.get(6)?,
                    owner_faction: row.get(7)?,
                    radius: row.get(8)?,
                    ruins: row.get(9)?,
                    farmland: row.get(10)?,
                    rare_ores: row.get(11)?,
                    ores: row.get(12)?,
                    volatiles: row.get(13)?,
                    organics: row.get(14)?,
                    accessibility_percent: row.get(15)?,
                    hazard_percent: row.get(16)?,
                    hazard_incomplete: row.get::<_, i64>(17)? != 0,
                    no_atmosphere: row.get::<_, i64>(18)? != 0,
                    very_hot: row.get::<_, i64>(19)? != 0,
                    gas_giant: row.get::<_, i64>(20)? != 0,
                    habitable: row.get::<_, i64>(21)? != 0,
                    extreme_activity: row.get::<_, i64>(22)? != 0,
                    water: row.get::<_, i64>(23)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub(crate) fn fetch_infrastructure(
        &self,
        system_id: i64,
        system_name: &str,
    ) -> Result<Vec<InfraRowDb>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT infrastructure_type, is_domain, is_damaged
            FROM infrastructure
            WHERE system_id = ?1
            ORDER BY infrastructure_type ASC
            "#,
        )?;
        let rows = stmt
            .query_map(params![system_id], |row| {
                Ok(InfraRowDb {
                    system_name: system_name.to_string(),
                    infrastructure_type: row.get(0)?,
                    is_domain: row.get::<_, i64>(1)? != 0,
                    is_damaged: row.get::<_, i64>(2)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn select_save_id(&self, save: Option<&str>) -> Result<i64> {
        if let Some(save) = save {
            let needle = format!("%{}%", save.to_lowercase());
            let id = self
                .conn
                .query_row(
                    r#"
                    SELECT id
                    FROM saves
                    WHERE LOWER(dir_name) LIKE ?1 OR LOWER(character_name) LIKE ?1
                    ORDER BY save_date DESC, extracted_at DESC, dir_name ASC
                    LIMIT 1
                    "#,
                    params![needle],
                    |row| row.get(0),
                )
                .optional()?;
            return id.ok_or_else(|| ExtractError::NotFound(format!("no save matching {save}")));
        }

        let id = self
            .conn
            .query_row(
                r#"
                SELECT id
                FROM saves
                ORDER BY save_date DESC, extracted_at DESC, dir_name ASC
                LIMIT 1
                "#,
                [],
                |row| row.get(0),
            )
            .optional()?;
        id.ok_or_else(|| ExtractError::NotFound("no extracted saves found".to_string()))
    }
}

#[derive(Debug, Clone)]
struct SearchCandidate {
    name: String,
    display_name: String,
    internal_id: String,
    x_ly: Option<f64>,
    y_ly: Option<f64>,
    dist_from_com_ly: Option<f64>,
    stable_points: u32,
    has_gate: bool,
    has_remnants: bool,
    remnant_damaged: bool,
    star_types: Vec<String>,
    save_dir_name: String,
    planet_count: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct SystemRowDb {
    pub(crate) id: i64,
    pub(crate) save_dir_name: String,
    pub(crate) name: String,
    pub(crate) display_name: String,
    pub(crate) internal_id: String,
    pub(crate) x_ly: Option<f64>,
    pub(crate) y_ly: Option<f64>,
    pub(crate) dist_from_com_ly: Option<f64>,
    pub(crate) stable_points: u32,
    pub(crate) has_gate: bool,
    pub(crate) has_remnants: bool,
    pub(crate) remnant_damaged: bool,
    pub(crate) star_types: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PlanetRowDb {
    pub(crate) id: i64,
    pub(crate) system_name: String,
    pub(crate) name: String,
    pub(crate) internal_id: Option<String>,
    pub(crate) planet_type: String,
    pub(crate) mapped_vanilla_type: Option<String>,
    pub(crate) is_moon: bool,
    pub(crate) survey_level: Option<String>,
    pub(crate) owner_faction: Option<String>,
    pub(crate) radius: f64,
    pub(crate) ruins: Option<f64>,
    pub(crate) farmland: Option<f64>,
    pub(crate) rare_ores: Option<f64>,
    pub(crate) ores: Option<f64>,
    pub(crate) volatiles: Option<f64>,
    pub(crate) organics: Option<f64>,
    pub(crate) accessibility_percent: Option<f64>,
    pub(crate) hazard_percent: f64,
    pub(crate) hazard_incomplete: bool,
    pub(crate) no_atmosphere: bool,
    pub(crate) very_hot: bool,
    pub(crate) gas_giant: bool,
    pub(crate) habitable: bool,
    pub(crate) extreme_activity: bool,
    pub(crate) water: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct InfraRowDb {
    pub(crate) system_name: String,
    pub(crate) infrastructure_type: String,
    pub(crate) is_domain: bool,
    pub(crate) is_damaged: bool,
}

fn insert_system(
    tx: &rusqlite::Transaction<'_>,
    save_id: i64,
    system: &MappedSystem,
) -> Result<i64> {
    tx.execute(
        r#"
        INSERT INTO systems(
            save_id, name, display_name, internal_id, x_ly, y_ly, dist_from_com_ly,
            stable_points, has_gate, has_remnants, remnant_damaged, star_types
        ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#,
        params![
            save_id,
            &system.system.name,
            &system.system.display_name,
            &system.system.internal_id,
            system.system.x_ly,
            system.system.y_ly,
            system.system.dist_from_com_ly,
            system.system.stable_points as i64,
            bool_to_i64(system.system.has_gate),
            bool_to_i64(system.system.has_remnants),
            bool_to_i64(system.system.remnant_damaged),
            system.system.star_types.join(","),
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

fn insert_planet(
    tx: &rusqlite::Transaction<'_>,
    system_id: i64,
    planet: &crate::extract::model::PlanetRow,
) -> Result<i64> {
    tx.execute(
        r#"
        INSERT INTO planets(
            system_id, name, internal_id, planet_type, mapped_vanilla_type, is_moon,
            survey_level, owner_faction, radius, ruins, farmland, rare_ores, ores,
            volatiles, organics, accessibility_percent, hazard_percent,
            hazard_incomplete, no_atmosphere, very_hot, gas_giant, habitable,
            extreme_activity, water
        ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)
        "#,
        params![
            system_id,
            &planet.name,
            planet.internal_id.as_deref(),
            &planet.planet_type,
            planet.mapped_vanilla_type.as_deref(),
            bool_to_i64(planet.is_moon),
            planet.survey_level.as_deref(),
            planet.owner_faction.as_deref(),
            planet.radius,
            planet.ruins,
            planet.farmland,
            planet.rare_ores,
            planet.ores,
            planet.volatiles,
            planet.organics,
            planet.accessibility_percent,
            planet.hazard_percent,
            bool_to_i64(planet.hazard_incomplete),
            bool_to_i64(planet.no_atmosphere),
            bool_to_i64(planet.very_hot),
            bool_to_i64(planet.gas_giant),
            bool_to_i64(planet.habitable),
            bool_to_i64(planet.extreme_activity),
            bool_to_i64(planet.water),
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

fn system_row_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SystemRowDb> {
    Ok(SystemRowDb {
        id: row.get(0)?,
        save_dir_name: row.get(1)?,
        name: row.get(2)?,
        display_name: row.get(3)?,
        internal_id: row.get(4)?,
        x_ly: row.get(5)?,
        y_ly: row.get(6)?,
        dist_from_com_ly: row.get(7)?,
        stable_points: row.get::<_, i64>(8)? as u32,
        has_gate: row.get::<_, i64>(9)? != 0,
        has_remnants: row.get::<_, i64>(10)? != 0,
        remnant_damaged: row.get::<_, i64>(11)? != 0,
        star_types: {
            let star_types: Option<String> = row.get(12)?;
            split_csv(star_types.as_deref().unwrap_or(""))
        },
    })
}

fn split_csv(input: &str) -> Vec<String> {
    if input.trim().is_empty() {
        return Vec::new();
    }
    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    category: u8,
    detail: u32,
}

fn search_rank(text: &str, query: &str) -> Option<Rank> {
    let text = text.to_lowercase();
    if text.starts_with(query) {
        return Some(Rank {
            category: 0,
            detail: 0,
        });
    }
    if let Some(pos) = text.find(query) {
        return Some(Rank {
            category: 1,
            detail: pos as u32,
        });
    }
    let distance = levenshtein_limited(&text, query, 2)?;
    Some(Rank {
        category: 2,
        detail: distance,
    })
}

fn levenshtein_limited(a: &str, b: &str, limit: usize) -> Option<u32> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let len_diff = a.len().abs_diff(b.len());
    if len_diff > limit {
        return None;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        let mut row_min = curr[0];
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
            row_min = row_min.min(curr[j + 1]);
        }
        if row_min > limit {
            return None;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    if prev[b.len()] > limit {
        None
    } else {
        Some(prev[b.len()] as u32)
    }
}

fn bool_to_i64(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn unix_seconds_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn write_planets_csv(path: impl AsRef<Path>, rows: &[PlanetRowDb]) -> Result<()> {
    let mut writer = WriterBuilder::new().has_headers(false).from_path(path)?;
    writer.write_record([
        "Planet Name",
        "System Name",
        "Ruins",
        "Farmland",
        "Rare Ores",
        "Ores",
        "Volatiles",
        "Organics",
        "Accessibility Percent",
        "Hazard Percent",
        "No Atmosphere",
        "Very Hot",
        "Gas Giant",
        "Habitable",
        "Extreme Activity",
        "Water",
    ])?;
    for row in rows {
        writer.write_record(vec![
            row.name.clone(),
            row.system_name.clone(),
            opt_num(row.ruins),
            opt_num(row.farmland),
            opt_num(row.rare_ores),
            opt_num(row.ores),
            opt_num(row.volatiles),
            opt_num(row.organics),
            opt_num(row.accessibility_percent),
            num(row.hazard_percent),
            bool_str(row.no_atmosphere).to_string(),
            bool_str(row.very_hot).to_string(),
            bool_str(row.gas_giant).to_string(),
            bool_str(row.habitable).to_string(),
            bool_str(row.extreme_activity).to_string(),
            bool_str(row.water).to_string(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn write_systems_csv(path: impl AsRef<Path>, rows: &[SystemRowDb]) -> Result<()> {
    let mut writer = WriterBuilder::new().has_headers(false).from_path(path)?;
    writer.write_record(["System Name", "Stable Points", "Gate", "Remnants"])?;
    for row in rows {
        writer.write_record(vec![
            row.name.clone(),
            row.stable_points.to_string(),
            bool_str(row.has_gate).to_string(),
            bool_str(row.has_remnants).to_string(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn write_infrastructure_csv(path: impl AsRef<Path>, rows: &[InfraRowDb]) -> Result<()> {
    let mut writer = WriterBuilder::new().has_headers(false).from_path(path)?;
    writer.write_record([
        "system_name",
        "infrastructure_type",
        "is_domain",
        "is_damaged",
    ])?;
    for row in rows {
        // The existing Infrastructure.csv uses lowercase booleans, unlike the
        // other two files; mirror that exactly.
        writer.write_record(vec![
            row.system_name.clone(),
            row.infrastructure_type.clone(),
            if row.is_domain { "true" } else { "false" }.to_string(),
            if row.is_damaged { "true" } else { "false" }.to_string(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn opt_num(value: Option<f64>) -> String {
    value.map(num).unwrap_or_default()
}

fn num(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON {
        format!("{:.0}", value)
    } else {
        let mut s = value.to_string();
        if s.contains('.') {
            while s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.pop();
            }
        }
        s
    }
}

fn bool_str(value: bool) -> &'static str {
    if value {
        "TRUE"
    } else {
        "FALSE"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::gamedata::{ConditionSpec, GameData, PlanetTypeSpec};
    use crate::extract::mapping::map_save;
    use crate::extract::model::InfraRow;
    use crate::extract::model::{
        MappedOutput, MappedSystem, PlanetRow, RawEntity, RawPlanet, RawSave, RawSystem, SystemRow,
    };
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("system_solver_db_{name}_{unique}.sqlite"))
    }

    fn sample_game_data() -> GameData {
        let mut conditions = HashMap::new();
        for (id, feature_key, hazard) in [
            ("habitable", "habitable:1", -0.25),
            ("water_surface", "surface:1", 0.25),
        ] {
            conditions.insert(
                id.to_string(),
                ConditionSpec {
                    id: id.to_string(),
                    group: String::new(),
                    rank: None,
                    hazard,
                    source: "vanilla".to_string(),
                    feature_key: feature_key.to_string(),
                },
            );
        }
        let mut planet_types = HashMap::new();
        planet_types.insert(
            "water".to_string(),
            PlanetTypeSpec {
                source: "vanilla".to_string(),
                is_star_like: false,
            },
        );
        GameData {
            conditions,
            planet_types,
            planetary_conditions: Default::default(),
        }
    }

    fn sample_output() -> MappedOutput {
        MappedOutput {
            systems: vec![MappedSystem {
                system: SystemRow {
                    name: "Alpha".to_string(),
                    display_name: "Alpha Star System".to_string(),
                    internal_id: "1".to_string(),
                    x_ly: Some(0.0),
                    y_ly: Some(0.0),
                    dist_from_com_ly: Some(0.0),
                    stable_points: 1,
                    has_gate: true,
                    has_remnants: false,
                    remnant_damaged: false,
                    star_types: vec!["star_yellow".to_string()],
                    tags: vec![],
                },
                planets: vec![PlanetRow {
                    name: "A".to_string(),
                    internal_id: Some("A".to_string()),
                    planet_type: "water".to_string(),
                    mapped_vanilla_type: Some("water".to_string()),
                    is_moon: false,
                    survey_level: Some("FULL".to_string()),
                    owner_faction: Some("hegemony".to_string()),
                    radius: 100.0,
                    ruins: Some(-1.0),
                    farmland: None,
                    rare_ores: None,
                    ores: Some(1.0),
                    volatiles: None,
                    organics: None,
                    accessibility_percent: Some(99.0),
                    hazard_percent: 125.0,
                    hazard_incomplete: false,
                    no_atmosphere: false,
                    very_hot: false,
                    gas_giant: true,
                    habitable: true,
                    extreme_activity: false,
                    water: true,
                    conditions: vec!["habitable".to_string(), "water_surface".to_string()],
                }],
                infrastructure: vec![InfraRow {
                    infrastructure_type: "CommRelay".to_string(),
                    is_domain: false,
                    is_damaged: false,
                }],
            }],
            unknown_conditions: vec![],
            type_mappings: vec![],
            unknown_types: vec![],
        }
    }

    fn discovery_planet(
        name: &str,
        survey_level: Option<&str>,
        owner_faction: Option<&str>,
    ) -> PlanetRow {
        PlanetRow {
            name: name.to_string(),
            internal_id: Some(name.to_string()),
            planet_type: "water".to_string(),
            mapped_vanilla_type: Some("water".to_string()),
            is_moon: false,
            survey_level: survey_level.map(|s| s.to_string()),
            owner_faction: owner_faction.map(|s| s.to_string()),
            radius: 100.0,
            ruins: None,
            farmland: None,
            rare_ores: None,
            ores: None,
            volatiles: None,
            organics: None,
            accessibility_percent: None,
            hazard_percent: 100.0,
            hazard_incomplete: false,
            no_atmosphere: false,
            very_hot: false,
            gas_giant: false,
            habitable: false,
            extreme_activity: false,
            water: false,
            conditions: vec![],
        }
    }

    fn discovery_output() -> MappedOutput {
        MappedOutput {
            systems: vec![
                MappedSystem {
                    system: SystemRow {
                        name: "Core".to_string(),
                        display_name: "Core Star System".to_string(),
                        internal_id: "core".to_string(),
                        x_ly: None,
                        y_ly: None,
                        dist_from_com_ly: None,
                        stable_points: 0,
                        has_gate: false,
                        has_remnants: false,
                        remnant_damaged: false,
                        star_types: vec![],
                        tags: vec!["theme_core".to_string(), "theme_core_populated".to_string()],
                    },
                    planets: vec![
                        discovery_planet("Core Null", None, Some("neutral")),
                        discovery_planet("Core Seen", Some("SEEN"), Some("hegemony")),
                        discovery_planet("Core Preliminary", Some("PRELIMINARY"), None),
                        discovery_planet("Core Full", Some("FULL"), Some("player")),
                    ],
                    infrastructure: vec![],
                },
                MappedSystem {
                    system: SystemRow {
                        name: "Fringe".to_string(),
                        display_name: "Fringe Star System".to_string(),
                        internal_id: "fringe".to_string(),
                        x_ly: None,
                        y_ly: None,
                        dist_from_com_ly: None,
                        stable_points: 0,
                        has_gate: false,
                        has_remnants: false,
                        remnant_damaged: false,
                        star_types: vec![],
                        tags: vec!["theme_misc".to_string()],
                    },
                    planets: vec![discovery_planet(
                        "Fringe Full",
                        Some("FULL"),
                        Some("hegemony"),
                    )],
                    infrastructure: vec![],
                },
            ],
            unknown_conditions: vec![],
            type_mappings: vec![],
            unknown_types: vec![],
        }
    }

    #[test]
    fn write_search_and_export_round_trip() {
        let db_path = temp_db_path("round_trip");
        let mut db = Db::open(&db_path).unwrap();
        let save = SaveInfo {
            dir_name: "save_alpha".to_string(),
            path: std::path::PathBuf::from("/tmp/save_alpha"),
            character_name: "Alpha".to_string(),
            save_date: "2026-06-09 00:00:00.000 UTC".to_string(),
            game_version: "0.98a".to_string(),
            character_level: 15,
            compressed: false,
            modified: SystemTime::now(),
        };

        let summary = db
            .write_extraction(&save, &sample_game_data(), &sample_output())
            .unwrap();
        assert_eq!(summary.systems, 1);
        assert_eq!(db.list_saves().unwrap().len(), 1);
        let hits = db.search_systems("Alp", Some("save_alpha"), 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Alpha");

        let out_dir = db_path.with_extension("out");
        db.export_csvs(&out_dir, Some("save_alpha"), &[]).unwrap();
        assert!(out_dir.join("Planets.csv").exists());
        assert!(out_dir.join("Systems.csv").exists());
        assert!(out_dir.join("Infrastructure.csv").exists());

        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn system_discovery_round_trips_tags_and_survey_metadata() {
        let db_path = temp_db_path("discovery");
        let mut db = Db::open(&db_path).unwrap();
        let save = SaveInfo {
            dir_name: "save_alpha".to_string(),
            path: std::path::PathBuf::from("/tmp/save_alpha"),
            character_name: "Alpha".to_string(),
            save_date: "2026-06-09 00:00:00.000 UTC".to_string(),
            game_version: "0.98a".to_string(),
            character_level: 15,
            compressed: false,
            modified: SystemTime::now(),
        };

        let output = discovery_output();
        db.write_extraction(&save, &sample_game_data(), &output)
            .unwrap();
        db.write_extraction(&save, &sample_game_data(), &output)
            .unwrap();

        let tag_count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM system_tags", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tag_count, 3);

        let rows = db.system_discovery(Some("alpha")).unwrap();
        assert_eq!(rows.len(), 2);

        let core = &rows[0];
        assert_eq!(core.system_name, "Core");
        assert_eq!(core.planet_count, 4);
        assert_eq!(core.surveyed_full, 1);
        assert_eq!(core.surveyed_any, 2);
        assert!(core.is_core);
        assert_eq!(core.npc_colonized_planets, 1);

        let fringe = &rows[1];
        assert_eq!(fringe.system_name, "Fringe");
        assert_eq!(fringe.planet_count, 1);
        assert_eq!(fringe.surveyed_full, 1);
        assert_eq!(fringe.surveyed_any, 1);
        assert!(!fringe.is_core);
        assert_eq!(fringe.npc_colonized_planets, 1);

        let _ = fs::remove_file(&db_path);
    }
}
