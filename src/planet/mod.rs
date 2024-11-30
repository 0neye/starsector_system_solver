mod facility;

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::hash::Hash;

use crate::constants::{AdminType, Resource};
use crate::solver::Action;
use crate::solver::Balance;

pub use facility::Facility;

#[derive(Debug, Clone)]
pub struct Planet {
    name: String,
    properties: HashMap<String, f32>,
    facilities: HashMap<String, Facility>,
    hazard_rating: f32,
    is_free_port: bool,
    size: u32,
    growth_progress: f32,  // Progress towards next size (0.0 to 1.0)
    free_port_days: u32,   // Days since becoming a free port
    has_hazard_pay: bool,
    has_decivilized: bool,
    has_colony: bool,
    stability: i32,
    admin: AdminType,
    system_stability_bonus: i32,
}

impl Hash for Planet {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.is_free_port.hash(state);
        self.size.hash(state);
        self.free_port_days.hash(state);
        self.has_hazard_pay.hash(state);
        self.has_decivilized.hash(state);
        self.has_colony.hash(state);
        self.stability.hash(state);
        self.admin.hash(state);
        self.system_stability_bonus.hash(state);
        
        // Hash f32 values by converting them to bits
        self.hazard_rating.to_bits().hash(state);
        self.growth_progress.to_bits().hash(state);
        
        // Hash maps by sorting their entries
        let mut prop_entries: Vec<_> = self.properties.iter().collect();
        prop_entries.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in prop_entries {
            k.hash(state);
            v.to_bits().hash(state);  // Convert f32 to bits before hashing
        }
        
