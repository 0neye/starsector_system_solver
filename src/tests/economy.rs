//! Economy regression tests for pooled export market-share income.
//!
//! These tests exercise the system-level export pool, the standalone planet
//! view, and scheduler income accrual in situations where multiple colonies
//! compete for the same commodity market.

use crate::constants::{FacilityType, Resource, FACILITY_DATA};
use crate::planet::Planet;
use crate::solver::decomp::{assert_factored_lookahead_matches_reference, SystemPlan};
use crate::solver::goal::{Goal, Metric};
use crate::solver::state::{Action, State};
use crate::system::System;

use super::support::{apply_all, colonized_state, rich_balance, PlanetBuilder};

const EPS: f64 = 1e-6;

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < EPS,
        "actual {actual} differed from expected {expected}"
    );
}

fn refining_exports(economy_planet: &Planet) -> Vec<(Resource, f64, f64)> {
    economy_planet
        .economy()
        .exports
        .iter()
        .copied()
        .filter(|(resource, _, _)| {
            matches!(resource, Resource::Metals | Resource::Transplutonics)
        })
        .collect()
}

fn hand_gross_from_planets(planets: &[&Planet]) -> f64 {
    let direct: f64 = planets
        .iter()
        .map(|planet| planet.economy().direct_income)
        .sum();

    let export_income: f64 = Resource::ALL
        .iter()
        .map(|resource| {
            let (raw, modded) = planets.iter().fold((0.0, 0.0), |(raw, modded), planet| {
                planet
                    .economy()
                    .exports
                    .iter()
                    .filter(|(export, _, _)| export == resource)
                    .fold((raw, modded), |(raw, modded), (_, export_raw, export_modded)| {
                        (raw + export_raw, modded + export_modded)
                    })
            });

            if raw == 0.0 {
                0.0
            } else {
                resource.market_value() as f64 * modded
                    / (resource.sector_supply() as f64 + raw)
            }
        })
        .sum();

    direct + export_income
}

fn hand_legacy_independent_gross(planet: &Planet) -> f64 {
    let economy = planet.economy();
    economy.direct_income
        + economy
            .exports
            .iter()
            .map(|(resource, _, modded)| {
                resource.market_value() as f64 * modded / resource.sector_supply() as f64
            })
            .sum::<f64>()
}

fn build_months(facility_type: FacilityType) -> u32 {
    (FACILITY_DATA[&facility_type].build_time as f64 / 30.0).ceil() as u32
}

fn base_accessibility_property_for(total_accessibility: f64) -> f64 {
    // Size 6 contributes +15%, and the completed default Spaceport contributes
    // +50%; set the CSV-style property so total accessibility hits the target.
    total_accessibility - 65.0
}

fn colonized_planet(name: &str, total_accessibility: f64) -> Planet {
    let (state, hash) = colonized_state(
        PlanetBuilder::new(name)
            .prop("size", 6.0)
            .prop(
                "accessibility percent",
                base_accessibility_property_for(total_accessibility),
            )
            .build(),
    );
    state
        .system()
        .get_planet_by_hash(hash)
        .expect("colonized planet should exist")
        .clone()
}

fn colonized_refining_planet(name: &str) -> Planet {
    colonized_refining_planet_with_accessibility(name, 100.0)
}

fn colonized_refining_planet_with_accessibility(name: &str, total_accessibility: f64) -> Planet {
    let (mut state, hash) = colonized_state(
        PlanetBuilder::new(name)
            .prop("size", 6.0)
            .prop(
                "accessibility percent",
                base_accessibility_property_for(total_accessibility),
            )
            .build(),
    );
    apply_all(
        &mut state,
        &[
            Action::AddFacility(hash, FacilityType::Refining),
            Action::Wait(build_months(FacilityType::Refining)),
        ],
    );
    state
        .system()
        .get_planet_by_hash(hash)
        .expect("refining planet should exist")
        .clone()
}

fn system_from_planets(planets: Vec<Planet>) -> System {
    let mut system = System::new("economy test".to_string());
    for planet in planets {
        system.add_planet(planet);
    }
    system
}

