//! Solver-level invariants: the action-sequence hash used to dedup search
//! nodes, and the apply/undo round-trip the search relies on to walk the tree.

use crate::constants::FacilityType;
use crate::planet::Planet;
use crate::solver::decomp::{decomp_search, decomp_search_maximize, simulate_plan, SystemPlan};
use crate::solver::goal::{Goal, Metric};
use crate::solver::state::{get_action_sequence_hash, Action, State};
use crate::system::System;
use crate::tests::support::{
    apply_all, colonized_state, rich_balance, single_planet_system, PlanetBuilder,
};

/// The sequence hash must be:
///  - order-sensitive across a wait/non-wait boundary (different timelines),
///  - order-insensitive within a run of non-wait actions (same reached state),
///  - stable across calls.
/// (Regression: audit #10.)
#[test]
fn action_sequence_hash_properties() {
    let h = Planet::_get_planet_name_hash("p");

    let a = vec![Action::Wait(1), Action::AddFacility(h, FacilityType::Mining)];
    let b = vec![Action::AddFacility(h, FacilityType::Mining), Action::Wait(1)];
    assert_ne!(
        get_action_sequence_hash(&a),
        get_action_sequence_hash(&b),
        "moving a wait across a non-wait action must change the hash"
    );

    // stable
    assert_eq!(get_action_sequence_hash(&a), get_action_sequence_hash(&a.clone()));
    assert_eq!(get_action_sequence_hash(&[]), get_action_sequence_hash(&[]));

    // within a non-wait run, reordering reaches the same state -> same key (dedup intact)
    let c = vec![Action::SetFreePort(h, true), Action::SetHazardPay(h, true)];
    let d = vec![Action::SetHazardPay(h, true), Action::SetFreePort(h, true)];
    assert_eq!(
        get_action_sequence_hash(&c),
        get_action_sequence_hash(&d),
        "reordering independent non-wait actions should hash equal"
    );
}

/// Replaying an action log from scratch must reproduce exactly the same
/// facilities and credits as undoing it step by step. This is the core
/// guarantee that lets the search undo its way back up the tree; the helper
/// panics on any drift.
#[test]
fn apply_undo_round_trip_is_consistent() {
    let (mut state, hash) = colonized_state(PlanetBuilder::new("Terran 1").build());
    apply_all(
        &mut state,
        &[
            Action::AddFacility(hash, FacilityType::Megaport),
            Action::Wait(2),
            Action::SetFreePort(hash, true),
            Action::AddFacility(hash, FacilityType::LightIndustry),
            Action::Wait(3),
            Action::AddFacility(hash, FacilityType::GroundDefenses),
        ],
    );
    crate::solver::_test_path_undo_consistency(&state);
}

// ---------------------------------------------------------------------------
// Two-level decomposition solver (src/solver/decomp.rs)
// ---------------------------------------------------------------------------

/// A fresh, uncolonized single-planet state plus its planet hash.
fn terran_base() -> (State, u64) {
    let planet = PlanetBuilder::new("Terran 1").build();
    let hash = planet.name_hash();
    let state = State::new(rich_balance(), single_planet_system(planet));
    (state, hash)
}

fn sum_wait_months(log: &[Action]) -> i32 {
    log.iter()
        .map(|a| match a {
            Action::Wait(m) => *m as i32,
            _ => 0,
        })
        .sum()
}

/// Replay an action log onto a fresh (uncolonized) base and report whether the
/// resulting state satisfies the goal. The logs the solver returns include the
/// initial `Colonize`, so they must be replayed from before colonization.
fn replay_satisfies(base: &State, log: &[Action], goal: &Goal) -> bool {
    let mut s = base.clone();
    apply_all(&mut s, log);
    goal.is_satisfied_quiet(&s)
}

/// Independent oracle for a *reachable* net income: greedily take any legal
/// non-wait action, otherwise wait, for a number of steps. Uses only
/// generator-approved actions so it doesn't assume specific facility gating.
fn reachable_income(base: &State, steps: u32) -> f64 {
    let mut s = base.clone();
    for _ in 0..steps {
        let actions = s.get_ordered_possible_actions(true);
        if let Some(a) = actions
            .iter()
            .find(|a| !matches!(a, Action::Wait(_)))
            .cloned()
        {
            s.apply_action_raw(&a, false);
        } else if let Some(w) = actions
            .iter()
            .find(|a| matches!(a, Action::Wait(_)))
            .cloned()
        {
            s.apply_action_raw(&w, false);
        } else {
            break;
        }
    }
    s.balance().net_income()
}

/// The inner simulator must return 0 months and emit no waits when the goal
/// already holds at t=0.
#[test]
fn decomp_inner_sim_zero_months_when_already_satisfied() {
    let (state, _hash) = colonized_state(PlanetBuilder::new("Terran 1").build());
    // A goal no income can fail to meet.
    let goal = Goal::new(f64::NEG_INFINITY, None, None);

    let plan = SystemPlan::permit_all(&state);
    let (actions, months) =
        simulate_plan(&state, &goal, &plan, true).expect("a trivially-satisfied goal is solvable");

    assert_eq!(months, 0, "no waiting needed when the goal holds at t=0");
    assert!(
        !actions.iter().any(|a| matches!(a, Action::Wait(_))),
        "no waits should be emitted for an already-satisfied goal"
    );
}

