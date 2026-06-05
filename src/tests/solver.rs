//! Solver-level invariants: the action-sequence hash used to dedup search
//! nodes, and the apply/undo round-trip the search relies on to walk the tree.

use crate::constants::FacilityType;
use crate::planet::Planet;
use crate::solver::state::{get_action_sequence_hash, Action};
use crate::tests::support::{apply_all, colonized_state, PlanetBuilder};

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
