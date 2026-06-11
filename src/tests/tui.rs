use std::collections::BTreeMap;
use std::time::Duration;

use crate::extract::db::{SaveRow, SystemDiscovery};
use crate::rank::{RankRow, RankScorer};
use crate::solver::pareto::ParetoSolve;
use crate::system::System;
use crate::tui::app::{
    estimate_rank_cost, filter_scope, format_system_time, App, RankCache, ScopeMode,
};
use crate::tui::config::{DiscoveryDefinition, TuiConfig};

fn discovery(
    name: &str,
    planet_count: u32,
    surveyed_any: u32,
    surveyed_full: u32,
    is_core: bool,
) -> SystemDiscovery {
    SystemDiscovery {
        system_name: name.to_string(),
        planet_count,
        surveyed_full,
        surveyed_any,
        is_core,
        npc_colonized_planets: 0,
    }
}

fn solve_with_score(score: f64) -> ParetoSolve {
    ParetoSolve {
        stability_frontier: Vec::new(),
        defense_frontier: Vec::new(),
        stability_auc: 0.0,
        defense_auc: 0.0,
        score,
        recommendation: None,
    }
}

fn rank_row(system: &str, score: f64) -> RankRow {
    RankRow {
        system: system.to_string(),
        solve: solve_with_score(score),
        seconds: 0.1,
    }
}

#[test]
fn tui_scope_filters_discovery_definition_and_core_worlds() {
    let rows = vec![
        discovery("partial", 3, 1, 0, false),
        discovery("full", 2, 2, 2, false),
        discovery("core", 1, 1, 1, true),
        discovery("hidden", 2, 0, 0, false),
    ];

    let names = filter_scope(
        &rows,
        DiscoveryDefinition::AtLeastOneSurveyed,
        false,
        ScopeMode::Discovered,
    );
    assert_eq!(
        names.into_iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["partial", "full"]
    );

    let names = filter_scope(
        &rows,
        DiscoveryDefinition::FullySurveyed,
        true,
        ScopeMode::Discovered,
    );
    assert_eq!(
        names.into_iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["full", "core"]
    );

    let names = filter_scope(
        &rows,
        DiscoveryDefinition::AtLeastOneSurveyed,
        false,
        ScopeMode::All,
    );
    assert_eq!(
        names.into_iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["partial", "full", "hidden"]
    );
}

#[test]
fn tui_rank_cost_estimate_matches_m1_constants() {
    assert_eq!(estimate_rank_cost(3, RankScorer::Quick), Duration::from_secs(30));
    assert_eq!(estimate_rank_cost(3, RankScorer::Bound), Duration::from_secs(3));
    assert_eq!(
        estimate_rank_cost(10, RankScorer::Template),
        Duration::from_millis(1500)
    );
}

#[test]
fn tui_mark_rank_stale_marks_existing_caches() {
    let config = TuiConfig::default();
    let mut app = App::new(config.clone(), None);
    app.rank_cache.push(RankCache {
        save: "save_alpha".to_string(),
        scorer: RankScorer::Quick,
        rows: vec![RankRow {
            system: "Alpha".to_string(),
            solve: ParetoSolve {
                stability_frontier: Vec::new(),
                defense_frontier: Vec::new(),
                stability_auc: 0.0,
                defense_auc: 0.0,
                score: 1.0,
                recommendation: None,
            },
            seconds: 0.1,
        }],
        stale: false,
        signature: config.balance_signature(),
    });

    app.mark_rank_stale();

    assert!(app.rank_cache[0].stale);
    assert!(app.status.contains("scores are stale"));
}

#[test]
fn tui_scorer_change_restores_cached_rows_and_resets_missing_selection() {
    let config = TuiConfig::default();
    let signature = config.balance_signature();
    let mut app = App::new(config, None);
    app.active_save = Some(SaveRow {
        id: 1,
        dir_name: "save_alpha".to_string(),
        path: String::new(),
        character_name: "Alpha".to_string(),
        save_date: String::new(),
        game_version: String::new(),
        character_level: 1,
        extracted_at: String::new(),
    });
    app.systems
        .insert("Alpha".to_string(), System::new("Alpha".to_string()));
    app.systems
        .insert("Beta".to_string(), System::new("Beta".to_string()));
    app.discovery = vec![
        discovery("Alpha", 1, 1, 1, false),
        discovery("Beta", 1, 1, 1, false),
    ];
    app.rank_rows = vec![rank_row("Alpha", 1.0), rank_row("Beta", 2.0)];
    app.rank_selection = 1;
    app.scorer_picker_original = Some(RankScorer::Quick);
    app.scorer = RankScorer::Template;
    app.rank_cache.push(RankCache {
        save: "save_alpha".to_string(),
        scorer: RankScorer::Template,
        rows: vec![rank_row("Alpha", 10.0)],
        stale: false,
        signature,
    });

    app.close_scorer_picker();

    assert_eq!(app.rank_rows.len(), 1);
    assert_eq!(app.rank_rows[0].system, "Alpha");
    assert_eq!(app.rank_selection, 0);
    assert_eq!(app.status, "scorer: template - r to re-rank");
}

#[test]
fn tui_format_system_time_renders_utc_date() {
    assert_eq!(format_system_time(std::time::UNIX_EPOCH), "1970-01-01");
    let recent = std::time::UNIX_EPOCH + Duration::from_secs(1_717_200_000);
    assert_eq!(format_system_time(recent), "2024-06-01");
}

#[test]
fn tui_config_round_trips_toml() {
    let path = std::env::temp_dir().join(format!(
        "system_solver_tui_config_test_{}_{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let config = TuiConfig {
        credits: 123.0,
        story_points: 7,
        discovery_definition: DiscoveryDefinition::FullySurveyed,
        include_core_worlds: true,
        colony_items: BTreeMap::from([("soil nanites".to_string(), 2)]),
        ..Default::default()
    };

    config.save(&path).unwrap();
    let (loaded, status) = TuiConfig::load(&path);
    let _ = std::fs::remove_file(&path);

    assert_eq!(status, None);
    assert_eq!(loaded, config);
}
