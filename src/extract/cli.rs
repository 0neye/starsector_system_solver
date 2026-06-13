//! Save-extraction CLI subcommands for `system_solver extract ...`.
//! See workspace/SAVE_EXTRACTION_DESIGN.md.

use std::io::IsTerminal;
use std::path::PathBuf;

use clap::Subcommand;

use crate::extract::db::Db;
use crate::extract::gamedata::load_game_data;
use crate::extract::locate;
use crate::extract::mapping::map_save;
use crate::extract::model::{MappedOutput, SaveInfo};
use crate::extract::save::{discover_saves, load_campaign_xml};
use crate::extract::scan::scan_save;
use crate::extract::{ExtractError, Result};

pub const DEFAULT_DB_PATH: &str = "save_data.db";
pub const DEFAULT_EXPORT_DIR: &str = "out";

#[derive(Subcommand, Debug)]
pub enum ExtractCommand {
    /// List Starsector saves on disk
    ListSaves {
        /// Saves directory. Defaults to `<starsector-dir>/saves` of the
        /// auto-detected install (STARSECTOR_DIR env var or common locations).
        #[arg(long)]
        saves_dir: Option<PathBuf>,
    },
    /// Parse a save's campaign.xml and write systems/planets/infrastructure to the DB
    Run {
        /// Saves directory. Defaults to `<starsector-dir>/saves`.
        #[arg(long)]
        saves_dir: Option<PathBuf>,
        #[arg(long)]
        save: Option<String>,
        #[arg(long)]
        latest: bool,
        /// Starsector install directory. When omitted, auto-detected from the
        /// STARSECTOR_DIR environment variable or common install locations.
        #[arg(long)]
        starsector_dir: Option<PathBuf>,
        #[arg(long, default_value = DEFAULT_DB_PATH)]
        db: PathBuf,
        #[arg(long)]
        system: Vec<String>,
    },
    /// Search extracted systems by name
    Search {
        query: String,
        #[arg(long, default_value = DEFAULT_DB_PATH)]
        db: PathBuf,
        #[arg(long)]
        save: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Export extracted data as Planets/Systems/Infrastructure CSVs
    Export {
        #[arg(long, default_value = DEFAULT_DB_PATH)]
        db: PathBuf,
        #[arg(long)]
        save: Option<String>,
        #[arg(long, default_value = DEFAULT_EXPORT_DIR)]
        out_dir: PathBuf,
        #[arg(long)]
        system: Vec<String>,
    },
}

pub fn run(command: ExtractCommand) -> Result<()> {
    match command {
        ExtractCommand::ListSaves { saves_dir } => {
            let saves_dir = match saves_dir {
                Some(dir) => dir,
                None => locate::default_saves_dir(&locate::resolve_starsector_dir(None)?),
            };
            let saves = discover_saves(&saves_dir)?;
            print_saves(&saves);
        }
        ExtractCommand::Run {
            saves_dir,
            save,
            latest,
            starsector_dir,
            db,
            system,
        } => {
            let starsector_dir = locate::resolve_starsector_dir(starsector_dir.as_deref())?;
            let saves_dir = saves_dir.unwrap_or_else(|| locate::default_saves_dir(&starsector_dir));
            let saves = discover_saves(&saves_dir)?;
            let save = select_save(&saves, save.as_deref(), latest)?;
            let campaign_xml = load_campaign_xml(save)?;
            let raw = scan_save(&campaign_xml)?;
            let game_data = load_game_data(&starsector_dir)?;
            let mapped = filter_mapped_output(map_save(raw, &game_data), &system);
            let mut db = Db::open(&db)?;
            let summary = db.write_extraction(save, &game_data, &mapped)?;
            print_run_summary(save, &summary, &mapped, &system);
        }
        ExtractCommand::Search {
            query,
            db,
            save,
            limit,
        } => {
            let db = Db::open(&db)?;
            let results = db.search_systems(&query, save.as_deref(), limit)?;
            print_search_results(&results);
        }
        ExtractCommand::Export {
            db,
            save,
            out_dir,
            system,
        } => {
            let db = Db::open(&db)?;
            db.export_csvs(&out_dir, save.as_deref(), &system)?;
            println!("exported CSVs to {}", out_dir.display());
        }
    }
    Ok(())
}

fn select_save<'a>(
    saves: &'a [SaveInfo],
    needle: Option<&str>,
    latest: bool,
) -> Result<&'a SaveInfo> {
    if saves.is_empty() {
        return Err(ExtractError::NotFound("no saves found".to_string()));
    }

    if let Some(needle) = needle {
        let needle = needle.to_lowercase();
        let mut matches = saves.iter().filter(|save| {
            save.dir_name.to_lowercase().contains(&needle)
                || save.character_name.to_lowercase().contains(&needle)
        });
        if let Some(save) = matches.next() {
            return Ok(save);
        }
        return Err(ExtractError::NotFound(format!("no save matching {needle}")));
    }

    if latest || !std::io::stdin().is_terminal() {
        return Ok(&saves[0]);
    }

    Ok(&saves[0])
}

