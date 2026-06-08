mod facility;

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;

use crate::constants::{FacilityData, FacilityType, MAX_FACILITIES, FACILITY_DATA};
use crate::constants::{AdminType, Resource};
use crate::solver::Action;
use crate::solver::Balance;
use rustc_hash::FxHashMap;

pub use facility::Facility;
use rustc_hash::FxHashSet;
use rustc_hash::FxHasher;

/// The facilities `facility_type` (transitively) upgrades from, nearest first.
///
/// Only *facility* requirements are followed (deposit/property requirements such
/// as "ores" or "farmland" are ignored), so this encodes the upgrade hierarchy:
/// `star fortress` yields `[battle station, orbital station]` and `high command`
/// yields `[military base, patrol hq]`. The bottom of a chain (or a facility
/// with no facility prerequisite) yields an empty vec.
pub fn upgrade_predecessors(facility_type: FacilityType) -> Vec<FacilityType> {
    let mut chain = Vec::new();
    let mut current = facility_type;
    while let Some(data) = FACILITY_DATA.get(&current) {
        let Some(prev) = data.requirements.iter().find_map(|req| FacilityType::from_str(req)) else {
            break;
        };
        if chain.contains(&prev) {
            break; // guard against malformed cyclic data
        }
        chain.push(prev);
        current = prev;
    }
    chain
}

#[derive(Debug, Clone)]
pub struct Planet {
    name: String,
    name_hash: u64,
    properties: FxHashMap<String, f64>,
    facilities: Vec<Facility>,
    hazard_rating: f64,
    is_free_port: bool,
    size: u32,
    growth_progress: f64, // % Progress towards next size 0 to 100
    free_port_days: u32,  // Days since becoming a free port
    has_hazard_pay: bool,
    has_decivilized: bool,
    has_colony: bool,
    stability: i32,
    admin: AdminType,
    system_stability_bonus: i32,
}

#[derive(Debug, Clone)]
pub(crate) struct PlanetWaitSnapshot {
    size: u32,
    growth_progress: f64,
    free_port_days: u32,
    facility_build_days: Vec<(i32, i32)>,
}

impl Hash for Planet {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name_hash.hash(state);
        self.is_free_port.hash(state);
        self.size.hash(state);
        self.free_port_days.hash(state);
        self.has_hazard_pay.hash(state);
        self.has_decivilized.hash(state);
        self.has_colony.hash(state);
        self.stability.hash(state);
        self.admin.hash(state);
        self.system_stability_bonus.hash(state);

        // Hash f64 values by converting them to bits
        self.hazard_rating.to_bits().hash(state);
        self.growth_progress.to_bits().hash(state);

        // Hash maps by sorting their entries
        let mut prop_entries: Vec<_> = self.properties.iter().collect();
        prop_entries.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in prop_entries {
            k.hash(state);
            v.to_bits().hash(state); // Convert f64 to bits before hashing
        }

        // Hash vector of facilities
        for facility in &self.facilities {
            facility.hash(state);
        }
    }
}

impl PartialEq for Planet {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.is_free_port == other.is_free_port
            && self.size == other.size
            && self.free_port_days == other.free_port_days
            && self.has_hazard_pay == other.has_hazard_pay
            && self.has_decivilized == other.has_decivilized
            && self.has_colony == other.has_colony
            && self.stability == other.stability
            && self.admin == other.admin
            && self.system_stability_bonus == other.system_stability_bonus
            && self.hazard_rating == other.hazard_rating
            && self.growth_progress == other.growth_progress
            && self.properties == other.properties
            && self.facilities == other.facilities
    }
}

impl Eq for Planet {}

impl Planet {
    pub fn new(name: String, properties: FxHashMap<String, f64>) -> Self {
        let hazard_rating = properties
            .get("hazard percent")
            .cloned()
            .expect("Planet must have a hazard rating");
        let size = properties.get("size").cloned().unwrap_or(0.0) as u32;

        let mut facilities = Vec::with_capacity(MAX_FACILITIES);

        // Add default facilities: Population and Spaceport
        if let Some(population) = Facility::new(FacilityType::Population) {
            facilities.push(population);
        }
        if let Some(spaceport) = Facility::new(FacilityType::Spaceport) {
            facilities.push(spaceport);
        }

        let name_hash = Self::_get_planet_name_hash(&name);

        Self {
            name,   
            name_hash,
            properties,
            facilities,
            hazard_rating,
            is_free_port: false,
            size,
            growth_progress: 0.0, // Progress towards next size 0 to 100
            free_port_days: 0,
            has_hazard_pay: false,
            has_decivilized: false,
            has_colony: false,
            stability: 5, // Default stability
            admin: AdminType::Base,
            system_stability_bonus: 0,
        }
    }

