use crate::constants::{ColonyItem, AdminType, FACILITY_DATA};
use crate::system::{System};
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

    pub fn spend_credits(&mut self, amount: f32) {
        self.credits -= amount;
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

    pub fn update_income(&mut self, gross: f32, net: f32) {
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
        // Much simpler and faster than hashing systems!
        // The action log is all we need to hash
        
        // First split the action log into sequences of wait and non-wait actions
        // So [Wait(2), Wait(1), AddFacility(_,_), AddImprovement(_,_), Wait(1)] 
        // becomes [[Wait(2), Wait(1)], [AddFacility(_,_), AddImprovement(_,_)], [Wait(1)]]
        let mut sequences = Vec::new();
        let mut current_sequence = Vec::new();
        let mut is_wait_sequence = self.action_log.first().map_or(false, |a| matches!(a, Action::Wait(_)));

        for action in &self.action_log {
            let is_wait = matches!(action, Action::Wait(_));
            if is_wait != is_wait_sequence && !current_sequence.is_empty() {
                sequences.push(std::mem::take(&mut current_sequence));
                is_wait_sequence = is_wait;
            }
            current_sequence.push(action);
        }

        if !current_sequence.is_empty() {
            sequences.push(current_sequence);
        }


        // Now we can simply add the hash values of all items in each sequence
        // since the order of the items in these sub-sequences doesn't matter
        let mut num_sequence = vec![];
        for sequence in sequences {
            let mut hash = 0u64;
            for item in sequence {
                hash = hash.wrapping_add(item.get_hash());
            }
            num_sequence.push(hash);
        }

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

    pub fn get_possible_actions(&self) -> Vec<Action> {
        self.system.get_possible_actions(&self.balance)
    }

    pub fn apply_action_raw(&mut self, action: &Action) {
        self.action_log.push(action.clone());
        match action {
            Action::AddFacility(planet_name, facility_name) => {
                // if facility_name == "spaceport" || facility_name == "population" {
                //     panic!("Can't add core facility {} to planet {}\n{:#?}", facility_name, planet_name, self);
                // }
                let planet = self.system_mut().get_planet_mut(planet_name).unwrap();
                if planet.add_facility(facility_name.clone()) {
                    if let Some(facility_data) = FACILITY_DATA.get(facility_name.as_str()) {
                        self.balance_mut().spend_credits(facility_data.build_cost as f32);
                    }
                }
            },
            Action::AddImprovement(planet_name, facility_name) => {
                let improvement_cost = 2_u32.pow(self.system().get_planet(planet_name).unwrap().get_num_facility_improvements());
                self.balance_mut().spend_story_points(improvement_cost);
                let planet = self.system_mut().get_planet_mut(planet_name).unwrap();
                let facility = planet.get_facility_mut(facility_name).unwrap();
                facility.add_improvements();
            },
            Action::AddAlphaCore(planet_name, facility_name) => {
                self.balance_mut().spend_alpha_cores(1);
                self.system_mut().get_planet_mut(planet_name).unwrap()
                    .get_facility_mut(facility_name).unwrap().add_alpha_core();
            },
            Action::InstallItem(planet_name, facility_name, item) => {
                self.balance_mut().remove_colony_item(item);
                self.system_mut().get_planet_mut(planet_name).unwrap().get_facility_mut(facility_name).unwrap().add_colony_item_raw(item.clone());
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
                self.balance.spend_credits(75000.0);
                self.system.get_planet_mut(planet_name).unwrap().set_has_colony(true);
            }
            Action::Wait(months) => {
                for planet in self.system.planets_mut().values_mut() {
                    if !planet.has_colony() {
                        continue;
                    }
                    let (_, net_from_wait) = planet.wait(*months);
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

    pub fn undo_last_action(&mut self) {
        let action = self.action_log.pop().unwrap();
        match action {
            Action::AddFacility(planet_name, facility_name) => {
                // if facility_name == "spaceport" || facility_name == "population" {
                //     panic!("Can't remove core facility {} to planet {}\n{:#?}", facility_name, planet_name, self);
                // }
                self.system_mut().get_planet_mut(&planet_name).unwrap().remove_facility(&facility_name);
                if let Some(facility_data) = FACILITY_DATA.get(facility_name.as_str()) {
                    self.balance_mut().add_credits(facility_data.build_cost as f32);
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
                self.balance_mut().add_credits(75000.0);
            },
            Action::Wait(months) => {
                for planet in self.system.planets_mut().values_mut() {
                    if !planet.has_colony() {
                        continue;
                    }
                    let (_, net_from_wait) = planet.undo_wait(months);
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

    pub fn score(&self) -> f32 {
        let mut score = 0.0;
        
        // Base score is current credits plus projected income
        score += self.balance.credits;
        score += self.balance.net_income * 2.0; // Project income a little into the future
        
        // Add value for each colonized planet
        for planet in self.system.planets().values() {
            if planet.has_colony() {    

                // Value for facilities
                for (name, _) in planet.facilities() {
                    match name.as_str() {
                        "waystation" => score += 10_000.0,
                        "Patrol HQ" => score += 15_000.0,
                        "planetary shield" => score += 20_000.0,
                        "megaport" => score += 15_000.0,
                        "ground defenses" => score += 10_000.0,
                        "star fortress" => score += 30_000.0,
                        "heavy batteries" => score += 15_000.0,
                        "orbital station" => score += 10_000.0,
                        "cryorevival facility" => score += 15_000.0,
                        "battle station" => score += 20_000.0,
                        _ => score += 50_000.0,
                    }
                }
                
                // Value for admin type
                match planet.admin() {
                    AdminType::Base => {},
                    AdminType::AlphaCore => score += 50_000.0,
                }
                
                // Value for improvements
                for facility in planet.facilities().values() {
                    if facility.has_improvements() {
                        score += 5_000.0;
                    }
                    if facility.has_alpha_core() {
                        score += 7_500.0;
                    }
                }
                
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

    pub fn get_ordered_possible_actions(&self) -> Vec<Action> {
        // println!("Starting get_ordered_possible_actions");
        // println!(" Getting possible actions");
        let mut actions = self.get_possible_actions();
        // println!(" Got {} possible actions", actions.len());
        
        // println!(" Sorting actions");
        actions.sort();
        // println!(" Actions sorted");
        
        let top_n = actions.len().min(5);
        // println!(" Will simulate top {} moves", top_n);
        
        if top_n > 0 {
            // println!(" Simulating and scoring top actions");
            let mut scores: Vec<(f32, usize)> = actions[..top_n]
                .iter()
                .enumerate()
                .map(|(i, action)| {
                    // println!(" Simulating action: {:?}", action);
                    let mut new_state = self.clone();
                    new_state.apply_action_raw(action);
                    let score = new_state.score();
                    // println!(" Action score: {}", score);
                    (score, i)
                })
                .collect();
            
            // println!(" Sorting scores");
            scores.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            // println!(" Scores sorted");
            
            // println!(" Reordering actions based on scores");
            let mut reordered: Vec<Action> = scores.into_iter()
                .map(|(_, i)| actions[i].clone())
                .collect();
            // println!(" Extending reordered actions with remaining actions");
            reordered.extend(actions[top_n..].iter().cloned());
            actions = reordered;
        }
        
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
    pub score: f32,
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
    let mut best_score = f32::NEG_INFINITY;
    let mut nodes_explored = 0;

    for depth in 0..=MAX_DEPTH {
        if info.is_time_up() {
            println!("Time limit reached at depth {}", depth);
            break;
        }

        println!("\nSearching at depth {}", depth);
        let result = dfs(&mut info, depth, best_score);
        if result.is_none() {
            println!("Search interrupted at depth {}", depth);
            break;
        }

        let result = result.unwrap();
        let score = result.score;
        println!("Depth {} completed. Score: {}, Nodes explored: {}", depth, score, result.nodes_explored);
        if score > best_score {
            best_score = score;
            best_actions = result.action_log;
            nodes_explored += result.nodes_explored;
            println!("New best score: {} at depth {}", best_score, depth);
        }
    }
    
    println!("Search completed. Best score: {}, Total nodes explored: {}", best_score, nodes_explored);
    println!("Ending State: {:#?}", info.state);
    SearchResult {
        score: best_score,
        nodes_explored,
        action_log: best_actions,
    }
}



fn dfs(info: &mut SearchInfo, depth: u32, alpha: f32) -> Option<SearchResult> {
    let indent = " ".repeat((MAX_DEPTH - depth + 1) as usize);
    // println!("{}Entering dfs: depth={}, alpha={}", indent, depth, alpha);
    
    if info.is_time_up() {
        // println!("{}Time up, exiting dfs", indent);
        return None;
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
        return Some(result);
    }

    // println!("{}Getting possible actions", indent);
    let actions = info.state.get_ordered_possible_actions();
    // println!("{}Number of possible actions: {}", indent, actions.len());
    // println!("{}Actions: {:#?}", indent, actions);
    let mut best_action_log = info.state.action_log.clone();
    let mut best_score = alpha;

    for (i, action) in actions.iter().enumerate() {
        // println!("{}Applying action {} of {}: {:?}", indent, i + 1, actions.len(), action);
        info.state.apply_action_raw(action);

        // println!("{}Recursive call: depth={}, best_score={}", indent, depth - 1, best_score);
        let sub_result = dfs(info, depth - 1, best_score);

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
            return None;
        }

        // println!("{}Undoing action", indent);
        info.state.undo_last_action();
    }

    result.score = best_score;
    result.action_log = best_action_log;
    // println!("{}Exiting dfs: best_score={}", indent, best_score);
    Some(result)
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
        let actions = info.state.get_ordered_possible_actions();
        if actions.is_empty() {
            println!("No valid actions available!");
            break;
        }
        
        // Try each action and pick the one that leads to highest immediate score
        let mut best_score = f32::NEG_INFINITY;
        let mut best_action = None;
        let mut best_next_state = None;
        
        for action in actions {
            let mut next_state = info.state.clone();
            next_state.apply_action_raw(&action);
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
                for (name, facility) in planet.facilities().iter() {
                    println!("    {} - Income: {} - Prod: {:#?}", name, facility.calculate_net_income(planet.size(), planet), facility.get_resource_production(planet.size(), 0.0, planet.is_free_port()));
                }
            }
        }
    }
    
    println!("\nSimulation complete!");
    println!("Final score: {}", info.state.score());
    println!("Final credits: {}", info.state.balance.credits);

    println!("\nAction sequence:");
    for action in info.action_log() {
        println!("  - {:?}", action);
    }
    
    info
}