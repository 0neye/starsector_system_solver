use nohash_hasher::BuildNoHashHasher;
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::constants::{
    ColonyItem, FacilityData, FacilityType, Resource, ResourceAmount, ResourceGetter,
    COLONY_ITEM_DATA, FACILITY_ALPHA_CORES, FACILITY_DATA, FACILITY_IMPROVEMENTS, MAX_DEMANDS,
    MAX_PRODUCTION, POSSIBLE_COLONY_ITEMS,
};
use crate::solver::{improvement_story_point_cost, Action, Balance};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone)]
pub struct Facility {
    facility_type: FacilityType,
    current_build_days: i32,
    total_build_days: i32,
    improvements: bool,
    alpha_core: bool,
    colony_item: Option<ColonyItem>,
    base_production: HashMap<Resource, ResourceAmount, BuildNoHashHasher<u8>>,
    base_demands: HashMap<Resource, ResourceAmount, BuildNoHashHasher<u8>>,
    production_cache: RefCell<ProductionCache>,
    upkeep_formula: fn(u32) -> f64,
    base_accessibility_bonus: f64,
    base_stability_bonus: i32,
    base_defense_multiplier: f64,
    base_income_multiplier: f64,
    is_structure: bool,
}

impl PartialEq for Facility {
    fn eq(&self, other: &Self) -> bool {
        self.facility_type == other.facility_type
            && self.current_build_days == other.current_build_days
            && self.total_build_days == other.total_build_days
            && self.improvements == other.improvements
            && self.alpha_core == other.alpha_core
            && self.colony_item == other.colony_item
            && self.is_structure == other.is_structure
            && self.base_accessibility_bonus == other.base_accessibility_bonus
            && self.base_stability_bonus == other.base_stability_bonus
            && self.base_defense_multiplier == other.base_defense_multiplier
            && self.base_income_multiplier == other.base_income_multiplier
    }
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

        // Hash f64 values by converting them to bits
        self.base_accessibility_bonus.to_bits().hash(state);
        self.base_defense_multiplier.to_bits().hash(state);
        self.base_income_multiplier.to_bits().hash(state);