    /// Get the name of this planet
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn name_hash(&self) -> u64 {
        self.name_hash
    }

    pub fn _get_planet_name_hash(name: &str) -> u64 {
        let mut hasher = FxHasher::default();
        name.hash(&mut hasher);
        hasher.finish()
    }

    /// Get the properties of this planet
    pub fn properties(&self) -> &FxHashMap<String, f64> {
        &self.properties
    }

    /// Get the facilities of this planet
    pub fn facilities(&self) -> &Vec<Facility> {
        &self.facilities
    }

    /// Get a facility by its type
    pub fn get_facility(&self, facility_type: FacilityType) -> Option<&Facility> {
        self.facilities.iter().find(|f| f.facility_type() == &facility_type)
    }

    /// Get a facility by its type, mutable
    pub fn get_facility_mut(&mut self, facility_type: FacilityType) -> Option<&mut Facility> {
        self.facilities.iter_mut().find(|f| f.facility_type() == &facility_type)
    }

    /// True if the planet has `facility_type`, or a facility that (transitively)
    /// upgrades from it — e.g. a star fortress satisfies a request for an
    /// orbital or battle station. Used to avoid treating a tier as still
    /// "pending" once a higher tier of the same chain has been built.
    pub fn has_facility_or_upgrade(&self, facility_type: FacilityType) -> bool {
        self.facilities.iter().any(|f| {
            *f.facility_type() == facility_type
                || upgrade_predecessors(*f.facility_type()).contains(&facility_type)
        })
    }

    /// Check if we can add an industry to this planet
    pub fn can_add_industry(&self) -> bool {
        self.facilities
            .iter()
            .filter(|f| !f.is_structure())
            .count()
            <= (self.size.saturating_sub(2)) as usize
    }

    /// Checks which facilities are not built yet
    pub fn unbuilt_facilities(&self, only_industries: bool) -> Vec<FacilityType> {
        FACILITY_DATA
            .iter()
            .filter(|&(facility_type, data)| {
                self.get_facility(*facility_type).is_none()
                    && (!data.is_structure || !only_industries)
            })
            .map(|(facility_type, _)| *facility_type)
            .collect()
    }

    /// Add a facility to this planet
    pub fn add_facility(&mut self, facility_type: FacilityType) -> bool {

        // Check if this is an upgrade by looking at requirements
        if let Some(data) = FACILITY_DATA.get(&facility_type) {
            for req in data.requirements.iter() {
                // If requirement matches an existing facility, this is an upgrade
                let req_fac = FacilityType::from_str(req);
                if req_fac.is_none() {
                    continue;
                }
                if let Some(fac) = self.get_facility_mut(req_fac.unwrap()) {
                    _ = fac.swap_raw_w_data(facility_type, data, false);
                    return true;
                }
            }
        }

        // If not an upgrade, just add as a new facility
        // Create the new facility
        let new_facility = match Facility::new(facility_type) {
            Some(f) => f,
            None => return false,
        };

        self.facilities.push(new_facility);
        true
    }

    /// Remove a facility from this planet
    pub fn remove_facility(&mut self, facility_type: FacilityType) -> bool {
        if facility_type == FacilityType::Population || facility_type == FacilityType::Spaceport {
            return false; // Can't remove core facilities
        }

        if let Some(data) = FACILITY_DATA.get(&facility_type) {
            for downgrade in data.requirements.iter() {
                let down_type = FacilityType::from_str(downgrade);
                if down_type.is_none() {
                    continue;
                }
                let down_type = down_type.unwrap();
                if let Some(fac) = self.get_facility_mut(facility_type) {
                    if fac.swap_raw_w_data(down_type, FACILITY_DATA.get(&down_type).unwrap(), true).is_some() {
                        return true;
                    }
                }
            }
        }

        self.facilities.retain(|f| f.facility_type() != &facility_type);
        true
    }

    /// Get the hazard rating of this planet
    pub fn hazard_rating(&self) -> f64 {
        self.hazard_rating
    }

    /// Get the total upkeep of this planet
    pub fn total_upkeep(&self) -> f64 {
        if !self.has_colony() {
            return 0.0;
        }

        let upkeep = self
            .facilities
            .iter()
            .map(|facility| facility.calculate_upkeep(self.hazard_rating, self.size))
            .sum();

        if upkeep < 0.0 {
            panic!("Upkeep is negative for {}: {}", self.name, upkeep);
        } else {
            upkeep
        }
    }

