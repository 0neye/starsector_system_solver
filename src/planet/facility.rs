use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use crate::constants::{ColonyItem, Resource, ResourceAmount, ResourceGetter, COLONY_ITEM_DATA, FACILITY_ALPHA_CORES, FACILITY_DATA, FACILITY_IMPROVEMENTS, POSSIBLE_COLONY_ITEMS};
use crate::solver::{Action, Balance};
use crate::utils::calculate_formula;

#[derive(Debug, Clone, PartialEq)]
pub struct Facility {
    name: String,
    improvements: bool,
    alpha_core: bool,
    colony_item: Option<ColonyItem>,
    base_upkeep: f32,
    accessibility_bonus: f32,
    stability_bonus: i32,
    defense_multiplier: f32,
    income_multiplier: f32,
    production: HashMap<Resource, String>,  // resource -> formula
    demands: HashMap<Resource, String>,     // resource -> formula
    is_structure: bool,
}

impl Hash for Facility {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.improvements.hash(state);
        self.alpha_core.hash(state);
        self.colony_item.hash(state);
        self.stability_bonus.hash(state);
        self.is_structure.hash(state);
        
        // Hash f32 values by converting them to bits
        self.base_upkeep.to_bits().hash(state);
        self.accessibility_bonus.to_bits().hash(state);
        self.defense_multiplier.to_bits().hash(state);
        self.income_multiplier.to_bits().hash(state);
        
        // Hash maps by sorting their entries
        let mut prod_entries: Vec<_> = self.production.iter().collect();
        prod_entries.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in prod_entries {
            k.hash(state);
            v.hash(state);
        }
        
