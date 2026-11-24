//! Materials and tool parts (spec-tools.md 1, 2 and 7; upstream `tconstruct.rs` 7080-8216): MaterialManager,
//! MaterialStatsManager and MaterialTraitsManager over the jar's material files (compiled in: `gen_tools.rs`), the
//! material ids (MaterialVariantId), the part items (ToolPartItem, RepairKitItem), their names, the material recipes
//! (`tconstruct:material`), the part builder's (PartRecipe, ItemPartRecipe), material casting and melting
//! (MaterialFluidRecipe, MaterialCastingLookup), the station layouts and the tool definitions.
//!
//! A part is its item with `minecraft:custom_data` {Material: "ns:path[#variant]"} (Java's root NBT `Material`; the
//! generated item models select on it), plus its `minecraft:item_name` ("Iron Pick Head").

use crate::smeltery::casting_recipes::{fluid_amount, fluid_ingredient, item_output, tag_items};
use crate::smeltery::recipes::{c_tag, chosen, condition_holds, ingredient_items};
use crate::smeltery::util::*;
use mod_api::serde_json::{self, Value};
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

/// MaterialRegistry's stat types: id, trait fallback, whether the type is repairable (IRepairableMaterialStats).
pub(crate) const STAT_TYPES: [(&str, Option<&str>, bool); 23] = [
    ("tconstruct:head", Some("tconstruct:melee_harvest"), true),
    ("tconstruct:handle", Some("tconstruct:melee_harvest"), false),
    ("tconstruct:binding", Some("tconstruct:melee_harvest"), false),
    ("tconstruct:limb", Some("tconstruct:ranged"), true),
    ("tconstruct:grip", Some("tconstruct:ranged"), false),
    ("tconstruct:bowstring", Some("tconstruct:ranged"), false),
    ("tconstruct:plating_helmet", Some("tconstruct:armor"), true),
    ("tconstruct:plating_chestplate", Some("tconstruct:armor"), true),
    ("tconstruct:plating_leggings", Some("tconstruct:armor"), true),
    ("tconstruct:plating_boots", Some("tconstruct:armor"), true),
    ("tconstruct:plating_shield", Some("tconstruct:armor"), true),
    ("tconstruct:cuirass", Some("tconstruct:armor"), false),
    ("tconstruct:maille", Some("tconstruct:armor"), false),
    ("tconstruct:shield_core", Some("tconstruct:armor"), false),
    ("tconstruct:slime", Some("tconstruct:armor"), true),
    ("tconstruct:skull", Some("tconstruct:armor"), true),
    ("tconstruct:ribcage", None, true),
    ("tconstruct:shell", Some("tconstruct:armor"), true),
    ("tconstruct:laces", Some("tconstruct:armor"), true),
    ("tconstruct:arrow_head", Some("tconstruct:ammo"), false),
    ("tconstruct:arrow_shaft", Some("tconstruct:ammo"), false),
    ("tconstruct:fletching", Some("tconstruct:ammo"), false),
    ("tconstruct:repair_kit", None, false),
];

/// TierSortingRegistry's order of the vanilla tiers: wood < gold < stone < iron < diamond < netherite.
pub(crate) const TIERS: [&str; 6] = ["minecraft:wood", "minecraft:gold", "minecraft:stone", "minecraft:iron", "minecraft:diamond", "minecraft:netherite"];

pub(crate) const UNKNOWN_MATERIAL: &str = "tconstruct:unknown";

/// A modifier and its level (ModifierEntry).
pub(crate) type Trait = (String, i32);
/// A material id and its variant ("" for none).
pub(crate) type MatId = (String, String);

/// A material's stats of one stat type (HeadMaterialStats, HandleMaterialStats... StatlessMaterialStats).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Stats {
    Head { durability: i32, mining_speed: f32, tier: usize, attack: f32 },
    Handle { durability: f32, mining_speed: f32, melee_speed: f32, melee_damage: f32 },
    Limb { durability: i32, draw_speed: f32, velocity: f32, accuracy: f32 },
    Grip { durability: f32, accuracy: f32, melee_damage: f32 },
    Plating { durability: i32, armor: f32, toughness: f32, knockback: f32 },
    Slime { durability: i32, overslime: i32 },
    Skull { durability: i32, armor: i32 },
    Repair { amount: i32 },
    Statless,
}

fn stat_float(o: &Value, key: &str, default: f32, min: Option<f32>) -> Option<f32> {
    match o.get(key) {
        None | Some(Value::Null) => Some(default),
        Some(v) => {
            let f = v.as_f64()? as f32;
            min.is_none_or(|m| f >= m).then_some(f)
        }
    }
}

pub(crate) fn stat_int(o: &Value, key: &str, default: Option<i32>, min: i32) -> Option<i32> {
    match o.get(key) {
        None | Some(Value::Null) => default,
        Some(v) => {
            let i = v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))?;
            (i >= i64::from(min) && i <= i64::from(i32::MAX)).then_some(i as i32)
        }
    }
}

pub(crate) fn pattern_id(s: &str) -> String {
    if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") }
}

