//! Facility construction bookkeeping: upgrading/downgrading a facility must
//! reset its build-day counters to the new facility's own build time rather
//! than carrying stale values across the swap.

use crate::constants::FacilityType;
use crate::solver::state::{Action, Balance, State};
use crate::tests::support::{build_days, colonized_state, single_planet_system, PlanetBuilder};

/// Upgrading spaceport -> megaport sets the megaport's build days to its own
/// build time (150d), and undoing restores a sane spaceport. (Regression: audit #5.)
#[test]
fn upgrade_downgrade_build_days_are_sane() {
    let (mut state, hash) = colonized_state(PlanetBuilder::new("Test 1").build());

    state.apply_action_raw(&Action::AddFacility(hash, FacilityType::Megaport), false);
    {
        let (cur, total) = build_days(&state, hash, FacilityType::Megaport);
        assert_eq!(
            total, 150,
            "megaport total_build_days must be its own build time, not stale"
        );
        assert_eq!(cur, 150, "megaport starts construction from scratch");
        let planet = state.system().get_planet_by_hash(hash).unwrap();
        assert!(planet.get_facility(FacilityType::Spaceport).is_none());
    }

    state.undo_last_action(false);
    let (cur, total) = build_days(&state, hash, FacilityType::Spaceport);
    assert_eq!(
        total, 15,
        "spaceport total_build_days restored to its own build time"
    );
    assert!(
        cur >= 0 && cur <= total,
        "spaceport build days must be sane after downgrade, got cur={cur} total={total}"
    );
    let planet = state.system().get_planet_by_hash(hash).unwrap();
    assert!(planet.get_facility(FacilityType::Megaport).is_none());
}

#[test]
fn facility_improvements_cost_two_then_double_per_planet() {
    let planet = PlanetBuilder::new("Test 1").build();
    let hash = planet.name_hash();
    let mut state = State::new(
        Balance::new(10_000_000.0, 20, 0),
        single_planet_system(planet),
    );
    state.apply_action_raw(&Action::Colonize(hash), false);

    state.apply_action_raw(
        &Action::AddImprovement(hash, FacilityType::Population),
        false,
    );
    assert_eq!(
        state.balance().story_points(),
        18,
        "first facility improvement on a planet costs 2 SP"
    );

    state.apply_action_raw(
        &Action::AddImprovement(hash, FacilityType::Spaceport),
        false,
    );
    assert_eq!(
        state.balance().story_points(),
        14,
        "second facility improvement on the same planet costs 4 SP"
    );

    state.undo_last_action(false);
    assert_eq!(
        state.balance().story_points(),
        18,
        "undo refunds the second improvement's 4 SP cost"
    );

    state.undo_last_action(false);
    assert_eq!(
        state.balance().story_points(),
        20,
        "undo refunds the first improvement's 2 SP cost"
    );
}

#[test]
fn first_facility_improvement_is_not_affordable_with_one_story_point() {
    let planet = PlanetBuilder::new("Test 1").build();
    let hash = planet.name_hash();
    let mut state = State::new(
        Balance::new(10_000_000.0, 1, 0),
        single_planet_system(planet),
    );
    state.apply_action_raw(&Action::Colonize(hash), false);

    assert!(
        !state
            .get_possible_actions(false)
            .iter()
            .any(|action| matches!(action, Action::AddImprovement(..))),
        "1 SP must not be enough for the first facility improvement"
    );
}