/// For a reachable positive income goal the inner simulator must produce a log
/// that (a) actually satisfies the goal when replayed and (b) whose waits sum to
/// the reported month cost.
#[test]
fn decomp_inner_sim_log_is_consistent_and_correct() {
    let (base, hash) = terran_base();
    let mut colonized = base.clone();
    colonized.apply_action_raw(&Action::Colonize(hash), false);

    let target = reachable_income(&colonized, 60);
    assert!(
        target > 0.0,
        "test setup expects a built-up Terran colony to reach positive income, got {target}"
    );
    // Half of a known-reachable income is comfortably reachable.
    let goal = Goal::new(target * 0.5, None, None);

    let plan = SystemPlan::permit_all(&colonized);
    let (log, months) =
        simulate_plan(&colonized, &goal, &plan, true).expect("half the reachable income is solvable");

    assert_eq!(
        sum_wait_months(&log),
        months,
        "reported cost must equal the summed Wait months in the log"
    );
    assert!(
        replay_satisfies(&base, &log, &goal),
        "replaying the returned plan must actually satisfy the goal"
    );
}

/// The inner simulator is deterministic: identical inputs yield an identical
/// (log, cost) result.
#[test]
fn decomp_inner_sim_is_deterministic() {
    let (base, hash) = terran_base();
    let mut colonized = base.clone();
    colonized.apply_action_raw(&Action::Colonize(hash), false);

    let target = reachable_income(&colonized, 60);
    let goal = Goal::new((target * 0.5).max(1.0), None, None);
    let plan = SystemPlan::permit_all(&colonized);

    let first = simulate_plan(&colonized, &goal, &plan, true);
    let second = simulate_plan(&colonized, &goal, &plan, true);
    assert_eq!(first, second, "the inner simulator must be deterministic");
}

/// The outer search must return a plan that satisfies a reachable goal, with a
/// cost consistent with its action log.
#[test]
fn decomp_search_returns_satisfying_plan() {
    let (base, hash) = terran_base();
    let mut colonized = base.clone();
    colonized.apply_action_raw(&Action::Colonize(hash), false);

    let target = reachable_income(&colonized, 60);
    assert!(target > 0.0, "expected a reachable positive income, got {target}");
    let goal = Goal::new(target * 0.5, None, None);

    let result = decomp_search(&mut colonized, &goal, 2_000, true)
        .expect("outer search should find a plan for a reachable goal");
    let log = result.solution.expect("a successful result carries a solution");

    assert_eq!(
        sum_wait_months(&log),
        result.cost,
        "reported cost must equal the summed Wait months"
    );
    assert!(
        replay_satisfies(&base, &log, &goal),
        "the returned plan must satisfy the goal when replayed"
    );
}

/// Maximize mode must (a) hold the floor on the non-maximized metric and
/// (b) push income comfortably past a known-reachable threshold, with a cost
/// consistent with the returned log.
#[test]
fn decomp_maximize_income_holds_stability_floor() {
    let (base, hash) = terran_base();
    let mut colonized = base.clone();
    colonized.apply_action_raw(&Action::Colonize(hash), false);

    let target = reachable_income(&colonized, 60);
    assert!(target > 0.0, "expected a reachable positive income, got {target}");

    // Maximize income with no income floor but a stability floor of 5.
    let floors = Goal::new(f64::NEG_INFINITY, None, Some(5));
    let result = decomp_search_maximize(&mut colonized, Metric::Income, &floors, 120, 3_000, true)
        .expect("a single colony can hold stability 5 while earning income");
    let log = result.solution.expect("a successful result carries a solution");

    assert_eq!(
        sum_wait_months(&log),
        result.cost,
        "reported cost must equal the summed Wait months at the best instant"
    );

    let mut replay = base.clone();
    apply_all(&mut replay, &log);
    assert!(
        replay.system().avg_stability() >= 5.0,
        "maximize must hold the stability floor, got {}",
        replay.system().avg_stability()
    );
    assert!(
        replay.balance().net_income() >= target * 0.5,
        "maximized income {} should beat half the reachable income {}",
        replay.balance().net_income(),
        target * 0.5
    );
}