/// MaterialStatsManager: one stat type's JSON; none for an unknown type or a field out of range.
pub(crate) fn parse_stats(stat: &str, o: &Value) -> Option<Stats> {
    if !o.is_object() {
        return None;
    }
    Some(match stat {
        "tconstruct:head" => {
            let tier = pattern_id(o.get("mining_tier").and_then(Value::as_str).unwrap_or("minecraft:wood"));
            Stats::Head {
                durability: stat_int(o, "durability", None, 1)?,
                mining_speed: stat_float(o, "mining_speed", 1.0, Some(0.0))?,
                tier: TIERS.iter().position(|t| *t == tier)?,
                attack: stat_float(o, "melee_attack", 1.0, Some(0.0))?,
            }
        }
        "tconstruct:handle" => Stats::Handle {
            durability: stat_float(o, "durability", 0.0, None)?,
            mining_speed: stat_float(o, "mining_speed", 0.0, None)?,
            melee_speed: stat_float(o, "melee_speed", 0.0, None)?,
            melee_damage: stat_float(o, "melee_damage", 0.0, None)?,
        },
        "tconstruct:limb" => Stats::Limb {
            durability: stat_int(o, "durability", None, 1)?,
            draw_speed: stat_float(o, "draw_speed", 0.0, None)?,
            velocity: stat_float(o, "velocity", 0.0, None)?,
            accuracy: stat_float(o, "accuracy", 0.0, None)?,
        },
        "tconstruct:grip" => Stats::Grip { durability: stat_float(o, "durability", 0.0, None)?, accuracy: stat_float(o, "accuracy", 0.0, None)?, melee_damage: stat_float(o, "melee_damage", 0.0, Some(0.0))? },
        "tconstruct:plating_helmet" | "tconstruct:plating_chestplate" | "tconstruct:plating_leggings" | "tconstruct:plating_boots" | "tconstruct:plating_shield" => Stats::Plating {
            durability: stat_int(o, "durability", None, 1)?,
            armor: if stat == "tconstruct:plating_shield" { 0.0 } else { stat_float(o, "armor", 0.0, Some(0.0))? },
            toughness: stat_float(o, "toughness", 0.0, Some(0.0))?,
            knockback: stat_float(o, "knockback_resistance", 0.0, Some(0.0))?,
        },
        "tconstruct:slime" => Stats::Slime { durability: stat_int(o, "durability", None, 1)?, overslime: stat_int(o, "overslime_capacity", None, 0)? },
        "tconstruct:skull" => Stats::Skull { durability: stat_int(o, "durability", None, 1)?, armor: stat_int(o, "armor", Some(0), 0)? },
        "tconstruct:ribcage" | "tconstruct:shell" | "tconstruct:laces" => Stats::Repair { amount: stat_int(o, "repair_amount", None, 1)? },
        _ if STAT_TYPES.iter().any(|(id, _, _)| *id == stat) => Stats::Statless,
        _ => return None,
    })
}

/// The stats a material lacking the stat type gives (MaterialStatsManager.getStatsOrDefault).
pub(crate) fn default_stats(stat: &str) -> Option<Stats> {
    let fields = match stat {
        "tconstruct:handle" | "tconstruct:grip" => serde_json::json!({}),
        "tconstruct:ribcage" | "tconstruct:shell" | "tconstruct:laces" => serde_json::json!({"repair_amount": 1}),
        "tconstruct:slime" => serde_json::json!({"durability": 1, "overslime_capacity": 0}),
        _ => serde_json::json!({"durability": 1}),
    };
    parse_stats(stat, &fields)
}

/// MaterialManager.loadMaterial's fields, its stats and traits.
#[allow(dead_code)]
pub(crate) struct Material {
    pub craftable: bool,
    pub hidden: bool,
    pub tier: i32,
    pub sort: i32,
    pub rarity: Option<String>,
    pub stats: HashMap<String, Stats>,
    pub traits_default: Vec<Trait>,
    pub traits_per_stat: HashMap<String, Vec<Trait>>,
}

impl Material {
    /// MaterialTraits.getTraits.
    pub(crate) fn traits(&self, stat: &str) -> &[Trait] {
        self.traits_per_stat.get(stat).map_or(&self.traits_default[..], Vec::as_slice)
    }

    /// IMaterial.getRarity as an ordinal: common 0, uncommon 1, rare 2, epic 3 (from the tier when unset).
    pub(crate) fn rarity_level(&self) -> i32 {
        match self.rarity.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("epic") => 3,
            Some("rare") => 2,
            Some("uncommon") => 1,
            Some("common") => 0,
            _ => match self.tier {
                5.. => 3,
                4 => 2,
                3 => 1,
                _ => 0,
            },
        }
    }
}

#[derive(Default)]
pub(crate) struct Materials {
    pub map: HashMap<String, Material>,
    pub redirects: HashMap<String, String>,
    pub tags: HashMap<String, HashSet<String>>,
}

impl Materials {
    pub(crate) fn resolve<'a>(&'a self, id: &'a str) -> &'a str {
        self.redirects.get(id).map_or(id, String::as_str)
    }
    pub(crate) fn get(&self, id: &str) -> Option<&Material> {
        self.map.get(self.resolve(id))
    }
    pub(crate) fn stats(&self, id: &str, stat: &str) -> Option<&Stats> {
        self.get(id)?.stats.get(stat)
    }
    pub(crate) fn in_tag(&self, id: &str, tag: &str) -> bool {
        self.tags.get(tag).is_some_and(|t| t.contains(self.resolve(id)))
    }
}

/// The jar's tables (tools/tool_data.py, compiled in as `gen_tools::TOOLS_JSON`).
pub(crate) struct Table {
    pub json: Value,
    pub colors: HashMap<String, u32>,
    pub render: HashMap<String, u32>,
}

pub(crate) fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        let json: Value = serde_json::from_str(crate::gen_tools::TOOLS_JSON).unwrap_or_default();
        let colors = json["colors"].as_object().map(|o| o.iter().filter_map(|(k, c)| Some((k.clone(), c.as_u64()? as u32))).collect()).unwrap_or_default();
        let r: Value = serde_json::from_str(crate::gen_colors::MATERIAL_COLORS).unwrap_or_default();
        let render = r.as_object().map(|o| o.iter().filter_map(|(k, c)| Some((k.clone(), c.as_u64()? as u32))).collect()).unwrap_or_default();
        Table { json, colors, render }
    })
}

fn parse_traits(v: &Value) -> Vec<Trait> {
    v.as_array().map(|a| a.iter().filter_map(|e| Some((e["name"].as_str()?.to_owned(), stat_int(e, "level", Some(1), 1)?))).collect()).unwrap_or_default()
}

