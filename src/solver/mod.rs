use crate::constants::{ColonyItem, AdminType, FACILITY_DATA};
use crate::system::{System};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone)]
pub enum Action {
    AddFacility(String, String),      // planet name, facility name
    AddImprovement(String, String),   // planet name, facility name
    AddAlphaCore(String, String),     // planet name, facility name
    InstallItem(String, String, ColonyItem),      // planet name, facility name, item
    SetFreePort(String, bool),        // planet name, is_free_port
    SetHazardPay(String, bool),       // planet name, has_hazard_pay
    UpgradeAdmin(String),  // Upgrade from Base to AlphaCore; planet name
    Colonize(String),                 // planet name
    Wait(u32),                        // number of months
}

#[derive(Debug, Clone)]
pub struct Balance {
    // Current balances
    credits: f32,
    story_points: u32,
    alpha_cores: u32,
    
    // Income tracking
    gross_income: f32,
    net_income: f32,
    
    // Available colony items and their counts
    colony_items: HashMap<ColonyItem, u32>,
}

impl Hash for Balance {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Hash f32 values by converting them to bits
        self.credits.to_bits().hash(state);
        self.story_points.hash(state);
        self.alpha_cores.hash(state);
        self.gross_income.to_bits().hash(state);
        self.net_income.to_bits().hash(state);
        
