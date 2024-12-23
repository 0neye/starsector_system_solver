use lazy_static::lazy_static;
use rustc_hash::FxHashMap;
use std::collections::HashMap;
use core::hash::BuildHasherDefault;
use nohash_hasher::NoHashHasher;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Resource {
    Supplies,
    Fuel,
    Food,
    Ore,
    Metals,
    TransplutonicOre,
    Transplutonics,
    Organics,
    Volatiles,
    DomesticGoods,
    LuxuryGoods,
    HeavyMachinery,
    HeavyArmaments,
    Drugs,
    HarvestedOrgans,
    ShipHullsAndWeapons,
    Crew,           // Special resources with no market value
    Marines,
}

impl Resource {

    #[inline(always)]
    pub fn base_price(&self) -> f64 {
        match self {
            Resource::Supplies => 250.0,
            Resource::Fuel => 750.0,
            Resource::Food => 1000.0,
            Resource::Ore => 1500.0,
            Resource::Metals => 3000.0,
            Resource::TransplutonicOre => 2000.0,
            Resource::Transplutonics => 6000.0,
            Resource::Organics => 500.0,
            Resource::Volatiles => 5000.0,
            Resource::DomesticGoods => 300.0,
            Resource::LuxuryGoods => 500.0,
            Resource::HeavyMachinery => 500.0,
            Resource::HeavyArmaments => 500.0,
            Resource::Drugs => 1500.0,
            Resource::HarvestedOrgans => 2000.0,
            Resource::ShipHullsAndWeapons => 300.0,
            Resource::Crew => 0.0,
            Resource::Marines => 0.0,
        }
    }

    #[inline(always)]
    pub fn sector_demand(&self) -> u32 {
        match self {
            Resource::Supplies => 267,
            Resource::Fuel => 230,
            Resource::Food => 266,
            Resource::Ore => 72,
            Resource::Metals => 44,
            Resource::TransplutonicOre => 59,
            Resource::Transplutonics => 30,
            Resource::Organics => 199,
            Resource::Volatiles => 25,
            Resource::DomesticGoods => 215,
            Resource::LuxuryGoods => 114,
            Resource::HeavyMachinery => 124,
            Resource::HeavyArmaments => 141,
            Resource::Drugs => 223,
            Resource::HarvestedOrgans => 114,
            Resource::ShipHullsAndWeapons => 250,
            Resource::Crew => 0,
            Resource::Marines => 0,
        }
    }

    #[inline(always)]
    pub fn market_value(&self) -> u32 {
        match self {
            Resource::Supplies => 66750,
            Resource::Fuel => 172500,
            Resource::Food => 266000,
            Resource::Ore => 108000,
            Resource::Metals => 132000,
            Resource::TransplutonicOre => 118000,
            Resource::Transplutonics => 180000,
            Resource::Organics => 99500,
            Resource::Volatiles => 125000,
            Resource::DomesticGoods => 64500,
            Resource::LuxuryGoods => 57000,
            Resource::HeavyMachinery => 62000,
            Resource::HeavyArmaments => 70500,
            Resource::Drugs => 334500,
            Resource::HarvestedOrgans => 228000,
            Resource::ShipHullsAndWeapons => 75000,
            Resource::Crew => 0,
            Resource::Marines => 0,
        }
    }

    #[inline(always)]
    pub fn sector_supply(&self) -> u32 {
        match self {
            Resource::Supplies => 50,
            Resource::Fuel => 25,
            Resource::Food => 58,
            Resource::Ore => 68,
            Resource::Metals => 62,
            Resource::TransplutonicOre => 34,
            Resource::Transplutonics => 43,
            Resource::Organics => 40,
            Resource::Volatiles => 29,
            Resource::DomesticGoods => 68,
            Resource::LuxuryGoods => 43,
            Resource::HeavyMachinery => 50,
            Resource::HeavyArmaments => 50,
            Resource::Drugs => 76,
            Resource::HarvestedOrgans => 35,
            Resource::ShipHullsAndWeapons => 48,
            Resource::Crew => 0,
            Resource::Marines => 0,
        }
    }

