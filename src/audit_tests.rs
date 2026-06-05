//! Regression tests for the audit fixes (#1-#11).
//! In-crate so they can reach `pub(crate)` build-day accessors.
#![cfg(test)]

use rustc_hash::FxHashMap;

use crate::constants::{FacilityType, Resource};
use crate::planet::Planet;
use crate::solver::state::{get_action_sequence_hash, Action, Balance, State};
use crate::system::System;

fn props(pairs: &[(&str, f64)]) -> FxHashMap<String, f64> {
    let mut m = FxHashMap::default();
    for (k, v) in pairs {
        m.insert((*k).to_string(), *v);
    }
    m
}

fn colonized_state(planet_props: &[(&str, f64)], credits: f64) -> (State, u64) {
    let mut system = System::new("Test".to_string());
    let planet = Planet::new("Test 1".to_string(), props(planet_props));
    let hash = planet.name_hash();
    system.add_planet(planet);
    let mut state = State::new(Balance::new(credits, 5, 5), system);
    state.apply_action_raw(&Action::Colonize(hash), false);
    (state, hash)
}

// ---- #3: mining buildable if ANY deposit present (not ALL) ----
#[test]
fn mining_requires_any_deposit_not_all() {
    let (state, hash) = colonized_state(&[("hazard percent", 100.0), ("organics", 2.0)], 10_000_000.0);
    let planet = state.system().get_planet_by_hash(hash).unwrap();
    let actions = planet.get_possible_actions(state.balance(), false);
    assert!(
        actions.contains(&Action::AddFacility(hash, FacilityType::Mining)),
        "mining should be buildable with only one deposit (organics) present"
    );

    let (state2, hash2) = colonized_state(&[("hazard percent", 100.0)], 10_000_000.0);
    let planet2 = state2.system().get_planet_by_hash(hash2).unwrap();
    let actions2 = planet2.get_possible_actions(state2.balance(), false);
    assert!(
        !actions2.contains(&Action::AddFacility(hash2, FacilityType::Mining)),
        "mining should NOT be buildable with no deposits present"
    );
}

// ---- #4: extraction production = base size formula + deposit modifier; absent deposit -> 0 ----
#[test]
fn extraction_adds_deposit_modifier_bonus() {
    let (mut state, hash) =
        colonized_state(&[("hazard percent", 100.0), ("organics", 2.0)], 10_000_000.0);
    state.apply_action_raw(&Action::AddFacility(hash, FacilityType::Mining), false);
    state.apply_action_raw(&Action::Wait(3), false); // mining build_time 60d -> finishes

    let planet = state.system().get_planet_by_hash(hash).unwrap();
    let size = planet.size() as f64;
    // organics base formula is `size`; with deposit modifier +2 -> size + 2.
    assert_eq!(
        planet.calculate_resource_production(Resource::Organics),
        size + 2.0,
        "organics should be size + deposit modifier (+2)"
    );
    assert_eq!(
        planet.calculate_resource_production(Resource::Ore),
        0.0,
        "ore must be gated out when no ore deposit column is present"
    );
}

// ---- #4 (wiki): a present deposit with modifier 0 or -1 still produces ----
#[test]
fn low_modifier_deposit_still_produces() {
    // ore column present with modifier 0; organics column absent entirely.
    let (mut state, hash) =
        colonized_state(&[("hazard percent", 100.0), ("ores", 0.0)], 10_000_000.0);
    state.apply_action_raw(&Action::AddFacility(hash, FacilityType::Mining), false);
    state.apply_action_raw(&Action::Wait(3), false);

    let planet = state.system().get_planet_by_hash(hash).unwrap();
    let size = planet.size() as f64;
    // ore = size + 0 (deposit present, modifier 0) -> still produces.
    assert_eq!(
        planet.calculate_resource_production(Resource::Ore),
        size,
        "an ore deposit with modifier 0 must still produce (size + 0)"
    );
    assert_eq!(
        planet.calculate_resource_production(Resource::Organics),
        0.0,
        "organics has no deposit column -> absent -> no production"
    );
}

// ---- #5: upgrade updates total_build_days; downgrade restores sane (non-stale) days ----
#[test]
fn upgrade_downgrade_build_days_are_sane() {
    let (mut state, hash) = colonized_state(&[("hazard percent", 100.0)], 10_000_000.0);

    state.apply_action_raw(&Action::AddFacility(hash, FacilityType::Megaport), false);
    {
        let planet = state.system().get_planet_by_hash(hash).unwrap();
        let mega = planet.get_facility(FacilityType::Megaport).expect("megaport present");
        let (cur, total) = mega.build_days_state();
        assert_eq!(total, 150, "megaport total_build_days must be its own build time, not stale");
        assert_eq!(cur, 150, "megaport starts construction from scratch");
        assert!(planet.get_facility(FacilityType::Spaceport).is_none());
    }

    state.undo_last_action(false);
    let planet = state.system().get_planet_by_hash(hash).unwrap();
    let sp = planet.get_facility(FacilityType::Spaceport).expect("spaceport restored");
    let (cur, total) = sp.build_days_state();
    assert_eq!(total, 15, "spaceport total_build_days restored to its own build time");
    assert!(
        cur >= 0 && cur <= total,
        "spaceport build days must be sane after downgrade, got cur={cur} total={total}"
    );
    assert!(planet.get_facility(FacilityType::Megaport).is_none());
}

// ---- #10: sequence hash is order-sensitive across wait/non-wait boundaries,
//           order-insensitive within a non-wait run (dedup preserved), and stable. ----
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

// ---- #11: averages only count colonized planets ----
#[test]
fn averages_ignore_uncolonized_planets() {
    let mut system = System::new("Test".to_string());
    let p1 = Planet::new("A".to_string(), props(&[("hazard percent", 100.0)]));
    let p2 = Planet::new("B".to_string(), props(&[("hazard percent", 100.0)]));
    let h1 = p1.name_hash();
    system.add_planet(p1);
    system.add_planet(p2);
    let mut state = State::new(Balance::new(10_000_000.0, 5, 5), system);
    state.apply_action_raw(&Action::Colonize(h1), false); // only A is colonized

    let sys = state.system();
    // Only the colonized planet should contribute; its stability equals its own value.
    let only = sys.get_planet_by_hash(h1).unwrap().stability() as f64;
    assert_eq!(sys.avg_stability(), only, "avg should be over colonized planets only");
}