fn print_saves(saves: &[SaveInfo]) {
    println!("dir_name\tcharacter_name\tlevel\tsave_date\tgame_version");
    for save in saves {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            save.dir_name,
            save.character_name,
            save.character_level,
            save.save_date,
            save.game_version
        );
    }
}

fn print_run_summary(
    save: &SaveInfo,
    summary: &crate::extract::db::WriteSummary,
    mapped: &MappedOutput,
    filter_systems: &[String],
) {
    println!("save: {} ({})", save.dir_name, save.character_name);
    println!(
        "systems={} planets={} infrastructure={} unknown_conditions={}",
        summary.systems,
        summary.planets,
        summary.infrastructure,
        summary.unknown_conditions
    );
    if !filter_systems.is_empty() {
        println!("system filter: {}", filter_systems.join(", "));
    }
    if !mapped.unknown_conditions.is_empty() {
        println!("unknown conditions:");
        for unknown in &mapped.unknown_conditions {
            println!(
                "  {} ({}x, example {})",
                unknown.condition_id, unknown.occurrences, unknown.example_planet
            );
        }
    }
}

fn filter_mapped_output(mut mapped: MappedOutput, systems: &[String]) -> MappedOutput {
    if systems.is_empty() {
        return mapped;
    }
    let filters: Vec<String> = systems.iter().map(|s| s.to_lowercase()).collect();
    mapped.systems.retain(|system| {
        let name = system.system.name.to_lowercase();
        let display_name = system.system.display_name.to_lowercase();
        filters
            .iter()
            .any(|filter| name == *filter || display_name == *filter)
    });
    mapped
}

fn print_search_results(results: &[crate::extract::db::SearchSystemRow]) {
    if results.is_empty() {
        println!("no matches");
        return;
    }
    println!("save_dir\tname\tdisplay_name\tstars\tplanets\tstable\tgate\tremnants");
    for row in results {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.save_dir_name,
            row.name,
            row.display_name,
            row.star_types.join(","),
            row.planet_count,
            row.stable_points,
            row.has_gate,
            row.has_remnants
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::time::{Duration, UNIX_EPOCH};

    fn sample_save(dir_name: &str, character_name: &str, modified_secs: u64) -> SaveInfo {
        SaveInfo {
            dir_name: dir_name.to_string(),
            path: PathBuf::from(dir_name),
            character_name: character_name.to_string(),
            save_date: "2026-06-09 00:00:00.000 UTC".to_string(),
            game_version: "0.98a".to_string(),
            character_level: 1,
            compressed: false,
            modified: UNIX_EPOCH + Duration::from_secs(modified_secs),
        }
    }

    #[test]
    fn picks_matching_save_by_recent_order() {
        let saves = vec![
            sample_save("save_old", "Alpha", 1),
            sample_save("save_new", "Beta Match", 2),
        ];
        let picked = select_save(&saves, Some("match"), false).unwrap();
        assert_eq!(picked.dir_name, "save_new");
    }

    #[test]
    fn cli_parses_subcommands() {
        #[derive(Parser, Debug)]
        struct TestCli {
            #[command(subcommand)]
            command: ExtractCommand,
        }
        let cli = TestCli::parse_from(["extract", "list-saves"]);
        match cli.command {
            ExtractCommand::ListSaves { .. } => {}
            _ => panic!("expected list-saves"),
        }
    }
}
