use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::rank::{filter_system_names, rank_systems, sort_rows_best_first, RankRow, RankScorer};
use crate::solver::pareto::ParetoSolve;
use crate::system::System;

use super::support::{rich_balance, single_planet_system, PlanetBuilder};

fn named_system(name: &str) -> System {
    single_planet_system(PlanetBuilder::new(&format!("{name} I")).build())
}

fn system_map(names: &[&str]) -> HashMap<String, System> {
    names
        .iter()
        .map(|name| ((*name).to_string(), named_system(name)))
        .collect()
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

#[test]
fn filter_system_names_matches_case_insensitive_any_filter_sorted() {
    let systems = system_map(&["Gamma", "alpha", "Beta"]);
    let filters = vec!["ALP".to_string(), "amm".to_string()];

    let names = filter_system_names(&systems, &filters).expect("filters should match");
    let names: Vec<&str> = names.iter().map(|name| name.as_str()).collect();

    assert_eq!(names, vec!["Gamma", "alpha"]);
}

#[test]
fn filter_system_names_empty_filters_returns_everything_sorted() {
    let systems = system_map(&["Gamma", "alpha", "Beta"]);

    let names = filter_system_names(&systems, &[]).expect("empty filters should match all");
    let names: Vec<&str> = names.iter().map(|name| name.as_str()).collect();

    assert_eq!(names, vec!["Beta", "Gamma", "alpha"]);
}

#[test]
fn filter_system_names_reports_no_match() {
    let systems = system_map(&["Alpha", "Beta"]);
    let filters = vec!["missing".to_string()];

    assert!(filter_system_names(&systems, &filters).is_err());
}

#[test]
fn sort_rows_best_first_breaks_equal_scores_by_name() {
    let mut rows = vec![
        RankRow {
            system: "Beta".to_string(),
            solve: solve_with_score(10.0),
            seconds: 0.0,
        },
        RankRow {
            system: "Gamma".to_string(),
            solve: solve_with_score(11.0),
            seconds: 0.0,
        },
        RankRow {
            system: "Alpha".to_string(),
            solve: solve_with_score(10.0),
            seconds: 0.0,
        },
    ];

    sort_rows_best_first(&mut rows);
    let names: Vec<&str> = rows.iter().map(|row| row.system.as_str()).collect();

    assert_eq!(names, vec!["Gamma", "Alpha", "Beta"]);
}

#[test]
fn rank_systems_streams_callback_in_name_order_before_returning() {
    let systems = system_map(&["Beta", "Alpha"]);
    let names = filter_system_names(&systems, &[]).expect("systems should be present");
    let returned = Cell::new(false);
    let streamed = RefCell::new(Vec::new());

    let rows = rank_systems(
        &systems,
        &rich_balance(),
        &names,
        0,
        1,
        RankScorer::Template,
        &mut |row| {
            assert!(!returned.get(), "callback should fire before return");
            streamed.borrow_mut().push(row.system.clone());
        },
    );
    returned.set(true);

    assert_eq!(streamed.into_inner(), vec!["Alpha", "Beta"]);
    assert_eq!(rows.len(), 2);
}