        let mut fac_entries: Vec<_> = self.facilities.iter().collect();
        fac_entries.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in fac_entries {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl PartialEq for Planet {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name &&
        self.is_free_port == other.is_free_port &&
        self.size == other.size &&
        self.free_port_days == other.free_port_days &&
        self.has_hazard_pay == other.has_hazard_pay &&
        self.has_decivilized == other.has_decivilized &&
        self.has_colony == other.has_colony &&
        self.stability == other.stability &&
        self.admin == other.admin &&
        self.system_stability_bonus == other.system_stability_bonus &&
        self.hazard_rating == other.hazard_rating &&
        self.growth_progress == other.growth_progress &&
        self.properties == other.properties &&
        self.facilities == other.facilities
    }
}

impl Eq for Planet {}

impl Planet {
    pub fn new(name: String, properties: HashMap<String, f32>) -> Self {
        let hazard_rating = properties.get("hazard percent").cloned().expect("Planet must have a hazard rating");
        let size = properties.get("size").cloned().unwrap_or(0.0) as u32;
        
        let mut facilities = HashMap::new();
        
        // Add default facilities: Population and Spaceport
        if let Some(population) = Facility::new("population".to_string()) {
            facilities.insert("population".to_string(), population);
        }
        if let Some(spaceport) = Facility::new("spaceport".to_string()) {
            facilities.insert("spaceport".to_string(), spaceport);
        }

        Self {
            name,
            properties,
            facilities,
            hazard_rating,
            is_free_port: false,
            size,
            growth_progress: 0.0,
            free_port_days: 0,
            has_hazard_pay: false,
            has_decivilized: false,
            has_colony: false,
            stability: 5,  // Default stability
            admin: AdminType::Base,
            system_stability_bonus: 0,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn properties(&self) -> &HashMap<String, f32> {
        &self.properties
    }

    pub fn facilities(&self) -> &HashMap<String, Facility> {
        &self.facilities
    }

    pub fn get_facility(&self, name: &str) -> Option<&Facility> {
        self.facilities.get(name)
    }

    pub fn get_facility_mut(&mut self, name: &str) -> Option<&mut Facility> {
        self.facilities.get_mut(name)
    }

    fn can_add_industry(&self) -> bool {
        self.facilities.iter()
            .filter(|(_, f)| !f.is_structure())
            .count() < (self.size - 2) as usize
    }

    pub fn add_facility(&mut self, name: String) -> bool {
        if self.facilities.contains_key(&name) {
            return false;
        }
        if let Some(facility) = Facility::new(name.clone()) {
            if !self.can_add_industry() {
                return false;
            }

            self.facilities.insert(name, facility);
            true
        } else {
            false
        }
    }

    pub fn remove_facility(&mut self, name: &str) -> bool {
        if name == "population" || name == "spaceport" {
            return false; // Can't remove core facilities
        }
        self.facilities.remove(name).is_some()
    }

    pub fn hazard_rating(&self) -> f32 {
        self.hazard_rating
    }

    pub fn total_upkeep(&self) -> f32 {
        self.facilities.values()
            .map(|facility| facility.calculate_upkeep(self.hazard_rating))
            .sum()
    }

    pub fn admin(&self) -> AdminType {
        self.admin
    }

    pub fn set_admin(&mut self, admin: AdminType) {
        self.admin = admin;
    }

    pub fn calculate_accessibility(&self) -> f32 {
        let mut accessibility = self.properties.get("accessibility percent").unwrap_or(&0.0).clone();
        
        // Add accessibility bonuses from all facilities
        for facility in self.facilities.values() {
            accessibility += facility.accessibility_bonus() * 100.0;
        }

        // Add admin bonus
        accessibility += self.admin.bonuses().accessibility;

        // Free port bonus
        if self.is_free_port {
            accessibility += 0.3;
        }

        accessibility
    }

    pub fn set_free_port(&mut self, is_free_port: bool) {
        self.is_free_port = is_free_port;
    }

    pub fn is_free_port(&self) -> bool {
        self.is_free_port
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn set_hazard_pay(&mut self, enabled: bool) {
        self.has_hazard_pay = enabled;
    }

    pub fn set_decivilized(&mut self, has_decivilized: bool) {
        self.has_decivilized = has_decivilized;
    }

    pub fn set_base_stability(&mut self, stability: i32) {
        self.stability = stability;
    }

    pub fn set_system_stability_bonus(&mut self, bonus: i32) {
        self.system_stability_bonus = bonus;
    }

    pub fn stability(&self) -> i32 {
        let mut total = self.stability;  // Base stability

        // Add bonuses from facilities
        for facility in self.facilities.values() {
            total += facility.stability_bonus();
        }

        // Add admin bonus
        total += self.admin.bonuses().stability;

        // Add system stability bonus
        total += self.system_stability_bonus;

        total
    }

    pub fn growth_progress(&self) -> f32 {
        self.growth_progress
    }

    pub fn set_has_colony(&mut self, has_colony: bool) {
        self.has_colony = has_colony;
    }

    pub fn has_colony(&self) -> bool {
        self.has_colony
    }

    pub fn get_num_facility_improvements(&self) -> u32 {
        self.facilities.iter().filter(|(_, f)| f.has_improvements()).count() as u32
    }

    pub fn update_growth(&mut self, days: u32, larger_friendly_colonies: Option<&[(String, u32)]>) {
        // Don't grow if already at max size
        if self.size >= 10 {
            return;
        }

        // Update free port days
        if self.is_free_port {
            self.free_port_days = self.free_port_days.saturating_add(days);
        } else {
            self.free_port_days = 0;
        }

        // Calculate growth points and change
        let growth_points = self.calculate_growth_points(larger_friendly_colonies);
        let growth_change = (growth_points as f32 * days as f32) / (100.0 * self.size as f32);
        self.growth_progress += growth_change;

        // Handle size changes
        if self.growth_progress >= 1.0 {
            self.size += 1;
            self.growth_progress = 0.0;
        } else if self.growth_progress <= -1.0 && self.size > 3 {
            self.size -= 1;
            self.growth_progress = 0.0;
        }
    }


    pub fn days_till_next_size(&self, larger_friendly_colonies: Option<&[(String, u32)]>) -> Option<u32> {
        if self.size >= 10 {
            return None;
        }

        let growth_points = self.calculate_growth_points(larger_friendly_colonies);
        if growth_points <= 0 {
            return None;
        }

        let remaining_growth = 1.0 - self.growth_progress;
        let days = (remaining_growth * 100.0 * self.size as f32 / growth_points as f32).ceil() as u32;
        Some(days)
    }


    pub fn calculate_growth_points(&self, larger_friendly_colonies: Option<&[(String, u32)]>) -> i32 {
        let mut points = 0;

        // Base points from stability
        points += self.stability;

        // Penalty for hazardous conditions
        let hazard_penalty = ((self.hazard_rating - 100.0) / 50.0).ceil() as i32;
        points -= hazard_penalty;

        // Bonus for hazard pay
        if self.has_hazard_pay {
            points += 2;
        }

        // Bonus for free port status
        if self.is_free_port {
            let months = self.free_port_days / 30;
            points += match months {
                0..=1 => 1,
                2..=3 => 2,
                _ => 3,
            };
        }

        // Penalty for decivilized status
        if self.has_decivilized {
            points -= 3;
        }

        // Bonuses for spaceport and megaport
        if let Some(spaceport) = self.get_facility("spaceport") {
            points += 2;
            if spaceport.has_improvements() {
                points += 1;
            }
        }
        if let Some(megaport) = self.get_facility("megaport") {
            points += self.size as i32;
            if megaport.has_improvements() {
                points += 2;
            }
        }

        // Bonus from larger friendly colonies
        if let Some(colonies) = larger_friendly_colonies {
            let largest_bonus = colonies.iter()
                .map(|(_, size)| (*size as i32 - self.size as i32).clamp(0, 3))
                .max()
                .unwrap_or(0);
            points += largest_bonus;
        }

        points
    }

    pub fn get_population(&self) -> f64 {
        // Population formula: P = 10^(n+g)
        // where n is colony size and g is growth progress
        10.0f64.powf((self.size as f64) + (self.growth_progress as f64))
    }

    /// Calculate the total production of a specific resource from all facilities
    pub fn calculate_resource_production(&self, resource: Resource) -> f32 {
        let mut total_production = 0.0;
        let production_bonus = 0.0; // Can be modified later to account for global bonuses

        for facility in self.facilities.values() {
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
    pub fn get_resource_production(&self) -> HashMap<Resource, f32> {
        let mut production = HashMap::new();
        let mut seen_resources = HashSet::new();

        // Collect all unique resources from all facilities
        for facility in self.facilities.values() {
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

    pub fn get_gross_income(&self) -> f32 {
        let mut gross_income = 0.0;
        let mut highest_income_mult = 1.0;
        for facility in self.facilities.values() {
            gross_income += facility.calculate_gross_income(self.size, self);
            let income_mult = facility.income_multiplier();
            highest_income_mult = if income_mult > highest_income_mult { income_mult } else { highest_income_mult };
        }

        gross_income * highest_income_mult
    }

    pub fn get_net_income(&self) -> f32 {
        let mut net_income = self.get_gross_income();
        let total_upkeep = self.total_upkeep();
        net_income -= total_upkeep;
        net_income
    }

    pub fn get_possible_actions(&self, balance: &Balance) -> Vec<Action> {
        let mut actions = Vec::new();
        let planet_name = self.name.clone();
        
        // If this planet is not colonized, the only possible action is to do so
        if !self.has_colony() {
            actions.push(Action::Colonize(planet_name.clone()));
            return actions;
        }

        // Free port toggle (only add if not already a free port)
        if !self.is_free_port() {
            actions.push(Action::SetFreePort(planet_name.clone(), true));
        }
        
        // Hazard pay toggle (only add if not already paying hazard pay)
        if !self.has_hazard_pay {
            actions.push(Action::SetHazardPay(planet_name.clone(), true));
        }

        // Admin upgrade (only if not already an alpha core)
        if self.admin == AdminType::Base {
            if balance.alpha_cores() > 0 {
                actions.push(Action::UpgradeAdmin(planet_name.clone()));
            }
        }
        
        // Add new facilities
        for (name, facility) in crate::constants::FACILITY_DATA.iter() {
            if !self.facilities().contains_key(*name) {
                if facility.is_structure || self.can_add_industry() {
                    if balance.credits() < facility.build_cost as f32 {
                        continue;
                    }
                    actions.push(Action::AddFacility(planet_name.clone(), name.to_string()));
                }
                else {
                    // Add a Wait action; need to calculate how long we need to grow
                    // to next colony size
                    let days = self.days_till_next_size(None);
                    if days.is_none() {
                        continue;
                    }
                    let months = (days.unwrap() as f32 / 30.0).ceil() as u32;
                    actions.push(Action::Wait(months));
                }
            }
        }

        // Get facility-specific actions
        for (_, facility) in self.facilities() {
            actions.extend(facility.get_possible_actions(self, balance));
        }

        actions
    }
}

impl fmt::Display for Planet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let size = if self.has_colony() {
            self.size
        } else {
            0
        };
        write!(f, "{} (Hazard: {}%, Size: {})", 
            self.name, 
            self.hazard_rating as i32,
            size
        )
    }
}