    /// Get the admin type of this planet
    pub fn admin(&self) -> AdminType {
        self.admin
    }

    /// Set the admin type of this planet
    pub fn set_admin(&mut self, admin: AdminType) {
        self.admin = admin;
    }

    /// Calculate the accessibility of this planet
    pub fn calculate_accessibility(&self) -> f64 {
        let mut accessibility = self
            .properties
            .get("accessibility percent")
            .unwrap_or(&0.0)
            .clone();

        // Add accessibility bonuses from all facilities
        for facility in &self.facilities {
            accessibility += facility.calculate_accessibility_bonus() * 100.0;
        }

        // Add admin bonus (convert from decimal to percentage)
        accessibility += self.admin.bonuses().accessibility * 100.0;

        // Add size bonus
        accessibility += match self.size {
            0..=4 => 0.0,
            5 => 10.0,
            _ => 10.0 + (self.size - 5) as f64 * 5.0,
        };

        // Free port bonus
        if self.is_free_port {
            // increases to +25% over a year
            // we're doing it in chunks (not perfectly accurate) so we can cache things later
            accessibility += match self.free_port_days / 30 {//self.free_port_days as f64 / 365.0 * 0.25;
                0..=6 => 8.33,
                7..=11 => 16.66,
                _ => 25.0,
            }
        }

        accessibility
    }

    /// Set the free port status of this planet
    pub fn set_free_port(&mut self, is_free_port: bool) {
        self.is_free_port = is_free_port;
    }

    /// Check if this planet has a free port
    pub fn is_free_port(&self) -> bool {
        self.is_free_port
    }

    /// Get the size of this planet
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Check if this planet has hazard pay
    pub fn has_hazard_pay(&self) -> bool {
        self.has_hazard_pay
    }

    /// Set the hazard pay status of this planet
    pub fn set_hazard_pay(&mut self, enabled: bool) {
        self.has_hazard_pay = enabled;
    }

    /// Check if this planet has decivilized
    pub fn has_decivilized(&self) -> bool {
        self.has_decivilized
    }

    /// Set the decivilized status of this planet
    pub fn set_decivilized(&mut self, has_decivilized: bool) {
        self.has_decivilized = has_decivilized;
    }

    /// Set the base stability of this planet
    pub fn set_base_stability(&mut self, stability: i32) {
        self.stability = stability;
    }

    /// Set the system stability bonus of this planet
    pub fn set_system_stability_bonus(&mut self, bonus: i32) {
        self.system_stability_bonus = bonus;
    }

    /// Get the stability of this planet
    pub fn stability(&self) -> i32 {
        let mut total = self.stability; // Base stability

        // Add bonuses from facilities
        for facility in &self.facilities {
            total += facility.calculate_stability_bonus();
        }

        // Add admin bonus
        total += self.admin.bonuses().stability;

        // Add system stability bonus
        total += self.system_stability_bonus;

        // Subtract freeport penalty
        if self.is_free_port {
            // Starts at -1 and goes to -3 over a year
            let penalty = match self.free_port_days / 30 {
                0..=6 => 1,
                7..=11 => 2,
                _ => 3,
            };
            total -= penalty;
        }

        // Subtract deciv subpop penalty
        if self.has_decivilized {
            total -= 2;
        }

        // Stability is clamped to 0..=10 in-game.
        total.clamp(0, 10)
    }

    pub fn ground_defense_strength(&self) -> f64 {
        if !self.has_colony() {
            return 0.0;
        }
        
        // base strength
        let mut strength = if self.size == 3 {
            50.0
        } else {
            100.0 * (self.size - 3) as f64
        };

        // Penalty for low stability
        strength *= 0.25 + self.stability() as f64 * 0.075;

        // Sum up all defense multiplier bonuses and apply them once
        let mut total_defense_bonus = 1.0;
        for facility in &self.facilities {
            total_defense_bonus += facility.calculate_defense_multiplier() - 1.0;
        }

        // Apply the total defense bonus
        strength *= total_defense_bonus;

        strength
    }

    /// Get the growth progress of this planet
    pub fn growth_progress(&self) -> f64 {
        self.growth_progress
    }

