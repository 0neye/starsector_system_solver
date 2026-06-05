//! Shared test fixtures and builders.
//!
//! Everything tests need to construct planets, systems and states lives here so
//! the individual test files stay focused on *behavior* rather than setup. Prefer
//! extending these builders over copy-pasting setup into a new test file.
//!
//! This is a shared toolkit, so not every helper is used by every test (or yet);
//! `dead_code` is allowed here so a complete, discoverable fixture API can exist
//! without warnings.
#![allow(dead_code)]

use rustc_hash::FxHashMap;

use crate::constants::FacilityType;
use crate::planet::Planet;
use crate::solver::state::{Action, Balance, State};
use crate::system::System;

/// Credits/story-points/alpha-cores generous enough that affordability never
/// gates a test. Use this unless a test is specifically about resource limits.
pub(crate) fn rich_balance() -> Balance {
    Balance::new(10_000_000.0, 5, 5)
}

/// Build a `FxHashMap` of planet properties from `(key, value)` pairs.
///
/// Property keys mirror the CSV columns the parser reads, e.g.
/// `"hazard percent"` or a deposit column like `"organics"` / `"ores"`.
pub(crate) fn props(pairs: &[(&str, f64)]) -> FxHashMap<String, f64> {
    let mut m = FxHashMap::default();
    for (k, v) in pairs {
        m.insert((*k).to_string(), *v);
    }
    m
}

/// Fluent builder for a single planet.
///
/// Defaults to `100%` hazard so the common case is one line:
/// `PlanetBuilder::new("Test 1").build()`. Add deposits/props as needed.
pub(crate) struct PlanetBuilder {
    name: String,
    props: FxHashMap<String, f64>,
}

impl PlanetBuilder {
    pub(crate) fn new(name: &str) -> Self {
        let mut props = FxHashMap::default();
        props.insert("hazard percent".to_string(), 100.0);
        Self {
            name: name.to_string(),
            props,
        }
    }

    /// Set an arbitrary property column.
    pub(crate) fn prop(mut self, key: &str, value: f64) -> Self {
        self.props.insert(key.to_string(), value);
        self
    }

    /// Override the default 100% hazard rating.
    pub(crate) fn hazard(self, percent: f64) -> Self {
        self.prop("hazard percent", percent)
    }

    /// Add a resource deposit column (e.g. `"organics"`, `"ores"`) with its
    /// richness modifier. A modifier of `0.0` still counts as "deposit present".
    pub(crate) fn deposit(self, key: &str, modifier: f64) -> Self {
        self.prop(key, modifier)
    }

    /// The hash this planet will be keyed by, available before `build()` so
    /// tests can reference it when constructing `Action`s.
    pub(crate) fn name_hash(&self) -> u64 {
        Planet::_get_planet_name_hash(&self.name)
    }

    pub(crate) fn build(self) -> Planet {
        Planet::new(self.name, self.props)
    }
}

/// Wrap a single planet in a fresh, named system.
pub(crate) fn single_planet_system(planet: Planet) -> System {
    let mut system = System::new("Test".to_string());
    system.add_planet(planet);
    system
}

/// Build a [`State`] whose only planet is already colonized.
///
/// Returns the state plus the planet's hash so callers can issue further
/// actions against it. Uses [`rich_balance`] so credits never gate the test.
pub(crate) fn colonized_state(planet: Planet) -> (State, u64) {
    let hash = planet.name_hash();
    let system = single_planet_system(planet);
    let mut state = State::new(rich_balance(), system);
    state.apply_action_raw(&Action::Colonize(hash), false);
    (state, hash)
}

/// Apply a sequence of actions to a state in order (no validation, no debug).
pub(crate) fn apply_all(state: &mut State, actions: &[Action]) {
    for action in actions {
        state.apply_action_raw(action, false);
    }
}

/// Read a facility's `(current_build_days, total_build_days)` for a planet,
/// panicking with a clear message if the planet or facility is missing.
pub(crate) fn build_days(state: &State, planet_hash: u64, facility: FacilityType) -> (i32, i32) {
    let planet = state
        .system()
        .get_planet_by_hash(planet_hash)
        .expect("planet should exist in system");
    planet
        .get_facility(facility)
        .unwrap_or_else(|| panic!("facility {facility:?} should be present on planet"))
        .build_days_state()
}
