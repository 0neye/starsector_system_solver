//! Planet-level behavior: which facilities a colony can build, and how mining
//! production responds to the presence/richness of resource deposits.

use crate::constants::{FacilityType, Resource};
use crate::solver::state::Action;
use crate::tests::support::{colonized_state, PlanetBuilder};

/// Mining is buildable when *any* mineable deposit is present, and unavailable
/// when the planet has none. (Regression: audit #3 — was requiring *all*.)
#[test]
fn mining_requires_any_deposit_not_all() {
    let (state, hash) = colonized_state(
        PlanetBuilder::new("Test 1")
            .deposit("organics", 2.0)
            .build(),
    );
    let planet = state.system().get_planet_by_hash(hash).unwrap();
    let actions = planet.get_possible_actions(state.balance(), false);
    assert!(
        actions.contains(&Action::AddFacility(hash, FacilityType::Mining)),
        "mining should be buildable with only one deposit (organics) present"
    );

    let (state2, hash2) = colonized_state(PlanetBuilder::new("Test 2").build());
    let planet2 = state2.system().get_planet_by_hash(hash2).unwrap();
    let actions2 = planet2.get_possible_actions(state2.balance(), false);
    assert!(
        !actions2.contains(&Action::AddFacility(hash2, FacilityType::Mining)),
        "mining should NOT be buildable with no deposits present"
    );
}

/// Extraction production = base size formula + the deposit's richness modifier,
/// and a resource whose deposit column is absent produces nothing.
/// (Regression: audit #4.)
#[test]
fn extraction_adds_deposit_modifier_bonus() {
    let (mut state, hash) = colonized_state(
        PlanetBuilder::new("Test 1")
            .deposit("organics", 2.0)
            .build(),
    );
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

/// A deposit that is present but with a low (0 or -1) modifier still produces.
/// (Regression: audit #4 — wiki behavior; presence, not richness, gates output.)
#[test]
fn low_modifier_deposit_still_produces() {
    // ore column present with modifier 0; organics column absent entirely.
    let (mut state, hash) =
        colonized_state(PlanetBuilder::new("Test 1").deposit("ores", 0.0).build());
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