    #[inline(always)]
    pub fn price_per_unit(&self) -> f64 {
        match self {
            Resource::Supplies => 1335.0,
            Resource::Fuel => 6900.0,
            Resource::Food => 4586.0,
            Resource::Ore => 1588.0,
            Resource::Metals => 2129.0,
            Resource::TransplutonicOre => 3471.0,
            Resource::Transplutonics => 4186.0,
            Resource::Organics => 2488.0,
            Resource::Volatiles => 4310.0,
            Resource::DomesticGoods => 949.0,
            Resource::LuxuryGoods => 1326.0,
            Resource::HeavyMachinery => 1240.0,
            Resource::HeavyArmaments => 1410.0,
            Resource::Drugs => 4401.0,
            Resource::HarvestedOrgans => 6514.0,
            Resource::ShipHullsAndWeapons => 1562.5,
            Resource::Crew => 0.0,
            Resource::Marines => 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceAmount {
    pub resource: Resource,
    pub amount_formula: fn(u32) -> f64,
}

pub trait ResourceGetter {
    fn get(&self, resource: Resource) -> Option<&ResourceAmount>;
}

impl ResourceGetter for Vec<ResourceAmount> {
    fn get(&self, resource: Resource) -> Option<&ResourceAmount> {
        self.iter().find(|r| r.resource == resource)
    }
}

#[derive(Debug, Clone)]
pub struct FacilityData {
    pub name: &'static str,
    pub build_cost: u32,
    pub build_time: u32,
    pub base_upkeep_formula: fn(u32) -> f64,
    pub accessibility_bonus: f64,
    pub stability_bonus: i32,
    pub defense_multiplier: f64,
    pub income_multiplier: f64,
    pub production: Vec<ResourceAmount>,
    pub demands: Vec<ResourceAmount>,
    pub special_effects: Vec<&'static str>,
    pub requirements: Vec<&'static str>,
    pub is_structure: bool,  // To differentiate between industries and structures
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum ColonyItem {
    SoilNanites,
    MantleBore,
    BiofactoryEmbryo,
    CatalyticCore,
    DroneReplicator,
    CorruptedNanoforge,
    PristineNanoforge,
    CryoarithmeticEngine,
    DealmakerHolosuite,
    FullereneSpool,
    PlasmaDynamo,
    SynchrotronCore,
}

impl ColonyItem {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "soil nanites" => Some(Self::SoilNanites),
            "mantle bore" => Some(Self::MantleBore),
            "biofactory embryo" => Some(Self::BiofactoryEmbryo),
            "catalytic core" => Some(Self::CatalyticCore),
            "drone replicator" => Some(Self::DroneReplicator),
            "corrupted nanoforge" => Some(Self::CorruptedNanoforge),
            "pristine nanoforge" => Some(Self::PristineNanoforge),
            "cryoarithmetic engine" => Some(Self::CryoarithmeticEngine),
            "dealmaker holosuite" => Some(Self::DealmakerHolosuite),
            "fullerene spool" => Some(Self::FullereneSpool),
            "plasma dynamo" => Some(Self::PlasmaDynamo),
            "synchrotron core" => Some(Self::SynchrotronCore),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ColonyItemEffect {
    pub compatible_facilities: Vec<&'static str>,
    pub production_bonuses: Vec<ResourceAmount>,
    pub defense_multiplier: f64,
    pub accessibility_bonus: f64,
    pub income_multiplier: f64,
}

lazy_static! {
    pub static ref COLONY_ITEM_DATA: HashMap<ColonyItem, ColonyItemEffect, BuildHasherDefault<NoHashHasher<u8>>> = {
        let mut map = HashMap::with_hasher(BuildHasherDefault::default());

        map.insert(ColonyItem::SoilNanites, ColonyItemEffect {
            compatible_facilities: vec!["farming"],
            production_bonuses: vec![ResourceAmount {
                resource: Resource::Food,
                amount_formula: |_| 2.0,
            }],
            defense_multiplier: 1.0,
            accessibility_bonus: 0.0,
            income_multiplier: 1.0,
        });

        map.insert(ColonyItem::MantleBore, ColonyItemEffect {
            compatible_facilities: vec!["mining"],
            production_bonuses: vec![
                ResourceAmount {
                    resource: Resource::Ore,
                    amount_formula: |_| 3.0,
                },
                ResourceAmount {
                    resource: Resource::TransplutonicOre,
                    amount_formula: |_| 3.0,
                },
                ResourceAmount {
                    resource: Resource::Organics,
                    amount_formula: |_| 3.0,
                },
            ],
            defense_multiplier: 1.0,
            accessibility_bonus: 0.0,
            income_multiplier: 1.0,
        });

        map.insert(ColonyItem::BiofactoryEmbryo, ColonyItemEffect {
            compatible_facilities: vec!["light industry"],
            production_bonuses: vec![
                ResourceAmount {
                    resource: Resource::DomesticGoods,
                    amount_formula: |_| 2.0,
                },
                ResourceAmount {
                    resource: Resource::LuxuryGoods,
                    amount_formula: |_| 2.0,
                },
                ResourceAmount {
                    resource: Resource::Drugs,
                    amount_formula: |_| 2.0,
                },
            ],
            defense_multiplier: 1.0,
            accessibility_bonus: 0.0,
            income_multiplier: 1.0,
        });

        map.insert(ColonyItem::CatalyticCore, ColonyItemEffect {
            compatible_facilities: vec!["refining"],
            production_bonuses: vec![
                ResourceAmount {
                    resource: Resource::Metals,
                    amount_formula: |_| 3.0,
                },
                ResourceAmount {
                    resource: Resource::Transplutonics,
                    amount_formula: |_| 3.0,
                },
            ],
            defense_multiplier: 1.0,
            accessibility_bonus: 0.0,
            income_multiplier: 1.0,
        });

        map.insert(ColonyItem::DroneReplicator, ColonyItemEffect {
            compatible_facilities: vec!["ground defenses", "heavy batteries"],
            production_bonuses: Vec::new(),
            defense_multiplier: 1.5,
            accessibility_bonus: 0.0,
            income_multiplier: 1.0,
        });

        map.insert(ColonyItem::CorruptedNanoforge, ColonyItemEffect {
            compatible_facilities: vec!["heavy industry"],
            production_bonuses: vec![
                ResourceAmount {
                    resource: Resource::HeavyMachinery,
                    amount_formula: |_| 1.0,
                },
                ResourceAmount {
                    resource: Resource::Supplies,
                    amount_formula: |_| 1.0,
                },
                ResourceAmount {
                    resource: Resource::HeavyArmaments,
                    amount_formula: |_| 1.0,
                },
                ResourceAmount {
                    resource: Resource::ShipHullsAndWeapons,
                    amount_formula: |_| 1.0,
                },
            ],
            defense_multiplier: 1.0,
            accessibility_bonus: 0.0,
            income_multiplier: 1.0,
        });

        map.insert(ColonyItem::PristineNanoforge, ColonyItemEffect {
            compatible_facilities: vec!["heavy industry"],
            production_bonuses: vec![
                ResourceAmount {
                    resource: Resource::HeavyMachinery,
                    amount_formula: |_| 3.0,
                },
                ResourceAmount {
                    resource: Resource::Supplies,
                    amount_formula: |_| 3.0,
                },
                ResourceAmount {
                    resource: Resource::HeavyArmaments,
                    amount_formula: |_| 3.0,
                },
                ResourceAmount {
                    resource: Resource::ShipHullsAndWeapons,
                    amount_formula: |_| 3.0,
                },
            ],
            defense_multiplier: 1.0,
            accessibility_bonus: 0.0,
            income_multiplier: 1.0,
        });

        map.insert(ColonyItem::CryoarithmeticEngine, ColonyItemEffect {
            compatible_facilities: vec!["patrol hq", "military base", "high command"],
            production_bonuses: Vec::new(),
            defense_multiplier: 2.0,
            accessibility_bonus: 0.0,
            income_multiplier: 1.0,
        });

        map.insert(ColonyItem::DealmakerHolosuite, ColonyItemEffect {
            compatible_facilities: vec!["commerce"],
            production_bonuses: Vec::new(),
            defense_multiplier: 1.0,
            accessibility_bonus: 0.0,
            income_multiplier: 1.5,
        });

        map.insert(ColonyItem::FullereneSpool, ColonyItemEffect {
            compatible_facilities: vec!["spaceport", "megaport"],
            production_bonuses: Vec::new(),
            defense_multiplier: 1.0,
            accessibility_bonus: 0.3,
            income_multiplier: 1.0,
        });

        map.insert(ColonyItem::PlasmaDynamo, ColonyItemEffect {
            compatible_facilities: vec!["mining"],
            production_bonuses: vec![ResourceAmount {
                resource: Resource::Volatiles,
                amount_formula: |_| 3.0,
            }],
            defense_multiplier: 1.0,
            accessibility_bonus: 0.0,
            income_multiplier: 1.0,
        });

        map.insert(ColonyItem::SynchrotronCore, ColonyItemEffect {
            compatible_facilities: vec!["fuel production"],
            production_bonuses: vec![ResourceAmount {
                resource: Resource::Fuel,
                amount_formula: |_| 3.0,
            }],
            defense_multiplier: 1.0,
            accessibility_bonus: 0.0,
            income_multiplier: 1.0,
        });

        map
    };
}

lazy_static! {
    pub static ref FACILITY_DATA: FxHashMap<&'static str, FacilityData> = {
        let mut map = FxHashMap::default();
        
        // Population & Infrastructure (special case, always present)
        map.insert("population", FacilityData {
            name: "population",
            build_cost: 0,
            build_time: 0,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 1500.0,
            accessibility_bonus: 0.0,
            stability_bonus: 0,
            defense_multiplier: 1.0,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::Crew, amount_formula: |size| (size as f64 - 3.0) },
                ResourceAmount { resource: Resource::Drugs, amount_formula: |size| (size as f64 - 4.0) },
                ResourceAmount { resource: Resource::HarvestedOrgans, amount_formula: |size| (size as f64 - 5.0) },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::Food, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::DomesticGoods, amount_formula: |size| (size as f64 - 1.0) },
                ResourceAmount { resource: Resource::LuxuryGoods, amount_formula: |size| (size as f64 - 3.0) },
                ResourceAmount { resource: Resource::Drugs, amount_formula: |size| (size as f64 - 2.0) },
                ResourceAmount { resource: Resource::HarvestedOrgans, amount_formula: |size| (size as f64 - 3.0) },
                ResourceAmount { resource: Resource::Organics, amount_formula: |size| (size as f64 - 1.0) },
            ],
            special_effects: vec![],
            requirements: vec![],
            is_structure: false,
        });

        // Spaceport - Also always present
        map.insert("spaceport", FacilityData {
            name: "spaceport",
            build_cost: 50000,
            build_time: 15,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 1500.0,
            accessibility_bonus: 0.5,
            stability_bonus: 0,
            defense_multiplier: 1.0,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::Crew, amount_formula: |size| (size as f64 - 1.0) },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::Fuel, amount_formula: |size| (size as f64 - 2.0) },
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| (size as f64 - 2.0) },
                ResourceAmount { resource: Resource::ShipHullsAndWeapons, amount_formula: |size| (size as f64 - 2.0) },
            ],
            special_effects: vec!["Population growth +2"],
            requirements: vec![],
            is_structure: true,
        });

        // Megaport
        map.insert("megaport", FacilityData {
            name: "megaport",
            build_cost: 300000,
            build_time: 150,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 2000.0,
            accessibility_bonus: 0.8,
            stability_bonus: 0,
            defense_multiplier: 1.0,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::Crew, amount_formula: |size| (size as f64 + 2.0) },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::Fuel, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::ShipHullsAndWeapons, amount_formula: |size| size as f64 },
            ],
            special_effects: vec!["Population growth +(colony size)"],
            requirements: vec!["spaceport"],
            is_structure: true,
        });

        // Farming
        map.insert("farming", FacilityData {
            name: "farming",
            build_cost: 75000,
            build_time: 60,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 500.0,
            accessibility_bonus: 0.0,
            stability_bonus: 0,
            defense_multiplier: 1.0,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::Food, amount_formula: |size| size as f64 },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::HeavyMachinery, amount_formula: |size| (size as f64 - 3.0) },
            ],
            special_effects: vec![],
            requirements: vec!["farmland"],
            is_structure: false,
        });

        // Mining
        map.insert("mining", FacilityData {
            name: "mining",
            build_cost: 100000,
            build_time: 60,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 1000.0,
            accessibility_bonus: 0.0,
            stability_bonus: 0,
            defense_multiplier: 1.0,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::Organics, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::Ore, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::TransplutonicOre, amount_formula: |size| (size as f64 - 2.0) },
                ResourceAmount { resource: Resource::Volatiles, amount_formula: |size| (size as f64 - 2.0) },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::HeavyMachinery, amount_formula: |size| (size as f64 - 3.0) },
                ResourceAmount { resource: Resource::Drugs, amount_formula: |size| size as f64 },
            ],
            special_effects: vec!["Resources only produced if present on planet"],
            requirements: vec!["ores", "rare ores", "volatiles", "organics"],
            is_structure: false,
        });

        // Refining
        map.insert("refining", FacilityData {
            name: "refining",
            build_cost: 225000,
            build_time: 90,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 1500.0,
            accessibility_bonus: 0.0,
            stability_bonus: 0,
            defense_multiplier: 1.0,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::Metals, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::Transplutonics, amount_formula: |size| (size as f64 - 2.0) },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::HeavyMachinery, amount_formula: |size| (size as f64 - 2.0) },
                ResourceAmount { resource: Resource::Ore, amount_formula: |size| (size as f64 + 2.0) },
                ResourceAmount { resource: Resource::TransplutonicOre, amount_formula: |size| size as f64 },
            ],
            special_effects: vec![],
            requirements: vec![],
            is_structure: false,
        });

        // Light Industry
        map.insert("light industry", FacilityData {
            name: "light industry",
            build_cost: 225000,
            build_time: 90,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 4000.0,
            accessibility_bonus: 0.0,
            stability_bonus: 0,
            defense_multiplier: 1.0,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::DomesticGoods, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::LuxuryGoods, amount_formula: |size| (size as f64 - 2.0) },
                ResourceAmount { resource: Resource::Drugs, amount_formula: |size| (size as f64 - 2.0) },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::Organics, amount_formula: |size| size as f64 },
            ],
            special_effects: vec!["Recreational Drugs only produced if colony is a Free Port"],
            requirements: vec![],
            is_structure: false,
        });

        // Heavy Industry
        map.insert("heavy industry", FacilityData {
            name: "heavy industry",
            build_cost: 500000,
            build_time: 120,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 6000.0,
            accessibility_bonus: 0.0,
            stability_bonus: 0,
            defense_multiplier: 1.0,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::HeavyMachinery, amount_formula: |size| (size as f64 - 2.0) },
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| (size as f64 - 2.0) },
                ResourceAmount { resource: Resource::HeavyArmaments, amount_formula: |size| (size as f64 - 2.0) },
                ResourceAmount { resource: Resource::ShipHullsAndWeapons, amount_formula: |size| (size as f64 - 2.0) },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::Metals, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::Transplutonics, amount_formula: |size| (size as f64 - 2.0) },
            ],
            special_effects: vec!["Allow usage of Doctrine Fleet", "Removes Cross-faction imports debuff (-25% ship quality)"],
            requirements: vec![],
            is_structure: false,
        });

        // Aquaculture
        map.insert("aquaculture", FacilityData {
            name: "aquaculture",
            build_cost: 250000,
            build_time: 60,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 1500.0,
            accessibility_bonus: 0.0,
            stability_bonus: 0,
            defense_multiplier: 1.0,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::Food, amount_formula: |size| size as f64 },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::HeavyMachinery, amount_formula: |size| size as f64 },
            ],
            special_effects: vec!["Exclusive to Water planets"],
            requirements: vec!["water"],
            is_structure: false,
        });

        // Commerce
        map.insert("commerce", FacilityData {
            name: "commerce",
            build_cost: 450000,
            build_time: 90,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 1500.0,
            accessibility_bonus: 0.0,
            stability_bonus: -3,
            defense_multiplier: 1.0,
            income_multiplier: 1.25,
            production: vec![],
            demands: vec![],
            special_effects: vec![ "+25% Colony Income", "Allow buying/selling In-colony"],
            requirements: vec![],
            is_structure: false,
        });

        map.insert("Patrol HQ", FacilityData {
            name: "patrol hq",
            build_cost: 300000,
            build_time: 60,
            base_upkeep_formula: |size| 4000.0,
            accessibility_bonus: 0.0,
            stability_bonus: 1,
            defense_multiplier: 1.1,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::Crew, amount_formula: |size| size as f64 },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| (size as f64 - 1.0) },
                ResourceAmount { resource: Resource::Fuel, amount_formula: |size| (size as f64 - 1.0) },
                ResourceAmount { resource: Resource::ShipHullsAndWeapons, amount_formula: |size| (size as f64 - 1.0) },
            ],
            special_effects: vec![],
            requirements: vec![],
            is_structure: true,
        });


        // Military Base
        map.insert("military base", FacilityData {
            name: "military base",
            build_cost: 450000,
            build_time: 120,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 5000.0,
            accessibility_bonus: 0.0,
            stability_bonus: 2,
            defense_multiplier: 1.2,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::Crew, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::Marines, amount_formula: |size| size as f64 },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::Fuel, amount_formula: |size| (size as f64 + 1.0) },
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| (size as f64 + 1.0) },
                ResourceAmount { resource: Resource::ShipHullsAndWeapons, amount_formula: |size| (size as f64 + 1.0) },
            ],
            special_effects: vec![],
            requirements: vec!["patrol hq"],
            is_structure: false,
        });

        // High Command
        map.insert("high command", FacilityData {
            name: "high command",
            build_cost: 150000,
            build_time: 120,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 7000.0,
            accessibility_bonus: 0.0,
            stability_bonus: 2,
            defense_multiplier: 1.3,
            income_multiplier: 1.0,
            production: vec![
                ResourceAmount { resource: Resource::Crew, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::Marines, amount_formula: |size| size as f64 },
            ],
            demands: vec![
                ResourceAmount { resource: Resource::Fuel, amount_formula: |size| (size as f64 + 2.0) },
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| (size as f64 + 2.0) },
                ResourceAmount { resource: Resource::ShipHullsAndWeapons, amount_formula: |size| (size as f64 + 2.0) },
            ],
            special_effects: vec![],
            requirements: vec!["military base"],
            is_structure: false,
        });

        // Waystation
        map.insert("waystation", FacilityData {
            name: "waystation",
            build_cost: 100000,
            build_time: 60,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 1000.0,
            accessibility_bonus: 0.1,
            stability_bonus: 0,
            defense_multiplier: 1.0,
            income_multiplier: 1.0,
            production: vec![],
            demands: vec![
                ResourceAmount { resource: Resource::Fuel, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::Crew, amount_formula: |size| size as f64 },
            ],
            special_effects: vec!["Supplied demand goes into colony stockpile"],
            requirements: vec![],
            is_structure: true,
        });

        // Ground Defenses
        map.insert("ground defenses", FacilityData {
            name: "ground defenses",
            build_cost: 150000,
            build_time: 60,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 1000.0,
            accessibility_bonus: 0.0,
            stability_bonus: 1,
            defense_multiplier: 2.0,
            income_multiplier: 1.0,
            production: vec![],
            demands: vec![
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::Marines, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::HeavyArmaments, amount_formula: |size| (size as f64 - 2.0) },
            ],
            special_effects: vec![],
            requirements: vec![],
            is_structure: true,
        });

        // Heavy Batteries
        map.insert("heavy batteries", FacilityData {
            name: "heavy batteries",
            build_cost: 300000,
            build_time: 90,
            base_upkeep_formula: |size| (size as f64 - 2.0) * 1500.0,
            accessibility_bonus: 0.0,
            stability_bonus: 1,
            defense_multiplier: 3.0,
            income_multiplier: 1.0,
            production: vec![],
            demands: vec![
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::Marines, amount_formula: |size| size as f64 },
                ResourceAmount { resource: Resource::HeavyArmaments, amount_formula: |size| (size as f64 - 2.0) },
            ],
            special_effects: vec![],
            requirements: vec!["ground defenses"],
            is_structure: true,
        });

        // Orbital Station
        map.insert("orbital station", FacilityData {
            name: "orbital station",
            build_cost: 150000,
            build_time: 90,
            base_upkeep_formula: |size| 1500.0,
            accessibility_bonus: 0.0,
            stability_bonus: 1,
            defense_multiplier: 1.5,
            income_multiplier: 1.0,
            production: vec![],
            demands: vec![
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| 3.0 },
                ResourceAmount { resource: Resource::Crew, amount_formula: |size| 3.0 },
            ],
            special_effects: vec!["50% Station CR", "Choices between: Low Tech, Midline, and High Tech"],
            requirements: vec![],
            is_structure: true,
        });

        // Battle Station
        map.insert("battle station", FacilityData {
            name: "battle station",
            build_cost: 500000,
            build_time: 120,
            base_upkeep_formula: |size| 6000.0,
            accessibility_bonus: 0.0,
            stability_bonus: 2,
            defense_multiplier: 2.0,
            income_multiplier: 1.0,
            production: vec![],
            demands: vec![
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| 5.0 },
                ResourceAmount { resource: Resource::Crew, amount_formula: |size| 5.0 },
            ],
            special_effects: vec!["75% Station CR"],
            requirements: vec!["Upgraded from Orbital Station"],
            is_structure: true,
        });

        // Star Fortress
        map.insert("star fortress", FacilityData {
            name: "star fortress",
            build_cost: 1000000,
            build_time: 180,
            base_upkeep_formula: |size| 12500.0,
            accessibility_bonus: 0.0,
            stability_bonus: 3,
            defense_multiplier: 3.0,
            income_multiplier: 1.0,
            production: vec![],
            demands: vec![
                ResourceAmount { resource: Resource::Supplies, amount_formula: |size| 7.0 },
                ResourceAmount { resource: Resource::Crew, amount_formula: |size| 7.0 },
            ],
            special_effects: vec!["100% Station CR"],
            requirements: vec!["Upgraded from Battlestation"],
            is_structure: true,
        });

        // // Cryorevival Facility
        // map.insert("cryorevival facility", FacilityData {
        //     name: "cryorevival facility",
        //     build_cost: 300000,
        //     build_time: 60,
        //     base_upkeep_formula: |size| (size as f64 - 2.0) * 2500.0,
        //     accessibility_bonus: 0.0,
        //     stability_bonus: 0,
        //     defense_multiplier: 1.0,
        //     income_multiplier: 1.0,
        //     production: vec![],
        //     demands: vec![
        //         ResourceAmount { resource: Resource::Organics, amount_formula: |size| 10.0 },
        //     ],
        //     special_effects: vec!["Increase population growth by (colony size * 10)"],
        //     requirements: vec!["Needs to be within 10ly of a Domain-era Cryosleeper"],
        //     is_structure: true,
        // });

        // // Planetary Shield
        // map.insert("planetary shield", FacilityData {
        //     name: "planetary shield",
        //     build_cost: 750000,
        //     build_time: 90,
        //     base_upkeep_formula: |size| 4500.0,
        //     accessibility_bonus: 0.0,
        //     stability_bonus: 0,
        //     defense_multiplier: 3.0,
        //     income_multiplier: 1.0,
        //     production: vec![],
        //     demands: vec![],
        //     special_effects: vec![],
        //     requirements: vec!["Blueprint gained from Story Mission"],
        //     is_structure: true,
        // });

        map
    };
}