fn build_materials(host: &dyn Host) -> Materials {
    let mut out = Materials::default();
    let t = &table().json;
    let holds = |c: &Value| c.is_null() || condition_holds(host, c);
    for (id, doc) in t["definition"].as_object().into_iter().flatten() {
        if let Some(target) = doc["redirect"].as_array().and_then(|r| r.iter().find(|e| holds(&e["condition"]))).and_then(|e| e["id"].as_str()) {
            out.redirects.insert(id.clone(), target.to_owned());
            continue;
        }
        if !holds(&doc["condition"]) {
            continue;
        }
        let Some(craftable) = doc["craftable"].as_bool() else { continue };
        let int = |k: &str, d: i64| doc[k].as_i64().unwrap_or(d).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        out.map.insert(id.clone(), Material {
            craftable,
            hidden: doc["hidden"].as_bool().unwrap_or(false),
            tier: int("tier", 0),
            sort: int("sortOrder", 100),
            rarity: doc["rarity"].as_str().map(str::to_owned),
            stats: HashMap::new(),
            traits_default: Vec::new(),
            traits_per_stat: HashMap::new(),
        });
    }
    let known: HashSet<String> = out.map.keys().cloned().collect();
    out.redirects.retain(|_, to| known.contains(to));
    for (id, doc) in t["stats"].as_object().into_iter().flatten() {
        let Some(m) = out.map.get_mut(id) else { continue };
        for (stat, fields) in doc["stats"].as_object().into_iter().flatten() {
            if let Some(s) = parse_stats(stat, fields) {
                m.stats.insert(stat.clone(), s);
            }
        }
    }
    for (id, doc) in t["traits"].as_object().into_iter().flatten() {
        let Some(m) = out.map.get_mut(id) else { continue };
        m.traits_default = parse_traits(&doc["default"]);
        m.traits_per_stat = doc["perStat"].as_object().map(|o| o.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), parse_traits(v))).collect()).unwrap_or_default();
        if !m.traits_per_stat.is_empty() {
            for (stat, fallback, _) in STAT_TYPES {
                if let Some(f) = fallback.and_then(|f| m.traits_per_stat.get(f).cloned()) {
                    m.traits_per_stat.entry(stat.to_owned()).or_insert(f);
                }
            }
        }
    }
    let raw: HashMap<&str, &Value> = t["tags"].as_object().map(|o| o.iter().map(|(k, v)| (k.as_str(), v)).collect()).unwrap_or_default();
    fn members(raw: &HashMap<&str, &Value>, tag: &str, seen: &mut HashSet<String>, out: &mut HashSet<String>) {
        if !seen.insert(tag.to_owned()) {
            return;
        }
        for v in raw.get(tag).and_then(|t| t["values"].as_array()).into_iter().flatten() {
            let Some(s) = v.as_str().or_else(|| v["id"].as_str()) else { continue };
            match s.strip_prefix('#') {
                Some(t) => members(raw, t, seen, out),
                None => {
                    out.insert(s.to_owned());
                }
            }
        }
    }
    for tag in raw.keys() {
        let mut set = HashSet::new();
        members(&raw, tag, &mut HashSet::new(), &mut set);
        out.tags.insert((*tag).to_owned(), set);
    }
    out
}

/// A cache key: the recipe tables (and so the tags their conditions read) of the current data load.
pub(crate) type Key = Vec<(usize, usize)>;

pub(crate) fn recipes_key(host: &dyn Host, kinds: &[&str]) -> Key {
    crate::smeltery::util::tables_key(host, kinds)
}

static MATERIALS: Mutex<Option<(Key, Arc<Materials>)>> = Mutex::new(None);

/// The material registry, parsed again on a new data load (its conditions read the game's tags).
pub(crate) fn materials(host: &dyn Host) -> Arc<Materials> {
    ensure_ids(host);
    let key = recipes_key(host, &["tconstruct:material"]);
    if let Some(m) = MATERIALS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, m)| m.clone()) {
        return m;
    }
    let out = Arc::new(build_materials(host));
    *MATERIALS.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, out.clone()));
    out
}

/// The last material registry read (for a command argument's parser, which has no host); None before any.
pub(crate) fn cached_materials() -> Option<Arc<Materials>> {
    MATERIALS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(_, m)| m.clone())
}

// ---- Material ids (MaterialVariantId)

pub(crate) fn parse_material(s: &str) -> Option<MatId> {
    let (id, variant) = s.split_once('#').unwrap_or((s, ""));
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    let ok = |s: &str, extra: &str| s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.".contains(c) || extra.contains(c));
    if ns.is_empty() || path.is_empty() || !ok(ns, "") || !ok(path, "/") || !ok(variant, "/") {
        return None;
    }
    let variant = if variant == "default" { "" } else { variant };
    Some((format!("{ns}:{path}"), variant.to_owned()))
}

pub(crate) fn material_string(m: &MatId) -> String {
    if m.1.is_empty() { m.0.clone() } else { format!("{}#{}", m.0, m.1) }
}

pub(crate) fn matches_variant(want: &MatId, have: &MatId) -> bool {
    want.0 == have.0 && (want.1.is_empty() || want.1 == have.1)
}

