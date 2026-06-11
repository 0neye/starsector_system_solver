use std::collections::BTreeMap;
use std::time::Duration;

use crate::extract::db::{SaveRow, SystemDiscovery};
use crate::rank::{RankRow, RankScorer};
use crate::solve::{solve_goal, solve_maximize};
use crate::solver::state::format_action;
use crate::solver::pareto::ParetoSolve;
use crate::solver::{Action, Goal, Metric};
use crate::system::System;
use crate::tui::app::{
    estimate_rank_cost, filter_scope, format_system_time, group_plan_actions, solve_cache_key,
    App, RankCache, ScopeMode, SolveMode, SolveParams,
};
use crate::tui::config::{DiscoveryDefinition, TuiConfig};
use crate::constants::{ColonyItem, FacilityType};

use super::support::{rich_balance, single_planet_system, PlanetBuilder};

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
fn tui_action_formatter_covers_every_action_variant() {
    let planet = PlanetBuilder::new("Corvus II");
    let hash = planet.name_hash();
    let names = std::collections::HashMap::from([(hash, "Corvus II".to_string())]);

    assert_eq!(
        format_action(&Action::Colonize(hash), &names),
        "Colonize Corvus II"
    );
    assert_eq!(
        format_action(&Action::AddFacility(hash, FacilityType::Megaport), &names),
        "Build Megaport on Corvus II"
    );
    assert_eq!(
        format_action(&Action::InstallItem(hash, FacilityType::HeavyIndustry, ColonyItem::CorruptedNanoforge), &names),
        "Install Corrupted Nanoforge in Heavy Industry on Corvus II"
    );
    assert_eq!(
        format_action(&Action::AddImprovement(hash, FacilityType::Megaport), &names),
        "Improve Megaport on Corvus II"
    );
    assert_eq!(
        format_action(&Action::AddAlphaCore(hash, FacilityType::Megaport), &names),
        "Install alpha core in Megaport on Corvus II"
    );
    assert_eq!(
        format_action(&Action::SetFreePort(hash, true), &names),
        "Enable free port on Corvus II"
    );
    assert_eq!(
        format_action(&Action::SetFreePort(hash, false), &names),
        "Disable free port on Corvus II"
    );
    assert_eq!(
        format_action(&Action::SetHazardPay(hash, true), &names),
        "Enable hazard pay on Corvus II"
    );
    assert_eq!(
        format_action(&Action::SetHazardPay(hash, false), &names),
        "Disable hazard pay on Corvus II"
    );
    assert_eq!(
        format_action(&Action::UpgradeAdmin(hash), &names),
        "Install alpha-core administrator on Corvus II"
    );
    assert_eq!(
        format_action(&Action::BuildMakeshiftCommRelay, &names),
        "Build makeshift comm relay"
    );
    assert_eq!(format_action(&Action::Wait(3), &names), "Wait 3 months");
    assert_eq!(
        format_action(&Action::Colonize(99), &names),
        "Colonize 99"
    );
}

#[test]
fn tui_plan_grouping_folds_waits_into_month_headers() {
    let planet = PlanetBuilder::new("Corvus II");
    let hash = planet.name_hash();
    let names = std::collections::HashMap::from([(hash, "Corvus II".to_string())]);
    let rows = group_plan_actions(
        &[
            Action::Colonize(hash),
            Action::Wait(3),
            Action::AddFacility(hash, FacilityType::Megaport),
            Action::Wait(2),
            Action::AddImprovement(hash, FacilityType::Megaport),
        ],
        &names,
    );

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].month, 0);
    assert_eq!(rows[0].text, "Colonize Corvus II");
    assert_eq!(rows[1].month, 3);
    assert_eq!(rows[1].text, "Build Megaport on Corvus II");
    assert_eq!(rows[2].month, 5);
    assert_eq!(rows[2].text, "Improve Megaport on Corvus II");
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
fn tui_solve_cache_key_includes_mode_params_and_balance() {
    let config = TuiConfig::default();
    let mut params = SolveParams::from_config(&config);
    let base = solve_cache_key("Alpha", &params, &config.balance_signature());

    params.mode = SolveMode::Goal;
    let changed_mode = solve_cache_key("Alpha", &params, &config.balance_signature());
    params.goal_income += 1.0;
    let changed_param = solve_cache_key("Alpha", &params, &config.balance_signature());

    let mut changed_config = config.clone();
    changed_config.credits += 1.0;
    let changed_balance = solve_cache_key("Alpha", &params, &changed_config.balance_signature());

    assert_ne!(base, changed_mode);
    assert_ne!(changed_mode, changed_param);
    assert_ne!(changed_param, changed_balance);
}

#[test]
fn solve_wrappers_replay_tiny_fixture_results() {
    let system = single_planet_system(PlanetBuilder::new("Tiny I").build());
    let balance = rich_balance();

    let goal = Goal::new(0.0, None, None);
    let goal_outcome = solve_goal(&system, &balance, &goal, 1).expect("goal should be reachable");
    assert!(goal_outcome.achieved_income >= 0.0);

    let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(0));
    let max_outcome = solve_maximize(&system, &balance, Metric::Income, &floors, 1, 1)
        .expect("maximize should return a tiny result");
    assert!(max_outcome.months >= 0);
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
