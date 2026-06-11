//! campaign.xml -> RawSave extraction. See SAVE_EXTRACTION_DESIGN.md.
//! PUBLIC SIGNATURE IS PINNED - bin/extract.rs is written against it.

use std::collections::HashMap;

use serde_json::Value;

use crate::extract::model::{PlayerBalance, RawEntity, RawPlanet, RawSave, RawSystem};
use crate::extract::xml::{self, XmlDoc, XmlNode};
use crate::extract::Result;

pub fn scan_save(xml_text: &str) -> Result<RawSave> {
    let doc = xml::parse(xml_text);
    let mut systems = build_systems(&doc);
    let index = build_system_index(&systems);

    for (node_idx, node) in doc.nodes.iter().enumerate() {
        // XStream serializes an object's definition at its first occurrence, which can
        // be under a field tag (e.g. <cL cl="Sstm" z=..>) instead of the class tag, so
        // classify nodes by their `cl` attribute falling back to the tag name.
        match node_class(node) {
            "Plnt" => {
                if let Some(system_z) = resolve_owner_system_z(&doc, node_idx) {
                    if let Some(&idx) = index.get(&system_z) {
                        if let Some(system) = systems.get_mut(idx) {
                            if let Some(planet) = parse_planet(&doc, node_idx, node) {
                                if planet.tags.iter().any(|tag| tag == "star") {
                                    system.star_types.push(planet.planet_type);
                                } else {
                                    system.planets.push(planet);
                                }
                            }
                        }
                    }
                }
            }
            "CCEnt" => {
                if let Some(system_z) = resolve_owner_system_z(&doc, node_idx) {
                    if let Some(&idx) = index.get(&system_z) {
                        if let Some(system) = systems.get_mut(idx) {
                            if let Some(entity) = parse_entity(&doc, node_idx, node) {
                                system.entities.push(entity);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(RawSave {
        systems: systems
            .into_iter()
            .map(|system| system.into_raw())
            .collect(),
        player: scan_player_balance(&doc),
    })
}

/// Player credits/story points/colony items for balance-from-save.
///
/// * credits: the fleet referenced by `<playerFleet ref>` has one direct(ish)
///   `<cargo>` descendant; its non-stack `<c><value>` child is the wallet.
/// * story points: `<characterData>` -> `<person ref>` -> `<stats sp="N">`.
/// * items/alpha cores: `CIStack`s in the player fleet cargo plus every
///   `<Submarket s="storage">` cargo — storage contents are player property
///   regardless of the hosting market. `t="SPECIAL"`/`SpID` stacks carry
///   special-item ids; `alpha_core` commodity stacks count as alpha cores.
fn scan_player_balance(doc: &XmlDoc) -> Option<PlayerBalance> {
    let fleet = doc.nodes.iter().find_map(|node| {
        if node.tag() == "playerFleet" {
            node.attr("ref")
                .and_then(parse_i64)
                .and_then(|z| doc.node_by_z(z))
        } else {
            None
        }
    })?;

    let mut balance = PlayerBalance::default();
    let mut items: HashMap<String, u32> = HashMap::new();

    if let Some(cargo) = find_descendant(doc, fleet, &mut |node| node.tag() == "cargo") {
        balance.credits = cargo_credits(doc, cargo).unwrap_or(0.0);
        collect_cargo_items(doc, cargo, &mut balance.alpha_cores, &mut items);
    }

    // Storage submarkets (skip XStream back-references to already-seen nodes).
    for node in &doc.nodes {
        if node.tag() == "Submarket"
            && node.attr("s") == Some("storage")
            && node.attr("ref").is_none()
        {
            if let Some(cargo) =
                find_descendant(doc, node, &mut |n| n.attr("cl") == Some("CargoData"))
            {
                collect_cargo_items(doc, cargo.resolve(doc), &mut balance.alpha_cores, &mut items);
            }
        }
    }

    balance.story_points = doc
        .nodes
        .iter()
        .find(|node| node.tag() == "characterData")
        .and_then(|cd| cd.child_by_tag(doc, "person"))
        .map(|person| person.resolve(doc))
        .and_then(|person| person.child_by_tag(doc, "stats"))
        .and_then(|stats| stats.attr("sp"))
        .and_then(|sp| sp.parse::<u32>().ok())
        .unwrap_or(0);

    let mut item_list: Vec<(String, u32)> = items.into_iter().collect();
    item_list.sort();
    balance.items = item_list;
    Some(balance)
}

/// Depth-first search of `node`'s subtree (excluding `node` itself) for the
/// first node matching `pred`, in document child order.
fn find_descendant<'a>(
    doc: &'a XmlDoc,
    node: &'a XmlNode,
    pred: &mut dyn FnMut(&XmlNode) -> bool,
) -> Option<&'a XmlNode> {
    for child in node.children(doc) {
        if pred(child) {
            return Some(child);
        }
        if let Some(found) = find_descendant(doc, child, pred) {
            return Some(found);
        }
    }
    None
}

/// The cargo's wallet: a direct `<c>` child (not a stack back-reference) with
/// a `<value>` child.
fn cargo_credits(doc: &XmlDoc, cargo: &XmlNode) -> Option<f64> {
    cargo
        .children(doc)
        .filter(|child| child.tag() == "c" && child.attr("ref").is_none())
        .find_map(|child| child.child_by_tag(doc, "value"))
        .and_then(|value| value.text().trim().parse::<f64>().ok())
}

/// Accumulate special-item and alpha-core counts from a cargo's `<s>` stacks.
fn collect_cargo_items(
    doc: &XmlDoc,
    cargo: &XmlNode,
    alpha_cores: &mut u32,
    items: &mut HashMap<String, u32>,
) {
    let Some(stacks) = cargo.child_by_tag(doc, "s") else {
        return;
    };
    for stack in stacks.children(doc) {
        if stack.tag() != "CIStack" {
            continue;
        }
        let count = stack
            .attr("s")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            .round() as u32;
        if count == 0 {
            continue;
        }
        let Some(data) = stack.child_by_tag(doc, "d") else {
            continue;
        };
        match data.attr("cl") {
            Some("st") if data.text().trim() == "alpha_core" => *alpha_cores += count,
            Some("SpID") => {
                if let Some(id) = data.attr("i") {
                    if crate::constants::ColonyItem::from_save_id(id).is_some() {
                        *items.entry(id.to_string()).or_insert(0) += count;
                    }
                }
            }
            _ => {}
        }
    }
}

struct SystemBuilder {
    z: i64,
    name: String,
    display_name: String,
    internal_id: String,
    hyper_loc: Option<(f64, f64)>,
    tags: Vec<String>,
    star_types: Vec<String>,
    planets: Vec<RawPlanet>,
    entities: Vec<RawEntity>,
}

impl SystemBuilder {
    fn into_raw(self) -> RawSystem {
        RawSystem {
            name: self.name,
            display_name: self.display_name,
            internal_id: self.internal_id,
            hyper_loc: self.hyper_loc,
            tags: self.tags,
            star_types: self.star_types,
            planets: self.planets,
            entities: self.entities,
        }
    }
}

fn build_systems(doc: &XmlDoc) -> Vec<SystemBuilder> {
    let mut systems = Vec::new();

    for node in &doc.nodes {
        if node_class(node) != "Sstm" || node.attr("ref").is_some() {
            continue;
        }

        let Some(z) = node.attr("z").and_then(parse_i64) else {
            continue;
        };

        let name = node.attr("bN").unwrap_or_default().to_string();
        let display_name = node.attr("dN").unwrap_or_default().to_string();
        let internal_id = extract_system_internal_id(doc, node, z);
        let tags = collect_tag_list(doc, node.child_by_tag(doc, "tags"));
        let star_types = Vec::new();
        let hyper_loc = extract_hyper_loc(doc, node);

        systems.push(SystemBuilder {
            z,
            name,
            display_name,
            internal_id,
            hyper_loc,
            tags,
            star_types,
            planets: Vec::new(),
            entities: Vec::new(),
        });
    }

    systems
}

fn build_system_index(systems: &[SystemBuilder]) -> HashMap<i64, usize> {
    let mut index = HashMap::new();
    for (idx, system) in systems.iter().enumerate() {
        index.insert(system.z, idx);
    }
    index
}

fn extract_system_internal_id(doc: &XmlDoc, node: &XmlNode, z: i64) -> String {
    let Some(j0) = node.child_by_tag(doc, "j0") else {
        return z.to_string();
    };
    let Ok(json) = serde_json::from_str::<Value>(j0.text()) else {
        return z.to_string();
    };
    json.get("f4")
        .and_then(json_value_to_string)
        .map(str::to_owned)
        .unwrap_or_else(|| z.to_string())
}

fn extract_hyper_loc(doc: &XmlDoc, node: &XmlNode) -> Option<(f64, f64)> {
    let h_a = node.child_by_tag(doc, "hA")?;
    let resolved = h_a.resolve(doc);
    let loc = resolved
        .child_by_tag(doc, "loc")
        .or_else(|| h_a.child_by_tag(doc, "loc"))?;
    let text = loc.text().trim();
    let (x, y) = text.split_once('|')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

fn parse_planet(doc: &XmlDoc, _node_idx: usize, node: &XmlNode) -> Option<RawPlanet> {
    let tags = collect_tag_list(doc, node.child_by_tag(doc, "tags"));
    let is_star = tags.iter().any(|tag| tag == "star");
    let is_planet = tags.iter().any(|tag| tag == "planet");

    if !is_star && !is_planet {
        return None;
    }

    let j0 = node.child_by_tag(doc, "j0")?;
    let json: Value = serde_json::from_str(j0.text()).ok()?;
    let name = json.get("f0").and_then(json_value_to_string)?.to_string();
    let planet_type = node.child_by_tag(doc, "type")?.text().trim().to_string();
    let internal_id = json
        .get("f4")
        .and_then(json_value_to_string)
        .map(|s| s.to_string());
    let radius = node
        .child_by_tag(doc, "radius")
        .and_then(|n| n.text().trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    let conditions = extract_conditions(doc, node);
    let survey_level = market_node(doc, node)
        .and_then(|market| market.child_by_tag(doc, "surveyLevel"))
        .map(|n| n.text().trim().to_string())
        .filter(|s| !s.is_empty());
    let market_size = market_size(doc, node);
    let owner_faction = extract_owner_faction(doc, node);
    let is_moon = extract_is_moon(doc, node);

    Some(RawPlanet {
        name,
        internal_id,
        planet_type,
        radius,
        tags,
        conditions,
        survey_level,
        owner_faction,
        market_size,
        is_moon,
    })
}

fn parse_entity(doc: &XmlDoc, _node_idx: usize, node: &XmlNode) -> Option<RawEntity> {
    let j0 = node.child_by_tag(doc, "j0")?;
    let json: Value = serde_json::from_str(j0.text()).ok()?;
    let spec_id = json.get("f3").and_then(json_value_to_string)?.to_string();
    let name = json
        .get("f0")
        .and_then(json_value_to_string)
        .map(|s| s.to_string());

    Some(RawEntity { spec_id, name })
}

fn resolve_owner_system_z(doc: &XmlDoc, node_idx: usize) -> Option<i64> {
    let node = doc.node(node_idx);

    if let Some(c_l) = node.child_by_tag(doc, "cL") {
        if c_l.attr("cl") == Some("Hyperspace") {
            return None;
        }

        let resolved = c_l.resolve(doc);
        if node_class(resolved) == "Sstm" {
            return resolved.attr("z").and_then(parse_i64);
        }
    }

    let mut current = node.parent();
    while let Some(idx) = current {
        let ancestor = doc.node(idx);
        if node_class(ancestor) == "Sstm" {
            if ancestor.attr("ref").is_none() {
                return ancestor.attr("z").and_then(parse_i64);
            }
        }
        current = ancestor.parent();
    }

    None
}

fn market_node<'a>(doc: &'a XmlDoc, node: &'a XmlNode) -> Option<&'a XmlNode> {
    let market = node.child_by_tag(doc, "market")?;
    Some(market.resolve(doc))
}

fn market_size(doc: &XmlDoc, node: &XmlNode) -> u32 {
    let Some(market) = market_node(doc, node) else {
        return 0;
    };

    match market.attr("cl") {
        Some("PCMarket") => 0,
        _ => market
            .child_by_tag(doc, "size")
            .and_then(|n| n.text().trim().parse::<u32>().ok())
            .or_else(|| {
                market
                    .attr("size")
                    .and_then(|value| value.parse::<u32>().ok())
            })
            .unwrap_or(0),
    }
}

fn extract_conditions(doc: &XmlDoc, node: &XmlNode) -> Vec<String> {
    let Some(market) = market_node(doc, node) else {
        return Vec::new();
    };

    // Uncolonized PCMarket: plain <cond><st>id</st>... list.
    let from_cond: Vec<String> = market
        .child_by_tag(doc, "cond")
        .map(|cond| {
            cond.children(doc)
                .filter(|child| child.tag() == "st")
                .map(|child| child.text().trim().to_string())
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if !from_cond.is_empty() {
        return from_cond;
    }

    // Colonized Market: <conditions> holds MCon elements with the id in `i`
    // (some are refs to MCons defined elsewhere — resolve them).
    market
        .child_by_tag(doc, "conditions")
        .map(|conditions| {
            conditions
                .children(doc)
                .map(|child| child.resolve(doc))
                .filter_map(|child| child.attr("i"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn extract_owner_faction(doc: &XmlDoc, node: &XmlNode) -> Option<String> {
    let ow = node.child_by_tag(doc, "ow")?;
    let resolved = ow.resolve(doc);
    resolved
        .child_by_tag(doc, "id")
        .map(|id| id.text().trim().to_string())
        .filter(|id| !id.is_empty())
}

fn extract_is_moon(doc: &XmlDoc, node: &XmlNode) -> bool {
    // A moon orbits another planet: resolve the orbit focus and check that it
    // is a Plnt whose tags do not include `star`.
    let Some(orbit) = node.child_by_tag(doc, "orbit") else {
        return false;
    };
    let Some(focus) = orbit.child_by_tag(doc, "f") else {
        return false;
    };
    let resolved = focus.resolve(doc);
    if node_class(resolved) != "Plnt" {
        return false;
    }
    let tags = collect_tag_list(doc, resolved.child_by_tag(doc, "tags"));
    !tags.iter().any(|tag| tag == "star")
}

fn collect_tag_list(doc: &XmlDoc, node: Option<&XmlNode>) -> Vec<String> {
    let Some(node) = node else {
        return Vec::new();
    };

    node.children(doc)
        .filter(|child| child.tag() == "st")
        .map(|child| child.text().trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_i64(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}

/// XStream class of a node: the `cl` attribute when present, else the tag name.
fn node_class<'a>(node: &'a XmlNode) -> &'a str {
    node.attr("cl").unwrap_or_else(|| node.tag())
}

fn json_value_to_string(value: &Value) -> Option<&str> {
    value.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI_SAVE: &str = r#"
<SaveGameData>
  <Sstm z="100" dN="Mini System" bN="Mini" ty="SINGLE">
    <j0>{"f0":"Mini System","f4":"mini_system"}</j0>
    <hA cl="LocationToken" ref="101" />
    <tags>
      <st>theme_remnant</st>
    </tags>
    <o>
      <saved>
        <Plnt z="200">
          <j0>{"f0":"Mini Star","f4":"mini_star"}</j0>
          <type>star_white</type>
          <tags>
            <st>star</st>
          </tags>
          <cL cl="Sstm" ref="100" />
        </Plnt>
        <Plnt z="201">
          <j0>{"f0":"Mini Planet","f4":"mini_planet"}</j0>
          <type>barren</type>
          <radius>250.0</radius>
          <tags>
            <st>planet</st>
          </tags>
          <cL cl="Sstm" ref="100" />
          <ow>
            <id>hegemony</id>
          </ow>
          <orbit>
            <f cl="Plnt" ref="200" />
          </orbit>
          <market cl="PCMarket">
            <surveyLevel>FULL</surveyLevel>
            <cond>
              <st>ore_abundant</st>
              <st>volatiles_trace</st>
            </cond>
          </market>
        </Plnt>
        <Plnt z="202">
          <j0>{"f0":"Mini Moon","f4":"mini_moon"}</j0>
          <type>barren</type>
          <radius>80.0</radius>
          <tags>
            <st>planet</st>
          </tags>
          <cL cl="Sstm" ref="100" />
          <orbit>
            <f cl="Plnt" ref="201" />
          </orbit>
        </Plnt>
        <CCEnt z="300">
          <j0>{"f0":"Relay","f3":"comm_relay","f4":"relay_1"}</j0>
          <cL cl="Sstm" ref="100" />
        </CCEnt>
      </saved>
    </o>
  </Sstm>
  <LocationToken z="101">
    <loc>12.5|34.5</loc>
  </LocationToken>
  <e cl="Flt" z="500" n="Player Fleet">
    <cargo z="501">
      <s z="502">
        <CIStack z="503" s="2.0" t="RESOURCES">
          <d cl="st">alpha_core</d>
          <c ref="501"></c>
        </CIStack>
        <CIStack z="504" s="1.0" t="SPECIAL">
          <d cl="SpID" z="505" i="corrupted_nanoforge"></d>
          <c ref="501"></c>
        </CIStack>
        <CIStack z="506" s="1.0" t="SPECIAL">
          <d cl="SpID" z="507" i="modspec" d="ecm"></d>
          <c ref="501"></c>
        </CIStack>
      </s>
      <c z="508">
        <value>123456.0</value>
      </c>
    </cargo>
  </e>
  <playerFleet ref="500"></playerFleet>
  <characterData z="510">
    <person ref="511"></person>
  </characterData>
  <commander cl="Person" z="511" fid="player">
    <stats z="512" sp="7"></stats>
  </commander>
  <Submarket z="520" s="storage">
    <p z="521">
      <c cl="CargoData" z="522">
        <s z="523">
          <CIStack z="524" s="1.0" t="SPECIAL">
            <d cl="SpID" z="525" i="synchrotron"></d>
            <c ref="522"></c>
          </CIStack>
          <CIStack z="526" s="1.0" t="RESOURCES">
            <d cl="st">alpha_core</d>
            <c ref="522"></c>
          </CIStack>
        </s>
      </c>
    </p>
  </Submarket>
</SaveGameData>
"#;

    /// Player credits come from the player fleet's cargo wallet; story points
    /// from characterData->person stats; colony items and alpha cores are
    /// summed over the fleet cargo and storage submarkets, with non-colony
    /// SPECIAL stacks (modspecs) ignored.
    #[test]
    fn extracts_player_balance() {
        let save = scan_save(MINI_SAVE).unwrap();
        let player = save.player.expect("player balance extracted");
        assert_eq!(player.credits, 123456.0);
        assert_eq!(player.story_points, 7);
        assert_eq!(player.alpha_cores, 3);
        assert_eq!(
            player.items,
            vec![
                ("corrupted_nanoforge".to_string(), 1),
                ("synchrotron".to_string(), 1),
            ]
        );
    }

    #[test]
    fn extracts_systems_planets_and_entities() {
        let save = scan_save(MINI_SAVE).unwrap();
        assert_eq!(save.systems.len(), 1);
        let system = &save.systems[0];
        assert_eq!(system.name, "Mini");
        assert_eq!(system.display_name, "Mini System");
        assert_eq!(system.internal_id, "mini_system");
        assert_eq!(system.hyper_loc, Some((12.5, 34.5)));
        assert_eq!(system.tags, vec!["theme_remnant"]);
        assert_eq!(system.star_types, vec!["star_white"]);
        assert_eq!(system.planets.len(), 2);
        assert_eq!(system.planets[0].name, "Mini Planet");
        assert_eq!(system.planets[0].market_size, 0);
        assert_eq!(system.planets[0].owner_faction.as_deref(), Some("hegemony"));
        // Mini Planet orbits the star, so it is not a moon; Mini Moon orbits it.
        assert!(!system.planets[0].is_moon);
        assert_eq!(
            system.planets[0].conditions,
            vec!["ore_abundant", "volatiles_trace"]
        );
        assert_eq!(system.planets[1].name, "Mini Moon");
        assert!(system.planets[1].is_moon);
        assert_eq!(system.entities.len(), 1);
        assert_eq!(system.entities[0].spec_id, "comm_relay");
    }

    /// Requires a local Starsector install with at least one save; run with
    /// `cargo test -- --ignored`. The install is found via STARSECTOR_DIR or
    /// common install locations (see `extract::locate`).
    #[test]
    #[ignore]
    fn scan_real_campaign_counts() {
        use crate::extract::locate;
        use crate::extract::save::{discover_saves, load_campaign_xml};

        let install = locate::detect_starsector_dir().expect(
            "ignored test requires a local Starsector install; set STARSECTOR_DIR \
             or install in a common location",
        );
        let saves = discover_saves(&locate::default_saves_dir(&install))
            .expect("failed to read the install's saves directory");
        // discover_saves sorts most-recently-modified first.
        let save = saves
            .first()
            .expect("ignored test requires at least one save in <install>/saves");
        let xml = load_campaign_xml(save).expect("failed to load campaign.xml");
        let save = scan_save(&xml).unwrap();
        let system_count = save.systems.len();
        let planet_count: usize = save.systems.iter().map(|system| system.planets.len()).sum();
        let entity_count: usize = save
            .systems
            .iter()
            .map(|system| system.entities.len())
            .sum();
        let comm_relay = save.systems.iter().any(|system| {
            system.entities.iter().any(|entity| {
                entity.spec_id == "comm_relay" || entity.spec_id == "comm_relay_makeshift"
            })
        });

        eprintln!(
            "systems={system_count} planets={planet_count} entities={entity_count} comm_relay={comm_relay}"
        );

        // Tolerant thresholds: a vanilla sector has well over 50 systems and
        // 200 planets; modded sectors have far more.
        assert!(system_count > 50);
        assert!(planet_count > 200);
        assert!(entity_count > 0);
        assert!(comm_relay);
        assert!(save.systems.iter().all(|system| !system.name.is_empty()));
    }
}