// ---- The part items

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum PartKind {
    Tool(&'static str),
    RepairKit,
    Fake(&'static str, f32),
}

pub(crate) const PARTS: [(&str, PartKind); 30] = [
    ("tconstruct:pick_head", PartKind::Tool("tconstruct:head")),
    ("tconstruct:hammer_head", PartKind::Tool("tconstruct:head")),
    ("tconstruct:small_axe_head", PartKind::Tool("tconstruct:head")),
    ("tconstruct:broad_axe_head", PartKind::Tool("tconstruct:head")),
    ("tconstruct:small_blade", PartKind::Tool("tconstruct:head")),
    ("tconstruct:broad_blade", PartKind::Tool("tconstruct:head")),
    ("tconstruct:adze_head", PartKind::Tool("tconstruct:head")),
    ("tconstruct:large_plate", PartKind::Tool("tconstruct:head")),
    ("tconstruct:bow_limb", PartKind::Tool("tconstruct:limb")),
    ("tconstruct:bow_grip", PartKind::Tool("tconstruct:grip")),
    ("tconstruct:bowstring", PartKind::Tool("tconstruct:bowstring")),
    ("tconstruct:arrow_head", PartKind::Tool("tconstruct:arrow_head")),
    ("tconstruct:arrow_shaft", PartKind::Tool("tconstruct:arrow_shaft")),
    ("tconstruct:fletching", PartKind::Tool("tconstruct:fletching")),
    ("tconstruct:tool_binding", PartKind::Tool("tconstruct:binding")),
    ("tconstruct:tough_binding", PartKind::Tool("tconstruct:binding")),
    ("tconstruct:tool_handle", PartKind::Tool("tconstruct:handle")),
    ("tconstruct:tough_handle", PartKind::Tool("tconstruct:handle")),
    ("tconstruct:helmet_plating", PartKind::Tool("tconstruct:plating_helmet")),
    ("tconstruct:chestplate_plating", PartKind::Tool("tconstruct:plating_chestplate")),
    ("tconstruct:leggings_plating", PartKind::Tool("tconstruct:plating_leggings")),
    ("tconstruct:boots_plating", PartKind::Tool("tconstruct:plating_boots")),
    ("tconstruct:maille", PartKind::Tool("tconstruct:maille")),
    ("tconstruct:shield_core", PartKind::Tool("tconstruct:shield_core")),
    ("tconstruct:ribcage", PartKind::Tool("tconstruct:ribcage")),
    ("tconstruct:shell", PartKind::Tool("tconstruct:shell")),
    ("tconstruct:laces", PartKind::Tool("tconstruct:laces")),
    ("tconstruct:repair_kit", PartKind::RepairKit),
    ("tconstruct:fake_ingot", PartKind::Fake("tconstruct:compatibility_metals", 1.0)),
    ("tconstruct:fake_storage_block", PartKind::Fake("tconstruct:compatibility_metals/blocks", 9.0)),
];

pub(crate) fn part_kind(item: &str) -> Option<PartKind> {
    PARTS.iter().find(|(id, _)| *id == item).map(|(_, k)| *k)
}

/// IMaterialItem.canUseMaterial (a material id, not a variant).
pub(crate) fn can_use_material(host: &dyn Host, mats: &Materials, item: &str, material: &str) -> bool {
    match part_kind(item) {
        Some(PartKind::Tool(stat)) => mats.stats(material, stat).is_some(),
        Some(PartKind::RepairKit) => mats.get(material).is_some_and(|m| m.stats.keys().any(|s| s == "tconstruct:repair_kit" || STAT_TYPES.iter().any(|(id, _, r)| *r && *id == s.as_str()))),
        Some(PartKind::Fake(tag, amount)) => {
            mats.in_tag(material, tag) && !material_recipes(host).values.iter().any(|r| r.material.0 == material && r.value as f32 / r.needed as f32 == amount)
        }
        None => false,
    }
}

/// IMaterialItem.getMaterial: the stack's `Material` (redirects resolved); none when absent, invalid or unknown.
pub(crate) fn part_material(mats: &Materials, stack: &ItemStack) -> Option<MatId> {
    let (id, variant) = parse_material(&gs(&custom_data(stack), "Material")?)?;
    (id != UNKNOWN_MATERIAL).then(|| (mats.resolve(&id).to_owned(), variant))
}

/// IMaterialItem.withMaterial: `count` of `item` of that material (a plain one when the item can't use it), named.
pub(crate) fn with_material(host: &dyn Host, mats: &Materials, item: &str, material: &MatId, count: u32) -> Option<ItemStack> {
    let mut stack = new_stack(item, count)?;
    if can_use_material(host, mats, item, &material.0) {
        let mut c = custom_data(&stack);
        ss(&mut c, "Material", &material_string(material));
        set_custom_data(&mut stack, c);
        set_component(&mut stack, "minecraft:item_name", Some(part_name(item, material)));
    }
    Some(stack)
}

// ---- Names (MaterialTooltipCache, MaterialItem.getName) as text components

fn text_tag(key: &str, with: Vec<Tag>, color: Option<u32>) -> Tag {
    let mut c = Compound::new();
    c.insert(js("translate"), Tag::String(js(key)));
    if !with.is_empty() {
        c.insert(js("with"), Tag::List(with));
    }
    if let Some(col) = color.filter(|c| c & 0xFF_FFFF != 0xFF_FFFF) {
        c.insert(js("color"), Tag::String(js(&format!("#{:06X}", col & 0xFF_FFFF))));
    }
    Tag::Compound(c)
}

pub(crate) fn material_key(id: &str, variant: &str) -> String {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    if variant.is_empty() { format!("material.{ns}.{path}") } else { format!("material.{ns}.{path}.{variant}") }
}

/// ResourceColorManager: the material's text colour (`colors.json`).
pub(crate) fn text_color(key: &str) -> Option<u32> {
    table().colors.get(key).copied()
}

/// The material's display name, coloured.
pub(crate) fn material_name(m: &MatId) -> Tag {
    let key = material_key(&m.0, "");
    text_tag(&key, Vec::new(), text_color(&key))
}

/// MaterialItem.getName(base, material): the material's `.format` (wood: "Wooden %s"), else "<material> <item>".
pub(crate) fn named(base: &str, m: &MatId) -> Tag {
    let key = material_key(&m.0, "");
    let item = text_tag(base, Vec::new(), None);
    if m.0 == "tconstruct:wood" {
        return text_tag(&format!("{key}.format"), vec![item], None);
    }
    text_tag("item.tconstruct.tool.format", vec![material_name(m), item], None)
}

pub(crate) fn part_name(item: &str, m: &MatId) -> Tag {
    let (ns, path) = item.split_once(':').unwrap_or(("minecraft", item));
    named(&format!("item.{ns}.{path}"), m)
}

// ---- Material values and the part builder's recipes

pub(crate) struct MaterialValueRecipe {
    pub items: HashSet<String>,
    pub value: i32,
    pub needed: i32,
    pub material: MatId,
    pub leftover: Option<(String, u32)>,
}

/// IMaterialValue: a material recipe, or a part in the material slot (its cast cost, no leftover).
#[derive(Clone)]
pub(crate) struct MaterialValue {
    pub material: MatId,
    pub value: i32,
    pub needed: i32,
    pub leftover: Option<(String, u32)>,
}

impl MaterialValue {
    pub(crate) fn items_used(&self, cost: i32) -> i32 {
        let needed = cost * self.needed;
        needed / self.value + i32::from(needed % self.value != 0)
    }
    fn remainder(&self, cost: i32) -> i32 {
        if self.value == 0 {
            return 0;
        }
        ((self.value - cost * self.needed) % self.value + self.value) % self.value
    }
    pub(crate) fn leftover(&self, cost: i32) -> Option<ItemStack> {
        let r = self.remainder(cost);
        let (item, n) = self.leftover.as_ref().filter(|_| r > 0)?;
        let count = *n as i32 * r / self.needed;
        (count > 0).then(|| new_stack(item, count as u32)).flatten()
    }
}

pub(crate) enum PartOutput {
    Part { item: String, count: u32, allow_uncraftable: bool },
    Item { result: (String, u32), material: Option<MatId> },
}

pub(crate) struct PartBuilderRecipe {
    pub id: String,
    pub pattern: String,
    pub pattern_items: HashSet<String>,
    pub cost: i32,
    pub output: PartOutput,
}

pub(crate) struct MaterialRecipes {
    pub values: Vec<MaterialValueRecipe>,
    /// By id (the recipe manager's order).
    pub part_builder: Vec<PartBuilderRecipe>,
    /// MaterialCastingLookup's item costs (each material casting recipe's result and `item_cost`).
    pub costs: HashMap<String, i32>,
    /// MaterialFluidRecipe: fluids, temperature, input (composite), output.
    pub fluids: Vec<MaterialFluid>,
    pub patterns_reusable: HashSet<String>,
    pub parts: HashSet<String>,
}

pub(crate) struct MaterialFluid {
    pub fluids: Vec<(bool, String, i32)>,
    pub temperature: i32,
    pub input: Option<MatId>,
    pub output: MatId,
}

pub(crate) const MATERIAL_CAST_KINDS: [&str; 4] = ["tconstruct:table_casting_material", "tconstruct:basin_casting_material", "tconstruct:table_casting_composite", "tconstruct:basin_casting_composite"];
const KINDS: [&str; 7] = ["tconstruct:material", "tconstruct:part_builder", "tconstruct:item_part_builder", "tconstruct:material_fluid", MATERIAL_CAST_KINDS[0], MATERIAL_CAST_KINDS[1], "tconstruct:material_melting"];

fn parse_part_builder(host: &dyn Host, id: &str, json: &Value) -> Option<PartBuilderRecipe> {
    let r = chosen(host, json)?;
    let pattern = parse_material(r["pattern"].as_str()?).map(|(p, _)| p)?;
    let pattern_items = if r["pattern_item"].is_null() { tag_items(host, "tconstruct:patterns/default") } else { ingredient_items(host, &r["pattern_item"], &mut Vec::new()) };
    let output = match r["type"].as_str()? {
        "tconstruct:part_builder" => {
            let item = r["result"]["item"].as_str().or_else(|| r["result"].as_str())?.to_owned();
            part_kind(&item)?;
            PartOutput::Part { item, count: r["result"]["count"].as_u64().unwrap_or(1).clamp(1, 64) as u32, allow_uncraftable: r["allow_uncraftable"].as_bool().unwrap_or(false) }
        }
        _ => {
            let material = match r["material"].as_str() {
                Some(m) => Some(parse_material(m)?).filter(|(id, _)| id != UNKNOWN_MATERIAL),
                None => None,
            };
            PartOutput::Item { result: item_output(host, &r["result"])?, material }
        }
    };
    let is_part = matches!(output, PartOutput::Part { .. });
    let cost = stat_int(r, "cost", if is_part { None } else { Some(0) }, i32::from(is_part))?;
    if matches!(&output, PartOutput::Item { material: Some(_), .. }) && cost == 0 {
        return None;
    }
    (!pattern_items.is_empty()).then(|| PartBuilderRecipe { id: id.to_owned(), pattern, pattern_items, cost, output })
}

static RECIPES: Mutex<Option<(Key, Arc<MaterialRecipes>)>> = Mutex::new(None);

/// The material values, the part builder's recipes and material casting's tables, read again on a new data load.
pub(crate) fn material_recipes(host: &dyn Host) -> Arc<MaterialRecipes> {
    ensure_ids(host);
    let key = recipes_key(host, &KINDS);
    if let Some(r) = RECIPES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, r)| r.clone()) {
        return r;
    }
    let values = host
        .recipes_of_type("tconstruct:material")
        .iter()
        .filter_map(|r| {
            let r = chosen(host, &r.json)?;
            let items = ingredient_items(host, &r["ingredient"], &mut Vec::new());
            let value = stat_int(r, "value", Some(1), 1)?;
            let needed = stat_int(r, "needed", Some(1), 1)?;
            let leftover = (value > 1).then(|| item_output(host, &r["leftover"])).flatten();
            (!items.is_empty()).then_some(MaterialValueRecipe { items, value, needed, material: parse_material(r["material"].as_str()?)?, leftover })
        })
        .collect();
    let mut part_builder: Vec<PartBuilderRecipe> = ["tconstruct:part_builder", "tconstruct:item_part_builder"].iter().flat_map(|k| host.recipes_of_type(k).iter().filter_map(|r| parse_part_builder(host, &r.id, &r.json)).collect::<Vec<_>>()).collect();
    part_builder.sort_by(|a, b| a.id.cmp(&b.id));
    let mut costs = HashMap::new();
    for k in &MATERIAL_CAST_KINDS[..2] {
        for r in host.recipes_of_type(k).iter() {
            let Some(j) = chosen(host, &r.json) else { continue };
            if let (Some(item), Some(cost)) = (j["result"].as_str().or_else(|| j["result"]["item"].as_str()), stat_int(j, "item_cost", None, 1)) {
                costs.entry(item.to_owned()).or_insert(cost);
            }
        }
    }
    let fluids = host
        .recipes_of_type("tconstruct:material_fluid")
        .iter()
        .filter_map(|r| {
            let r = chosen(host, &r.json)?;
            let input = match r["input"].as_str() {
                Some(s) => Some(parse_material(s)?),
                None => None,
            };
            Some(MaterialFluid { fluids: fluid_ingredient(&r["fluid"]), temperature: stat_int(r, "temperature", None, 0)?, input, output: parse_material(r["output"].as_str()?)? })
        })
        .collect();
    let out = Arc::new(MaterialRecipes {
        values,
        part_builder,
        costs,
        fluids,
        patterns_reusable: tag_items(host, "tconstruct:patterns/reusable"),
        parts: tag_items(host, "tconstruct:parts"),
    });
    *RECIPES.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, out.clone()));
    out
}

