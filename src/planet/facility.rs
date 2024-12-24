use crate::constants::{
    ColonyItem, FacilityData, Resource, ResourceAmount, ResourceGetter, FacilityType, COLONY_ITEM_DATA,
    FACILITY_ALPHA_CORES, FACILITY_DATA, FACILITY_IMPROVEMENTS, POSSIBLE_COLONY_ITEMS, MAX_PRODUCTION, MAX_DEMANDS
};
use crate::solver::{Action, Balance};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, PartialEq)]
pub struct Facility {
    facility_type: FacilityType,
    improvements: bool,
    alpha_core: bool,
    colony_item: Option<ColonyItem>,
    upkeep_formula: fn(u32) -> f64,
    base_accessibility_bonus: f64,
    base_stability_bonus: i32,
    base_defense_multiplier: f64,
    base_income_multiplier: f64,
    // // Current cached values
    // current_accessibility_bonus: f64,
    // current_stability_bonus: i32,
    // current_defense_multiplier: f64,
    // current_income_multiplier: f64,
    
    base_production: Vec<ResourceAmount>,
    base_demands: Vec<ResourceAmount>,
    is_structure: bool,
    current_build_days: i32,
    total_build_days: i32,
}

impl Hash for Facility {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.facility_type.hash(state);
        self.improvements.hash(state);
        self.alpha_core.hash(state);
        self.colony_item.hash(state);
        self.base_stability_bonus.hash(state);
        self.is_structure.hash(state);
        self.current_build_days.hash(state);
        self.total_build_days.hash(state);
        self.upkeep_formula.hash(state);

        // Hash f64 values by converting them to bits
        self.base_accessibility_bonus.to_bits().hash(state);
        self.base_defense_multiplier.to_bits().hash(state);
        self.base_income_multiplier.to_bits().hash(state);

        // Hash vectors by sorting their entries
        let mut prod_entries: Vec<_> = self.base_production.iter().collect();
        prod_entries.sort_by(|a, b| a.resource.cmp(&b.resource));
        for resource_amount in prod_entries {
            resource_amount.resource.hash(state);
            resource_amount.amount_formula.hash(state);
        }

        let mut demand_entries: Vec<_> = self.base_demands.iter().collect();
        demand_entries.sort_by(|a, b| a.resource.cmp(&b.resource));
        for resource_amount in demand_entries {
            resource_amount.resource.hash(state);
            resource_amount.amount_formula.hash(state);
        }
    }
}

impl Facility {
    pub fn new(fac_type: FacilityType) -> Option<Self> {
        let data = FACILITY_DATA.get(&fac_type)?;

        let mut production = Vec::with_capacity(MAX_PRODUCTION);
        for res in &data.production {
            production.push(ResourceAmount {
                resource: res.resource,
                amount_formula: res.amount_formula,
            });
        }

        let mut demands = Vec::with_capacity(MAX_DEMANDS);
        for res in &data.demands {
            demands.push(ResourceAmount {
                resource: res.resource,
                amount_formula: res.amount_formula,
            });
        }

        let facility = Self {
            facility_type: fac_type,
            improvements: false,
            alpha_core: false,
            colony_item: None,
            upkeep_formula: data.base_upkeep_formula,
            base_accessibility_bonus: data.accessibility_bonus,
            base_stability_bonus: data.stability_bonus,
            base_defense_multiplier: data.defense_multiplier,
            base_income_multiplier: data.income_multiplier,
            // current_accessibility_bonus: data.accessibility_bonus,
            // current_stability_bonus: data.stability_bonus,
            // current_defense_multiplier: data.defense_multiplier,
            // current_income_multiplier: data.income_multiplier,
            base_production: production,
            base_demands: demands,
            is_structure: data.is_structure,
            current_build_days: data.build_time as i32,
            total_build_days: data.build_time as i32,
        };

        Some(facility)
    }

    pub fn facility_type(&self) -> &FacilityType {
        &self.facility_type
    }