    pub(crate) fn snapshot_wait_state(&self) -> PlanetWaitSnapshot {
        PlanetWaitSnapshot {
            size: self.size,
            growth_progress: self.growth_progress,
            free_port_days: self.free_port_days,
            facility_build_days: self
                .facilities
                .iter()
                .map(|f| f.build_days_state())
                .collect(),
        }
    }

    pub(crate) fn restore_wait_state(&mut self, snapshot: &PlanetWaitSnapshot) {
        self.size = snapshot.size;
        self.growth_progress = snapshot.growth_progress;
        self.free_port_days = snapshot.free_port_days;

        if self.facilities.len() != snapshot.facility_build_days.len() {
            panic!(
                "restore_wait_state facility count mismatch for {}: {} != {}",
                self.name,
                self.facilities.len(),
                snapshot.facility_build_days.len()
            );
        }

        for (facility, (cur, total)) in self
            .facilities
            .iter_mut()
            .zip(snapshot.facility_build_days.iter().copied())
        {
            facility.set_build_days_state(cur, total);
        }
    }

    /// Set the has colony status of this planet
    pub fn set_has_colony(&mut self, has_colony: bool) {
        self.has_colony = has_colony;
        if has_colony {
            self.size = 3;
            self.growth_progress = 0.0;
        } else {
            self.size = 0;
            self.growth_progress = 0.0;
        }
    }

    /// Check if this planet has a colony
    pub fn has_colony(&self) -> bool {
        self.has_colony
    }

    /// Get the number of facilities with improvements
    pub fn get_num_facility_improvements(&self) -> u32 {
        self.facilities
            .iter()
            .filter(|f| f.has_improvements())
            .count() as u32
    }

    /// Update the growth of this planet
    pub fn update_growth(&mut self, days: i32, larger_friendly_colonies: Option<&[(String, u32)]>) {
        const MAX_SIZE: u32 = 6;
        const MIN_SIZE: u32 = 3;

        // Don't grow under minimum
        if self.size <= MIN_SIZE && days < 0 && self.growth_progress <= 0.0 {
            return;
        }

        let is_undoing = days < 0;
        let mut remaining_days = days.abs() as u32;
        let mut growth_points = self.calculate_growth_points(larger_friendly_colonies);
        // println!("Days: {} - Growth points: {} - Growth progress: {}", days, growth_points, self.growth_progress);
        
        // First calculate size changes
        loop {
            let days_to_size_change = if is_undoing {
                self.days_from_last_size_change(Some(growth_points))
            } else {
                self.days_till_next_size(Some(growth_points), larger_friendly_colonies)
            };

            // break if not applicable
            if days_to_size_change.is_none() || days_to_size_change.unwrap() > remaining_days {
                break;
            }

            let days_to_size_change = days_to_size_change.unwrap();

            remaining_days = remaining_days.saturating_sub(days_to_size_change);
            
            if is_undoing && self.size > MIN_SIZE {
                self.size = self.size.saturating_sub(1);
                growth_points = self.calculate_growth_points(larger_friendly_colonies);
                self.growth_progress = 100.0;
            } else if self.size < MAX_SIZE {
                self.size = self.size.saturating_add(1);
                growth_points = self.calculate_growth_points(larger_friendly_colonies);
                self.growth_progress = 0.0;
            }
        }

        // Then update growth progress for the remaining days
        // If at max size, will still increase growth, purely for reversability
        let growth_per_day = growth_points as f64 / 30.0;
        let growth = remaining_days as f64 * growth_per_day;
        if is_undoing {
            self.growth_progress -= growth;
        } else {
            self.growth_progress += growth;
        }

        // Update size in the off chance we missed a change
        // TODO: rewrite function to only have to do this once
        if self.growth_progress >= 100.0 && self.size < MAX_SIZE {
            self.size = self.size.saturating_add(1);
            self.growth_progress = self.growth_progress - 100.0;
        } else if self.growth_progress < 0.0 && self.size > MIN_SIZE {
            self.size = self.size.saturating_sub(1);
            self.growth_progress = 100.0 + self.growth_progress;
        }

        // Round growth progress to make reversable
        self.growth_progress = self.growth_progress.round()
    }

    /// Get the number of days until the next size change
    pub fn days_till_next_size(
        &self,
        growth_points: Option<i32>,
        larger_friendly_colonies: Option<&[(String, u32)]>,
    ) -> Option<u32> {
        if !self.has_colony() || self.size >= 6 || self.size <= 2 {
            return None;
        }

        let growth_points =
            growth_points.unwrap_or_else(|| self.calculate_growth_points(larger_friendly_colonies));
        if growth_points <= 0 {
            return None;
        }

        let remaining_growth = 100.0 - self.growth_progress;
        // println!(" Growth Progress: {}", self.growth_progress);

        let days = (remaining_growth / growth_points as f64 * 30.0).ceil() as u32;

        Some(days)
    }

