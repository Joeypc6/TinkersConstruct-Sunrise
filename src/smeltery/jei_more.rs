//! JEIPlugin's alloying, entity melting, severing and part building categories (AlloyRecipeCategory,
//! EntityMeltingRecipeCategory, SeveringCategory, PartBuilderCategory) through the JEI fork's Rust API. The JEI fork
//! draws items and fluids only, so an entity is shown by its spawn egg (Java renders the mob), and a part builder card
//! shows the pattern, the cost and the part (Java also cycles the materials through the part; the part here is the
//! plain item). The modifier category (ModifierRecipeCategory: tools cycling through each modifier recipe's inputs)
//! is here too (inputs around the tools, the tool as the output, the modifier's name and slot).

use super::recipes::{self, fluid_tag};
use super::util::*;
use mod_api::*;
use std::sync::Mutex;

const ALLOY: &str = "tconstruct:alloy";
const ENTITY_MELTING: &str = "tconstruct:entity_melting";
const SEVERING: &str = "tconstruct:severing";
const PART_BUILDER: &str = "tconstruct:part_builder";
const MODIFIERS: &str = "tconstruct:modifiers";
const GRAY: u32 = 0xFF80_8080;

static REGISTERED: Mutex<bool> = Mutex::new(false);
static WORDS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

const KEYS: [(&str, &str); 11] = [
    ("jei.tconstruct.temperature", "%s°C"),
    ("jei.tconstruct.modifiers.slot", "Requires 1 %s slot to apply"),
    ("jei.tconstruct.modifiers.slots", "Requires %s %s slots to apply"),
    ("jei.tconstruct.modifiers.free", "No modifier slots are required to apply"),
    ("jei.tconstruct.modifiers.incremental", "Recipe is incremental, you can apply part of the input for a partial effect"),
    ("jei.tconstruct.modifiers.level.min", "Min level: %s"),
    ("jei.tconstruct.modifiers.level.max", "Max level: %s"),
    ("jei.tconstruct.modifiers.level.range", "For levels: %s-%s"),
    ("jei.tconstruct.modifiers.level.exact", "For level: %s"),
    ("jei.tconstruct.entity_melting.per_heart", "%s mb per heart"),
    ("jei.tconstruct.part_builder.cost", "Cost: %s"),
];

fn word(key: &str) -> String {
    let w = WORDS.lock().unwrap_or_else(|e| e.into_inner());
    w.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).or_else(|| KEYS.iter().find(|(k, _)| *k == key).map(|(_, e)| (*e).to_owned())).unwrap_or_else(|| key.to_owned())
}

pub(crate) fn register(host: &mut dyn ClientHost) {
    let mut done = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
    if *done {
        return;
    }
    *done = true;
    let mut words: Vec<(String, String)> = KEYS.iter().map(|(k, e)| ((*k).to_owned(), lang(&*host, k, e))).collect();
    // The modifiers' names and the slot types' (the recipe function gets no translations).
    for m in crate::tools::modrecipe::jei_modifiers(&*host) {
        let key = format!("modifier.{}", m.result.replace(':', "."));
        let english = words_of(m.result.rsplit(':').next().unwrap_or(""));
        words.push((key.clone(), lang(&*host, &key, &english)));
        if let Some((slot, _)) = &m.slots {
            let k = format!("stat.tconstruct.slot.prefix.{slot}");
            words.push((k.clone(), lang(&*host, &k, &format!("{}:", words_of(slot)))));
            let k = format!("stat.tconstruct.slot.{slot}");
            words.push((k.clone(), lang(&*host, &k, &words_of(slot))));
        }
        if let Some(k) = &m.requirement {
            words.push((k.clone(), lang(&*host, k, "The tool does not meet this modifier's requirements")));
        }
    }
    *WORDS.lock().unwrap_or_else(|e| e.into_inner()) = words;
    use jei::api::*;
    register_category(CategoryDef::new(ALLOY, "jei.tconstruct.alloy.title", "tconstruct:smeltery_controller").catalyst("tconstruct:foundry_controller").catalyst("tconstruct:scorched_alloyer").catalyst("tconstruct:seared_alloyer"));
    register_category(CategoryDef::new(ENTITY_MELTING, "jei.tconstruct.entity_melting.title", "tconstruct:smeltery_controller").catalyst("tconstruct:foundry_controller"));
    register_category(CategoryDef::new(SEVERING, "jei.tconstruct.severing.title", "tconstruct:cleaver"));
    register_category(CategoryDef::new(MODIFIERS, "jei.tconstruct.modifiers.title", "tconstruct:tinker_station").catalyst("tconstruct:tinkers_anvil").catalyst("tconstruct:scorched_anvil"));
    register_category(CategoryDef::new(PART_BUILDER, "jei.tconstruct.part_builder.title", "tconstruct:part_builder"));
    // TinkerStationTransferInfo(MODIFIERS): "+" on a modifier card while a tinker station or an anvil is open.
    register_transfer(crate::tools::station::TINKER_STATION, MODIFIERS);
    register_transfer(crate::tools::station::TINKERS_ANVIL, MODIFIERS);
    register_recipes("tconstruct:more_categories", cards);
}

