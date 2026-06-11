//! CSV- and DB-loading regression tests against the bundled game data.

use crate::parser::{load_game_data, load_game_data_from_db};
use crate::system::Infrastructure;

/// The bundled `Infrastructure.csv` spells nav buoys with Starsector's internal
/// typo "NavBouy" (e.g. the Mia Bravos row). The parser must accept that spelling
/// — matching only "NavBuoy" silently dropped the row into the unknown-type arm,
/// so the system lost its nav buoy. (Regression: P2 nav-buoy parsing.)
#[test]
fn nav_buoy_csv_spelling_loads() {
    let systems = load_game_data("Planets.csv", "Systems.csv", "Infrastructure.csv")
        .expect("game data CSVs load from the crate root during tests");

    let mia = systems
        .get("Mia Bravos")
        .expect("Mia Bravos is present in Planets.csv");

    let has_nav_buoy = mia
        .infrastructure()
        .values()
        .any(|i| matches!(i, Infrastructure::NavBuoy { .. }));

    assert!(
        has_nav_buoy,
        "Mia Bravos should expose its NavBouy infrastructure after loading"
    );
}

#[test]
fn systems_csv_stable_points_load() {
    let systems = load_game_data("Planets.csv", "Systems.csv", "Infrastructure.csv")
        .expect("game data CSVs load from the crate root during tests");

    let mia = systems
        .get("Mia Bravos")
        .expect("Mia Bravos is present in Planets.csv");

    assert_eq!(mia.stable_points(), 3);
}

/// The DB loader must produce the same systems the CSV round trip
/// (`extract export` + `load_game_data`) would: numeric columns present only
/// when non-NULL, boolean columns always present as 1.0/0.0, infrastructure
/// variants and stable points intact.
#[test]
fn db_loader_matches_csv_semantics() {
    use crate::extract::db::Db;
    use crate::extract::gamedata::GameData;
    use crate::extract::model::{
        InfraRow, MappedOutput, MappedSystem, PlanetRow, SaveInfo, SystemRow,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = std::env::temp_dir().join(format!("system_solver_parser_db_{unique}.sqlite"));

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
    let game_data = GameData {
        conditions: Default::default(),
        planet_types: Default::default(),
        planetary_conditions: Default::default(),
    };
    let output = MappedOutput {
        systems: vec![MappedSystem {
            system: SystemRow {
                name: "Alpha".to_string(),
                display_name: "Alpha Star System".to_string(),
                internal_id: "1".to_string(),
                x_ly: Some(0.0),
                y_ly: Some(0.0),
                dist_from_com_ly: Some(0.0),
                stable_points: 2,
                has_gate: true,
                has_remnants: false,
                remnant_damaged: false,
                star_types: vec!["star_yellow".to_string()],
                tags: vec![],
            },
            planets: vec![PlanetRow {
                name: "A".to_string(),
                internal_id: Some("A".to_string()),
                planet_type: "terran".to_string(),
                mapped_vanilla_type: Some("terran".to_string()),
                is_moon: false,
                survey_level: Some("FULL".to_string()),
                owner_faction: None,
                radius: 100.0,
                ruins: Some(-1.0),
                farmland: Some(1.0),
                rare_ores: None,
                ores: Some(0.0),
                volatiles: None,
                organics: Some(2.0),
                accessibility_percent: Some(70.0),
                hazard_percent: 150.0,
                hazard_incomplete: false,
                no_atmosphere: false,
                very_hot: false,
                gas_giant: false,
                habitable: true,
                extreme_activity: false,
                water: false,
                conditions: vec!["habitable".to_string()],
            }],
            infrastructure: vec![
                InfraRow {
                    infrastructure_type: "NavBouy".to_string(),
                    is_domain: false,
                    is_damaged: false,
                },
                InfraRow {
                    infrastructure_type: "Gate".to_string(),
                    is_domain: true,
                    is_damaged: false,
                },
                InfraRow {
                    // Forward-compatible extra type: must be skipped, not error.
                    infrastructure_type: "CoronalTap".to_string(),
                    is_domain: true,
                    is_damaged: false,
                },
            ],
        }],
        unknown_conditions: vec![],
        type_mappings: vec![],
        unknown_types: vec![],
    };

    let mut db = Db::open(&db_path).unwrap();
    db.write_extraction(&save, &game_data, &output).unwrap();
    drop(db);

    let systems = load_game_data_from_db(&db_path, None).unwrap();
    let _ = std::fs::remove_file(&db_path);

    let alpha = systems.get("Alpha").expect("system loads by name");
    assert_eq!(alpha.stable_points(), 2);
    assert_eq!(alpha.planets().len(), 1);

    let planet = alpha.planets().values().next().unwrap();
    let props = planet.properties();
    // Non-NULL numeric columns present with their values; NULL columns absent.
    assert_eq!(props.get("ruins"), Some(&-1.0));
    assert_eq!(props.get("farmland"), Some(&1.0));
    assert_eq!(props.get("ores"), Some(&0.0));
    assert_eq!(props.get("organics"), Some(&2.0));
    assert_eq!(props.get("accessibility percent"), Some(&70.0));
    assert_eq!(props.get("hazard percent"), Some(&150.0));
    assert!(!props.contains_key("rare ores"));
    assert!(!props.contains_key("volatiles"));
    // Booleans always present as 1.0/0.0 (CSV TRUE/FALSE parity).
    assert_eq!(props.get("habitable"), Some(&1.0));
    assert_eq!(props.get("gas giant"), Some(&0.0));
    assert_eq!(props.get("water"), Some(&0.0));

    let has_nav_buoy = alpha
        .infrastructure()
        .values()
        .any(|i| matches!(i, Infrastructure::NavBuoy { .. }));
    let has_gate = alpha
        .infrastructure()
        .values()
        .any(|i| matches!(i, Infrastructure::Gate));
    assert!(has_nav_buoy && has_gate);
    assert_eq!(alpha.infrastructure().len(), 2, "CoronalTap row is skipped");
}
