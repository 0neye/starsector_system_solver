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