/// PartBuilderContainerWrapper.getMaterial: a part is worth its cast cost of its material, any other item the first
/// material recipe that takes it (with a known material).
pub(crate) fn material_value(host: &dyn Host, stack: &ItemStack) -> Option<MaterialValue> {
    let mats = materials(host);
    let recipes = material_recipes(host);
    let id = sid(stack);
    if recipes.parts.contains(id) {
        let cost = recipes.costs.get(id).copied().unwrap_or(0);
        if let Some(m) = part_material(&mats, stack).filter(|(m, _)| cost != 0 && mats.get(m).is_some()) {
            return Some(MaterialValue { material: m, value: cost, needed: 1, leftover: None });
        }
    }
    recipes.values.iter().find(|r| mats.get(&r.material.0).is_some() && r.items.contains(id)).map(|r| MaterialValue { material: r.material.clone(), value: r.value, needed: r.needed, leftover: r.leftover.clone() })
}

impl PartBuilderRecipe {
    /// IPartBuilderRecipe.partialMatch: whether its button shows.
    pub(crate) fn partial_match(&self, host: &dyn Host, mats: &Materials, pattern: Option<&ItemStack>, material: Option<&ItemStack>, value: Option<&MaterialValue>) -> bool {
        if !pattern.is_some_and(|p| self.pattern_items.contains(sid(p))) {
            return false;
        }
        let Some(stack) = material else { return true };
        match &self.output {
            PartOutput::Part { item, allow_uncraftable, .. } => {
                sid(stack) != item && value.is_some_and(|v| (*allow_uncraftable || mats.get(&v.material.0).is_some_and(|m| m.craftable)) && can_use_material(host, mats, item, &v.material.0))
            }
            PartOutput::Item { material: want, .. } => want.as_ref().is_some_and(|w| value.is_some_and(|v| matches_variant(w, &v.material))),
        }
    }