fn state_from_planets(planets: Vec<Planet>) -> State {
    let mut state = State::new(rich_balance(), system_from_planets(planets));
    let (gross, upkeep) = state.system().gross_income_and_upkeep();
    state.balance_mut().update_income(gross, gross - upkeep);
    state
}

fn assert_state_restored(actual: &State, expected: &State) {
    assert_eq!(actual.balance().credits(), expected.balance().credits());
    assert_eq!(
        actual.balance().story_points(),
        expected.balance().story_points()
    );
    assert_eq!(
        actual.balance().alpha_cores(),
        expected.balance().alpha_cores()
    );
    assert_close(
        actual.balance().gross_income(),
        expected.balance().gross_income(),
    );
    assert_close(actual.balance().net_income(), expected.balance().net_income());
    assert_eq!(actual.system().planets(), expected.system().planets());
    assert_eq!(
        actual.system().infrastructure(),
        expected.system().infrastructure()
    );
    assert_eq!(actual.system().stable_points(), expected.system().stable_points());
    assert_eq!(actual.action_log(), expected.action_log());
    assert_eq!(actual.get_deep_hash(), expected.get_deep_hash());
}

#[test]
fn single_producer_matches_pooled_formula() {
    let planet = colonized_refining_planet("Asharu");
    let system = system_from_planets(vec![planet.clone()]);

    let expected = hand_gross_from_planets(&[&planet]);

    assert_close(system.get_gross_income(), expected);
    assert_close(planet.get_gross_income(), expected);
    assert!(
        expected < hand_legacy_independent_gross(&planet),
        "standalone income should include own raw supply in the denominator"
    );
}

#[test]
fn duplicate_producers_have_diminishing_returns() {
    let one_planet = colonized_refining_planet("Asharu");
    let one = system_from_planets(vec![one_planet.clone()]).get_gross_income();

    let two = system_from_planets(vec![
        one_planet,
        colonized_refining_planet("Chaxiraxi"),
    ])
    .get_gross_income();

    assert!(two > one, "adding a producer should increase gross income");
    assert!(
        two < 2.0 * one,
        "duplicate producers should compete for finite market share"
    );
}

#[test]
fn accessibility_splits_market_share() {
    let high_access = colonized_refining_planet_with_accessibility("Asharu", 150.0);
    let normal_access = colonized_refining_planet("Chaxiraxi");

    let high_metals_raw = refining_exports(&high_access)
        .iter()
        .find(|(resource, _, _)| *resource == Resource::Metals)
        .map(|(_, raw, _)| *raw)
        .expect("high-access planet should export metals");
    let normal_metals_raw = refining_exports(&normal_access)
        .iter()
        .find(|(resource, _, _)| *resource == Resource::Metals)
        .map(|(_, raw, _)| *raw)
        .expect("normal-access planet should export metals");

    assert_close(
        high_metals_raw / normal_metals_raw,
        high_access.calculate_accessibility() / normal_access.calculate_accessibility(),
    );

    let system = system_from_planets(vec![high_access.clone(), normal_access.clone()]);
    assert_close(
        system.get_gross_income(),
        hand_gross_from_planets(&[&high_access, &normal_access]),
    );
}

#[test]
fn low_accessibility_caps_exports() {
    let target_accessibility = 25.0;
    let (mut state, hash) = colonized_state(
        PlanetBuilder::new("Low Access Refinery")
            .prop("accessibility percent", target_accessibility - 50.0)
            .build(),
    );
    apply_all(
        &mut state,
        &[
            Action::AddFacility(hash, FacilityType::Refining),
            Action::Wait(build_months(FacilityType::Refining)),
        ],
    );
    let planet = state
        .system()
        .get_planet_by_hash(hash)
        .expect("low-access refinery should exist");
    let accessibility = planet.calculate_accessibility();
    let export_cap = (accessibility / 10.0).floor();
    let production = planet.get_resource_production();
    let economy = planet.economy();

    for &(resource, raw, _) in &economy.exports {
        let production_units = production
            .get(&resource)
            .copied()
            .expect("exported resource should have production units");
        assert_close(
            raw,
            production_units.min(export_cap) * accessibility / 100.0,
        );
    }

    let metals_production = *production
        .get(&Resource::Metals)
        .expect("refining should produce metals");
    assert!(
        export_cap < metals_production,
        "metals cap should bind: cap {export_cap}, production {metals_production}"
    );
}