/// A longer horizon can only help: the best income reachable within 120 months
/// must be at least the best reachable within 12 (a superset of instants over
/// the same plan space).
#[test]
fn decomp_maximize_longer_horizon_is_no_worse() {
    let (base, hash) = terran_base();
    let mut short_state = base.clone();
    short_state.apply_action_raw(&Action::Colonize(hash), false);
    let mut long_state = short_state.clone();

    let floors = Goal::new(f64::NEG_INFINITY, None, None);

    let replay_income = |result: Option<crate::solver::AStarSearchResult>| -> f64 {
        let log = result.expect("income is maximizable with no floor").solution.unwrap();
        let mut s = base.clone();
        apply_all(&mut s, &log);
        s.balance().net_income()
    };

    let short = replay_income(decomp_search_maximize(
        &mut short_state, Metric::Income, &floors, 12, 3_000, true,
    ));
    let long = replay_income(decomp_search_maximize(
        &mut long_state, Metric::Income, &floors, 120, 3_000, true,
    ));

    assert!(
        long >= short,
        "a longer horizon must not yield less income: 12mo={short}, 120mo={long}"
    );
}

/// A fresh, uncolonized two-planet system.
fn two_planet_base() -> State {
    let mut system = System::new("Test".to_string());
    system.add_planet(PlanetBuilder::new("Terran 1").build());
    system.add_planet(PlanetBuilder::new("Terran 2").build());
    State::new(rich_balance(), system)
}

/// Number of distinct planets colonized in a log.
fn distinct_colonized(log: &[Action]) -> usize {
    let mut set = std::collections::HashSet::new();
    for a in log {
        if let Action::Colonize(h) = a {
            set.insert(*h);
        }
    }
    set.len()
}

/// The joint solver must satisfy a *system-wide* goal that no single planet can
/// reach alone by developing several planets on one shared timeline — and the
/// legacy per-planet split must fail at the same goal. This is the core proof of
/// multi-planet interleaving.
#[test]
fn decomp_joint_interleaves_planets_for_system_goal() {
    // Solo income one Terran colony can reach on its own.
    let (solo_base, hash) = terran_base();
    let mut solo = solo_base.clone();
    solo.apply_action_raw(&Action::Colonize(hash), false);
    let solo_income = reachable_income(&solo, 80);
    assert!(solo_income > 0.0, "expected positive solo income, got {solo_income}");

    // A system goal beyond one planet's solo capacity but within two planets'.
    let goal = Goal::new(solo_income * 1.5, None, None);

    let base = two_planet_base();

    // Joint solve on one shared timeline.
    let mut joint = base.clone();
    let result = decomp_search(&mut joint, &goal, 4_000, true)
        .expect("two planets together should reach the goal");
    let log = result.solution.unwrap();

    assert_eq!(
        sum_wait_months(&log),
        result.cost,
        "cost must equal summed waits"
    );
    assert!(
        replay_satisfies(&base, &log, &goal),
        "the joint plan must satisfy the system goal when replayed"
    );
    assert_eq!(
        distinct_colonized(&log),
        2,
        "a goal beyond solo capacity should force developing both planets"
    );

    // The legacy per-planet split solves each planet in isolation, so it cannot
    // reach a goal that only the planets *combined* can meet.
    let mut split = base.clone();
    let split_results =
        crate::solver::archive::split::search_all_planets_decomp(&mut split, &goal, 4_000, true);
    assert!(
        split_results.len() < 2,
        "per-planet decomposition should not solve a goal that needs planets combined"
    );
}

/// Regression: once a planet has reached the top of an upgrade chain, the lower
/// tiers must not be offered again. The orbital-station chain has three tiers
/// (orbital -> battle -> star fortress), and a one-level upgrade check only
/// noticed that *something* requires "battle station" — nothing on the planet
/// directly requires "orbital station" anymore — so the generator re-offered the
/// orbital station as a brand-new facility, producing action sequences that
/// built a star fortress and *then* an orbital station.
#[test]
fn generator_does_not_reoffer_lower_tiers_of_a_completed_chain() {
    let (mut state, hash) = colonized_state(PlanetBuilder::new("Terran 1").build());

    // Walk the orbital-station chain to the top (each AddFacility upgrades the
    // previous tier in place).
    state.apply_action_raw(&Action::AddFacility(hash, FacilityType::OrbitalStation), false);
    state.apply_action_raw(&Action::AddFacility(hash, FacilityType::BattleStation), false);
    state.apply_action_raw(&Action::AddFacility(hash, FacilityType::StarFortress), false);

    let planet = state.system().get_planet_by_hash(hash).unwrap();
    assert!(
        planet.get_facility(FacilityType::StarFortress).is_some(),
        "the chain should have topped out at a star fortress"
    );
    assert!(
        planet.has_facility_or_upgrade(FacilityType::OrbitalStation),
        "a star fortress must satisfy a request for an orbital station"
    );
    assert!(
        planet.has_facility_or_upgrade(FacilityType::BattleStation),
        "a star fortress must satisfy a request for a battle station"
    );

    let reoffered: Vec<_> = state
        .get_ordered_possible_actions(true)
        .into_iter()
        .filter(|a| {
            matches!(
                a,
                Action::AddFacility(_, FacilityType::OrbitalStation)
                    | Action::AddFacility(_, FacilityType::BattleStation)
            )
        })
        .collect();
    assert!(
        reoffered.is_empty(),
        "lower station tiers must not be re-offered once a star fortress exists, got {reoffered:?}"
    );
}
