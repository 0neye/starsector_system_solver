//! Facility construction bookkeeping: upgrading/downgrading a facility must
//! reset its build-day counters to the new facility's own build time rather
//! than carrying stale values across the swap.

use crate::constants::FacilityType;
use crate::solver::state::Action;
use crate::tests::support::{build_days, colonized_state, PlanetBuilder};

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
fn force_complete_build_restore_round_trips_mid_build_facility() {
    let (mut state, hash) = colonized_state(PlanetBuilder::new("Test 1").build());
    state.apply_action_raw(
        &Action::AddFacility(hash, FacilityType::HeavyIndustry),
        false,
    );
    state.apply_action_raw(&Action::Wait(2), false);

    let before = state.system().get_planet_by_hash(hash).unwrap().clone();
    let prior = {
        let planet = state.system_mut().get_planet_mut_by_hash(hash).unwrap();
        let prior = planet
            .force_complete_build(FacilityType::HeavyIndustry)
            .expect("heavy industry should be present");
        assert!(prior > 0, "test must exercise a non-complete build");
        assert_eq!(
            planet
                .get_facility(FacilityType::HeavyIndustry)
                .unwrap()
                .remaining_build_days(),
            0
        );
        prior
    };

    state
        .system_mut()
        .get_planet_mut_by_hash(hash)
        .unwrap()
        .restore_build_days(FacilityType::HeavyIndustry, prior);

    let after = state.system().get_planet_by_hash(hash).unwrap();
    assert_eq!(&before, after);
}