    /// Get the number of days since the last size change
    pub fn days_from_last_size_change(&self, growth_points: Option<i32>) -> Option<u32> {
        if !self.has_colony() || self.size <= 3 {
            return None;
        }

        let growth_points = growth_points.unwrap_or_else(|| self.calculate_growth_points(None));
        if growth_points <= 0 {
            return None;
        }

        let growth_per_day = growth_points as f64 / 30.0;
        Some((self.growth_progress as f64 / growth_per_day).ceil() as u32)
    }

    /// Calculate the growth points for this planet (% of size per month)
    pub fn calculate_growth_points(
        &self,
        larger_friendly_colonies: Option<&[(String, u32)]>,
    ) -> i32 {
        if !self.has_colony() {
            return 0;
        }
        // TODO: make this based on colony size

        let mut points = 0;

        // Base points from stability
        let stability = self.stability();
        points += stability;

        // Penalty for hazardous conditions
        let hazard_penalty = ((self.hazard_rating - 100.0) / 50.0).ceil() as i32;
        points -= hazard_penalty;

        // Bonus for hazard pay
        if self.has_hazard_pay {
            points += 2;
        }

        // Bonus for habitable property
        if self.properties.get("habitable").is_some() {
            points += (self.size - 1) as i32;
        }

        // Bonus for deciv subpop
        if self.has_decivilized {
            points += self.size as i32;
        }

        // Penalty for <5 stability
        if stability < 5 {
            points -= (5 - stability) as i32;
        }

        // TODO: add mild climate property so we can give a bonus for it too

        // Bonus for free port status
        if self.is_free_port {
            let months = self.free_port_days / 30;
            points += match months {
                0..=6 => 1,
                7..=11 => 2,
                _ => 3,
            };
        }

        // Bonuses for spaceport and megaport
        if let Some(spaceport) = self.get_facility(FacilityType::Spaceport) {
            points += 2;
            if spaceport.has_improvements() {
                points += 1;
            }
        }
        if let Some(megaport) = self.get_facility(FacilityType::Megaport) {
            points += self.size as i32;
            if megaport.has_improvements() {
                points += 2;
            }
        }

        // Bonus from larger friendly colonies
        if let Some(colonies) = larger_friendly_colonies {
            let largest_bonus = colonies
                .iter()
                .map(|(_, size)| (*size as i32 - self.size as i32).clamp(0, 3))
                .max()
                .unwrap_or(0);
            points += largest_bonus;
        }

        points
    }

    /// Get the population of this planet
    pub fn get_population(&self) -> f64 {
        // Population formula: P = 10^(n+g)
        // where n is colony size and g is growth progress
        10.0f64.powf((self.size as f64) + (self.growth_progress as f64 / 100.0))
    }

    /// Calculate the total production of a specific resource from all facilities
    pub fn calculate_resource_production(&self, resource: Resource) -> f64 {
        if !self.has_colony() {
            return 0.0;
        }

        // Extraction resources are gated on deposit presence and get the deposit's
        // abundance modifier added as a bonus; non-deposit resources are unaffected.
        let production_bonus = match facility::deposit_status(resource, self) {
            facility::DepositStatus::Absent => return 0.0, // no deposit -> no production
            facility::DepositStatus::Present(modifier) => modifier,
            facility::DepositStatus::NotDeposit => 0.0,
        };

        let mut total_production = 0.0;
        for facility in &self.facilities {
            total_production += facility.calculate_resource_production(
                resource,
                self.size,
                production_bonus,
                self.is_free_port,
            );
        }

        total_production
    }

    /// Get a map of all resources produced by this planet and their amounts
    pub fn get_resource_production(&self) -> FxHashMap<Resource, f64> {
        if !self.has_colony() {
            return FxHashMap::default();
        }

        let mut production = FxHashMap::default();
        let mut seen_resources = FxHashSet::default();

        // Collect all unique resources from all facilities
        for facility in &self.facilities {
            for resource in facility.production().keys() {
                seen_resources.insert(*resource);
            }
        }

        // Calculate production for each resource
        for resource in seen_resources {
            let amount = self.calculate_resource_production(resource);
            if amount > 0.0 {
                production.insert(resource, amount);
            }
        }

        production
    }