/// A fluid ingredient's first fluid (a tag's first member).
fn first_fluid(tag: bool, name: &str) -> Option<String> {
    if tag { fluid_tag(name).first().cloned() } else { Some(name.to_owned()) }
}

fn words_of(path: &str) -> String {
    path.split('_').map(|w| { let mut c = w.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }).collect::<Vec<_>>().join(" ")
}

fn spawn_egg(entity: &str) -> String {
    format!("{entity}_spawn_egg")
}

/// The entity types in an entity type tag that have a spawn egg (no API lists a tag's members: each egg's entity is
/// asked).
fn tag_types(host: &dyn Host, tag: &str) -> Vec<String> {
    let mut out: Vec<String> = host.item_keys().into_iter().filter_map(|k| key_id(k).strip_suffix("_spawn_egg").map(str::to_owned)).filter(|t| host.entity_type_in_tag(t, tag)).collect();
    out.sort();
    out
}

fn cards(host: &dyn Host) -> Vec<(String, jei::api::RecipeDef)> {
    use jei::api::{RecipeDef, SlotRole};
    ensure_ids(host);
    let mut out = Vec::new();
    let heat = super::structure::heat_recipes(host);
    // AlloyRecipeCategory (Java's places): alloy.png 0,0 172x62, the inputs' wells across 19,11 (48x32, split evenly),
    // the arrow (alloy.png 172,0) at 90,21, the output's well at 137,11 (16x32), the capacity the largest amount, the
    // temperature centred in 33..172 at y 5.
    for (i, a) in heat.alloys.iter().enumerate() {
        let mut card = RecipeDef::new(&format!("{ALLOY}/{i}"), [172.0, 62.0]).background("tconstruct:gui/jei/alloy", [0.0, 0.0, 172.0, 62.0]);
        let ins: Vec<(String, u32)> = a.inputs.iter().filter_map(|(ingredient, _)| {
            let (tag, name, amount) = ingredient.first()?;
            Some((first_fluid(*tag, name)?, (*amount).max(1) as u32))
        }).collect();
        let cap = ins.iter().map(|(_, n)| *n).chain([a.output.1.max(1) as u32]).max().unwrap_or(1);
        let n = ins.len().max(1) as f32;
        let w = 48.0 / n;
        for (k, (f, amount)) in ins.iter().enumerate() {
            let x0 = (19.0 + k as f32 * w).round();
            let x1 = (19.0 + (k + 1) as f32 * w).round();
            card = card.fluid([x0, 11.0, x1 - x0, 32.0], SlotRole::Input, f, *amount, cap);
        }
        let output = a.output.0.clone();
        card = card.fluid([137.0, 11.0, 16.0, 32.0], SlotRole::Output, &output, a.output.1.max(1) as u32, cap)
            .sprite("tconstruct:gui/jei/alloy", [172.0, 0.0, 24.0, 17.0], 90.0, 21.0);
        let t = word("jei.tconstruct.temperature").replacen("%s", &a.temperature.to_string(), 1);
        let tx = 33.0 + ((139.0 - 6.0 * t.chars().count() as f32) / 2.0).floor();
        card = card.text(tx, 5.0, &t);
        card.texts.last_mut().map(|t| t.2 = GRAY);
        out.push((ALLOY.to_owned(), card));
    }
    // EntityMeltingRecipeCategory: the entity (its spawn egg), the fluid a heart and the damage.
    for (i, m) in heat.melts.iter().enumerate() {
        let Some((fluid, amount)) = m.result.clone() else { continue };
        let mut eggs: Vec<String> = m.types.iter().map(|t| spawn_egg(t)).filter(|e| item_key(e).is_some()).collect();
        for tag in &m.tags {
            for t in tag_types(host, tag) {
                let e = spawn_egg(&t);
                if item_key(&e).is_some() && !eggs.contains(&e) {
                    eggs.push(e);
                }
            }
        }
        if eggs.is_empty() {
            continue;
        }
        eggs.sort();
        let items: Vec<(&str, u32)> = eggs.iter().map(|e| (e.as_str(), 1)).collect();
        let mut card = RecipeDef::new(&format!("{ENTITY_MELTING}/{i}"), [150.0, 62.0])
            .input(27.0, 22.0, &items)
            .fluid([115.0, 11.0, 16.0, 32.0], SlotRole::Output, &fluid, amount.max(1) as u32, 1000)
            .arrow(71.0, 21.0);
        let per = word("jei.tconstruct.entity_melting.per_heart").replacen("%s", &(amount * 2 / m.damage.max(1)).to_string(), 1);
        card = card.text(4.0, 50.0, &per);
        card.texts.last_mut().map(|t| t.2 = GRAY);
        out.push((ENTITY_MELTING.to_owned(), card));
    }
    // SeveringCategory: the entity, the head it drops (and the child's).
    for r in ["tconstruct:severing", "tconstruct:ageable_severing"].iter().flat_map(|k| host.recipes_of_type(k).iter().cloned().collect::<Vec<_>>()) {
        let Some(j) = recipes::chosen(host, &r.json) else { continue };
        let mut eggs = Vec::new();
        let entity = &j["entity"];
        let mut types: Vec<String> = entity["types"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        if let Some(t) = entity["type"].as_str() {
            types.push(t.to_owned());
        }
        if let Some(tag) = entity["tag"].as_str() {
            types.extend(tag_types(host, &recipes::c_tag(tag)));
        }
        for t in types {
            let e = spawn_egg(&t);
            if item_key(&e).is_some() && !eggs.contains(&e) {
                eggs.push(e);
            }
        }
        let res = if j["adult_result"].is_null() { &j["result"] } else { &j["adult_result"] };
        let result = res["item"].as_str().or_else(|| res["id"].as_str()).or_else(|| res.as_str()).unwrap_or("").to_owned();
        if eggs.is_empty() || item_key(&result).is_none() {
            continue;
        }
        let count = res["count"].as_u64().unwrap_or(1) as u32;
        let items: Vec<(&str, u32)> = eggs.iter().map(|e| (e.as_str(), 1)).collect();
        let mut card = RecipeDef::new(&format!("{SEVERING}/{}", r.id.to_string().replace(':', "/")), [100.0, 38.0])
            .input(3.0, 11.0, &items)
            .other(33.0, 11.0, &[("tconstruct:cleaver", 1)])
            .output(77.0, 11.0, &[(result.as_str(), count)]);
        let child = j["child_result"]["item"].as_str().or_else(|| j["child_result"].as_str()).unwrap_or("");
        if item_key(child).is_some() {
            card = card.output(77.0, 29.0, &[(child, j["child_result"]["count"].as_u64().unwrap_or(1) as u32)]);
        }
        out.push((SEVERING.to_owned(), card));
    }
    // PartBuilderCategory: the pattern (the items it takes), the cost, the part.
    let mats = crate::tools::data::material_recipes(host);
    for (i, p) in mats.part_builder.iter().enumerate() {
        let (item, count) = match &p.output {
            crate::tools::data::PartOutput::Part { item, count, .. } => (item.clone(), *count),
            crate::tools::data::PartOutput::Item { result, .. } => result.clone(),
        };
        if item_key(&item).is_none() {
            continue;
        }
        let mut patterns: Vec<&String> = p.pattern_items.iter().collect();
        patterns.sort();
        let pat: Vec<(&str, u32)> = if patterns.is_empty() { vec![("tconstruct:pattern", 1)] } else { patterns.iter().map(|s| (s.as_str(), 1)).collect() };
        let mut card = RecipeDef::new(&format!("{PART_BUILDER}/{i}"), [121.0, 46.0])
            .input(25.0, 16.0, &pat)
            .output(96.0, 15.0, &[(item.as_str(), count)])
            .arrow(57.0, 15.0);
        card = card.text(3.0, 38.0, &word("jei.tconstruct.part_builder.cost").replacen("%s", &p.cost.to_string(), 1));
        card.texts.last_mut().map(|t| t.2 = GRAY);
        out.push((PART_BUILDER.to_owned(), card));
    }
    // ModifierRecipeCategory (AbstractTinkerStationCategory, 128x77, Java's places): five inputs around the tool (an
    // empty one shows its tinker_station.png icon), the tools cycling at 25,38, the result at 105,34, the arrow at
    // 71,33; the modifier's name at 3,3, the level line at y 16 (centred in 43..128), the requirements icon at 66,58
    // and the incremental icon at 83,59 (their texts as tooltips), the slot cost at 102,58 (count, the slot's icon).
    const SPOTS: [[f32; 2]; 5] = [[3.0, 33.0], [25.0, 15.0], [47.0, 33.0], [43.0, 58.0], [7.0, 58.0]];
    const STATION: &str = "tconstruct:gui/jei/tinker_station";
    let centred = |t: &str, from: f32, width: f32| from + ((width - 6.0 * t.chars().count() as f32) / 2.0).max(0.0);
    for m in crate::tools::modrecipe::jei_modifiers(host).iter() {
        if m.tools.is_empty() || m.inputs.iter().any(|v| v.is_empty()) {
            continue;
        }
        let tools: Vec<(&str, u32)> = m.tools.iter().filter(|t| item_key(t).is_some()).map(|t| (t.as_str(), 1)).collect();
        if tools.is_empty() {
            continue;
        }
        // The recipe's own id: "+" sends it back to the server (`tools::transfer`).
        let mut card = RecipeDef::new(&crate::tools::transfer::card_id(&m.id), [128.0, 77.0]);
        for (k, spot) in SPOTS.iter().enumerate() {
            let items: Vec<(&str, u32)> = m.inputs.get(k).map(|input| input.iter().filter(|(x, _)| item_key(x).is_some()).map(|(x, n)| (x.as_str(), *n)).collect()).unwrap_or_default();
            card = card.input(spot[0], spot[1], &items);
            if items.is_empty() {
                card = card.sprite(STATION, [128.0 + (k as f32 + 1.0) * 16.0, 0.0, 16.0, 16.0], spot[0], spot[1]);
            }
        }
        card = card.other(25.0, 38.0, &tools).arrow(71.0, 33.0).output(105.0, 34.0, &tools);
        let key = format!("modifier.{}", m.result.replace(':', "."));
        let w = word(&key);
        let w = if w == key { words_of(m.result.rsplit(':').next().unwrap_or("")) } else { w };
        let name = format!("{w} {}", roman(m.level));
        let name = name.trim();
        card = card.text(centred(name, 3.0, 124.0), 3.0, name);
        let (min, max) = m.levels;
        let fill = |k: &str, a: &[i32]| a.iter().fold(word(k), |t, v| t.replacen("%s", &v.to_string(), 1));
        let line = if min <= 1 && max >= 32767 {
            None
        } else if min == max {
            Some(fill("jei.tconstruct.modifiers.level.exact", &[min]))
        } else if max >= 32767 {
            Some(fill("jei.tconstruct.modifiers.level.min", &[min]))
        } else if min <= 1 {
            Some(fill("jei.tconstruct.modifiers.level.max", &[max]))
        } else {
            Some(fill("jei.tconstruct.modifiers.level.range", &[min, max]))
        };
        if let Some(line) = line {
            card = card.text(centred(&line, 43.0, 85.0), 16.0, &line);
            card.texts.last_mut().map(|t| t.2 = GRAY);
        }
        if let Some(k) = &m.requirement {
            card = card.sprite(STATION, [128.0, 17.0, 16.0, 16.0], 66.0, 58.0).hover([66.0, 58.0, 16.0, 16.0], &[&word(k)]);
        }
        if m.incremental {
            card = card.sprite(STATION, [128.0, 33.0, 16.0, 16.0], 83.0, 59.0).hover([83.0, 59.0, 16.0, 16.0], &[&word("jei.tconstruct.modifiers.incremental")]);
        }
        let cost = match &m.slots {
            Some((slot, n)) if *n > 0 => {
                let sw = word(&format!("stat.tconstruct.slot.{slot}"));
                let sw = if sw.starts_with("stat.") { words_of(slot) } else { sw };
                let count = n.to_string();
                card = card.text(102.0 + 9.0 - 6.0 * count.len() as f32, 63.0, &count);
                card.texts.last_mut().map(|t| t.2 = GRAY);
                // The slot type's icon (creative_slot.json's textures: upgrades -> upgrade, abilities -> ability).
                let icon = match slot.as_str() { "upgrades" => "upgrade", "abilities" => "ability", "defense" => "defense", _ => "default" };
                card = card.sprite(&format!("tconstruct:item/slot/{icon}"), [0.0, 0.0, 16.0, 16.0], 110.0, 58.0);
                if *n == 1 { word("jei.tconstruct.modifiers.slot").replacen("%s", &sw, 1) } else { word("jei.tconstruct.modifiers.slots").replacen("%s", &count, 1).replacen("%s", &sw, 1) }
            }
            _ => {
                card = card.sprite("tconstruct:item/slot/slotless", [0.0, 0.0, 16.0, 16.0], 110.0, 58.0);
                word("jei.tconstruct.modifiers.free")
            }
        };
        card = card.hover([102.0, 58.0, 24.0, 16.0], &[&cost]);
        out.push((MODIFIERS.to_owned(), card));
    }
    let n = |c: &str| out.iter().filter(|(k, _)| k == c).count();
    tlog(&format!("jei cards: {} alloy, {} entity melting, {} severing, {} part builder, {} modifiers", n(ALLOY), n(ENTITY_MELTING), n(SEVERING), n(PART_BUILDER), n(MODIFIERS)));
    out
}

fn roman(n: i32) -> &'static str {
    ["", "", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"].get(n.max(0) as usize).copied().unwrap_or("")
}

/// Tests only (debug on): `config/tconstruct-jei-show.txt` ("<item> [uses]") opens JEI's recipes (or uses) of that item
/// once, then the file goes (tests/jei_layouts.sh screenshots the cards).
pub(crate) fn debug_show() {
    if !debug_on() {
        return;
    }
    let path = std::path::Path::new("config/tconstruct-jei-show.txt");
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let _ = std::fs::remove_file(path);
    let mut words = text.split_whitespace();
    if let Some(item) = words.next() {
        let uses = words.next() == Some("uses");
        jei::api::show(item, uses);
        tlog(&format!("jei show {item} uses={uses}"));
    }
}