        let mut demand_entries: Vec<_> = self.demands.iter().collect();
        demand_entries.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in demand_entries {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl Facility {
    pub fn new(name: String) -> Option<Self> {
        let data = FACILITY_DATA.get(name.as_str())?;
        
        let mut production = HashMap::new();
        for res in &data.production {
            production.insert(res.resource, res.amount_formula.to_string());
        }
        
        let mut demands = HashMap::new();
        for res in &data.demands {
            demands.insert(res.resource, res.amount_formula.to_string());
        }
        
        Some(Self {
            name: name.clone(),
            improvements: false,
            alpha_core: false,
            colony_item: None,
            base_upkeep: data.base_upkeep_formula.parse().unwrap_or(0.0),
            accessibility_bonus: data.accessibility_bonus,
            stability_bonus: data.stability_bonus,
            defense_multiplier: data.defense_multiplier,
            income_multiplier: data.income_multiplier,
            production,
            demands,
            is_structure: data.is_structure,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
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

    pub fn add_improvements(&mut self) -> bool {
        if !self.improvements {
            self.improvements = true;
            true
        } else {
            false
        }
    }

    pub fn add_alpha_core(&mut self) -> bool {
        if !self.alpha_core {
            self.alpha_core = true;
            true
        } else {
            false
        }
    }

    pub fn can_install_colony_item(&self, item: &ColonyItem, planet: &dyn PlanetConditionChecker) -> bool {
        match item {
            ColonyItem::SoilNanites => {
                !planet.has_property("volatiles") && 
                !planet.has_property("rare ores") && 
                planet.has_property("farmland") && 
                planet.get_property("water world") <= 0.0
            },
            ColonyItem::BiofactoryEmbryo => {
                planet.get_property("habitable") > 0.0
            },
            ColonyItem::PristineNanoforge | ColonyItem::CorruptedNanoforge => {
                planet.get_property("habitable") <= 0.0
            },
            ColonyItem::MantleBore => {
                planet.get_property("gas giant") <= 0.0 && 
                planet.get_property("habitable") <= 0.0
            },
            ColonyItem::CatalyticCore | ColonyItem::SynchrotronCore => {
                planet.get_property("no atmosphere") > 0.0
            },
            ColonyItem::PlasmaDynamo => {
                planet.get_property("gas giant") > 0.0
            },
            ColonyItem::CryoarithmeticEngine => {
                planet.get_property("heat") > 0.0
            },
            ColonyItem::FullereneSpool => {
                planet.get_property("gas giant") <= 0.0 && 
                planet.get_property("extreme activity") <= 0.0
            },
            _ => true // Other items have no planetary requirements
        }
    }

    pub fn get_possible_colony_items(&self, planet: &dyn PlanetConditionChecker) -> Vec<ColonyItem> {
        POSSIBLE_COLONY_ITEMS.iter()
            .filter_map(|item_str| {
                if let Some(item) = ColonyItem::from_str(item_str) {
                    if self.can_install_colony_item(&item, planet) {
                        Some(item)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn add_colony_item(&mut self, item: ColonyItem, planet: &dyn PlanetConditionChecker) -> bool {
        if self.can_install_colony_item(&item, planet) {
            self.colony_item = Some(item);
            true
        } else {
            false
        }
    }

    pub fn add_colony_item_raw(&mut self, item: ColonyItem) {
        self.colony_item = Some(item);
    }

    pub fn remove_colony_item(&mut self) -> Option<ColonyItem> {
        self.colony_item.take()
    }

    pub fn has_colony_item(&self) -> bool {
        self.colony_item.is_some()
    }

    pub fn get_colony_item(&self) -> Option<ColonyItem> {
        self.colony_item
    }

    pub fn calculate_upkeep(&self, hazard_rating: f32) -> f32 {
        let mut upkeep = self.base_upkeep;
        if self.alpha_core {
            upkeep *= 0.75; // 25% reduction for alpha core
        }
        upkeep * (hazard_rating / 100.0)
    }

    pub fn accessibility_bonus(&self) -> f32 {
        let mut bonus = self.accessibility_bonus;
        
        // Add improvement bonus
        if self.improvements {
            if let Some(effects) = FACILITY_IMPROVEMENTS.get(self.name.to_lowercase().as_str()) {
                bonus += effects.accessibility_bonus;
            }
        }

        // Add alpha core bonus
        if self.alpha_core {
            if let Some(effects) = FACILITY_ALPHA_CORES.get(self.name.to_lowercase().as_str()) {
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

    pub fn stability_bonus(&self) -> i32 {
        let mut bonus = self.stability_bonus;
        
        // Add improvement bonus
        if self.improvements {
            if let Some(effects) = FACILITY_IMPROVEMENTS.get(self.name.to_lowercase().as_str()) {
                bonus += effects.stability_bonus;
            }
        }

        bonus
    }

    pub fn defense_multiplier(&self) -> f32 {
        let mut multiplier = self.defense_multiplier;
        
        // Add improvement bonus
        if self.improvements {
            if let Some(effects) = FACILITY_IMPROVEMENTS.get(self.name.to_lowercase().as_str()) {
                multiplier += effects.defense_multiplier;
            }
        }

        // Add alpha core bonus
        if self.alpha_core {
            if let Some(effects) = FACILITY_ALPHA_CORES.get(self.name.to_lowercase().as_str()) {
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

    pub fn income_multiplier(&self) -> f32 {
        let mut multiplier = self.income_multiplier;
        
        // Add improvement bonus
        if self.improvements {
            if let Some(effects) = FACILITY_IMPROVEMENTS.get(self.name.to_lowercase().as_str()) {
                multiplier += effects.income_bonus;
            }
        }

        // Add alpha core bonus
        if self.alpha_core {
            if let Some(effects) = FACILITY_ALPHA_CORES.get(self.name.to_lowercase().as_str()) {
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

    pub fn production(&self) -> &HashMap<Resource, String> {
        &self.production
    }

    pub fn demands(&self) -> &HashMap<Resource, String> {
        &self.demands
    }
    
    pub fn calculate_resource_production(&self, resource: Resource, size: u32, bonus: f32, is_free_port: bool) -> f32 {
        // Special case: Recreational Drugs are only produced if colony is a Free Port
        if resource == Resource::Drugs && self.name == "light industry" && !is_free_port {
            return 0.0;
        }
        // Same with harvested organs and population
        if resource == Resource::HarvestedOrgans && self.name == "population" && !is_free_port {
            return 0.0;
        }

        let mut amount = 0.0;

        if let Some(formula) = self.production.get(&resource) {
            amount = calculate_formula(formula, size);
        }

        // Apply bonuses from improvements
        let mut total_bonus: f32 = bonus;
        if self.improvements {
            if let Some(effects) = FACILITY_IMPROVEMENTS.get(self.name.to_lowercase().as_str()) {
                total_bonus += effects.production_bonus;
            }
        }

        // Add alpha core bonus
        if self.alpha_core {
            if let Some(effects) = FACILITY_ALPHA_CORES.get(self.name.to_lowercase().as_str()) {
                total_bonus += effects.production_bonus;
            }
        }

        // Apply colony item production bonus if applicable
        if let Some(item) = self.colony_item {
            if let Some(item_data) = COLONY_ITEM_DATA.get(&item) {
                if let Some(resource_amount) = item_data.production_bonuses.get(resource) {
                    let this_bonus = calculate_formula(&resource_amount.amount_formula, size);
                    total_bonus += this_bonus;
                }
            }
        }

        amount + total_bonus
    }

    pub fn calculate_resource_demand(&self, resource: Resource, size: u32) -> f32 {
        match self.demands.get(&resource) {
            Some(formula) => calculate_formula(formula, size),
            None => 0.0,
        }
    }

    pub fn calculate_gross_income(&self, size: u32, planet: &dyn PlanetConditionChecker) -> f32 {
        let mut gross_income = 0.0;

        for resource in self.production.keys() {
            let production = self.calculate_resource_production(*resource, size, 0.0, planet.is_free_port());
            let accessability = planet.accessability();
            let total_market_value = resource.market_value();
            let market_share_percent = (production * accessability / 100.0) / total_market_value as f32;
            gross_income += market_share_percent * total_market_value as f32;
        }
        
        gross_income
    }

    //TODO: upkeep needs to take into account same-faction supply of demanded resources
    pub fn calculate_net_income(&self, size: u32, planet: &dyn PlanetConditionChecker) -> f32 {
        let gross = self.calculate_gross_income(size, planet);
        let upkeep = self.calculate_upkeep(planet.get_property("hazard percent"));
        gross - upkeep
    }

    pub fn get_possible_actions(&self, planet: &dyn PlanetConditionChecker, balance: &Balance) -> Vec<Action> {
        let mut actions = Vec::new();
        let facility_name = self.name().to_string();
        let planet_name = planet.name().to_string();

        // Add improvements if not present
        if !self.has_improvements() {
            let improvement_cost = 2_u32.pow(planet.improvements());
            if balance.story_points() >= improvement_cost {
                actions.push(Action::AddImprovement(planet_name.clone(), facility_name.to_string()));
            }
        }

        // Add alpha core if not present
        if !self.has_alpha_core() {
            if balance.alpha_cores() >= 1 {
                actions.push(Action::AddAlphaCore(planet_name.clone(), facility_name.to_string()));
            }
        }

        // Add possible colony items if none present
        if !self.has_colony_item() {
            // Now we can use the planet reference to get possible colony items
            for item in self.get_possible_colony_items(planet) {
                if balance.colony_items().contains_key(&item) {
                    actions.push(Action::InstallItem(planet_name.clone(), facility_name.to_string(), item));
                }
            }
        }

        actions
    }
}

pub trait PlanetConditionChecker {
    fn name(&self) -> &str;
    fn has_property(&self, property: &str) -> bool;
    fn get_property(&self, property: &str) -> f32;
    fn accessability(&self) -> f32;
    fn improvements(&self) -> u32;
    fn is_free_port(&self) -> bool;
}

impl PlanetConditionChecker for super::Planet {
    fn name(&self) -> &str {
        &self.name
    }

    fn has_property(&self, property: &str) -> bool {
        self.properties.get(property).is_some()
    }

    fn get_property(&self, property: &str) -> f32 {
        *self.properties.get(property).unwrap_or(&0.0)
    }

    fn accessability(&self) -> f32 {
        self.calculate_accessibility()
    }

    fn improvements(&self) -> u32 {
        self.get_num_facility_improvements()
    }

    fn is_free_port(&self) -> bool {
        self.is_free_port()
    }
}