    pub(crate) fn matches(&self, host: &dyn Host, mats: &Materials, material: Option<&ItemStack>, value: Option<&MaterialValue>) -> bool {
        let count = material.map_or(0, |s| s.count() as i32);
        match &self.output {
            PartOutput::Part { item, allow_uncraftable, .. } => value.is_some_and(|v| {
                (*allow_uncraftable || mats.get(&v.material.0).is_some_and(|m| m.craftable)) && can_use_material(host, mats, item, &v.material.0) && count >= v.items_used(self.cost)
            }),
            PartOutput::Item { material: None, .. } => material.is_none(),
            PartOutput::Item { material: Some(w), .. } => value.is_some_and(|v| matches_variant(w, &v.material) && count >= v.items_used(self.cost)),
        }
    }

    pub(crate) fn items_used(&self, value: Option<&MaterialValue>) -> i32 {
        match (&self.output, value) {
            (PartOutput::Item { material: None, .. }, _) | (_, None) => 0,
            (_, Some(v)) => v.items_used(self.cost),
        }
    }

    /// assemble: more of the result when the material is worth more than the cost and leaves nothing over.
    pub(crate) fn assemble(&self, host: &dyn Host, mats: &Materials, value: Option<&MaterialValue>) -> Option<ItemStack> {
        let scale = |count: u32| match value {
            Some(v) if v.leftover.is_none() && v.value > self.cost && self.cost > 0 => (count as i32 * v.value / self.cost).clamp(1, 64) as u32,
            _ => count,
        };
        match &self.output {
            PartOutput::Part { item, count, .. } => match value {
                Some(v) => with_material(host, mats, item, &v.material, scale(*count)),
                None => new_stack(item, *count),
            },
            PartOutput::Item { result, .. } => new_stack(&result.0, scale(result.1)),
        }
    }
}

/// PartBuilderBlockEntity.getSortedButtons: the recipes whose button shows, the first per pattern, by cost then
/// pattern id (indices into `part_builder`).
pub(crate) fn part_builder_buttons(host: &dyn Host, pattern: Option<&ItemStack>, material: Option<&ItemStack>) -> Vec<usize> {
    let recipes = material_recipes(host);
    let mats = materials(host);
    let value = material.and_then(|m| material_value(host, m));
    let mut seen = HashSet::new();
    let mut out: Vec<usize> = (0..recipes.part_builder.len())
        .filter(|&i| recipes.part_builder[i].partial_match(host, &mats, pattern, material, value.as_ref()) && seen.insert(recipes.part_builder[i].pattern.clone()))
        .collect();
    out.sort_by(|&a, &b| recipes.part_builder[a].cost.cmp(&recipes.part_builder[b].cost).then_with(|| recipes.part_builder[a].pattern.cmp(&recipes.part_builder[b].pattern)));
    out
}

// ---- Material casting and melting (MaterialCastingRecipe, CompositeCastingRecipe, MaterialMeltingRecipe)

/// ICastingRecipe.calcCoolingTime / IMeltingRecipe.calcTimeForAmount.
pub(crate) fn calc_time(temperature: i32, amount: i32) -> i32 {
    let factor = f64::from(amount as f32 / 90.0).sqrt() as f32;
    (f64::from(temperature + 300).powf(f64::from(0.585f32)) * f64::from(factor)).round() as i32
}

fn material_allowed(mats: &Materials, p: &Value, m: &MatId) -> bool {
    match p["type"].as_str() {
        None if p.is_null() => true,
        Some("mantle:always" | "tconstruct:any") => true,
        Some("mantle:inverted") => !material_allowed(mats, &p["inverted"], m),
        Some("mantle:and") => p["predicates"].as_array().is_none_or(|a| a.iter().all(|x| material_allowed(mats, x, m))),
        Some("mantle:or") => p["predicates"].as_array().is_some_and(|a| a.iter().any(|x| material_allowed(mats, x, m))),
        Some("tconstruct:stat_type") => p["stat_type"].as_str().is_some_and(|s| mats.stats(&m.0, s).is_some()),
        Some("tconstruct:variant") => p["material"].as_str().and_then(parse_material).is_some_and(|w| matches_variant(&w, m)),
        Some("tconstruct:tag") => p["tag"].as_str().is_some_and(|t| mats.in_tag(&m.0, t)),
        _ => false,
    }
}

