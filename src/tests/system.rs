//! System-wide aggregates: averages must be computed over colonized planets
//! only, so uncolonized planets in the system don't drag the numbers down.

use crate::solver::state::{Action, Balance, State};
use crate::system::System;
use crate::tests::support::PlanetBuilder;

/// `avg_stability` (and friends) average over colonized planets only.
/// (Regression: audit #11.)
#[test]
fn averages_ignore_uncolonized_planets() {
    let mut system = System::new("Test".to_string());
    let colonized = PlanetBuilder::new("A");
    let h1 = colonized.name_hash();
    system.add_planet(colonized.build());
    system.add_planet(PlanetBuilder::new("B").build()); // never colonized

    let mut state = State::new(Balance::new(10_000_000.0, 5, 5), system);
    state.apply_action_raw(&Action::Colonize(h1), false); // only A is colonized

    let sys = state.system();
    // Only the colonized planet should contribute; its stability equals its own value.
    let only = sys.get_planet_by_hash(h1).unwrap().stability() as f64;
    assert_eq!(sys.avg_stability(), only, "avg should be over colonized planets only");
}

#[test]
fn makeshift_comm_relay_uses_stable_point_and_affects_all_colonies() {
    let mut system = System::new("Test".to_string());
    system.set_stable_points(1);
    let a = PlanetBuilder::new("A");
    let h1 = a.name_hash();
    let b = PlanetBuilder::new("B");
    let h2 = b.name_hash();
    system.add_planet(a.build());
    system.add_planet(b.build());

    let mut state = State::new(Balance::new(10_000_000.0, 5, 5), system);
    state.apply_action_raw(&Action::Colonize(h1), false);
    state.apply_action_raw(&Action::Colonize(h2), false);

    let before_a = state.system().get_planet_by_hash(h1).unwrap().stability();
    let before_b = state.system().get_planet_by_hash(h2).unwrap().stability();

    assert!(
        state
            .get_possible_actions(false)
            .contains(&Action::BuildMakeshiftCommRelay)
    );

    state.apply_action_raw(&Action::BuildMakeshiftCommRelay, false);

    assert_eq!(
        state.system().get_planet_by_hash(h1).unwrap().stability(),
        before_a + 2
    );
    assert_eq!(
        state.system().get_planet_by_hash(h2).unwrap().stability(),
        before_b + 2
    );
    assert_eq!(state.system().available_stable_points(), 0);
    assert!(
        !state
            .get_possible_actions(false)
            .contains(&Action::BuildMakeshiftCommRelay)
    );

    state.undo_last_action(false);

    assert_eq!(
        state.system().get_planet_by_hash(h1).unwrap().stability(),
        before_a
    );
    assert_eq!(
        state.system().get_planet_by_hash(h2).unwrap().stability(),
        before_b
    );
    assert_eq!(state.system().available_stable_points(), 1);
}