    /// Free-port maturity bucket; boundaries match the accessibility/stability tiers
    /// in `calculate_accessibility` and `stability` (changes at month 7 and month 12).
    fn free_port_bucket(free_port_days: u32) -> u32 {
        match free_port_days / 30 {
            0..=6 => 0,
            7..=11 => 1,
            _ => 2,
        }
    }

    /// Get the gross income of this planet (per month)
    pub fn get_gross_income(&self) -> f64 {
        if !self.has_colony() {
            return 0.0;
        }

        let mut gross_income: f64 = 0.0;
        let mut highest_income_mult: f64 = 1.0;
        let accessibility = self.calculate_accessibility();
        for facility in &self.facilities {
            let facility_income = facility.calculate_gross_income(self.size, self, accessibility);
            gross_income += facility_income;
            let income_mult = facility.calculate_income_multiplier();
            highest_income_mult = highest_income_mult.max(income_mult);
        }

        // Subtract <5 stability penalty
        let stability = self.stability();
        if stability < 5 {
            gross_income *= 1.0 - (0.2 * (5 - stability) as f64);
        }

        gross_income * highest_income_mult
    }

    /// Get the net income of this planet (per month)
    pub fn get_net_income(&self) -> f64 {
        if !self.has_colony() {
            return 0.0;
        }

        let mut net_income = self.get_gross_income();
        let total_upkeep = self.total_upkeep();
        net_income -= total_upkeep;
        net_income
    }

    /// Will progress buildings and growth, and return incomes
    pub fn wait(&mut self, months: u32, debug: bool) -> (f64, f64) {
        if !self.has_colony() {
            return (0.0, 0.0);
        }

        let mut gross_income: f64 = 0.0;
        let mut net_income: f64 = 0.0;

        if debug {
            println!(
                "\nWAIT - Growth: {}, Size: {}",
                self.growth_progress.round(), self.size
            );
            println!(
                "Accumulated income: Gross: {}, Net: {}",
                gross_income.round(), net_income.round()
            );
        }

        // Iterate through each month
        let mut last_free_port_days = self.free_port_days;
        let mut last_fac_build_days = Vec::with_capacity(self.facilities.len());
        let mut last_size = self.size;
        let mut last_gross = 0.0;
        let mut last_net = 0.0;
        for facility in &self.facilities {
            last_fac_build_days.push(facility.remaining_build_days());
        }
        for i in 0..months {
            // Update free port days if applicable
            if self.is_free_port {
                last_free_port_days = self.free_port_days;
                self.free_port_days = self.free_port_days.saturating_add(30);
            }

            // Update planet growth
            last_size = self.size;
            self.update_growth(30, None);


            // Progress build days for all facilities
            for (index, facility) in &mut self.facilities.iter_mut().enumerate() {
                last_fac_build_days[index] = facility.remaining_build_days();
                facility.progress_build_days(30);
            }

            // Calculate monthly income
            if i == 0 || 
            (Self::free_port_bucket(last_free_port_days) != Self::free_port_bucket(self.free_port_days)) ||
            self.facilities.iter().zip(last_fac_build_days.iter()).any(|(fac, last)| fac.remaining_build_days() <= 0 && *last > 0) 
            || self.size != last_size {
                last_gross = self.get_gross_income();
                last_net = last_gross - self.total_upkeep();
            }

            // Accumulate income
            gross_income += last_gross;
            net_income += last_net;

            if debug {
                println!(
                    "WAIT - Gross: {:.2}, Net: {:.2}, Growth: {:.2}, Growth P: {}, Size: {}",
                    last_gross.round(), last_net.round(), self.growth_progress.round(), self.calculate_growth_points(None), self.size
                );
                println!(
                    "Accumulated income: Gross: {:.2}, Net: {:.2}",
                    gross_income.round(), net_income.round()
                );
            }
        }

        // Return total income
        (gross_income, net_income)
    }

