use crate::constants::{ColonyItem, AdminType, FACILITY_DATA};
use crate::system::{System};
use core::panic;
use std::collections::{HashMap, hash_map};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use std::time::Instant;
use std::vec;

#[derive(Debug, Clone, Eq, Hash)]
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

impl Action {
    pub fn get_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }

    fn priority(&self) -> i32 {
        match self {
            // Highest priority: Direct income improvements
            Action::AddFacility(_, name) => {
                if let Some(facility_data) = FACILITY_DATA.get(name.as_str()) {
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

impl PartialEq for Action {
    fn eq(&self, other: &Self) -> bool {
        self.priority() == other.priority()
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
    colony_items: HashMap<ColonyItem, u32>,
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
            colony_items: HashMap::new(),
        }
    }

    // Getters
    pub fn credits(&self) -> f64 { self.credits }
    pub fn story_points(&self) -> u32 { self.story_points }
    pub fn alpha_cores(&self) -> u32 { self.alpha_cores }
    pub fn gross_income(&self) -> f64 { self.gross_income }
    pub fn net_income(&self) -> f64 { self.net_income }
    pub fn colony_items(&self) -> &HashMap<ColonyItem, u32> { &self.colony_items }

    // Mutators
    pub fn add_credits(&mut self, amount: f64) {
        self.credits += amount.ceil();
    }

    pub fn spend_credits(&mut self, amount: f64) {
        self.credits -= amount.ceil();
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
}

impl Hash for State {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Initialize a vector to store hashes of action sequences
        let mut num_sequence = Vec::new();
        let mut current_hash = 0u64;
        let mut is_wait_sequence = false;

        // Iterate through each action in the action log
        for action in &self.action_log {
            // Check if the current action is a Wait action
            let is_wait = matches!(action, Action::Wait(_));
            
            // If we transition between Wait and non-Wait actions, push the current hash
            if is_wait != is_wait_sequence && current_hash != 0 {
                num_sequence.push(current_hash);
                current_hash = 0;
            }
            
            // Update the wait sequence flag
            is_wait_sequence = is_wait;
            
            // Add the hash of the current action to the running hash
            // Using wrapping_add to handle potential overflow
            current_hash = current_hash.wrapping_add(action.get_hash());
        }

        // Push the final hash if there's any remaining
        if current_hash != 0 {
            num_sequence.push(current_hash);
        }

        // Hash the entire sequence of hashes
        num_sequence.hash(state);
    }
}

impl State {
    pub fn new(balance: Balance, system: System) -> Self {
        Self { balance, system, action_log: Vec::new() }
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

    pub fn action_log(&self) -> &Vec<Action> {
        &self.action_log
    }

    pub fn action_log_mut(&mut self) -> &mut Vec<Action> {
        &mut self.action_log
    }

    pub fn get_possible_actions(&self, slim: bool) -> Vec<Action> {
        self.system.get_possible_actions(&self.balance, slim)
    }

    pub fn apply_action_raw(&mut self, action: &Action, debug: bool) {
        self.action_log.push(action.clone());
        match action {
            Action::AddFacility(planet_name, facility_name) => {
                // if facility_name == "spaceport" || facility_name == "population" {
                //     panic!("Can't add core facility {} to planet {}\n{:#?}", facility_name, planet_name, self);
                // }
                let planet = self.system_mut().get_planet_mut(planet_name).unwrap();
                if planet.add_facility(facility_name.clone()) {
                    if let Some(facility_data) = FACILITY_DATA.get(facility_name.as_str()) {
                        self.balance_mut().spend_credits(facility_data.build_cost as f64);
                    }
                }
            },
            Action::AddImprovement(planet_name, facility_name) => {
                let improvement_cost = 2_u32.pow(self.system().get_planet(planet_name).unwrap().get_num_facility_improvements());
                self.balance_mut().spend_story_points(improvement_cost);
                let planet = self.system_mut().get_planet_mut(planet_name).unwrap();
                let fac = planet.get_facility_mut(facility_name).unwrap();
                fac.add_improvements();
                // fac.update_accessibility_bonus();
            },
            Action::AddAlphaCore(planet_name, facility_name) => {
                self.balance_mut().spend_alpha_cores(1);
                let fac = self.system_mut().get_planet_mut(planet_name).unwrap()
                    .get_facility_mut(facility_name).unwrap();
                fac.add_alpha_core();
                // fac.update_accessibility_bonus();
            },
            Action::InstallItem(planet_name, facility_name, item) => {
                self.balance_mut().remove_colony_item(item);
                let fac = self.system_mut().get_planet_mut(planet_name).unwrap().get_facility_mut(facility_name).unwrap();
                fac.add_colony_item_raw(item.clone());
                // fac.update_accessibility_bonus();
            },
            Action::SetFreePort(planet_name, is_free_port) => {
                self.system_mut().get_planet_mut(planet_name).unwrap().set_free_port(*is_free_port);
            },
            Action::SetHazardPay(planet_name, has_hazard_pay) => {
                self.system_mut().get_planet_mut(planet_name).unwrap().set_hazard_pay(*has_hazard_pay);
            }
            Action::UpgradeAdmin(planet_name) => {
                self.balance_mut().spend_alpha_cores(1);
                self.system.get_planet_mut(planet_name).unwrap().set_admin(AdminType::AlphaCore);
            }
            Action::Colonize(planet_name) => {
                self.balance.spend_credits(125000.0);
                self.system.get_planet_mut(planet_name).unwrap().set_has_colony(true);
            }
            Action::Wait(months) => {
                for planet in self.system.planets_mut().values_mut() {
                    if !planet.has_colony() {
                        continue;
                    }
                    let (_, net_from_wait) = planet.wait(*months, debug);
                    self.balance.add_credits(net_from_wait);
                    // println!("Net income from waiting for planet {} is {}", planet.name(), net_from_wait);
                }
            }
        }
        let gross_income = self.system.get_gross_income();
        let net_income = gross_income - self.system.total_upkeep();
        // println!("Gross income is {}, net income is {}", gross_income, net_income);
        self.balance.update_income(gross_income, net_income);
    }

    pub fn undo_last_action(&mut self, debug: bool) {
        let action = self.action_log.pop();
        if action.is_none() {
            return;
        }
        let action = action.unwrap();
        match action {
            Action::AddFacility(planet_name, facility_name) => {
                // if facility_name == "spaceport" || facility_name == "population" {
                //     panic!("Can't remove core facility {} to planet {}\n{:#?}", facility_name, planet_name, self);
                // }
                self.system_mut().get_planet_mut(&planet_name).unwrap().remove_facility(&facility_name);
                if let Some(facility_data) = FACILITY_DATA.get(facility_name.as_str()) {
                    self.balance_mut().add_credits(facility_data.build_cost as f64);
                }
            },
            Action::AddImprovement(planet_name, facility_name) => {
                let improvement_cost = 2u32.pow(self.system().get_planet(&planet_name).unwrap().get_num_facility_improvements()-1);
                self.system_mut().get_planet_mut(&planet_name).unwrap().get_facility_mut(&facility_name).unwrap().remove_improvements();
                self.balance_mut().add_story_points(improvement_cost);
            },
            Action::AddAlphaCore(planet_name, facility_name) => {
                self.system_mut().get_planet_mut(&planet_name).unwrap().get_facility_mut(&facility_name).unwrap().remove_alpha_core();
                self.balance_mut().add_alpha_cores(1);
            },
            Action::InstallItem(planet_name, facility_name, item) => {
                self.system_mut().get_planet_mut(&planet_name).unwrap().get_facility_mut(&facility_name).unwrap().remove_colony_item();
                self.balance_mut().add_colony_item(item);
            },
            Action::SetFreePort(planet_name, is_free_port) => {
                self.system_mut().get_planet_mut(&planet_name).unwrap().set_free_port(!is_free_port);
            },
            Action::SetHazardPay(planet_name, has_hazard_pay) => {
                self.system_mut().get_planet_mut(&planet_name).unwrap().set_hazard_pay(!has_hazard_pay);
            },
            Action::UpgradeAdmin(planet_name) => {
                self.system.get_planet_mut(&planet_name).unwrap().set_admin(AdminType::Base);
                self.balance_mut().add_alpha_cores(1);
            },
            Action::Colonize(planet_name) => {
                self.system.get_planet_mut(&planet_name).unwrap().set_has_colony(false);
                self.balance_mut().add_credits(125000.0);
            },
            Action::Wait(months) => {
                for planet in self.system.planets_mut().values_mut() {
                    if !planet.has_colony() {
                        continue;
                    }
                    let (_, net_from_wait) = planet.undo_wait(months, debug);
                    self.balance.spend_credits(net_from_wait);
                    // println!("Undo net income from waiting for planet {} is {}", planet.name(), net_from_wait);
                }
            },
        }
        let gross_income = self.system.get_gross_income();
        let net_income = gross_income - self.system.total_upkeep();
        // println!("Gross income is {}, net income is {}", gross_income, net_income);
        self.balance.update_income(gross_income, net_income);
        
    }

    pub fn score(&self) -> f64 {
        let mut score = 0.0;
        
        // Base score is current credits plus projected income
        score += self.balance.credits;
        score += self.balance.net_income * 12.0; // Project income a little into the future
        
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
                    AdminType::Base => {},
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
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }

    pub fn get_deep_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        // self.balance.hash(&mut hasher);
        self.system.hash(&mut hasher);
        hasher.finish()
    }

    pub fn get_ordered_possible_actions(&self, slim: bool) -> Vec<Action> {
        // println!("Starting get_ordered_possible_actions");
        // println!(" Getting possible actions");
        let mut actions = self.get_possible_actions(slim);
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

#[derive(Debug, Clone)]
pub struct SearchInfo {
    state: State,
    start_time: Instant,
    time_limit: u32,  // in milliseconds
}

impl SearchInfo {
    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn action_log(&self) -> &Vec<Action> {
        &self.state.action_log()
    }

    pub fn start_time(&self) -> Instant {
        self.start_time
    }

    pub fn time_limit(&self) -> u32 {
        self.time_limit
    }

    pub fn is_time_up(&self) -> bool {
        self.start_time.elapsed().as_millis() as u32 >= self.time_limit
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub score: f64,
    pub nodes_explored: u32,
    pub action_log: Vec<Action>,
}

const MAX_DEPTH: u32 = 100;

pub fn search(initial_state: &State, time_limit: u32) -> SearchResult {
    println!("Starting search with time limit: {} ms", time_limit);
    
    let mut info = SearchInfo {
        state: initial_state.clone(),
        start_time: std::time::Instant::now(),
        time_limit,
    };

    println!("Initial state score: {}", initial_state.score());

    let mut best_actions = Vec::new();
    let mut best_score = f64::NEG_INFINITY;
    let mut nodes_explored = 0;

    for depth in 0..=MAX_DEPTH {
        // TODO: make useful between depths
        let mut tt = HashMap::new();

        if info.is_time_up() {
            println!("Time limit reached at depth {}", depth);
            break;
        }

        let depth_start_time = std::time::Instant::now();
        println!("\nSearching at depth {}", depth);
        let result = dfs(&mut info, depth, best_score, &mut tt);
        if result.is_none() {
            println!("Search interrupted at depth {}", depth);
            break;
        }

        let result = result.unwrap();
        let score = result.score;
        let depth_duration = depth_start_time.elapsed();
        println!("Depth {} completed in {:?}. Score: {}, Nodes explored: {}, Unique positions explored: {}", depth, depth_duration, score, result.nodes_explored, tt.len());
        if score > best_score {
            best_score = score;
            best_actions = result.action_log;
            nodes_explored += result.nodes_explored;
            println!("New best score: {} at depth {}", best_score, depth);
        }
    }
    
    println!("Search completed. Best score: {}, Total nodes explored: {}", best_score, nodes_explored);
    println!("Ending State: {:#?}\n{:#?}", info.state.balance, info.state.action_log);
    SearchResult {
        score: best_score,
        nodes_explored,
        action_log: best_actions,
    }
}



fn dfs(info: &mut SearchInfo, depth: u32, alpha: f64, tt: &mut HashMap<u64, SearchResult>) -> Option<SearchResult> {
    // let indent = " ".repeat((MAX_DEPTH - depth + 1) as usize);
    // println!("{}Entering dfs: depth={}, alpha={}", indent, depth, alpha);
    
    if info.is_time_up() {
        // println!("{}Time up, exiting dfs", indent);
        return None;
    }

    let hash = info.state.get_hash();
    if let Some(result) = tt.get(&hash) {
        // println!("{}Found in transposition table: {}", indent, result.score);
        return Some(result.clone());
    }

    let mut result = SearchResult {
        score: 0.0,
        nodes_explored: 1,
        action_log: info.state.action_log.clone(),
    };

    if depth == 0 {
        // println!("{}Reached depth 0, calculating score", indent);
        result.score = info.state.score();
        // println!("{}Leaf node score: {}", indent, result.score);
        // _test_path_undo_consistency(&info.state);
        return Some(result);
    }

    // println!("{}Getting possible actions", indent);
    let actions = info.state.get_ordered_possible_actions(true);
    // println!("{}Number of possible actions: {}", indent, actions.len());
    // println!("{}Actions: {:#?}", indent, actions);
    let mut best_action_log = info.state.action_log.clone();
    let mut best_score = alpha;

    for (i, action) in actions.iter().enumerate() {
        // let pre_action_credits = info.state.balance().credits();
        // println!("{}Applying action {} of {}: {:?}", indent, i + 1, actions.len(), action);
        let orig_state = info.state.clone();

        info.state.apply_action_raw(action, false);

        // println!("{}Recursive call: depth={}, best_score={}", indent, depth - 1, best_score);
        let sub_result = dfs(info, depth - 1, best_score, tt);

        if let Some(sub_result) = sub_result {
            result.nodes_explored += sub_result.nodes_explored;
            // println!("{}Subresult score: {}", indent, sub_result.score);
            if sub_result.score > best_score {
                best_score = sub_result.score;
                best_action_log = sub_result.action_log.clone();
                // println!("{}New best score: {}", indent, best_score);
            }
        } else {
            // println!("{}Time up during recursive call, exiting", indent);
            info.state.undo_last_action(false);
            return None;
        }

        // println!("{}Undoing action", indent);
        info.state.undo_last_action(false);
        
        let diffs = orig_state.system()._get_differences(&info.state.system());
        if !diffs.is_empty() {
            orig_state.system()._print_differences(&info.state.system());
            panic!("State inconsistency detected");
        }
        // if (pre_action_credits - info.state.balance().credits()).abs() > 1e-6 && depth == 1 {
        //     println!("Inconsistency found! Action: {:?}, Pre-action credits: {}, Post-undo credits: {}", action, pre_action_credits, info.state.balance().credits());
        //     println!("{:#?}", info.state.action_log());
        //     // info.state.apply_action_raw(action);
        //     // let actions = info.state.get_ordered_possible_actions();
        //     // _test_action_undo_consistency(&mut info.state, Some(actions.clone()));
        //     // println!("Further testing showed no depth+1 inconsistency.");
        //     panic!("Action undo inconsistency detected");
        // }
    }

    result.score = best_score;
    result.action_log = best_action_log;

    tt.insert(hash, result.clone());
    // println!("{}Exiting dfs: best_score={}", indent, best_score);
    Some(result)
}

fn _fac_inconsistency(state1: &State, state2: &State) -> bool {
    for planet_name in state1.system().planets().keys() {
        let facilities1 = &state1.system().planets()[planet_name].facilities();
        let facilities2 = &state2.system().planets()[planet_name].facilities();

        if facilities1.len() != facilities2.len() {
            return true;
        }

        if !facilities1.iter().zip(facilities2.iter()).all(|(fac1, fac2)| fac1.name() == fac2.name()) {
            return true;
        }
    }
    false
}

pub fn _test_path_undo_consistency(state: &State) {
    let actions = state.action_log().clone();
    let mut temp_state = state.clone();
    let fac = temp_state.system().planets().get(&"Terran 1".to_string()).unwrap().facilities().iter().find(|f| f.name() == "spaceport");
    if let Some(fac) = fac {
        if fac.remaining_build_days() > 30 {
            println!("\nInconsistency found at initial state");
            dbg!(&fac);
            println!("{:?}", temp_state.action_log());
            panic!();
        }
    }

    for _ in 0..=actions.len() {
        temp_state.undo_last_action(false);
    }
    let blank_state = temp_state.clone();

    let mut state_map = vec![];

    for action in actions.clone() {
        temp_state.apply_action_raw(&action, false);
        state_map.push(temp_state.clone());
    }

    for i in (0..state_map.len()).rev() {
        let should_be = &state_map[i];
        let is = &temp_state;
        let mut issue = false;
        if _fac_inconsistency(should_be, is) {
            println!("\nInconsistency found at action {:?}", actions[i]);
            for planet_name in should_be.system().planets().keys() {
                println!("Planet: {}", planet_name);
                println!("Should be: {:#?}", should_be.system().planets().get(planet_name).unwrap().facilities().iter().map(|f| f.name()).collect::<Vec<_>>());
                println!("Is: {:#?}", is.system().planets().get(planet_name).unwrap().facilities().iter().map(|f| f.name()).collect::<Vec<_>>());
            }
            issue = true;
        }
        if should_be.balance().credits() != is.balance().credits() {
            println!("\nInconsistency found at action {:?} - Credits", actions[i]);
            println!("Should be: {}", should_be.balance().credits());
            println!("Is: {}", is.balance().credits());
            issue = true;
        }
        if issue {
            println!("\nBlank State - Credits: {}", blank_state.balance().credits());
            println!("Blank State - Facilities: {:?}", blank_state.system().planets().values().flat_map(|p| p.facilities().iter().map(|f| (f.name(), f.remaining_build_days()))).collect::<Vec<_>>());
            println!("Should be State - Credits: {}", should_be.balance().credits());
            println!("Should be State - Facilities: {:?}", should_be.system().planets().values().flat_map(|p| p.facilities().iter().map(|f| (f.name(), f.remaining_build_days()))).collect::<Vec<_>>());
            println!("Is State - Credits: {}", is.balance().credits());
            println!("Is State - Facilities: {:?}", is.system().planets().values().flat_map(|p| p.facilities().iter().map(|f| (f.name(), f.remaining_build_days()))).collect::<Vec<_>>());
            println!("Action log: {:?}", actions);
            panic!();
        }
        temp_state.undo_last_action(false);
    }
}



pub fn simulate_linear(initial_state: &State, num_turns: u32) -> SearchInfo {
    let mut info = SearchInfo {
        state: initial_state.clone(),
        start_time: std::time::Instant::now(),
        time_limit: 5000,
    };

    println!("\nStarting linear simulation for {} turns...", num_turns);
    println!("Initial state score: {}", info.state.score());
    println!("Initial credits: {}", info.state.balance.credits as i32);
    
    for turn in 0..num_turns {
        println!("\nTurn {}", turn + 1);
        println!("Credits: {}", info.state.balance.credits as i32);
        
        // Get all possible actions
        let actions = info.state.get_ordered_possible_actions(true);
        if actions.is_empty() {
            println!("No valid actions available!");
            break;
        }
        
        // Try each action and pick the one that leads to highest immediate score
        let mut best_score = f64::NEG_INFINITY;
        let mut best_action = None;
        let mut best_next_state = None;
        
        for action in actions {
            let mut next_state = info.state.clone();
            next_state.apply_action_raw(&action, false);
            let score = next_state.score();
            
            println!("  Action: {:?} -> Score: {}", action, score as i32);
            
            if score > best_score {
                best_score = score;
                best_action = Some(action);
                best_next_state = Some(next_state);
            }
        }
        
        // Apply the best action
        if let (Some(action), Some(next_state)) = (best_action, best_next_state) {
            println!("Choosing action: {:?}", action);
            info.state = next_state;
        } else {
            println!("No valid action found!");
            break;
        }
        
        // Print colony status
        println!("\nColony Status:");
        for (name, planet) in info.state.system.planets().iter() {
            if planet.has_colony() {
                println!("\n  {} - Income: {} - Size: {}", name, planet.get_net_income(), planet.size());
                println!("    Facility Status:");
                for facility in planet.facilities().iter() {
                    let name = facility.name();
                    println!("    {} - Income: {} - Prod: {:#?}", name, facility.calculate_net_income(planet.size(), planet), facility.get_resource_production(planet.size(), 0.0, planet.is_free_port()));
                }
            }
        }
    }

    println!("\nAction sequence:");
    for action in info.action_log() {
        println!("  - {:?}", action);
    }

    for _ in 0..num_turns {
        info.state.undo_last_action(false);
    }
    
    println!("\nSimulation complete!");
    println!("Final score: {}", info.state.score());
    println!("Final credits: {}", info.state.balance.credits);
    
    info
}