#[test]
fn commerce_multiplier_applies_to_own_allocation_only() {
    let mut commerce_state = state_from_planets(vec![colonized_refining_planet("Asharu")]);
    let commerce_id = Planet::_get_planet_name_hash("Asharu");
    apply_all(
        &mut commerce_state,
        &[
            Action::AddFacility(commerce_id, FacilityType::Commerce),
            Action::Wait(build_months(FacilityType::Commerce)),
        ],
    );
    let commerce = commerce_state
        .system()
        .get_planet_by_hash(commerce_id)
        .expect("commerce planet should exist")
        .clone();

    let plain = colonized_refining_planet("Chaxiraxi");

    // Each planet's modded supply must carry exactly its OWN colony modifiers
    // (highest income multiplier x low-stability penalty); the Commerce
    // multiplier must show up only on the Commerce planet's contribution.
    let income_mult = |planet: &Planet| {
        planet
            .facilities()
            .iter()
            .map(|f| f.calculate_income_multiplier())
            .fold(1.0f64, f64::max)
    };
    let expected_modifier = |planet: &Planet| {
        let stability = planet.stability();
        let stability_factor = if stability < 5 {
            1.0 - 0.2 * (5 - stability) as f64
        } else {
            1.0
        };
        income_mult(planet) * stability_factor
    };
    assert!(
        income_mult(&commerce) > 1.0,
        "a built Commerce facility should multiply income, got {}",
        income_mult(&commerce)
    );
    assert_close(income_mult(&plain), 1.0);
    for (planet, label) in [(&commerce, "Commerce"), (&plain, "plain")] {
        let modifier = expected_modifier(planet);
        for (resource, raw, modded) in refining_exports(planet) {
            assert!(
                (modded / raw - modifier).abs() < EPS,
                "{label} planet's {resource:?} modded/raw {} should equal its \
                 income_mult x stability modifier {modifier}",
                modded / raw
            );
        }
    }

    let system = system_from_planets(vec![commerce.clone(), plain.clone()]);

    assert_close(
        system.get_gross_income(),
        hand_gross_from_planets(&[&commerce, &plain]),
    );
}

#[test]
fn wait_accrues_system_level_income_and_undoes_exactly() {
    let first = colonized_refining_planet("Asharu");
    let second = colonized_planet("Chaxiraxi", 100.0);

    let mut state = state_from_planets(vec![first, second]);
    let before = state.clone();

    let second_id = Planet::_get_planet_name_hash("Chaxiraxi");
    state.apply_action_raw(&Action::AddFacility(second_id, FacilityType::Refining), false);

    let mut split = state.clone();
    let build_months = build_months(FacilityType::Refining);
    let wait_months = build_months + 2;

    split.apply_action_raw(&Action::Wait(build_months), false);
    split.apply_action_raw(&Action::Wait(wait_months - build_months), false);

    state.apply_action_raw(&Action::Wait(wait_months), false);

    assert_eq!(state.balance().credits(), split.balance().credits());

    state.undo_last_action(false);
    state.undo_last_action(false);

    assert_state_restored(&state, &before);
}

#[test]
fn lookahead_factored_parity_with_share_coupling() {
    let system = system_from_planets(vec![
        PlanetBuilder::new("Asharu")
            .prop("size", 6.0)
            .prop("accessibility percent", base_accessibility_property_for(100.0))
            .build(),
        PlanetBuilder::new("Chaxiraxi")
            .prop("size", 6.0)
            .prop("accessibility percent", base_accessibility_property_for(100.0))
            .build(),
    ]);
    let mut state = State::new(rich_balance(), system);
    for hash in [
        Planet::_get_planet_name_hash("Asharu"),
        Planet::_get_planet_name_hash("Chaxiraxi"),
    ] {
        state.apply_action_raw(&Action::Colonize(hash), false);
    }
    let floors = Goal::new(f64::NEG_INFINITY, None, None);
    let plan = SystemPlan::permit_all(&state);

    let compared = assert_factored_lookahead_matches_reference(
        &state,
        Metric::Income,
        &floors,
        120,
        &plan,
        false,
        50,
    );
    assert!(compared >= 50, "expected at least 50 comparisons, got {compared}");
}