    /// Will undo the effects of wait and return negative incomes
    pub fn undo_wait(&mut self, months: u32, debug: bool) -> (f64, f64) {
        if !self.has_colony() {
            return (0.0, 0.0);
        }

        let mut gross_income: f64 = 0.0;
        let mut net_income: f64 = 0.0;

        if debug {
            println!(
                "\nUNDO WAIT - Growth: {}, Size: {}",
                self.growth_progress.round(), self.size
            );
            println!(
                "Accumulated income: Gross: {}, Net: {}",
                gross_income.round(), net_income.round()
            );
        }

        // For each month, we need to:
        // 1. Calculate income for that month (before undoing changes)
        // 2. Undo growth progress
        // 3. Undo facility build progress
        let mut last_free_port_days = self.free_port_days;
        let mut last_fac_build_days = Vec::with_capacity(self.facilities.len());
        let mut last_size = self.size;
        let mut last_gross: f64 = 0.0;
        let mut last_net: f64 = 0.0;
        for facility in &self.facilities {
            last_fac_build_days.push(facility.remaining_build_days());
        }
        for i in 0..months {
            // First calculate income for this month before undoing changes
            if i == 0 ||
            (Self::free_port_bucket(last_free_port_days) != Self::free_port_bucket(self.free_port_days)) ||
            self.facilities.iter().zip(last_fac_build_days.iter()).any(|(fac, last)| fac.remaining_build_days() > 0 && *last <= 0) 
            || last_size != self.size {
                last_gross = self.get_gross_income();
                last_net = last_gross - self.total_upkeep();
            }

            gross_income += last_gross;
            net_income += last_net;

            // Undo facility build progress
            for (index, facility) in &mut self.facilities.iter_mut().enumerate() {
                last_fac_build_days[index] = facility.remaining_build_days();
                facility.progress_build_days(-30);
            }

            // Undo growth progress - now uses negative days
            last_size = self.size;
            self.update_growth(-30, None);

            // Update free port days if applicable
            if self.is_free_port {
                last_free_port_days = self.free_port_days;
                self.free_port_days = self.free_port_days.saturating_sub(30);
            }

            if debug {
                println!(
                    "UNDO WAIT - Gross: {:.2}, Net: {:.2}, Growth: {:.2}, Growth P: {}, Size: {}",
                    last_gross.round(), last_net.round(), self.growth_progress.round(), self.calculate_growth_points(None), self.size
                );
                println!(
                    "Accumulated income: Gross: {:.2}, Net: {:.2}",
                    gross_income.round(), net_income.round()
                );
            }
        }

        (gross_income, net_income)
    }

    /// Check if this planet meets the requirements for a facility.
    ///
    /// Facility requirements (e.g. megaport needs a spaceport) are ALL required.
    /// Property/deposit requirements (e.g. mining lists ores/rare ores/volatiles/organics)
    /// are satisfied if ANY one of them is present on the planet.
    fn meets_facility_requirements(&self, requirements: &[&str]) -> bool {
        let mut has_deposit_req = false;
        let mut deposit_satisfied = false;

        for req in requirements {
            if FacilityType::from_str(req).is_some() {
                // Facility requirement: must be present (these are AND'd together).
                if !self.facilities.iter().any(|f| f.name() == *req) {
                    return false;
                }
            } else {
                // Property/deposit requirement: any one being present is enough (OR).
                // A deposit counts as present if its column exists at all (its abundance
                // modifier may be 0 or -1); `water` is a boolean condition, so it must be true.
                has_deposit_req = true;
                let present = if *req == "water" {
                    self.properties.get(*req).copied().unwrap_or(0.0) > 0.0
                } else {
                    self.properties.contains_key(*req)
                };
                if present {
                    deposit_satisfied = true;
                }
            }
        }

        !has_deposit_req || deposit_satisfied
    }