/// AbstractMaterialCastingRecipe with the cast and fluid: the fluid amount, the cooling time and the part; none when
/// the recipe doesn't take them (`casting_recipes::material_cast`, stage 2's hook).
pub(crate) fn material_cast(host: &dyn Host, kind: &str, json: &str, cast: &Option<ItemStack>, fluid: &str) -> Option<(i32, i32, ItemStack)> {
    let r: Value = serde_json::from_str(json).ok()?;
    let result = r["result"].as_str().or_else(|| r["result"]["item"].as_str())?.to_owned();
    let cost = stat_int(&r, "item_cost", None, 1)?;
    let mats = materials(host);
    let recipes = material_recipes(host);
    let usable = |m: &&MaterialFluid| mats.get(&m.output.0).is_some() && fluid_amount(&m.fluids, fluid).is_some();
    if kind.ends_with("_tool_casting") {
        // ToolCastingRecipe (the plate shield): the cast is a part of a material ingredient (its material is the
        // first, `cast_purpose` first_material), the fluid's material the next; the tool is built from both.
        // Each refusal is logged (`tlog`) with the cast and the fluid.
        let why = |w: &str| {
            crate::smeltery::util::tlog(&format!("tool casting {result} refused ({w}): cast {:?}, fluid {fluid}", cast.as_ref().map(|c| sid(c).to_owned())));
            None
        };
        let Some(c) = cast.as_ref() else { return why("no cast") };
        if !ingredient_items(host, &r["cast"], &mut Vec::new()).contains(sid(c)) && r["cast"]["item"].as_str() != Some(sid(c)) {
            return None;
        }
        let Some(first) = part_material(&mats, c) else { return why("the cast has no material") };
        let stat = match result.as_str() {
            "tconstruct:plate_shield" => "tconstruct:plating_shield",
            _ => return why("unknown tool"),
        };
        let Some(m) = recipes.fluids.iter().filter(|m| m.input.is_none()).filter(|m| mats.stats(&m.output.0, stat).is_some()).find(usable) else {
            return why("no material fluid with plating stats");
        };
        let id = |m: &MatId| if m.1.is_empty() { m.0.clone() } else { format!("{}#{}", m.0, m.1) };
        let Some(mut out) = new_stack(&result, 1) else { return why("no such item") };
        if !crate::tools::tool::write_tool(host, &mut out, &[id(&first), id(&m.output)]) {
            return why("write_tool failed");
        }
        let Some(per) = fluid_amount(&m.fluids, fluid) else { return why("no amount") };
        let amount = per * cost;
        crate::smeltery::util::tlog(&format!("tool casting {result}: {amount} mB of {fluid} ({:?} + {:?})", first, m.output));
        return Some((amount, calc_time(m.temperature, amount), out));
    }
    let m = if kind.ends_with("_composite") {
        // CompositeCastingRecipe: the cast is a part of the result's kind, turned into the composite's output.
        let c = cast.as_ref().filter(|c| sid(c) == result)?;
        if let Some(stat) = r["casting_stat_conflict"].as_str()
            && recipes.fluids.iter().filter(|m| m.input.is_none()).find(usable).is_some_and(|p| mats.stats(&p.output.0, stat).is_some())
        {
            return None;
        }
        let material = part_material(&mats, c)?;
        recipes.fluids.iter().filter(|m| m.input.as_ref().is_some_and(|i| matches_variant(i, &material)) && m.output != material).find(usable)?
    } else {
        // MaterialCastingRecipe: the cast ingredient (none: an empty table).
        let ok = match (&r["cast"], cast) {
            (Value::Null, None) => true,
            (v, Some(c)) if !v.is_null() => ingredient_items(host, v, &mut Vec::new()).contains(sid(c)),
            _ => false,
        };
        if !ok {
            return None;
        }
        recipes.fluids.iter().filter(|m| m.input.is_none()).find(usable)?
    };
    if !material_allowed(&mats, &r["allowed_materials"], &m.output) || !can_use_material(host, &mats, &result, &m.output.0) {
        return None;
    }
    let amount = fluid_amount(&m.fluids, fluid)? * cost;
    Some((amount, calc_time(m.temperature, amount), with_material(host, &mats, &result, &m.output, 1)?))
}

// ---- Station layouts and tool definitions (StationSlotLayoutLoader, ToolDefinitionLoader)

/// Mantle's block predicates, as the tool definitions use them.
#[derive(Clone, Debug)]
pub(crate) enum BlockPred {
    Any,
    Tag(String),
    Set(Vec<String>),
    And(Vec<BlockPred>),
    Or(Vec<BlockPred>),
    Not(Box<BlockPred>),
    Never,
}

pub(crate) fn parse_block_pred(v: &Value, key: &str) -> BlockPred {
    let list = |k: &str| -> Vec<BlockPred> { v[k].as_array().into_iter().flatten().map(|p| parse_block_pred(p, "type")).collect() };
    match v[key].as_str().unwrap_or("") {
        "mantle:any" | "mantle:always" => BlockPred::Any,
        "mantle:tag" => BlockPred::Tag(c_tag(&pattern_id(v["tag"].as_str().unwrap_or("")))),
        "mantle:set" => BlockPred::Set(v["blocks"].as_array().into_iter().flatten().filter_map(|b| b.as_str().map(pattern_id)).collect()),
        "mantle:and" => BlockPred::And(list("predicates")),
        "mantle:or" => BlockPred::Or(list("predicates")),
        "mantle:inverted" => BlockPred::Not(Box::new(parse_block_pred(v, "inverted_type"))),
        _ => BlockPred::Never,
    }
}

impl BlockPred {
    pub(crate) fn matches(&self, host: &dyn Host, block: BlockKey, name: &str) -> bool {
        self.test(&|t| host.block_in_tag(block, t).unwrap_or(false), name)
    }

    /// With the tag test given (a level host's, where no `Host` is).
    pub(crate) fn test(&self, in_tag: &dyn Fn(&str) -> bool, name: &str) -> bool {
        match self {
            BlockPred::Any => true,
            BlockPred::Tag(t) => in_tag(t),
            BlockPred::Set(s) => s.iter().any(|b| b == name),
            BlockPred::And(l) => l.iter().all(|p| p.test(in_tag, name)),
            BlockPred::Or(l) => l.iter().any(|p| p.test(in_tag, name)),
            BlockPred::Not(p) => !p.test(in_tag, name),
            BlockPred::Never => false,
        }
    }
}

/// A tool definition's modules (ToolDefinitionData).
#[derive(Default)]
pub(crate) struct ToolDef {
    pub parts: Vec<String>,
    pub stat_types: Vec<String>,
    pub scales: Vec<f32>,
    pub traits: Vec<Trait>,
    pub fixed_name: Option<usize>,
    pub base_stats: Vec<(String, f32)>,
    pub base_tier: Option<usize>,
    pub multipliers: Vec<(String, f64)>,
    pub slots: Vec<(String, i32)>,
    pub effective: Option<(BlockPred, bool)>,
    pub speed_mods: Vec<(BlockPred, f32)>,
    pub max_tier: Option<usize>,
}