        // NOTE: base_production / base_demands / upkeep_formula are uniquely
        // determined by facility_type, so they are intentionally not hashed
        // (hashing the fn pointers inside them is unreliable anyway).
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct ProductionCache {
    size: u32,
    is_free_port: bool,
    improvements: bool,
    alpha_core: bool,
    colony_item: Option<ColonyItem>,
    is_built: bool, // Track if facility is complete (build_days <= 0)
    cached_production: HashMap<Resource, f64, BuildNoHashHasher<u8>>,
}

impl Facility {
    pub fn new(facility_type: FacilityType) -> Option<Self> {
        let mut base_production = HashMap::with_hasher(BuildNoHashHasher::default());
        let mut base_demands = HashMap::with_hasher(BuildNoHashHasher::default());

        if let Some(facility_data) = FACILITY_DATA.get(&facility_type) {
            for production in &facility_data.production {
                base_production.insert(production.resource, production.clone());
            }
            for demand in &facility_data.demands {
                base_demands.insert(demand.resource, demand.clone());
            }

            let facility = Self {
                facility_type,
                current_build_days: facility_data.build_time as i32,
                improvements: false,
                alpha_core: false,
                colony_item: None,
                base_production,
                base_demands,
                production_cache: RefCell::new(ProductionCache::default()),
                upkeep_formula: facility_data.base_upkeep_formula,
                base_accessibility_bonus: facility_data.accessibility_bonus,
                base_stability_bonus: facility_data.stability_bonus,
                base_defense_multiplier: facility_data.defense_multiplier,
                base_income_multiplier: facility_data.income_multiplier,
                is_structure: facility_data.is_structure,
                total_build_days: facility_data.build_time as i32,
            };

            Some(facility)
        } else {
            None
        }
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

    pub fn get_data(&self) -> Option<&FacilityData> {
        crate::constants::FACILITY_DATA.get(&self.facility_type)
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

    pub(crate) fn build_days_state(&self) -> (i32, i32) {
        (self.current_build_days, self.total_build_days)
    }

    pub(crate) fn set_build_days_state(&mut self, current_build_days: i32, total_build_days: i32) {
        self.current_build_days = current_build_days;
        self.total_build_days = total_build_days;
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
    pub fn swap_raw_w_data(
        &mut self,
        new_type: FacilityType,
        data: &FacilityData,
        downgrade: bool,
    ) -> Option<&Self> {
        self.facility_type = new_type;
        if downgrade {
            self.current_build_days = 0;
            self.total_build_days = data.build_time as i32;
        } else {
            self.current_build_days = data.build_time as i32;
            self.total_build_days = data.build_time as i32;
        }

        self.base_production = data
            .production
            .iter()
            .map(|ra| (ra.resource, ra.clone()))
            .collect();
        self.base_demands = data
            .demands
            .iter()
            .map(|ra| (ra.resource, ra.clone()))
            .collect();
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
        ColonyItem::all()
            .into_iter()
            .filter(|&item| {
                COLONY_ITEM_DATA.get(&item).map_or(false, |data| {
                    data.compatible_facilities.contains(&self.facility_type)
                        && self.can_install_colony_item(&item, planet)
                })
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
        // if upkeep < 0.0 {
        //     panic!("Upkeep is negative for {:?}: {}", self.facility_type, upkeep);
        // }
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

    pub fn production(&self) -> &HashMap<Resource, ResourceAmount, BuildNoHashHasher<u8>> {
        &self.base_production
    }

    pub fn demands(&self) -> &HashMap<Resource, ResourceAmount, BuildNoHashHasher<u8>> {
        &self.base_demands
    }

    fn get_or_calculate_production(
        &self,
        resource: Resource,
        size: u32,
        bonus: f64,
        is_free_port: bool,
    ) -> f64 {
        let mut production_cache = self.production_cache.borrow_mut();
        let is_built = self.current_build_days <= 0;

        // Check if cache is valid (all relevant state matches)
        if production_cache.size == size
            && production_cache.is_free_port == is_free_port
            && production_cache.improvements == self.improvements
            && production_cache.alpha_core == self.alpha_core
            && production_cache.colony_item == self.colony_item
            && production_cache.is_built == is_built
        {
            if let Some(&cached) = production_cache.cached_production.get(&resource) {
                return cached;
            }
        } else {
            // Cache is invalid, clear it
            production_cache.cached_production.clear();
        }

        // Update cache state
        production_cache.size = size;
        production_cache.is_free_port = is_free_port;
        production_cache.improvements = self.improvements;
        production_cache.alpha_core = self.alpha_core;
        production_cache.colony_item = self.colony_item;
        production_cache.is_built = is_built;

        let production = self.calculate_resource_production(resource, size, bonus, is_free_port);
        production_cache
            .cached_production
            .insert(resource, production);
        production
    }

    pub fn calculate_resource_production(
        &self,
        resource: Resource,
        size: u32,
        bonus: f64,
        is_free_port: bool,
    ) -> f64 {
        if self.current_build_days > 0
            || (!is_free_port
                && (resource == Resource::Drugs || resource == Resource::HarvestedOrgans))
        {
            return 0.0;
        }

        self.base_production
            .get(&resource)
            .map(|resource_amount| {
                let amount = (resource_amount.amount_formula)(size);
                if amount <= 0.0 {
                    return 0.0;
                }

                let mut total_bonus = bonus;

                if self.improvements {
                    total_bonus += FACILITY_IMPROVEMENTS
                        .get(&self.facility_type)
                        .map_or(0.0, |effects| effects.production_bonus);
                }

                if self.alpha_core {
                    total_bonus += FACILITY_ALPHA_CORES
                        .get(&self.facility_type)
                        .map_or(0.0, |effects| effects.production_bonus);
                }

                if let Some(item) = self.colony_item {
                    total_bonus += COLONY_ITEM_DATA
                        .get(&item)
                        .and_then(|item_data| item_data.production_bonuses.get(resource))
                        .map_or(0.0, |resource_amount| {
                            (resource_amount.amount_formula)(size)
                        });
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
    ) -> FxHashMap<Resource, f64> {
        let mut result = FxHashMap::default();
        for (resource, resource_amount) in &self.base_production {
            let amount = self.calculate_resource_production(*resource, size, bonus, is_free_port);
            if amount > 0.0 {
                result.insert(*resource, amount);
            }
        }
        result
    }

    pub fn calculate_resource_demand(&self, resource: Resource, size: u32) -> f64 {
        if self.current_build_days > 0 {
            return 0.0;
        }

        if let Some(resource_amount) = self.base_demands.get(&resource) {
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

    /// Market-share economy inputs, per month: adds this facility's
    /// marketable production (units) per commodity into `production_units`,
    /// and returns its direct (non-export) income. The planet caps the
    /// per-resource totals by accessibility-derived export capacity and
    /// weights them by accessibility; the market-share division against
    /// sector supply happens at system scope, where all player producers of a
    /// commodity share one denominator.
    pub fn collect_export_production(
        &self,
        size: u32,
        planet: &dyn PlanetConditionChecker,
        accessibility: f64,
        production_units: &mut [f64; Resource::COUNT],
    ) -> f64 {
        if self.current_build_days > 0 || accessibility == 0.0 {
            return 0.0;
        }

        let mut direct_income = 0.0;
        if self.facility_type == FacilityType::Population {
            direct_income += 10000.0 * (size as f64 - 2.0);
        }

        let freeport = planet.is_free_port();
        for resource in self.base_production.keys() {
            // Skip resources with no market value (Crew and Marines)
            if resource.market_value() == 0 {
                continue;
            }

            // Extraction resources are gated on deposit presence and get the deposit's
            // abundance modifier added as a bonus; non-deposit resources are unaffected.
            let deposit_bonus = match deposit_status(*resource, planet) {
                DepositStatus::Absent => continue, // no deposit -> no production
                DepositStatus::Present(modifier) => modifier,
                DepositStatus::NotDeposit => 0.0,
            };

            let production =
                self.get_or_calculate_production(*resource, size, deposit_bonus, freeport);
            if production > 0.0 {
                production_units[*resource as usize] += production;
            }
        }

        direct_income
    }

    /// Per month. Independent-denominator income (player supply excluded from
    /// the market-share denominator); the standalone single-colony view,
    /// superseded at system scope by `collect_export_supply` + market share.
    pub fn calculate_gross_income(
        &self,
        size: u32,
        planet: &dyn PlanetConditionChecker,
        accessibility: f64,
    ) -> f64 {
        if self.current_build_days > 0 || accessibility == 0.0 {
            return 0.0;
        }

        let mut gross_income = 0.0;

        if self.facility_type == FacilityType::Population {
            gross_income += 10000.0 * (size as f64 - 2.0);
        }

        let freeport = planet.is_free_port();

        // Calculate market share per resource
        // let resources = self.base_production.keys().map(|r| *r).collect::<Vec<Resource>>();
        for resource in self.base_production.keys() {
            let market_value = resource.market_value() as f64;
            // Skip resources with no market value (Crew and Marines)
            if market_value == 0.0 {
                continue;
            }

            // Extraction resources are gated on deposit presence and get the deposit's
            // abundance modifier added as a bonus; non-deposit resources are unaffected.
            let deposit_bonus = match deposit_status(*resource, planet) {
                DepositStatus::Absent => continue, // no deposit -> no production
                DepositStatus::Present(modifier) => modifier,
                DepositStatus::NotDeposit => 0.0,
            };

            let production =
                self.get_or_calculate_production(*resource, size, deposit_bonus, freeport);

            // Calculate income: production scaled by accessibility, divided by sector supply
            // to get market share, then multiplied by market value
            let sector_supply = resource.sector_supply() as f64;
            let market_share = (production * accessibility / 100.0) / sector_supply;
            gross_income += market_share * market_value;
        }

        gross_income
    }

    //TODO: upkeep needs to take into account same-faction supply of demanded resources
    /// Per month
    pub fn calculate_net_income(
        &self,
        size: u32,
        planet: &dyn PlanetConditionChecker,
        accessibility: f64,
    ) -> f64 {
        let gross = self.calculate_gross_income(size, planet, accessibility);
        let upkeep = self.calculate_upkeep(planet.get_property("hazard percent"), planet.size());
        gross - upkeep
    }

    pub fn get_possible_actions(
        &self,
        planet: &dyn PlanetConditionChecker,
        balance: &Balance,
        exclude_upgrades: bool,
    ) -> Vec<Action> {
        // TODO: experiment with allowing adding items before building is done

        let mut actions = Vec::with_capacity(if !exclude_upgrades { 4 } else { 2 });
        let planet_name_hash = planet.name_hash();

        // Add possible colony items if none present
        if !self.has_colony_item() {
            // Now we can use the planet reference to get possible colony items
            for item in self.get_possible_colony_items(planet) {
                if balance.colony_items().contains_key(&item) {
                    actions.push(Action::InstallItem(
                        planet_name_hash,
                        self.facility_type,
                        item,
                    ));
                }
            }
        }

        if !exclude_upgrades {
            // Add improvements if not present
            if !self.has_improvements() {
                let improvement_cost = improvement_story_point_cost(planet.improvements());
                if balance.story_points() >= improvement_cost {
                    actions.push(Action::AddImprovement(planet_name_hash, self.facility_type));
                }
            }

            // Add alpha core if not present
            if !self.has_alpha_core() {
                if balance.alpha_cores() >= 1 {
                    actions.push(Action::AddAlphaCore(planet_name_hash, self.facility_type));
                }
            }
        }

        if self.current_build_days > 0 {
            let wait = Action::Wait((self.current_build_days as f64 / 30.0).ceil() as u32);
            actions.push(wait);
        }

        actions
    }

    pub fn _get_differences(&self, other: &Facility) -> Vec<String> {
        let mut differences = Vec::new();

        if self.facility_type != other.facility_type {
            differences.push(format!(
                "Facility type changed from {:?} to {:?}",
                self.facility_type, other.facility_type
            ));
        }
        if self.current_build_days != other.current_build_days {
            differences.push(format!(
                "Remaining build days changed from {} to {}",
                self.current_build_days, other.current_build_days
            ));
        }
        if self.improvements != other.improvements {
            differences.push(format!(
                "Improvements changed from {} to {}",
                self.improvements, other.improvements
            ));
        }
        if self.alpha_core != other.alpha_core {
            differences.push(format!(
                "Alpha core changed from {} to {}",
                self.alpha_core, other.alpha_core
            ));
        }
        if self.colony_item != other.colony_item {
            differences.push(format!(
                "Colony item changed from {:?} to {:?}",
                self.colony_item, other.colony_item
            ));
        }
        // base_production / base_demands / upkeep_formula are determined by
        // facility_type and contain fn pointers, which can't be compared reliably.
        if self.base_accessibility_bonus != other.base_accessibility_bonus {
            differences.push(format!(
                "Base accessibility bonus changed from {} to {}",
                self.base_accessibility_bonus, other.base_accessibility_bonus
            ));
        }
        if self.base_stability_bonus != other.base_stability_bonus {
            differences.push(format!(
                "Base stability bonus changed from {} to {}",
                self.base_stability_bonus, other.base_stability_bonus
            ));
        }
        if self.base_defense_multiplier != other.base_defense_multiplier {
            differences.push(format!(
                "Base defense multiplier changed from {} to {}",
                self.base_defense_multiplier, other.base_defense_multiplier
            ));
        }
        if self.base_income_multiplier != other.base_income_multiplier {
            differences.push(format!(
                "Base income multiplier changed from {} to {}",
                self.base_income_multiplier, other.base_income_multiplier
            ));
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

/// Deposit-based gating/bonus for extraction resources.
///
/// In Starsector a deposit's abundance modifier ranges roughly -1..=+3 and is *added*
/// to the base (size-derived) production. A deposit with a modifier of -1 or 0 is still
/// present and still produces. In the data, a *missing* column means "no deposit", while
/// a present column (even with value 0 or -1) means the deposit exists with that modifier.
#[derive(Clone, Copy)]
pub enum DepositStatus {
    /// Not tied to a planetary deposit (manufacturing); produce normally, no bonus.
    NotDeposit,
    /// Deposit-gated resource whose deposit is absent on this planet; do not produce.
    Absent,
    /// Deposit present with the given abundance modifier (may be negative).
    Present(f64),
}

pub fn deposit_status(resource: Resource, planet: &dyn PlanetConditionChecker) -> DepositStatus {
    planet.deposit_status(resource)
}

pub trait PlanetConditionChecker {
    fn name(&self) -> &str;
    fn name_hash(&self) -> u64;
    fn size(&self) -> u32;
    fn has_property(&self, property: &str) -> bool;
    fn get_property(&self, property: &str) -> f64;
    fn accessibility(&self) -> f64;
    fn improvements(&self) -> u32;
    fn is_free_port(&self) -> bool;
    fn deposit_status(&self, resource: Resource) -> DepositStatus {
        let prop = match resource {
            Resource::Ore => "ores",
            Resource::TransplutonicOre => "rare ores",
            Resource::Volatiles => "volatiles",
            Resource::Organics => "organics",
            Resource::Food => {
                // Farming uses a Farmland deposit (modifier may be <= 0); Aquaculture uses a
                // Water-covered world (a boolean condition with no abundance modifier).
                let farmland = if self.has_property("farmland") {
                    Some(self.get_property("farmland"))
                } else {
                    None
                };
                let water = self.get_property("water") > 0.0;
                return match (farmland, water) {
                    (None, false) => DepositStatus::Absent,
                    (Some(m), false) => DepositStatus::Present(m),
                    (None, true) => DepositStatus::Present(0.0),
                    (Some(m), true) => DepositStatus::Present(m.max(0.0)),
                };
            }
            _ => return DepositStatus::NotDeposit,
        };

        if self.has_property(prop) {
            DepositStatus::Present(self.get_property(prop))
        } else {
            DepositStatus::Absent
        }
    }
}

impl PlanetConditionChecker for super::Planet {
    fn name(&self) -> &str {
        &self.name
    }

    fn name_hash(&self) -> u64 {
        self.name_hash
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

    fn accessibility(&self) -> f64 {
        self.calculate_accessibility()
    }

    fn improvements(&self) -> u32 {
        self.get_num_facility_improvements()
    }

    fn is_free_port(&self) -> bool {
        self.is_free_port()
    }

    fn deposit_status(&self, resource: Resource) -> DepositStatus {
        self.property_cache.deposit_status(resource)
    }
}