    /// Get all possible actions for this planet
    pub fn get_possible_actions(&self, balance: &Balance, slim: bool) -> Vec<Action> {
        let num_actions_estimate = 4 + self.facilities.len() * 2;
        let mut actions = Vec::with_capacity(num_actions_estimate);

        // If this planet is not colonized, return empty list since colonization is handled by System
        if !self.has_colony() {
            return actions;
        }

        // Free port toggle (only add if not already a free port)
        if !self.is_free_port() {
            actions.push(Action::SetFreePort(self.name_hash, true));
        }

        // Hazard pay toggle (only add if not already paying hazard pay)
        if !self.has_hazard_pay {
            actions.push(Action::SetHazardPay(self.name_hash, true));
        }

        // Admin upgrade (only if not already an alpha core)
        if self.admin == AdminType::Base {
            if balance.alpha_cores() > 0 {
                actions.push(Action::UpgradeAdmin(self.name_hash));
            }
        }

        // Add new facilities
        for (facility_type, facility) in FACILITY_DATA.iter() {
            if let Some(facility) = self.get_facility(*facility_type) {
                continue;
            }

            // Check if we meet the requirements for this facility
            if !self.meets_facility_requirements(&facility.requirements) {
                continue;
            }

            // TODO; check if this is what's causing irreversiblity
            // play with linear simulator; farming seems to disapear

            // Check if this is an upgrade (any requirement matches an existing facility)
            let upgrade_from = facility
                .requirements
                .iter()
                .find(|req| self.facilities.iter().any(|f| f.name() == **req))
                .and_then(|name| self.get_facility(FacilityType::from_str(name).unwrap()));

            // Check if we already have an upgrade of this facility. This must
            // follow the *whole* chain: a star fortress upgrades from a battle
            // station which upgrades from an orbital station, so a planet that
            // already has a star fortress must not be offered either lower tier
            // again. A one-level check (only "does any facility directly require
            // this one") misses that, and re-offers the orbital/battle station
            // as a brand-new facility after the chain has topped out.
            let has_upgrade = self
                .facilities
                .iter()
                .any(|f| upgrade_predecessors(*f.facility_type()).contains(facility_type));

            if has_upgrade {
                continue;
            }

            // Determine if we can add this facility
            let can_add_industry = self.can_add_industry();
            let can_add = match upgrade_from {
                // If it's an upgrade, check if we need a new industry slot
                // And whether the old facility is done building
                Some(old_facility) => {
                    // Need industry slot if upgrading from structure to industry
                    (!old_facility.is_structure()
                        || facility.is_structure
                        || can_add_industry)
                        && old_facility.remaining_build_days() <= 0
                }
                // Not an upgrade - normal structure/industry check
                None => facility.is_structure || can_add_industry,
            };

            if can_add && balance.credits() >= facility.build_cost as f64 {
                actions.push(Action::AddFacility(self.name_hash, facility_type.clone()));
            } else if !facility.is_structure && !can_add_industry {
                // Add a Wait action; need to calculate how long we need to grow
                // to next colony size
                let days = self.days_till_next_size(None, None);
                if let Some(days) = days {
                    let months = (days as f64 / 30.0).ceil() as u32;
                    if months > 0 {
                        let action = Action::Wait(months);
                        if !actions.contains(&action) {
                            actions.push(action);
                        }
                    }
                }
            }
        }

        // Get facility-specific actions
        for facility in &self.facilities {
            actions.extend(facility.get_possible_actions(self, balance, slim));
        }

        // Deduplicate Wait actions
        let mut seen_waits = std::collections::HashSet::new();
        actions.retain(|action| {
            if let Action::Wait(months) = action {
                seen_waits.insert(*months)
            } else {
                true
            }
        });

        actions
    }

    pub fn _get_differences(&self, other: &Planet) -> Vec<String> {
        let mut differences = Vec::new();
        if self.name != other.name {
            differences.push(format!(" Name changed from {} to {}", self.name, other.name));
        }
        
        for (key, value) in self.properties.iter() {
            if let Some(other_value) = other.properties.get(key) {
                if value != other_value {
                    differences.push(format!(" {} changed from {} to {}", key, value, other_value));
                }
            } else {
                differences.push(format!(" {} was removed", key));
            }
        }
        
        for (key, value) in other.properties.iter() {
            if !self.properties.contains_key(key) {
                differences.push(format!(" {} was added with value {}", key, value));
            }
        }
        
        if self.admin != other.admin {
            differences.push(format!(" Admin changed from {:?} to {:?}", self.admin, other.admin));
        }
        
        if self.is_free_port != other.is_free_port {
            differences.push(format!(" Free port status changed from {} to {}", self.is_free_port, other.is_free_port));
        }
        
        if self.has_hazard_pay != other.has_hazard_pay {
            differences.push(format!(" Hazard pay status changed from {} to {}", self.has_hazard_pay, other.has_hazard_pay));
        }
        
        if self.has_decivilized != other.has_decivilized {
            differences.push(format!(" Decivilized status changed from {} to {}", self.has_decivilized, other.has_decivilized));
        }
        
        for (self_facility, other_facility) in self.facilities.iter().zip(other.facilities.iter()) {
            let facility_differences = self_facility._get_differences(other_facility);
            for diff in facility_differences {
                differences.push(format!("  Facility {}: {}", self_facility.name(), diff));
            }
        }
        
        if self.facilities.len() != other.facilities.len() {
            differences.push(format!("Number of facilities changed from {} to {}", self.facilities.len(), other.facilities.len()));
        }
        
        differences
    }
}

impl fmt::Display for Planet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let size = if self.has_colony() { self.size } else { 0 };
        write!(
            f,
            "{} (Hazard: {}%, Size: {})",
            self.name, self.hazard_rating as i32, size
        )
    }
}
