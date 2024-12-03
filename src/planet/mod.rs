mod facility;

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::hash::Hash;

use crate::constants::FacilityData;
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
            .count() <= (self.size.saturating_sub(2)) as usize
    }

    pub fn add_facility(&mut self, name: String) -> bool {
        // Create the new facility
        let new_facility = match Facility::new(name.clone()) {
            Some(f) => f,
            None => return false,
        };

        // Check if this is an upgrade by looking at requirements
        if let Some(data) = crate::constants::FACILITY_DATA.get(name.as_str()) {
            for req in &data.requirements {
                // If requirement matches an existing facility, this is an upgrade
                if let Some(old_facility) = self.facilities.remove(*req) {
                    // Transfer metadata from old facility to new one
                    let mut upgraded = new_facility;
                    if old_facility.has_improvements() { 
                        upgraded.add_improvements(); 
                    }
                    if old_facility.has_alpha_core() {
                        upgraded.add_alpha_core();
                    }
                    if let Some(item) = old_facility.get_colony_item() {
                        upgraded.add_colony_item_raw(item);
                    }
                    
                    // Add the upgraded facility
                    self.facilities.insert(name, upgraded);
                    return true;
                }
            }
        }

        // If not an upgrade, just add as a new facility
        self.facilities.insert(name, new_facility);
        true
    }

    pub fn remove_facility(&mut self, name: &str) -> bool {
        if name == "population" || name == "spaceport" {
            return false; // Can't remove core facilities
        }
        
        if let Some(facility_data) = crate::constants::FACILITY_DATA.get(name) {
            if let Some(downgrade) = facility_data.requirements.first() {
                if let Some(mut downgraded_facility) = Facility::new(downgrade.to_string()) {
                    if let Some(old_facility) = self.facilities.remove(name) {
                        // Transfer metadata from old facility to downgraded one
                        if old_facility.has_improvements() {
                            downgraded_facility.add_improvements();
                        }
                        if old_facility.has_alpha_core() {
                            downgraded_facility.add_alpha_core();
                        }
                        if let Some(item) = old_facility.get_colony_item() {
                            downgraded_facility.add_colony_item_raw(item);
                        }
                        
                        // Add the downgraded facility
                        self.facilities.insert(downgrade.to_string(), downgraded_facility);
                        return true;
                    }
                }
            }
        }
        
        self.facilities.remove(name).is_some()
    }

    pub fn hazard_rating(&self) -> f32 {
        self.hazard_rating
    }

    pub fn total_upkeep(&self) -> f32 {
        if !self.has_colony() {
            return 0.0;
        }

        let upkeep = self.facilities.values()
            .map(|facility| facility.calculate_upkeep(self.hazard_rating, self.size))
            .sum();

        if upkeep < 0.0 {
            panic!("Upkeep is negative for {}: {}", self.name, upkeep);
        } else {
            upkeep
        }
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

    pub fn has_hazard_pay(&self) -> bool {
        self.has_hazard_pay
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
        if has_colony {
            self.size = 3;
            self.growth_progress = 0.0;
        } else {
            self.size = 0;
            self.growth_progress = 0.0;
        }
    }

    pub fn has_colony(&self) -> bool {
        self.has_colony
    }

    pub fn get_num_facility_improvements(&self) -> u32 {
        self.facilities.iter().filter(|(_, f)| f.has_improvements()).count() as u32
    }

    pub fn update_growth(&mut self, days: i32, larger_friendly_colonies: Option<&[(String, u32)]>) {
        const MAX_SIZE: u32 = 6;
        const MIN_SIZE: u32 = 3;

        // TODO: look into how colony size changes growth rate
        
        // Don't grow past limits
        if (self.size >= MAX_SIZE && days > 0) || (self.size <= MIN_SIZE && days < 0 && self.growth_progress <= 0.0) {
            return;
        }

        let is_undoing = days < 0;
        let mut remaining_days = days.abs() as u32;

        // Update free port days if applicable
        if self.is_free_port {
            self.free_port_days = if is_undoing {
                self.free_port_days.saturating_sub(remaining_days)
            } else {
                self.free_port_days.saturating_add(remaining_days)
            };
        }

        // Process growth day by day
        let mut growth_points = self.calculate_growth_points(larger_friendly_colonies);
        while remaining_days > 0 {
            // println!("Growth points (% of size per month): {}", growth_points);
            if growth_points <= 0 {
                break;
            }

            // Calculate daily growth
            let growth_per_day = growth_points as f32 / 30.0 / 100.0;
            // println!("Growth per day (% of size per day): {}", growth_per_day);

            // Handle forward or reverse growth
            let growth_change = if is_undoing { -growth_per_day } else { growth_per_day };

            self.growth_progress += growth_change;

            // Update size if necessary
            if !is_undoing && self.growth_progress >= 1.0 && self.size < MAX_SIZE {
                self.size += 1;
                self.growth_progress -= 1.0;
                growth_points = self.calculate_growth_points(larger_friendly_colonies);
            } else if is_undoing && self.growth_progress < 0.0 && self.size > MIN_SIZE {
                self.size -= 1;
                self.growth_progress += 1.0;
                growth_points = self.calculate_growth_points(larger_friendly_colonies);
            }

            // Ensure growth_progress stays within [0, 1) range
            self.growth_progress = self.growth_progress.clamp(0.0, 0.99999);
            if self.growth_progress < 0.00001 {
                self.growth_progress = 0.0;
            }

            // println!(" Growth Progress: {}", self.growth_progress);

            remaining_days -= 1;
        }
    }

    pub fn days_till_next_size(&self, growth_points: Option<i32>, larger_friendly_colonies: Option<&[(String, u32)]>) -> Option<u32> {
        if !self.has_colony() || self.size >= 6 || self.size <= 2 {
            return None;
        }

        let growth_points = growth_points.unwrap_or_else(|| self.calculate_growth_points(larger_friendly_colonies));
        if growth_points <= 0 {
            return None;
        }

        let remaining_growth = 1.0 - self.growth_progress;
        // println!(" Growth Progress: {}", self.growth_progress);
        // println!(" Remaining growth: {}", remaining_growth);

        let growth_per_month = growth_points as f32 / 100.0;
        let days = (remaining_growth / growth_per_month * 30.0).ceil() as u32;

        Some(days)
    }

    pub fn calculate_growth_points(&self, larger_friendly_colonies: Option<&[(String, u32)]>) -> i32 {
        if !self.has_colony() {
            return 0;
        }

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
        if !self.has_colony() {
            return 0.0;
        }

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
        if !self.has_colony() {
            return HashMap::new();
        }

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

    /// Per month
    pub fn get_gross_income(&self) -> f32 {
        if !self.has_colony() {
            return 0.0;
        }

        let mut gross_income: f32 = 0.0;
        let mut highest_income_mult: f32 = 1.0;
        for facility in self.facilities.values() {
            let facility_income = facility.calculate_gross_income(self.size, self);
            gross_income += facility_income;
            let income_mult = facility.income_multiplier();
            highest_income_mult = highest_income_mult.max(income_mult);
        }

        gross_income * highest_income_mult
    }

    /// Per month
    pub fn get_net_income(&self) -> f32 {
        if !self.has_colony() {
            return 0.0;
        }

        let mut net_income = self.get_gross_income();
        let total_upkeep = self.total_upkeep();
        net_income -= total_upkeep;
        net_income
    }

    /// Will progress buildings and growth, and return incomes
    pub fn wait(&mut self, months: u32) -> (f32, f32) {
        if !self.has_colony() {
            return (0.0, 0.0);
        }

        let mut gross_income = 0.0;
        let mut net_income = 0.0;

        // Iterate through each month
        for _ in 0..months {
            // Update planet growth
            self.update_growth(30, None);

            // Progress build days for all facilities
            for facility in self.facilities.values_mut() {
                facility.progress_build_days(30);
            }

            // Calculate monthly income
            let monthly_gross = self.get_gross_income();
            let monthly_net = monthly_gross - self.total_upkeep();
            // println!("WAIT - Gross: {}, Net: {}, Growth: {}, Size: {}", monthly_gross, monthly_net, self.growth_progress, self.size);
            
            // Accumulate income
            gross_income += monthly_gross;
            net_income += monthly_net;
        }

        // Return total income
        (gross_income, net_income)
    }

    /// Will undo the effects of wait and return negative incomes
    pub fn undo_wait(&mut self, months: u32) -> (f32, f32) {
        if !self.has_colony() {
            return (0.0, 0.0);
        }

        let mut gross_income = 0.0;
        let mut net_income = 0.0;
        
        // For each month, we need to:
        // 1. Calculate income for that month (before undoing changes)
        // 2. Undo growth progress
        // 3. Undo facility build progress
        for _ in 0..months {
            // First calculate income for this month before undoing changes
            let monthly_gross = self.get_gross_income();
            let monthly_net = monthly_gross - self.total_upkeep();
            // println!("UNDO WAIT - Gross: {}, Net: {}, Growth: {}, Size: {}", monthly_gross, monthly_net, self.growth_progress, self.size);

            gross_income += monthly_gross;
            net_income += monthly_net;

            // Undo growth progress - now uses negative days
            self.update_growth(-30, None);

            // Undo facility build progress
            for facility in self.facilities.values_mut() {
                facility.progress_build_days(-30);
            }
        }

        (gross_income, net_income)
    }

    fn meets_facility_requirements(&self, requirements: &[&str]) -> bool {
        for req in requirements {
            // First check if it's a property requirement
            if let Some(value) = self.properties.get(*req) {
                if *value <= 0.0 {
                    return false;
                }
                continue;
            }

            // If not a property, check if it's a required facility
            if !self.facilities.contains_key(*req) {
                return false;
            }
        }
        true
    }

    pub fn get_possible_actions(&self, balance: &Balance) -> Vec<Action> {
        let mut actions = Vec::new();
        let planet_name = self.name.clone();
        
        // If this planet is not colonized, return empty list since colonization is handled by System
        if !self.has_colony() {
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
                // Check if we meet the requirements for this facility
                if !self.meets_facility_requirements(&facility.requirements) {
                    continue;
                }

                // Check if this is an upgrade (any requirement matches an existing facility)
                let upgrade_from = facility.requirements.iter()
                    .find(|req| self.facilities().contains_key(**req))
                    .and_then(|name| self.facilities().get(*name));
                
                // Check if we already have an upgrade of this facility
                let has_upgrade = self.facilities().values().any(|f| 
                    if let Some(data) = f.get_data() {
                        data.requirements.contains(name)
                    } else {
                        false
                    }
                );

                if has_upgrade {
                    continue;
                }

                // Determine if we can add this facility
                let can_add = match upgrade_from {
                    // If it's an upgrade, check if we need a new industry slot
                    Some(old_facility) => {
                        // Need industry slot if upgrading from structure to industry
                        !old_facility.is_structure() || facility.is_structure || self.can_add_industry()
                    },
                    // Not an upgrade - normal structure/industry check
                    None => facility.is_structure || self.can_add_industry()
                };

                if can_add && balance.credits() >= facility.build_cost as f32 {
                    actions.push(Action::AddFacility(planet_name.clone(), name.to_string()));
                }
                else if !facility.is_structure && !self.can_add_industry() {
                    // Add a Wait action; need to calculate how long we need to grow
                    // to next colony size
                    let days = self.days_till_next_size(None, None);
                    if let Some(days) = days {
                        let months = (days as f32 / 30.0).ceil() as u32;
                        if months > 0 {
                            let action = Action::Wait(months);
                            if !actions.contains(&action) {
                                actions.push(action);
                            }
                        }
                    }
                }
            }
        }

        // Get facility-specific actions
        for (_, facility) in self.facilities() {
            actions.extend(facility.get_possible_actions(self, balance));
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