        // Hash map by sorting entries
        let mut colony_item_entries: Vec<_> = self.colony_items.iter().collect();
        colony_item_entries.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in colony_item_entries {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl Balance {
    pub fn new(initial_credits: f32, initial_sp: u32, initial_cores: u32) -> Self {
        Self {
            credits: initial_credits,
            story_points: initial_sp,
            alpha_cores: initial_cores,
            gross_income: 0.0,
            net_income: 0.0,
            colony_items: HashMap::new(),
        }
    }

    // Getters
    pub fn credits(&self) -> f32 { self.credits }
    pub fn story_points(&self) -> u32 { self.story_points }
    pub fn alpha_cores(&self) -> u32 { self.alpha_cores }
    pub fn gross_income(&self) -> f32 { self.gross_income }
    pub fn net_income(&self) -> f32 { self.net_income }
    pub fn colony_items(&self) -> &HashMap<ColonyItem, u32> { &self.colony_items }

    // Mutators
    pub fn add_credits(&mut self, amount: f32) {
        self.credits += amount;
    }

    pub fn spend_credits(&mut self, amount: f32) -> bool {
        if self.credits >= amount {
            self.credits -= amount;
            true
        } else {
            false
        }
    }

    pub fn add_story_point(&mut self) {
        self.story_points += 1;
    }

    pub fn spend_story_points(&mut self, amount: u32) -> bool {
        if self.story_points >= amount {
            self.story_points -= amount;
            true
        } else {
            false
        }
    }

    pub fn add_alpha_core(&mut self) {
        self.alpha_cores += 1;
    }

    pub fn spend_alpha_cores(&mut self, amount: u32) -> bool {
        if self.alpha_cores >= amount {
            self.alpha_cores -= amount;
            true
        } else {
            false
        }
    }

    pub fn add_colony_item(&mut self, item: ColonyItem) {
        *self.colony_items.entry(item).or_insert(0) += 1;
    }

    pub fn remove_colony_item(&mut self, item: &ColonyItem) -> bool {
        if let Some(count) = self.colony_items.get_mut(item) {
            if *count > 0 {
                *count -= 1;
                if *count == 0 {
                    self.colony_items.remove(item);
                }
                return true;
            }
        }
        false
    }

    pub fn update_income(&mut self, gross: f32, net: f32) {
        self.gross_income = gross;
        self.net_income = net;
    }
}

#[derive(Debug, Clone, Hash)]
pub struct State {
    balance: Balance,
    system: System,
}

impl State {
    pub fn new(balance: Balance, system: System) -> Self {
        Self {
            balance,
            system,
        }
    }

    pub fn balance(&self) -> &Balance {
        &self.balance
    }

    pub fn balance_mut(&mut self) -> &mut Balance {
        &mut self.balance
    }

    pub fn system(&self) -> &System {
        &self.system
    }

    pub fn system_mut(&mut self) -> &mut System {
        &mut self.system
    }

    pub fn get_possible_actions(&self) -> Vec<Action> {
        let mut actions = Vec::new();
        actions.extend(self.system.get_possible_actions(&self.balance));
        actions
    }

    pub fn try_apply_action(&mut self, action: Action) -> bool {
        match action {
            Action::AddFacility(planet_name, facility_name) => {
                let facility_data = FACILITY_DATA.get(facility_name.as_str()).unwrap();
                if self.balance.spend_credits(facility_data.build_cost as f32) {
                    self.system.get_planet_mut(&planet_name).unwrap().add_facility(facility_name);
                    true
                } else {
                    false
                }
            }
            Action::AddImprovement(planet_name, facility_name) => {
                let planet = self.system.get_planet_mut(&planet_name).unwrap();
                let num_improvements = planet.get_num_facility_improvements();
                if self.balance.spend_story_points(2_u32.pow(num_improvements)) {
                    planet.get_facility_mut(&facility_name).unwrap().add_improvements();
                    true
                } else {
                    false
                }
            }
            Action::AddAlphaCore(planet_name, facility_name) => {
                if self.balance.spend_alpha_cores(1) {
                    self.system.get_planet_mut(&planet_name).unwrap().get_facility_mut(&facility_name).unwrap().add_alpha_core();
                    true
                } else {
                    false
                }
            }
            Action::InstallItem(planet_name, facility_name, item) => {
                if self.balance.remove_colony_item(&item) {
                    let planet = self.system.get_planet_mut(&planet_name).unwrap();
                    let possible_colony_items = planet.get_facility(&facility_name)
                        .map(|facility| facility.get_possible_colony_items(planet))
                        .unwrap_or_default();
                    if possible_colony_items.contains(&item) {
                        planet.get_facility_mut(&facility_name).unwrap().add_colony_item_raw(item);
                        true
                    } else {
                        self.balance.add_colony_item(item);
                        false
                    }
                } else {
                    false
                }
            }
            Action::SetFreePort(planet_name, is_free_port) => {
                self.system.get_planet_mut(&planet_name).unwrap().set_free_port(is_free_port);
                true
            }
            Action::SetHazardPay(planet_name, has_hazard_pay) => {
                self.system.get_planet_mut(&planet_name).unwrap().set_hazard_pay(has_hazard_pay);
                true
            }
            Action::UpgradeAdmin(planet_name) => {
                if self.balance.spend_alpha_cores(1) {
                    self.system.get_planet_mut(&planet_name).unwrap().set_admin(AdminType::AlphaCore);
                    true
                } else {
                    false
                }
            }
            Action::Colonize(planet_name) => {
                self.system.get_planet_mut(&planet_name).unwrap().set_has_colony(true);
                true
            }
            Action::Wait(months) => {
                for planet in self.system.planets_mut().values_mut() {
                    let net_income = planet.wait(months);
                    self.balance.add_credits(net_income);
                }
                true
            }
        }
    }    

    /// Will apply the action without checking if it is valid
    pub fn apply_action_raw(&mut self, action: Action) {
        match action {
            Action::AddFacility(planet_name, facility_name) => {
                self.balance_mut().spend_credits(FACILITY_DATA[facility_name.as_str()].build_cost as f32);
                self.system.get_planet_mut(&planet_name).unwrap().add_facility(facility_name);
            }
            Action::AddImprovement(planet_name, facility_name) => {
                let improvement_cost = 2_u32.pow(self.system.get_planet(&planet_name).unwrap().get_num_facility_improvements());
                self.balance_mut().spend_story_points(improvement_cost);
                self.system.get_planet_mut(&planet_name).unwrap().get_facility_mut(&facility_name).unwrap().add_improvements();
            }
            Action::AddAlphaCore(planet_name, facility_name) => {
                self.balance_mut().spend_alpha_cores(1);
                self.system.get_planet_mut(&planet_name).unwrap().get_facility_mut(&facility_name).unwrap().add_alpha_core();
            }
            Action::InstallItem(planet_name, facility_name, item) => {
                self.balance_mut().remove_colony_item(&item);
                self.system.get_planet_mut(&planet_name).unwrap().get_facility_mut(&facility_name).unwrap().add_colony_item_raw(item);
            }
            Action::SetFreePort(planet_name, is_free_port) => {
                self.system.get_planet_mut(&planet_name).unwrap().set_free_port(is_free_port);
            }
            Action::SetHazardPay(planet_name, has_hazard_pay) => {
                self.system.get_planet_mut(&planet_name).unwrap().set_hazard_pay(has_hazard_pay);
            }
            Action::UpgradeAdmin(planet_name) => {
                self.balance_mut().spend_alpha_cores(1);
                self.system.get_planet_mut(&planet_name).unwrap().set_admin(AdminType::AlphaCore);
            }
            Action::Colonize(planet_name) => {
                self.system.get_planet_mut(&planet_name).unwrap().set_has_colony(true);
            }
            Action::Wait(months) => {
                for planet in self.system.planets_mut().values_mut() {
                    let net_income = planet.wait(months);
                    self.balance.add_credits(net_income);
                }
            }
        }
    }

}



struct SearchInfo {
    state: State,
    actions: Vec<Action>,
    depth: u32,
}