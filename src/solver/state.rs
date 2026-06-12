use crate::planet::PlanetWaitSnapshot;
use crate::{
    constants::{AdminType, ColonyItem, FacilityType, FACILITY_DATA},
    System,
};
use nohash_hasher::NoHashHasher;
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, DefaultHasher, Hash, Hasher},
};

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub enum Action {
    AddFacility(u64, FacilityType),    // planet name hash, facility type
    AddImprovement(u64, FacilityType), // planet name hash, facility type
    AddAlphaCore(u64, FacilityType),   // planet name hash, facility type
    InstallItem(u64, FacilityType, ColonyItem), // planet name hash, facility type, item
    SetFreePort(u64, bool),            // planet name hash, is_free_port
    SetHazardPay(u64, bool),           // planet name hash, has_hazard_pay
    UpgradeAdmin(u64),                 // Upgrade from Base to AlphaCore; planet name hash
    BuildMakeshiftCommRelay,           // system-wide stable-point development
    Colonize(u64),                     // planet name hash
    Wait(u32),                         // number of months
}

pub fn format_action(action: &Action, planet_names: &HashMap<u64, String>) -> String {
    match action {
        Action::AddFacility(planet_hash, facility_type) => format!(
            "Build {} on {}",
            title_case(facility_type.as_str()),
            planet_name(*planet_hash, planet_names)
        ),
        Action::AddImprovement(planet_hash, facility_type) => format!(
            "Improve {} on {}",
            title_case(facility_type.as_str()),
            planet_name(*planet_hash, planet_names)
        ),
        Action::AddAlphaCore(planet_hash, facility_type) => format!(
            "Install alpha core in {} on {}",
            title_case(facility_type.as_str()),
            planet_name(*planet_hash, planet_names)
        ),
        Action::InstallItem(planet_hash, facility_type, item) => format!(
            "Install {} in {} on {}",
            title_case(item.name()),
            title_case(facility_type.as_str()),
            planet_name(*planet_hash, planet_names)
        ),
        Action::SetFreePort(planet_hash, enabled) => format!(
            "{} free port on {}",
            if *enabled { "Enable" } else { "Disable" },
            planet_name(*planet_hash, planet_names)
        ),
        Action::SetHazardPay(planet_hash, enabled) => format!(
            "{} hazard pay on {}",
            if *enabled { "Enable" } else { "Disable" },
            planet_name(*planet_hash, planet_names)
        ),
        Action::UpgradeAdmin(planet_hash) => {
            format!(
                "Install alpha-core administrator on {}",
                planet_name(*planet_hash, planet_names)
            )
        }
        Action::BuildMakeshiftCommRelay => "Build makeshift comm relay".to_string(),
        Action::Colonize(planet_hash) => {
            format!("Colonize {}", planet_name(*planet_hash, planet_names))
        }
        Action::Wait(months) => format!(
            "Wait {} {}",
            months,
            if *months == 1 { "month" } else { "months" }
        ),
    }
}

fn planet_name(planet_hash: u64, planet_names: &HashMap<u64, String>) -> String {
    planet_names
        .get(&planet_hash)
        .cloned()
        .unwrap_or_else(|| planet_hash.to_string())
}