fn parse_tool_def(doc: &Value) -> ToolDef {
    let mut def = ToolDef::default();
    let mut parts_hook: Option<Vec<String>> = None;
    let names = |m: &Value| -> Vec<String> { m["parts"].as_array().into_iter().flatten().filter_map(|p| p.as_str().or_else(|| p["item"].as_str()).map(str::to_owned)).collect() };
    for m in doc["modules"].as_array().into_iter().flatten() {
        match m["type"].as_str().unwrap_or("") {
            "tconstruct:part_stats" => {
                for p in m["parts"].as_array().into_iter().flatten() {
                    let Some(item) = p.as_str().or_else(|| p["item"].as_str()) else { continue };
                    if let Some(PartKind::Tool(stat)) = part_kind(item) {
                        def.stat_types.push(stat.to_owned());
                        def.scales.push(p["scale"].as_f64().unwrap_or(1.0) as f32);
                    }
                }
                parts_hook.get_or_insert(names(m));
            }
            "tconstruct:material_stats" => {
                for s in m["stat_types"].as_array().into_iter().flatten() {
                    if let Some(id) = s.as_str().or_else(|| s["stat"].as_str()) {
                        def.stat_types.push(id.to_owned());
                        def.scales.push(s["scale"].as_f64().unwrap_or(1.0) as f32);
                    }
                }
            }
            "tconstruct:base_stats" => {
                for (k, v) in m["stats"].as_object().into_iter().flatten() {
                    match (v.as_str(), v.as_f64()) {
                        (Some(t), _) if k == "tconstruct:harvest_tier" => def.base_tier = TIERS.iter().position(|x| *x == pattern_id(t)),
                        (_, Some(f)) => def.base_stats.push((k.clone(), f as f32)),
                        _ => {}
                    }
                }
            }
            "tconstruct:multiply_stats" => def.multipliers.extend(m["multipliers"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?)))),
            "tconstruct:modifier_slots" => def.slots.extend(m["slots"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_i64()? as i32)))),
            "tconstruct:is_effective" => def.effective = Some((parse_block_pred(m, "predicate_type"), m["ignore_tier"].as_bool().unwrap_or(false))),
            "tconstruct:mining_speed_modifier" => def.speed_mods.push((parse_block_pred(m, "predicate_type"), m["modifier"].as_f64().unwrap_or(1.0) as f32)),
            "tconstruct:max_tier" => def.max_tier = m["tier"].as_str().and_then(|t| TIERS.iter().position(|x| *x == pattern_id(t))),
            "tconstruct:tool_parts" => {
                parts_hook.get_or_insert(names(m));
            }
            "tconstruct:traits" => def.traits.extend(parse_traits(&m["traits"])),
            "tconstruct:fixed_material_name" => def.fixed_name = m["index"].as_u64().map(|i| i as usize),
            _ => {}
        }
    }
    def.parts = parts_hook.unwrap_or_default();
    def
}

/// A StationSlotLayout's slot: its position (-1, -1 hidden) and filter.
#[allow(dead_code)]
pub(crate) struct LayoutSlot {
    pub x: i32,
    pub y: i32,
    pub filter: Option<HashSet<String>>,
}

pub(crate) struct Layout {
    pub tool: Option<LayoutSlot>,
    pub inputs: Vec<LayoutSlot>,
}

#[allow(dead_code)]
pub(crate) struct Tooling {
    pub defs: HashMap<String, ToolDef>,
    pub layouts: HashMap<String, Layout>,
    /// `tconstruct:tool_building` recipes by id: result, count, extra requirements, parts override, extra materials.
    pub building: Vec<ToolBuild>,
    pub modifiable: HashSet<String>,
    pub durability: HashSet<String>,
    pub harvest: HashSet<String>,
    pub harvest_primary: HashSet<String>,
    pub melee_primary: HashSet<String>,
    pub melee_weapon: HashSet<String>,
    pub unarmed: HashSet<String>,
}

pub(crate) struct ToolBuild {
    pub result: String,
    pub count: u32,
    pub extra: Vec<HashSet<String>>,
    pub parts: Option<Vec<String>>,
    pub materials: Vec<String>,
}

static TOOLING: Mutex<Option<(Key, Arc<Tooling>)>> = Mutex::new(None);

fn layout_slot(host: &dyn Host, v: &Value) -> LayoutSlot {
    let int = |k: &str| v[k].as_i64().map_or(-1, |n| n.clamp(-1, 1000) as i32);
    LayoutSlot { x: int("x"), y: int("y"), filter: (!v["filter"].is_null()).then(|| ingredient_items(host, &v["filter"], &mut Vec::new())) }
}

/// The last tool data read (for hooks without a `Host`).
pub(crate) fn cached_tooling() -> Option<Arc<Tooling>> {
    TOOLING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(_, t)| t.clone())
}

/// The tool definitions, the station layouts, the tool building recipes and the tool tags of the data load.
pub(crate) fn tooling(host: &dyn Host) -> Arc<Tooling> {
    ensure_ids(host);
    let key = recipes_key(host, &["tconstruct:tool_building", "tconstruct:material"]);
    if let Some(t) = TOOLING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, t)| t.clone()) {
        return t;
    }
    let t = &table().json;
    let mut defs: HashMap<String, ToolDef> = t["tool_definitions"].as_object().into_iter().flatten().map(|(id, doc)| (id.clone(), parse_tool_def(doc))).collect();
    if let Some(doc) = t["tool_definitions"].get("tconstruct:slime_chestplate") {
        defs.entry("tconstruct:slimy_chestplate".to_owned()).or_insert_with(|| parse_tool_def(doc));
    }
    let layouts = t["layouts"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, doc)| {
            let tool = (!doc["tool_slot"].is_null()).then(|| layout_slot(host, &doc["tool_slot"]));
            let inputs = doc["input_slots"].as_array().map(|a| a.iter().map(|s| layout_slot(host, s)).collect()).unwrap_or_default();
            (id.clone(), Layout { tool, inputs })
        })
        .collect();
    let mut building: Vec<(String, ToolBuild)> = host
        .recipes_of_type("tconstruct:tool_building")
        .iter()
        .filter_map(|r| {
            let j = chosen(host, &r.json)?;
            Some((r.id.clone(), ToolBuild {
                result: j["result"].as_str().or_else(|| j["result"]["item"].as_str())?.to_owned(),
                count: j["result_count"].as_u64().unwrap_or(1).clamp(1, 64) as u32,
                extra: j["extra_requirements"].as_array().into_iter().flatten().map(|i| ingredient_items(host, i, &mut Vec::new())).collect(),
                parts: j["parts_override"].as_array().map(|a| a.iter().filter_map(|p| p.as_str().or_else(|| p["item"].as_str()).map(str::to_owned)).collect()),
                materials: j["extra_materials"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(str::to_owned)).collect(),
            }))
        })
        .collect();
    building.sort_by(|a, b| a.0.cmp(&b.0));
    let out = Arc::new(Tooling {
        defs,
        layouts,
        building: building.into_iter().map(|(_, r)| r).collect(),
        modifiable: tag_items(host, "tconstruct:modifiable"),
        durability: tag_items(host, "tconstruct:modifiable/durability"),
        harvest: tag_items(host, "tconstruct:modifiable/harvest"),
        harvest_primary: tag_items(host, "tconstruct:modifiable/harvest/primary"),
        melee_primary: tag_items(host, "tconstruct:modifiable/melee/primary"),
        melee_weapon: tag_items(host, "tconstruct:modifiable/melee/weapon"),
        unarmed: tag_items(host, "tconstruct:modifiable/melee/unarmed"),
    });
    *TOOLING.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, out.clone()));
    out
}