lazy_static! {
    pub static ref FACILITY_IMPROVEMENTS: FxHashMap<&'static str, FacilityEffects> = {
        let mut map = FxHashMap::default();
        
        // Commerce
        map.insert("commerce", FacilityEffects {
            income_bonus: 0.25,  // +25% income
            ..Default::default()
        });

        // Spaceport & Megaport
        map.insert("spaceport", FacilityEffects {
            accessibility_bonus: 0.20,  // +20% accessibility
            ..Default::default()
        });
        map.insert("megaport", FacilityEffects {
            accessibility_bonus: 0.20,  // +20% accessibility
            ..Default::default()
        });

        // Waystation
        map.insert("waystation", FacilityEffects {
            accessibility_bonus: 0.20,  // +20% accessibility
            ..Default::default()
        });

        // Orbital Station, Battlestation, Star Fortress
        map.insert("orbital station", FacilityEffects {
            stability_bonus: 1,
            ..Default::default()
        });
        map.insert("battlestation", FacilityEffects {
            stability_bonus: 1,
            ..Default::default()
        });
        map.insert("star fortress", FacilityEffects {
            stability_bonus: 1,
            ..Default::default()
        });

        // Ground Defenses, Heavy Batteries, Planetary Shield
        map.insert("ground defenses", FacilityEffects {
            defense_multiplier: 0.25,  // x1.25 defense
            ..Default::default()
        });
        map.insert("heavy batteries", FacilityEffects {
            defense_multiplier: 0.25,  // x1.25 defense
            ..Default::default()
        });
        map.insert("planetary shield", FacilityEffects {
            defense_multiplier: 0.25,  // x1.25 defense
            ..Default::default()
        });

        // Population & Infrastructure
        map.insert("population", FacilityEffects {
            stability_bonus: 1,
            production_bonus: 1.0,
            ..Default::default()
        });

        // Production facilities with +1 bonus
        for facility in &[
            "aquaculture", "light industry", "refining", 
            "fuel production", "heavy industry", "orbital works",
            "mining"
        ] {
            map.insert(*facility, FacilityEffects {
                production_bonus: 1.0,
                ..Default::default()
            });
        }

        // Farming (special case with +2 production)
        map.insert("farming", FacilityEffects {
            production_bonus: 2.0,
            ..Default::default()
        });

        map
    };

    pub static ref FACILITY_ALPHA_CORES: FxHashMap<&'static str, FacilityEffects> = {
        let mut map = FxHashMap::default();
        
        // Commerce
        map.insert("commerce", FacilityEffects {
            income_bonus: 0.25,  // +25% income
            ..Default::default()
        });

        // Spaceport & Megaport
        map.insert("spaceport", FacilityEffects {
            accessibility_bonus: 0.20,  // +20% accessibility
            ..Default::default()
        });
        map.insert("megaport", FacilityEffects {
            accessibility_bonus: 0.20,  // +20% accessibility
            ..Default::default()
        });

        // Ground Defenses, Heavy Batteries, Planetary Shield
        map.insert("ground defenses", FacilityEffects {
            defense_multiplier: 0.50,  // x1.5 defense
            ..Default::default()
        });
        map.insert("heavy batteries", FacilityEffects {
            defense_multiplier: 0.50,  // x1.5 defense
            ..Default::default()
        });
        map.insert("planetary shield", FacilityEffects {
            defense_multiplier: 0.50,  // x1.5 defense
            ..Default::default()
        });

        // Production facilities with +1 bonus
        for facility in &[
            "aquaculture", "farming", "light industry", "refining",
            "fuel production", "heavy industry", "orbital works",
            "mining", "population"
        ] {
            map.insert(*facility, FacilityEffects {
                production_bonus: 1.0,
                ..Default::default()
            });
        }

        map
    };
}