fn title_case(input: &str) -> String {
    input
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Action {
    /// Custom hasher implementation for speed with better collision resistance
    pub fn get_hash(&self) -> u64 {
        const PRIME1: u64 = 11400714785074694791;
        const PRIME2: u64 = 14029467366897019727;
        const PRIME3: u64 = 1609587929392839161;

        // Use discriminant in hash to avoid collisions between action types
        let disc_val: u64 = match self {
            Action::AddFacility(..) => 1,
            Action::AddImprovement(..) => 2,
            Action::AddAlphaCore(..) => 3,
            Action::InstallItem(..) => 4,
            Action::SetFreePort(..) => 5,
            Action::SetHazardPay(..) => 6,
            Action::UpgradeAdmin(..) => 7,
            Action::BuildMakeshiftCommRelay => 8,
            Action::Colonize(..) => 9,
            Action::Wait(..) => 10,
        };

        let hash = match self {
            Action::AddFacility(planet_hash, facility_type) => {
                planet_hash.wrapping_mul(PRIME3) ^ ((*facility_type as u64) << 8)
            }
            Action::AddImprovement(planet_hash, facility_type) => {
                planet_hash.wrapping_mul(PRIME3) ^ ((*facility_type as u64) << 8)
            }
            Action::AddAlphaCore(planet_hash, facility_type) => {
                planet_hash.wrapping_mul(PRIME3) ^ ((*facility_type as u64) << 8)
            }
            Action::InstallItem(planet_hash, facility_type, item) => {
                planet_hash.wrapping_mul(PRIME3)
                    ^ ((*facility_type as u64) << 8)
                    ^ ((*item as u64) << 16)
            }
            Action::SetFreePort(planet_hash, is_free_port) => {
                planet_hash.wrapping_mul(PRIME3) ^ ((*is_free_port as u64) << 8)
            }
            Action::SetHazardPay(planet_hash, has_hazard_pay) => {
                planet_hash.wrapping_mul(PRIME3) ^ ((*has_hazard_pay as u64) << 8)
            }
            Action::UpgradeAdmin(planet_hash) => planet_hash.wrapping_mul(PRIME3),
            Action::BuildMakeshiftCommRelay => PRIME3,
            Action::Colonize(planet_hash) => planet_hash.wrapping_mul(PRIME3),
            Action::Wait(months) => (*months as u64).wrapping_mul(PRIME3),
        };

        // Mix in discriminant to differentiate action types
        let combined = hash ^ disc_val.wrapping_mul(PRIME2);
        combined.wrapping_mul(PRIME1) ^ (combined >> 33).wrapping_add(PRIME2)
    }

    fn priority(&self) -> i32 {
        match self {
            // Highest priority: Direct income improvements
            Action::AddFacility(_, facility_type) => {
                if let Some(facility_data) = FACILITY_DATA.get(facility_type) {
                    if facility_data.income_multiplier > 1.0 {
                        return 1000;
                    }
                }
                500
            }
            Action::InstallItem(_, _, _) => 900,

            // Medium priority: Wait actions
            Action::Wait(_) => 800,

            // Lower priority: Colony management
            Action::UpgradeAdmin(_) => 700,
            Action::AddAlphaCore(_, _) => 600,
            Action::AddImprovement(_, _) => 500,
            Action::SetHazardPay(_, true) => 400,
            Action::SetFreePort(_, true) => 300,
            Action::BuildMakeshiftCommRelay => 250,
            Action::Colonize(_) => 200,

            // Lowest priority: Disabling bonuses
            Action::SetHazardPay(_, false) => -500,
            Action::SetFreePort(_, false) => -1000,
        }
    }
}

impl PartialOrd for Action {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Action {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Higher priority should come first
        other.priority().cmp(&self.priority())
    }
}

#[derive(Debug, Clone)]
pub struct Balance {
    // Current balances
    credits: f64,
    story_points: u32,
    alpha_cores: u32,

    // Income tracking
    gross_income: f64,
    net_income: f64,

    // Available colony items and their counts
    colony_items: HashMap<ColonyItem, u32, BuildHasherDefault<NoHashHasher<u8>>>,
}

impl Hash for Balance {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Hash f64 values by converting them to bits
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
    pub fn new(initial_credits: f64, initial_sp: u32, initial_cores: u32) -> Self {
        Self {
            credits: initial_credits,
            story_points: initial_sp,
            alpha_cores: initial_cores,
            gross_income: 0.0,
            net_income: 0.0,
            colony_items: HashMap::with_hasher(BuildHasherDefault::<NoHashHasher<u8>>::default()),
        }
    }

    // Getters
    pub fn credits(&self) -> f64 {
        self.credits
    }
    pub fn story_points(&self) -> u32 {
        self.story_points
    }
    pub fn alpha_cores(&self) -> u32 {
        self.alpha_cores
    }
    pub fn gross_income(&self) -> f64 {
        self.gross_income
    }
    pub fn net_income(&self) -> f64 {
        self.net_income
    }
    pub fn colony_items(&self) -> &HashMap<ColonyItem, u32, BuildHasherDefault<NoHashHasher<u8>>> {
        &self.colony_items
    }

    // Mutators
    pub fn add_credits(&mut self, amount: f64) {
        self.credits += amount.floor();
    }

    pub fn spend_credits(&mut self, amount: f64) {
        self.credits -= amount.floor();
    }

    pub fn set_credits(&mut self, amount: f64) {
        self.credits = amount.floor();
    }

    pub fn add_story_points(&mut self, amount: u32) {
        self.story_points += amount;
    }

    pub fn spend_story_points(&mut self, amount: u32) -> bool {
        if self.story_points >= amount {
            self.story_points -= amount;
            true
        } else {
            false
        }
    }

    pub fn add_alpha_cores(&mut self, amount: u32) {
        self.alpha_cores += amount;
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

    pub fn update_income(&mut self, gross: f64, net: f64) {
        self.gross_income = gross;
        self.net_income = net;
    }
}

#[derive(Debug, Clone)]
pub struct State {
    balance: Balance,
    system: System,
    action_log: Vec<Action>,
    wait_undo_stack: Vec<WaitUndoRecord>,
}

#[derive(Debug, Clone)]
struct WaitUndoRecord {
    credits_delta: f64,
    planet_snapshots: Vec<(u64, PlanetWaitSnapshot)>,
}

#[inline]
fn mix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

pub fn get_action_sequence_hash(actions: &[Action]) -> u64 {
    const SEQ_PRIME: u64 = 0x100000001b3;
    let mut seq_hash: u64 = 0xcbf29ce484222325;
    let mut seg_hash: u64 = 0;
    let mut in_wait = false;
    let mut started = false;

    for action in actions {
        let is_wait = matches!(action, Action::Wait(_));
        if started && is_wait != in_wait {
            seq_hash = (seq_hash ^ mix64(seg_hash)).wrapping_mul(SEQ_PRIME);
            seg_hash = 0;
        }
        in_wait = is_wait;
        started = true;
        seg_hash = seg_hash.wrapping_add(mix64(action.get_hash()));
    }

    if started {
        seq_hash = (seq_hash ^ mix64(seg_hash)).wrapping_mul(SEQ_PRIME);
    }

    seq_hash
}

impl Hash for State {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(get_action_sequence_hash(&self.action_log));
    }
}

impl State {
    pub fn new(balance: Balance, system: System) -> Self {
        Self {
            balance,
            system,
            action_log: Vec::with_capacity(20),
            wait_undo_stack: Vec::new(),
        }
    }

    pub fn balance(&self) -> &Balance {
        &self.balance
    }

    pub fn balance_mut(&mut self) -> &mut Balance {
        &mut self.balance
    }

    pub fn set_balance(&mut self, balance: Balance) {
        self.balance = balance;
    }

    pub fn system(&self) -> &System {
        &self.system
    }

    pub fn system_mut(&mut self) -> &mut System {
        &mut self.system
    }

    pub fn action_log(&self) -> &Vec<Action> {
        &self.action_log
    }

    pub fn action_log_mut(&mut self) -> &mut Vec<Action> {
        &mut self.action_log
    }

    pub fn get_possible_actions(&self, exclude_upgrades: bool) -> Vec<Action> {
        self.system
            .get_possible_actions(&self.balance, exclude_upgrades)
    }

    /// Returns a vector of State objects with each planet in their own State
    pub fn to_vec_by_planet(&self) -> Vec<State> {
        let mut states = Vec::with_capacity(self.system.planets().len());
        for (_, planet) in self.system.planets() {
            let mut new_system = System::new(self.system().name().to_string());
            new_system.add_planet(planet.clone());
            let new_state = State::new(self.balance.clone(), new_system);
            states.push(new_state);
        }
        states
    }

    pub fn apply_action_raw(&mut self, action: &Action, debug: bool) {
        self.action_log.push(action.clone());
        match action {
            Action::AddFacility(planet_hash, facility_type) => {
                let planet = self
                    .system_mut()
                    .get_planet_mut_by_hash(*planet_hash)
                    .unwrap();
                if planet.add_facility(*facility_type) {
                    if let Some(facility_data) = FACILITY_DATA.get(facility_type) {
                        self.balance_mut()
                            .spend_credits(facility_data.build_cost as f64);
                    }
                }
            }
            Action::AddImprovement(planet_hash, facility_type) => {
                let improvement_cost = 2_u32.pow(
                    self.system()
                        .get_planet_by_hash(*planet_hash)
                        .unwrap()
                        .get_num_facility_improvements(),
                );
                self.balance_mut().spend_story_points(improvement_cost);
                let planet = self
                    .system_mut()
                    .get_planet_mut_by_hash(*planet_hash)
                    .unwrap();
                let fac = planet.get_facility_mut(*facility_type).unwrap();
                fac.add_improvements();
            }
            Action::AddAlphaCore(planet_hash, facility_type) => {
                self.balance_mut().spend_alpha_cores(1);
                let fac = self
                    .system_mut()
                    .get_planet_mut_by_hash(*planet_hash)
                    .unwrap()
                    .get_facility_mut(*facility_type)
                    .unwrap();
                fac.add_alpha_core();
            }
            Action::InstallItem(planet_hash, facility_type, item) => {
                self.balance_mut().remove_colony_item(item);
                let fac = self
                    .system_mut()
                    .get_planet_mut_by_hash(*planet_hash)
                    .unwrap()
                    .get_facility_mut(*facility_type)
                    .unwrap();
                fac.add_colony_item_raw(item.clone());
            }
            Action::SetFreePort(planet_hash, is_free_port) => {
                self.system_mut()
                    .get_planet_mut_by_hash(*planet_hash)
                    .unwrap()
                    .set_free_port(*is_free_port);
            }
            Action::SetHazardPay(planet_hash, has_hazard_pay) => {
                self.system_mut()
                    .get_planet_mut_by_hash(*planet_hash)
                    .unwrap()
                    .set_hazard_pay(*has_hazard_pay);
            }
            Action::UpgradeAdmin(planet_hash) => {
                self.balance_mut().spend_alpha_cores(1);
                self.system
                    .get_planet_mut_by_hash(*planet_hash)
                    .unwrap()
                    .set_admin(AdminType::AlphaCore);
            }
            Action::BuildMakeshiftCommRelay => {
                self.system.add_makeshift_comm_relay();
            }
            Action::Colonize(planet_hash) => {
                self.balance.spend_credits(125000.0);
                self.system
                    .get_planet_mut_by_hash(*planet_hash)
                    .unwrap()
                    .set_has_colony(true);
            }
            Action::Wait(months) => {
                let credits_before = self.balance.credits();
                let mut wait_undo = WaitUndoRecord {
                    credits_delta: 0.0,
                    planet_snapshots: Vec::new(),
                };

                for planet in self.system.planets_mut().values_mut() {
                    if !planet.has_colony() {
                        continue;
                    }
                    wait_undo
                        .planet_snapshots
                        .push((planet.name_hash(), planet.snapshot_wait_state()));
                    let (_, net_from_wait) = planet.wait(*months, debug);
                    self.balance.add_credits(net_from_wait);
                }

                wait_undo.credits_delta = self.balance.credits() - credits_before;
                self.wait_undo_stack.push(wait_undo);
            }
        }
        let gross_income = self.system.get_gross_income();
        let net_income = gross_income - self.system.total_upkeep();
        self.balance.update_income(gross_income, net_income);
    }

    pub fn undo_last_action(&mut self, debug: bool) {
        let action = self.action_log.pop();
        if action.is_none() {
            return;
        }
        let action = action.unwrap();
        match action {
            Action::AddFacility(planet_hash, facility_type) => {
                self.system_mut()
                    .get_planet_mut_by_hash(planet_hash)
                    .unwrap()
                    .remove_facility(facility_type);
                if let Some(facility_data) = FACILITY_DATA.get(&facility_type) {
                    self.balance_mut()
                        .add_credits(facility_data.build_cost as f64);
                }
            }
            Action::AddImprovement(planet_hash, facility_type) => {
                let improvement_cost = 2u32.pow(
                    self.system()
                        .get_planet_by_hash(planet_hash)
                        .unwrap()
                        .get_num_facility_improvements()
                        - 1,
                );
                self.system_mut()
                    .get_planet_mut_by_hash(planet_hash)
                    .unwrap()
                    .get_facility_mut(facility_type)
                    .unwrap()
                    .remove_improvements();
                self.balance_mut().add_story_points(improvement_cost);
            }
            Action::AddAlphaCore(planet_hash, facility_type) => {
                self.system_mut()
                    .get_planet_mut_by_hash(planet_hash)
                    .unwrap()
                    .get_facility_mut(facility_type)
                    .unwrap()
                    .remove_alpha_core();
                self.balance_mut().add_alpha_cores(1);
            }
            Action::InstallItem(planet_hash, facility_type, item) => {
                self.system_mut()
                    .get_planet_mut_by_hash(planet_hash)
                    .unwrap()
                    .get_facility_mut(facility_type)
                    .unwrap()
                    .remove_colony_item();
                self.balance_mut().add_colony_item(item);
            }
            Action::SetFreePort(planet_hash, is_free_port) => {
                self.system_mut()
                    .get_planet_mut_by_hash(planet_hash)
                    .unwrap()
                    .set_free_port(!is_free_port);
            }
            Action::SetHazardPay(planet_hash, has_hazard_pay) => {
                self.system_mut()
                    .get_planet_mut_by_hash(planet_hash)
                    .unwrap()
                    .set_hazard_pay(!has_hazard_pay);
            }
            Action::UpgradeAdmin(planet_hash) => {
                self.system
                    .get_planet_mut_by_hash(planet_hash)
                    .unwrap()
                    .set_admin(AdminType::Base);
                self.balance_mut().add_alpha_cores(1);
            }
            Action::BuildMakeshiftCommRelay => {
                self.system.remove_makeshift_comm_relay();
            }
            Action::Colonize(planet_hash) => {
                self.system
                    .get_planet_mut_by_hash(planet_hash)
                    .unwrap()
                    .set_has_colony(false);
                self.balance_mut().add_credits(125000.0);
            }
            Action::Wait(months) => {
                if let Some(wait_undo) = self.wait_undo_stack.pop() {
                    for (planet_hash, snapshot) in wait_undo.planet_snapshots {
                        self.system
                            .get_planet_mut_by_hash(planet_hash)
                            .unwrap()
                            .restore_wait_state(&snapshot);
                    }
                    self.balance.add_credits(-wait_undo.credits_delta);
                } else {
                    // Fallback to best-effort undo if no snapshot is available.
                    for planet in self.system.planets_mut().values_mut() {
                        if !planet.has_colony() {
                            continue;
                        }
                        let (_, net_from_wait) = planet.undo_wait(months, debug);
                        self.balance.spend_credits(net_from_wait);
                    }
                }
            }
        }
        let gross_income = self.system.get_gross_income();
        let net_income = gross_income - self.system.total_upkeep();
        self.balance.update_income(gross_income, net_income);
    }

    pub fn score(&self) -> f64 {
        let mut score = 0.0;

        // Base score is current credits plus projected income
        // score += self.balance.credits;
        score += self.balance.net_income * 24.0; // Project income into the future

        // Add value for each colonized planet
        for planet in self.system.planets().values() {
            if planet.has_colony() {
                // // Value for facilities
                // for (name, _) in planet.facilities() {
                //     match name.as_str() {
                //         "waystation" => score += 10_000.0,
                //         "Patrol HQ" => score += 15_000.0,
                //         "planetary shield" => score += 20_000.0,
                //         "megaport" => score += 15_000.0,
                //         "ground defenses" => score += 10_000.0,
                //         "star fortress" => score += 30_000.0,
                //         "heavy batteries" => score += 15_000.0,
                //         "orbital station" => score += 10_000.0,
                //         "cryorevival facility" => score += 15_000.0,
                //         "battle station" => score += 20_000.0,
                //         _ => score += 50_000.0,
                //     }
                // }

                // Value for admin type
                match planet.admin() {
                    AdminType::Base => {}
                    AdminType::AlphaCore => score += 50_000.0,
                }

                // // Value for improvements
                // for facility in planet.facilities().values() {
                //     if facility.has_improvements() {
                //         score += 5_000.0;
                //     }
                //     if facility.has_alpha_core() {
                //         score += 7_500.0;
                //     }
                // }

                // Penalties for not having hazard pay on high hazard worlds
                let avg_hazard = 150.0;
                if planet.hazard_rating() > avg_hazard && !planet.has_hazard_pay() {
                    score -= 1_000.0 * (planet.hazard_rating() - avg_hazard);
                }
            }
        }

        score
    }

    pub fn get_hash(&self) -> u64 {
        get_action_sequence_hash(&self.action_log)
    }

    pub fn get_deep_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.balance.hash(&mut hasher);
        self.system.hash(&mut hasher);
        hasher.finish()
    }

    // pub fn rate_action(&self, action: &Action) -> f64 {
    //     // Start with the base priority from the action
    //     let mut rating = action.priority() as f64;

    //     match action {
    //         Action::AddFacility(planet_name, facility_type) => {
    //             if let Some(planet) = self.system.get_planet(planet_name) {
    //                 // Higher rating for facilities that synergize with planet conditions
    //                 if let Some(facility_data) = FACILITY_DATA.get(facility_type) {
    //                     // Boost rating if this is an income-generating facility
    //                     if facility_data.income_multiplier > 1.0 {
    //                         rating *= 1.5;
    //                     }

    //                     // Consider build cost vs current credits
    //                     let cost_ratio = facility_data.build_cost as f64 / self.balance.credits;
    //                     if cost_ratio > 0.8 {
    //                         // Penalize expensive facilities when low on credits
    //                         rating *= 0.5;
    //                     }
    //                 }
    //             }
    //         },
    //         Action::AddImprovement(planet_name, facility_type) => {
    //             if let Some(planet) = self.system.get_planet(planet_name) {
    //                 // Higher rating for improving income-generating facilities
    //                 if let Some(facility) = planet.get_facility(*facility_type) {
    //                     if facility.income > 0.0 {
    //                         rating *= 1.3;
    //                     }
    //                 }

    //                 // Consider story point cost
    //                 let improvement_cost = 2_u32.pow(planet.get_num_facility_improvements());
    //                 if improvement_cost > self.balance.story_points() {
    //                     rating *= 0.4;
    //                 }
    //             }
    //         },
    //         Action::AddAlphaCore(planet_name, facility_type) => {
    //             // Only rate highly if we have cores to spare
    //             if self.balance.alpha_cores() <= 1 {
    //                 rating *= 0.3;
    //             }

    //             if let Some(planet) = self.system.get_planet(planet_name) {
    //                 // Higher rating for alpha cores in income facilities
    //                 if let Some(facility) = planet.get_facility(*facility_type) {
    //                     if facility.income > 0.0 {
    //                         rating *= 1.4;
    //                     }
    //                 }
    //             }
    //         },
    //         Action::InstallItem(planet_name, facility_type, item) => {
    //             if let Some(planet) = self.system.get_planet(planet_name) {
    //                 if let Some(facility) = planet.get_facility(*facility_type) {
    //                     // Higher rating for items that boost income
    //                     if facility.income > 0.0 {
    //                         rating *= 1.2;
    //                     }
    //                 }
    //             }
    //         },
    //         Action::SetFreePort(planet_name, is_free_port) => {
    //             if let Some(planet) = self.system.get_planet(planet_name) {
    //                 // Higher rating for enabling free port on high income planets
    //                 if *is_free_port && planet.get_base_income() > 5000.0 {
    //                     rating *= 1.5;
    //                 }
    //             }
    //         },
    //         Action::SetHazardPay(planet_name, has_hazard_pay) => {
    //             if let Some(planet) = self.system.get_planet(planet_name) {
    //                 // Higher rating for hazard pay on high hazard planets
    //                 if *has_hazard_pay && planet.hazard_rating() > 150.0 {
    //                     rating *= 1.4;
    //                 }
    //             }
    //         },
    //         Action::UpgradeAdmin(planet_name) => {
    //             // Only rate highly if we have cores to spare
    //             if self.balance.alpha_cores() <= 1 {
    //                 rating *= 0.3;
    //             }

    //             if let Some(planet) = self.system.get_planet(planet_name) {
    //                 // Higher rating for upgrading admins on high income planets
    //                 if planet.get_base_income() > 5000.0 {
    //                     rating *= 1.3;
    //                 }
    //             }
    //         },
    //         Action::Colonize(planet_name) => {
    //             // Consider colonization cost vs current credits
    //             let cost_ratio = 125000.0 / self.balance.credits;
    //             if cost_ratio > 0.8 {
    //                 rating *= 0.4;
    //             }

    //             if let Some(planet) = self.system.get_planet(planet_name) {
    //                 // Lower rating for colonizing high hazard planets early
    //                 if planet.hazard_rating() > 175.0 {
    //                     rating *= 0.7;
    //                 }
    //             }
    //         },
    //         Action::Wait(months) => {
    //             // Higher rating for waiting when we have good income
    //             if self.balance.net_income() > 5000.0 {
    //                 rating *= 1.2;
    //             }
    //             // Lower rating for long waits
    //             if *months > 6 {
    //                 rating *= 0.8;
    //             }
    //         }
    //     }

    //     rating
    // }

    pub fn get_ordered_possible_actions(&self, exclude_upgrades: bool) -> Vec<Action> {
        // println!("Starting get_ordered_possible_actions");
        // println!(" Getting possible actions");
        let mut actions = self.get_possible_actions(exclude_upgrades);
        // println!(" Got {} possible actions", actions.len());

        // println!(" Sorting actions");
        actions.sort();
        // println!(" Actions sorted");

        // let top_n = actions.len().min(5);
        // // println!(" Will simulate top {} moves", top_n);

        // if top_n > 0 {
        //     // println!(" Simulating and scoring top actions");
        //     let mut scores: Vec<(f64, usize)> = actions[..top_n]
        //         .iter()
        //         .enumerate()
        //         .map(|(i, action)| {
        //             // println!(" Simulating action: {:?}", action);
        //             let mut new_state = self.clone();
        //             new_state.apply_action_raw(action);
        //             let score = new_state.score();
        //             // println!(" Action score: {}", score);
        //             (score, i)
        //         })
        //         .collect();

        //     // println!(" Sorting scores");
        //     scores.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        //     // println!(" Scores sorted");

        //     // println!(" Reordering actions based on scores");
        //     let mut reordered: Vec<Action> = scores.into_iter()
        //         .map(|(_, i)| actions[i].clone())
        //         .collect();
        //     // println!(" Extending reordered actions with remaining actions");
        //     reordered.extend(actions[top_n..].iter().cloned());
        //     actions = reordered;
        // }

        // println!(" Returning {} ordered actions", actions.len());
        actions
    }
}
