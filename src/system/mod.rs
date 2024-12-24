use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHasher};

use crate::solver::{Action, Balance};
use crate::planet::Planet;

// Additional infrastructure affecting the system
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum Infrastructure {
    CommRelay {domain: bool}, // only one relevant for simulation; gives +2 stability
    NavBouy {domain: bool},
    SensorArray {domain: bool},
    Gate,
    Remnants {damaged: bool}
}

#[derive(Debug, Clone)]
pub struct System {
    name: String,
    planets: FxHashMap<String, Planet>,
    infrastructure: FxHashMap<String, Infrastructure>,
}

impl Hash for System {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        let mut sorted_planets: Vec<_> = self.planets.iter().collect();
        sorted_planets.sort_by(|a, b| a.0.cmp(b.0));
        for (key, value) in sorted_planets {
            key.hash(state);
            value.hash(state);
        }
        let mut sorted_infrastructure: Vec<_> = self.infrastructure.iter().collect();
        sorted_infrastructure.sort_by(|a, b| a.0.cmp(b.0));
        for (key, value) in sorted_infrastructure {
            key.hash(state);
            value.hash(state);
        }
    }
}

impl System {
    // Add a method to check for comm relay
    pub fn has_comm_relay(&self) -> bool {
        self.infrastructure.values().any(|infra| {
            matches!(infra, Infrastructure::CommRelay { domain: _ })
        })
    }

    pub fn update_infrastructure_bonuses(&mut self) {
        let stability_bonus = if self.has_comm_relay() { 2 } else { 0 };
        
        for planet in self.planets.values_mut() {
            planet.set_system_stability_bonus(stability_bonus);
        }
    }

    pub fn new(name: String) -> Self {
        Self {
            name,
            planets: FxHashMap::with_capacity_and_hasher(5, FxBuildHasher::default()), // usually never more than 5
            infrastructure: FxHashMap::with_capacity_and_hasher(5, FxBuildHasher::default()), // usually never more than 5
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn add_planet(&mut self, name: String, planet: Planet) {
        self.planets.insert(name, planet);
        self.update_infrastructure_bonuses();
    }

    pub fn remove_planet(&mut self, name: &str) -> Option<Planet> {
        let planet = self.planets.remove(name);
        self.update_infrastructure_bonuses();
        planet
    }

    pub fn get_planet(&self, name: &str) -> Option<&Planet> {
        self.planets.get(name)
    }

    pub fn get_planet_mut(&mut self, name: &str) -> Option<&mut Planet> {
        self.planets.get_mut(name)
    }

    pub fn planets(&self) -> &FxHashMap<String, Planet> {
        &self.planets
    }

    pub fn planets_mut(&mut self) -> &mut FxHashMap<String, Planet> {
        &mut self.planets
    }

    pub fn add_infrastructure(&mut self, name: String, infra: Infrastructure) {
        self.infrastructure.insert(name, infra);
        self.update_infrastructure_bonuses();
    }

    pub fn remove_infrastructure(&mut self, name: &str) -> Option<Infrastructure> {
        let infra = self.infrastructure.remove(name);
        self.update_infrastructure_bonuses();
        infra
    }

    pub fn get_infrastructure(&self, name: &str) -> Option<&Infrastructure> {
        self.infrastructure.get(name)
    }

    pub fn infrastructure(&self) -> &FxHashMap<String, Infrastructure> {
        &self.infrastructure
    }

    pub fn get_gross_income(&self) -> f64 {
        self.planets.values().map(|planet| planet.get_gross_income()).sum()
    }

    pub fn get_net_income(&self) -> f64 {
        self.planets.values().map(|planet| planet.get_net_income()).sum()
    }

    pub fn total_upkeep(&self) -> f64 {
        self.planets.values().map(|planet| planet.total_upkeep()).sum()
    }

    pub fn avg_stability(&self) -> f64 {
        self.planets.values().map(|planet| planet.stability()).sum::<i32>() as f64 / self.planets.len() as f64
    }

    pub fn avg_ground_defense(&self) -> f64 {
        self.planets.values().map(|planet| planet.ground_defense_strength()).sum::<f64>() / self.planets.len() as f64
    }

    pub fn get_possible_actions(&self, balance: &Balance, slim: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        
        // First, check for uncolonized planets that we can colonize
        for (name, planet) in &self.planets {
            if !planet.has_colony() && balance.credits() >= 75000.0 {
                actions.push(Action::Colonize(name.clone()));
            }
        }
        
        // Then get actions from each colonized planet
        for planet in self.planets.values() {
            if planet.has_colony() {
                actions.extend(planet.get_possible_actions(balance, slim));
            }
        }
        
        actions
    }

    pub fn _get_differences(&self, other: &System) -> Vec<String> {
        let mut differences = Vec::new();
        
        for (name, other_planet) in &other.planets {
            if let Some(planet) = self.planets.get(name) {
                let diffs = planet._get_differences(other_planet);
                for diff in diffs {
                    differences.push(format!("Planet {}: {}", name, diff));
                }
            } else {
                differences.push(format!("Planet {} has been removed", name));
            }
        }
        
        differences
    }

    pub fn _print_differences(&self, other: &System) {
        for diff in self._get_differences(other) {
            println!("{:#?}", diff);
        }
    }
}