#[derive(Debug, Clone, Copy)]
pub struct AdminBonuses {
    pub accessibility: f64,  // +10%
    pub fleet_size: f64,    // +20%
    pub defense: f64,       // +50%
    pub stability: i32,     // +1
    pub production: i32,    // +1 unit
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdminType {
    Base,
    AlphaCore,
}

impl AdminType {
    pub fn bonuses(&self) -> AdminBonuses {
        match self {
            Self::Base => AdminBonuses {
                accessibility: 0.0,
                fleet_size: 0.0,
                defense: 0.0,
                stability: 0,
                production: 0,
            },
            Self::AlphaCore => AdminBonuses {
                accessibility: 0.10,
                fleet_size: 0.20,
                defense: 0.50,
                stability: 1,
                production: 1,
            },
        }
    }
}

// Possible colony items for input validation
pub const POSSIBLE_COLONY_ITEMS: [&str; 13] = [
    "soil nanites",
    "biofactory embryo",
    "pristine nanoforge",
    "corrupted nanoforge",
    "mantle bore",
    "catalytic core",
    "synchrotron core",
    "plasma dynamo",
    "cryoarithmetic engine",
    "fullerene spool",
    "fusion lamp",
    "drone replicator",
    "dealmaker holosuite"
];

// Possible facilities
pub const POSSIBLE_FACILITIES: [&str; 22] = [
    "farming",
    "aquaculture",
    "mining",
    "refining",
    "light industry",
    "fuel production",
    "heavy industry",
    "commerce",
    "military base",
    "high command",
    "population",
    "spaceport",
    "megaport",
    "waystation",
    "patrol hq",
    "ground defenses",
    "heavy batteries",
    "orbital station",
    "battle station",
    "star fortress",
    "planetary shield",
    "cryorevival facility",
];

// pop + industries + structures
pub const MAX_FACILITIES: usize = 1 + 4 + 7;
pub const MAX_PRODUCTION: usize = 4;
pub const MAX_DEMANDS: usize = 6;


// File paths for example files
pub const PLANETS_PATH: &str = "planets.csv";
pub const SYSTEMS_PATH: &str = "systems.csv";

#[derive(Debug, Clone)]
pub struct FacilityEffects {
    pub accessibility_bonus: f64,
    pub stability_bonus: i32,
    pub defense_multiplier: f64,
    pub income_bonus: f64,
    pub production_bonus: f64,
    pub fleet_size_multiplier: f64,
}

impl Default for FacilityEffects {
    fn default() -> Self {
        Self {
            accessibility_bonus: 0.0,
            stability_bonus: 0,
            defense_multiplier: 0.0,
            income_bonus: 0.0,
            production_bonus: 0.0,
            fleet_size_multiplier: 0.0,
        }
    }
}