    pub fn name(&self) -> &'static str {
        self.facility_type.as_str()
    }

    pub fn is_structure(&self) -> bool {
        self.is_structure
    }

    pub fn has_improvements(&self) -> bool {
        self.improvements
    }

    pub fn has_alpha_core(&self) -> bool {
        self.alpha_core
    }

    pub fn get_data(&self) -> Option<FacilityData> {
        crate::constants::FACILITY_DATA
            .get(&self.facility_type)
            .cloned()
    }

    pub fn add_improvements(&mut self) -> bool {
        if !self.improvements {
            self.improvements = true;
            // self.update_all();
            true
        } else {
            false
        }
    }

    pub fn add_alpha_core(&mut self) -> bool {
        if !self.alpha_core {
            self.alpha_core = true;
            // self.update_all();
            true
        } else {
            false
        }
    }

    pub fn remove_improvements(&mut self) -> bool {
        if self.improvements {
            self.improvements = false;
            // self.update_all();
            true
        } else {
            false
        }
    }

    pub fn remove_alpha_core(&mut self) -> bool {
        if self.alpha_core {
            self.alpha_core = false;
            // self.update_all();
            true
        } else {
            false
        }
    }

    pub fn remaining_build_days(&self) -> i32 {
        self.current_build_days
    }

    /// Won't handle income/expenses over wait time
    pub fn progress_build_days(&mut self, days: i32) {
        // Correct behavior; required to properly undo progress past completion
        self.current_build_days = self.current_build_days.saturating_sub(days);
        self.total_build_days = self.total_build_days.saturating_sub(days);
        // Incorrect behavior; will mess up everything and cause comp time to explode
        // self.remaining_build_days = self.remaining_build_days.max(0);
    }

    /// Upgrades/downgrades a facility in-place, doesn't check if it's possible
    pub fn swap_raw_w_data(&mut self, new_type: FacilityType, data: &FacilityData, downgrade: bool) -> Option<&Self> {
        self.facility_type = new_type;
        self.current_build_days = if downgrade { self.total_build_days } else { data.build_time as i32 };

        self.base_production.clear();
        self.base_production.extend(data.production.iter().cloned());
        self.base_demands.clear();
        self.base_demands.extend(data.demands.iter().cloned());
        self.upkeep_formula = data.base_upkeep_formula;
        self.base_accessibility_bonus = data.accessibility_bonus;
        self.base_stability_bonus = data.stability_bonus;
        self.base_defense_multiplier = data.defense_multiplier;
        self.base_income_multiplier = data.income_multiplier;
        self.is_structure = data.is_structure;

        Some(self)
    }

    /// Upgrades/downgrades a facility in-place, doesn't check if it's possible
    pub fn swap_raw(&mut self, new_type: FacilityType, downgrade: bool) -> Option<&Self> {
        let data = FACILITY_DATA.get(&new_type)?;

        self.swap_raw_w_data(new_type, data, downgrade)
    }

    pub fn can_install_colony_item(
        &self,
        item: &ColonyItem,
        planet: &dyn PlanetConditionChecker,
    ) -> bool {
        match item {
            ColonyItem::SoilNanites => {
                !planet.has_property("volatiles")
                    && !planet.has_property("rare ores")
                    && planet.has_property("farmland")
                    && planet.get_property("water world") <= 0.0
            }
            ColonyItem::BiofactoryEmbryo => planet.get_property("habitable") > 0.0,
            ColonyItem::PristineNanoforge | ColonyItem::CorruptedNanoforge => {
                planet.get_property("habitable") <= 0.0
            }
            ColonyItem::MantleBore => {
                planet.get_property("gas giant") <= 0.0 && planet.get_property("habitable") <= 0.0
            }
            ColonyItem::CatalyticCore | ColonyItem::SynchrotronCore => {
                planet.get_property("no atmosphere") > 0.0
            }
            ColonyItem::PlasmaDynamo => planet.get_property("gas giant") > 0.0,
            ColonyItem::CryoarithmeticEngine => planet.get_property("heat") > 0.0,
            ColonyItem::FullereneSpool => {
                planet.get_property("gas giant") <= 0.0
                    && planet.get_property("extreme activity") <= 0.0
            }
            _ => true, // Other items have no planetary requirements
        }
    }

    pub fn get_possible_colony_items(
        &self,
        planet: &dyn PlanetConditionChecker,
    ) -> Vec<ColonyItem> {
        POSSIBLE_COLONY_ITEMS
            .iter()
            .filter_map(|item_str| {
                if let Some(item) = ColonyItem::from_str(item_str) {
                    if let Some(data) = COLONY_ITEM_DATA.get(&item) {
                        if !data.compatible_facilities.contains(&self.facility_type) {
                            return None;
                        }
                        if self.can_install_colony_item(&item, planet) {
                            Some(item)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn add_colony_item(
        &mut self,
        item: ColonyItem,
        planet: &dyn PlanetConditionChecker,
    ) -> bool {
        if self.can_install_colony_item(&item, planet) {
            self.colony_item = Some(item);
            // self.update_all();
            true
        } else {
            false
        }
    }

    pub fn add_colony_item_raw(&mut self, item: ColonyItem) {
        self.colony_item = Some(item);
        // self.update_all();
    }

    pub fn remove_colony_item(&mut self) -> Option<ColonyItem> {
        let item = self.colony_item.take();
        // self.update_all();
        item
    }

    pub fn has_colony_item(&self) -> bool {
        self.colony_item.is_some()
    }

    pub fn get_colony_item(&self) -> Option<ColonyItem> {
        self.colony_item
    }

    pub fn calculate_upkeep(&self, hazard_rating: f64, size: u32) -> f64 {
        // Upkeep costs are active while building
        let mut upkeep = (self.upkeep_formula)(size);
        if self.alpha_core {
            upkeep *= 0.75; // 25% reduction for alpha core
        }
        upkeep *= hazard_rating / 100.0;
        if upkeep < 0.0 {
            panic!("Upkeep is negative for {:?}: {}", self.facility_type, upkeep);
        }
        upkeep
    }

    // #[inline]
    // pub fn accessibility_bonus(&self) -> f64 {
    //     self.current_accessibility_bonus
    // }

    // #[inline]
    // pub fn stability_bonus(&self) -> i32 {
    //     self.current_stability_bonus
    // }

    // #[inline]
    // pub fn defense_multiplier(&self) -> f64 {
    //     self.current_defense_multiplier
    // }

    // #[inline]
    // pub fn income_multiplier(&self) -> f64 {
    //     self.current_income_multiplier
    // }

    pub fn calculate_accessibility_bonus(&self) -> f64 {
        if self.current_build_days > 0 {  
            return 0.0;
        }

        let mut bonus = self.base_accessibility_bonus;

        // Add improvement bonus
        if self.improvements {
            if let Some(effects) = FACILITY_IMPROVEMENTS.get(&self.facility_type) {
                bonus += effects.accessibility_bonus;
            }
        }

        // Add alpha core bonus
        if self.alpha_core {
            if let Some(effects) = FACILITY_ALPHA_CORES.get(&self.facility_type) {
                bonus += effects.accessibility_bonus;
            }
        }

        // Add colony item bonus
        if let Some(item) = self.colony_item {
            if let Some(item_data) = COLONY_ITEM_DATA.get(&item) {
                bonus += item_data.accessibility_bonus;
            }
        }

        bonus
    }

    pub fn calculate_stability_bonus(&self) -> i32 {
        if self.current_build_days > 0 {
            return 0;
        }

        let mut bonus = self.base_stability_bonus;

        // Add improvement bonus
        if self.improvements {
            if let Some(effects) = FACILITY_IMPROVEMENTS.get(&self.facility_type) {
                bonus += effects.stability_bonus;
            }
        }

        bonus
    }

    pub fn calculate_defense_multiplier(&self) -> f64 {
        if self.current_build_days > 0 {
            return 1.0;
        }

        let mut multiplier = self.base_defense_multiplier;

        // Add improvement bonus
        if self.improvements {
            if let Some(effects) = FACILITY_IMPROVEMENTS.get(&self.facility_type) {
                multiplier += effects.defense_multiplier;
            }
        }

        // Add alpha core bonus
        if self.alpha_core {
            if let Some(effects) = FACILITY_ALPHA_CORES.get(&self.facility_type) {
                multiplier += effects.defense_multiplier;
            }
        }

        // Add colony item bonus
        if let Some(item) = self.colony_item {
            if let Some(item_data) = COLONY_ITEM_DATA.get(&item) {
                multiplier += item_data.defense_multiplier;
            }
        }

        multiplier
    }

    pub fn calculate_income_multiplier(&self) -> f64 {
        if self.current_build_days > 0 {
            return 1.0;
        }

        let mut multiplier = self.base_income_multiplier;

        // Add improvement bonus
        if self.improvements {
            if let Some(effects) = FACILITY_IMPROVEMENTS.get(&self.facility_type) {
                multiplier += effects.income_bonus;
            }
        }

        // Add alpha core bonus
        if self.alpha_core {
            if let Some(effects) = FACILITY_ALPHA_CORES.get(&self.facility_type) {
                multiplier += effects.income_bonus;
            }
        }

        // Apply colony item income multiplier if applicable
        if let Some(item) = self.colony_item {
            if let Some(item_data) = COLONY_ITEM_DATA.get(&item) {
                multiplier += item_data.income_multiplier;
            }
        }

        multiplier
    }

    pub fn production(&self) -> &Vec<ResourceAmount> {
        &self.base_production
    }

    pub fn demands(&self) -> &Vec<ResourceAmount> {
        &self.base_demands
    }

    pub fn calculate_resource_production(
        &self,
        resource: Resource,
        size: u32,
        bonus: f64,
        is_free_port: bool,
    ) -> f64 {
        if self.current_build_days > 0 || 
           (!is_free_port && (resource == Resource::Drugs || resource == Resource::HarvestedOrgans)) {
            return 0.0;
        }

        self.base_production.get(resource)
            .map(|resource_amount| {
                let amount = (resource_amount.amount_formula)(size);
                if amount <= 0.0 { return 0.0; }

                let mut total_bonus = bonus;

                if self.improvements {
                    total_bonus += FACILITY_IMPROVEMENTS.get(&self.facility_type)
                        .map_or(0.0, |effects| effects.production_bonus);
                }

                if self.alpha_core {
                    total_bonus += FACILITY_ALPHA_CORES.get(&self.facility_type)
                        .map_or(0.0, |effects| effects.production_bonus);
                }

                if let Some(item) = self.colony_item {
                    total_bonus += COLONY_ITEM_DATA.get(&item)
                        .and_then(|item_data| item_data.production_bonuses.get(resource))
                        .map_or(0.0, |resource_amount| (resource_amount.amount_formula)(size));
                }

                amount + total_bonus
            })
            .unwrap_or(0.0)
    }

    pub fn get_resource_production(
        &self,
        size: u32,
        bonus: f64,
        is_free_port: bool,
    ) -> HashMap<Resource, f64> {
        let mut result = HashMap::new();
        for resource_amount in &self.base_production {
            let amount = self.calculate_resource_production(
                resource_amount.resource,
                size,
                bonus,
                is_free_port,
            );
            if amount > 0.0 {
                result.insert(resource_amount.resource, amount);
            }
        }
        result
    }

    pub fn calculate_resource_demand(&self, resource: Resource, size: u32) -> f64 {
        if self.current_build_days > 0 {
            return 0.0;
        }

        if let Some(resource_amount) = self.base_demands.iter().find(|ra| ra.resource == resource) {
            let mut amount = (resource_amount.amount_formula)(size);

            // Apply reduction from alpha core
            if self.alpha_core {
                amount -= 1.0;
            }

            amount
        } else {
            0.0
        }
    }

    /// Per month
    pub fn calculate_gross_income(&self, size: u32, planet: &dyn PlanetConditionChecker, accessibility: f64) -> f64 {
        if self.current_build_days > 0 {
            return 0.0;
        }

        let mut gross_income = 0.0;

        if self.facility_type == FacilityType::Population {
            gross_income += 10000.0 * (size as f64 - 2.0);
        }

        for resource_amount in &self.base_production {
            let production = self.calculate_resource_production(
                resource_amount.resource,
                size,
                0.0,
                planet.is_free_port(),
            );
            let market_value = resource_amount.resource.market_value() as f64;
            let sector_supply = resource_amount.resource.sector_supply() as f64;
            // Skip resources with no market value (Crew and Marines)
            if market_value == 0.0 {
                continue;
            }
            let market_share = (production * accessibility) / sector_supply;
            gross_income += market_share / market_value;
        }

        gross_income
    }

    //TODO: upkeep needs to take into account same-faction supply of demanded resources
    /// Per month
    pub fn calculate_net_income(&self, size: u32, planet: &dyn PlanetConditionChecker, accessibility: f64) -> f64 {
        let gross = self.calculate_gross_income(size, planet, accessibility);
        let upkeep = self.calculate_upkeep(planet.get_property("hazard percent"), planet.size());
        gross - upkeep
    }

    pub fn get_possible_actions(
        &self,
        planet: &dyn PlanetConditionChecker,
        balance: &Balance,
        slim: bool,
    ) -> Vec<Action> {
        if self.current_build_days > 0 {
            let wait = Action::Wait((self.current_build_days as f64 / 30.0).ceil() as u32);
            return vec![wait];
        }

        let mut actions = Vec::with_capacity(3);
        let planet_name = planet.name().to_string();

        if !slim {
            // Add improvements if not present
            if !self.has_improvements() {
                let improvement_cost = 2_u32.pow(planet.improvements());
                if balance.story_points() >= improvement_cost {
                    actions.push(Action::AddImprovement(
                        planet_name.clone(),
                        self.facility_type,
                    ));
                }
            }

            // Add alpha core if not present
            if !self.has_alpha_core() {
                if balance.alpha_cores() >= 1 {
                    actions.push(Action::AddAlphaCore(
                        planet_name.clone(),
                        self.facility_type,
                    ));
                }
            }
        }

        // Add possible colony items if none present
        if !self.has_colony_item() {
            // Now we can use the planet reference to get possible colony items
            for item in self.get_possible_colony_items(planet) {
                if balance.colony_items().contains_key(&item) {
                    actions.push(Action::InstallItem(
                        planet_name.clone(),
                        self.facility_type,
                        item,
                    ));
                }
            }
        }

        actions
    }

    pub fn _get_differences(&self, other: &Facility) -> Vec<String> {
        let mut differences = Vec::new();

        if self.facility_type != other.facility_type {
            differences.push(format!("Facility type changed from {:?} to {:?}", self.facility_type, other.facility_type));
        }
        if self.current_build_days != other.current_build_days {
            differences.push(format!("Remaining build days changed from {} to {}", self.current_build_days, other.current_build_days));
        }
        if self.improvements != other.improvements {
            differences.push(format!("Improvements changed from {} to {}", self.improvements, other.improvements));
        }
        if self.alpha_core != other.alpha_core {
            differences.push(format!("Alpha core changed from {} to {}", self.alpha_core, other.alpha_core));
        }
        if self.colony_item != other.colony_item {
            differences.push(format!("Colony item changed from {:?} to {:?}", self.colony_item, other.colony_item));
        }
        if self.base_production != other.base_production {
            differences.push(format!("Base production changed from {:?} to {:?}", self.base_production, other.base_production));
        }
        if self.base_demands != other.base_demands {
            differences.push(format!("Base demands changed from {:?} to {:?}", self.base_demands, other.base_demands));
        }
        if self.upkeep_formula != other.upkeep_formula {
            differences.push("Upkeep formula changed".to_string());
        }
        if self.base_accessibility_bonus != other.base_accessibility_bonus {
            differences.push(format!("Base accessibility bonus changed from {} to {}", self.base_accessibility_bonus, other.base_accessibility_bonus));
        }
        if self.base_stability_bonus != other.base_stability_bonus {
            differences.push(format!("Base stability bonus changed from {} to {}", self.base_stability_bonus, other.base_stability_bonus));
        }
        if self.base_defense_multiplier != other.base_defense_multiplier {
            differences.push(format!("Base defense multiplier changed from {} to {}", self.base_defense_multiplier, other.base_defense_multiplier));
        }
        if self.base_income_multiplier != other.base_income_multiplier {
            differences.push(format!("Base income multiplier changed from {} to {}", self.base_income_multiplier, other.base_income_multiplier));
        }

        differences
    }

    // pub fn update_accessibility_bonus(&mut self) {
    //     self.current_accessibility_bonus = self.calculate_accessibility_bonus();
    // }

    // pub fn update_stability_bonus(&mut self) {
    //     self.current_stability_bonus = self.calculate_stability_bonus();
    // }

    // pub fn update_defense_multiplier(&mut self) {
    //     self.current_defense_multiplier = self.calculate_defense_multiplier();
    // }

    // pub fn update_income_multiplier(&mut self) {
    //     self.current_income_multiplier = self.calculate_income_multiplier();
    // }

    // pub fn update_all(&mut self) {
    //     self.update_accessibility_bonus();
    //     self.update_stability_bonus();
    //     self.update_defense_multiplier();
    //     self.update_income_multiplier();
    // }
}

pub trait PlanetConditionChecker {
    fn name(&self) -> &str;
    fn size(&self) -> u32;
    fn has_property(&self, property: &str) -> bool;
    fn get_property(&self, property: &str) -> f64;
    fn accessability(&self) -> f64;
    fn improvements(&self) -> u32;
    fn is_free_port(&self) -> bool;
}

impl PlanetConditionChecker for super::Planet {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u32 {
        self.size
    }

    fn has_property(&self, property: &str) -> bool {
        self.properties.get(property).is_some()
    }

    fn get_property(&self, property: &str) -> f64 {
        *self.properties.get(property).unwrap_or(&0.0)
    }

    fn accessability(&self) -> f64 {
        self.calculate_accessibility()
    }

    fn improvements(&self) -> u32 {
        self.get_num_facility_improvements()
    }

    fn is_free_port(&self) -> bool {
        self.is_free_port()
    }
}
