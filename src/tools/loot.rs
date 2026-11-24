//! Tinkers' and Mantle's global loot modifiers (`data/tconstruct/loot_modifiers/`): Mantle's `add_entry` (lustrous
//! nuggets from ores, chrysophilite gold nuggets, tasty bacon), Mantle's `replace_item` (necrotic bones from wither
//! skeletons) and `tconstruct:modifier_hook` (the tool's `processLoot` modifiers: severing). Sunrise names a loot
//! modifier type after the mod that registers it, so Mantle's two are `tconstruct:add_entry` and
//! `tconstruct:replace_item` in the port's files.
//!
//! The serializers decode each data file into a [`Rule`] and register it here; the modifier itself returns the loot
//! unchanged. The rules are applied in the block and living drop hooks, which have what the rules read (the tool's
//! modifiers, block, item and entity tags, the player's chrysophilite attribute, the recipes): Sunrise's loot
//! modifier callback gets only the roll's facts. A pack that adds a file under `loot_modifiers/` adds a rule the same
//! way; the conditions, entries and functions Tinkers' own files use are the ones read here.

use crate::smeltery::util::{LivingStacks, component, gi, item_key, key_id, sid};
use crate::tools::modifier::{modifier_level, rand_f32};
use mod_api::*;
use mod_api::serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A condition of a rule (Mantle's and Tinkers' loot conditions in Tinkers' files).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cond {
    /// `mantle:block_tag`: the broken block is in the tag.
    BlockTag(String),
    /// `mantle:tag_filled`: the item tag has an item.
    TagFilled(String),
    /// `tconstruct:has_modifier`: the tool has the modifier.
    HasModifier(String),
    /// `tconstruct:has_context_set` with `minecraft:entity`: a mob's drops.
    EntityRoll,
    /// `tconstruct:has_context_set` with `minecraft:block`: a block's drops.
    BlockRoll,
    /// `minecraft:entity_properties` on `this` with a `type` tag.
    EntityTypeTag(String),
    /// `forge:loot_table_id` / `neoforge:loot_table_id`: the table rolled.
    LootTable(String),
    /// `tconstruct:config`: a config switch, on by default.
    Config,
    /// `tconstruct:block_or_entity`: a block or mob roll.
    BlockOrEntity,
    /// `tconstruct:has_chrysophilite`: the player wears chrysophilite (attribute at least 1).
    Chrysophilite,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Entry {
    Item(String),
    /// `mantle:tag_preference`: the preferred item of an item tag.
    TagPreference(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Formula {
    /// `minecraft:ore_drops`.
    OreDrops,
    /// `minecraft:uniform_bonus_count` with its multiplier.
    UniformBonus(i32),
    /// `minecraft:binomial_with_bonus_count`.
    Binomial { extra: i32, probability: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Func {
    SetCount { min: f32, max: f32, add: bool },
    /// `tconstruct:modifier_bonus`: the formula at the tool's modifier level (less one without `include_base`).
    ModifierBonus { modifier: String, formula: Formula, include_base: bool },
    /// `tconstruct:chrysophilite_bonus`: the formula at the player's chrysophilite level.
    ChrysophiliteBonus { formula: Formula, include_base: bool },
    /// `minecraft:looting_enchant`: plus uniform(min, max) per looting level.
    Looting { min: f32, max: f32 },
}

/// One decoded loot modifier file.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Rule {
    Add { conds: Vec<Cond>, entry: Entry, funcs: Vec<Func>, absent: Vec<String> },
    Replace { conds: Vec<Cond>, original: String, replacement: String },
    Hook { conds: Vec<Cond> },
}

/// The decoded rules, by their file's text (a reload decodes the same files again: the same rules).
static RULES: Mutex<Vec<(String, Arc<Rule>)>> = Mutex::new(Vec::new());
/// What a block drop needs that only a `Host` reads, refreshed on the server tick: each tag preference's item (and
/// whether the tag has one), each online player's chrysophilite level.
static FACTS: Mutex<Option<Facts>> = Mutex::new(None);

#[derive(Default)]
struct Facts {
    preferred: HashMap<String, Option<String>>,
    chrysophilite: HashMap<u128, i32>,
    /// The items of each post condition's tag (the loot must hold none of them).
    tag_items: HashMap<String, Vec<String>>,
}

/// Forge's tag names are `c:` on 26.3.
fn tag(name: &str) -> String {
    match name.strip_prefix("forge:") {
        Some(rest) => format!("c:{rest}"),
        None => name.to_owned(),
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

fn conds(list: &Value) -> Result<Vec<Cond>, String> {
    let mut out = Vec::new();
    for c in list.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let kind = s(c, "condition").or_else(|| s(c, "type")).unwrap_or_default();
        out.push(match kind.as_str() {
            "mantle:block_tag" => Cond::BlockTag(tag(&s(c, "tag").ok_or("block_tag: no tag")?)),
            "mantle:tag_filled" => Cond::TagFilled(tag(&s(c, "tag").ok_or("tag_filled: no tag")?)),
            "tconstruct:has_modifier" => Cond::HasModifier(s(c, "modifier").ok_or("has_modifier: no modifier")?),
            "tconstruct:has_context_set" => match s(c, "set").as_deref() {
                Some("minecraft:entity") => Cond::EntityRoll,
                Some("minecraft:block") => Cond::BlockRoll,
                other => return Err(format!("has_context_set: {other:?}")),
            },
            "minecraft:entity_properties" => {
                let t = c["predicate"]["type"].as_str().and_then(|t| t.strip_prefix('#')).ok_or("entity_properties: only a type tag")?;
                Cond::EntityTypeTag(t.to_owned())
            }
            "forge:loot_table_id" | "neoforge:loot_table_id" => Cond::LootTable(s(c, "loot_table_id").ok_or("loot_table_id: no id")?),
            "tconstruct:config" => Cond::Config,
            "tconstruct:block_or_entity" => Cond::BlockOrEntity,
            "tconstruct:has_chrysophilite" => Cond::Chrysophilite,
            other => return Err(format!("unknown condition {other}")),
        });
    }
    Ok(out)
}

fn formula(f: &Value) -> Result<Formula, String> {
    let p = &f["parameters"];
    Ok(match f["formula"].as_str().unwrap_or("minecraft:ore_drops") {
        "minecraft:ore_drops" => Formula::OreDrops,
        "minecraft:uniform_bonus_count" => Formula::UniformBonus(p["bonusMultiplier"].as_i64().unwrap_or(1) as i32),
        "minecraft:binomial_with_bonus_count" => Formula::Binomial {
            extra: p["extra"].as_i64().unwrap_or(0) as i32,
            probability: p["probability"].as_f64().unwrap_or(0.0) as f32,
        },
        other => return Err(format!("unknown formula {other}")),
    })
}

/// A number provider's range: a constant or `minecraft:uniform`.
fn range(v: &Value) -> (f32, f32) {
    match v.as_f64() {
        Some(n) => (n as f32, n as f32),
        None => (v["min"].as_f64().unwrap_or(0.0) as f32, v["max"].as_f64().unwrap_or(0.0) as f32),
    }
}

fn funcs(list: &Value) -> Result<Vec<Func>, String> {
    let mut out = Vec::new();
    for f in list.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        match f["function"].as_str().unwrap_or_default() {
            "minecraft:set_count" => {
                let (min, max) = range(&f["count"]);
                out.push(Func::SetCount { min, max, add: f["add"].as_bool().unwrap_or(false) });
            }
            "tconstruct:modifier_bonus" => out.push(Func::ModifierBonus {
                modifier: s(f, "modifier").ok_or("modifier_bonus: no modifier")?,
                formula: formula(f)?,
                include_base: f["include_base"].as_bool().unwrap_or(true),
            }),
            "tconstruct:chrysophilite_bonus" => out.push(Func::ChrysophiliteBonus {
                formula: formula(f)?,
                include_base: f["include_base"].as_bool().unwrap_or(true),
            }),
            "minecraft:looting_enchant" => {
                let (min, max) = range(&f["count"]);
                out.push(Func::Looting { min, max });
            }
            // A tool breaking a block is no explosion: nothing to decay.
            "minecraft:explosion_decay" => {}
            other => return Err(format!("unknown function {other}")),
        }
    }
    Ok(out)
}

/// Decodes one loot modifier file of `kind` into a rule.
pub(crate) fn decode(kind: &str, v: &Value) -> Result<Rule, String> {
    let conditions = conds(&v["conditions"])?;
    Ok(match kind {
        "add_entry" => {
            let e = &v["entry"];
            let entry = match e["type"].as_str().unwrap_or_default() {
                "minecraft:item" => Entry::Item(s(e, "name").ok_or("entry: no name")?),
                "mantle:tag_preference" => Entry::TagPreference(tag(&s(e, "tag").ok_or("entry: no tag")?)),
                other => return Err(format!("unknown entry {other}")),
            };
            // `mantle:inverted` of `mantle:contains_item` with a tag ingredient: added only when the loot holds
            // none of that tag (a silk-touched ore keeps its block and gets no nuggets).
            let mut absent = Vec::new();
            for p in v["post_conditions"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                let inner = &p["condition"];
                match (p["type"].as_str(), inner["type"].as_str(), inner["ingredient"]["tag"].as_str()) {
                    (Some("mantle:inverted"), Some("mantle:contains_item"), Some(t)) => absent.push(tag(t)),
                    _ => return Err("unknown post condition".into()),
                }
            }
            Rule::Add { conds: conditions, entry, funcs: funcs(&v["functions"])?, absent }
        }
        "replace_item" => Rule::Replace {
            conds: conditions,
            original: v["original"]["item"].as_str().ok_or("replace_item: no original item")?.to_owned(),
            replacement: s(v, "replacement").ok_or("replace_item: no replacement")?,
        },
        "modifier_hook" => Rule::Hook { conds: conditions },
        other => return Err(format!("unknown kind {other}")),
    })
}

fn add_rule(text: String, rule: Rule) {
    let mut rules = RULES.lock().unwrap_or_else(|e| e.into_inner());
    if !rules.iter().any(|(t, _)| *t == text) {
        rules.push((text, Arc::new(rule)));
    }
    *FACTS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn rules() -> Vec<Arc<Rule>> {
    RULES.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, r)| r.clone()).collect()
}

/// The serializer of one kind: `mantle:add_entry`, `mantle:replace_item`, `tconstruct:modifier_hook`.
pub(crate) struct Serializer(pub(crate) &'static str);

struct Recorded;

impl GlobalLootModifier for Recorded {
    // The drop hooks apply the rule (see the module's comment).
    fn apply(&self, _context: &mut dyn LootModifierContext, generated: Vec<LootStack>) -> Result<Vec<LootStack>, LootModifierError> {
        Ok(generated)
    }
}

impl GlobalLootModifierSerializer for Serializer {
    fn decode(&self, data: &Value, _load: &dyn DataLoadContext) -> Result<Arc<dyn GlobalLootModifier>, LootModifierError> {
        let rule = decode(self.0, data).map_err(|e| LootModifierError::new("", &e))?;
        add_rule(data.to_string(), rule);
        Ok(Arc::new(Recorded))
    }
}

pub(crate) fn register(r: &mut dyn Registrar) {
    // A type id is the registering mod's: Mantle's `mantle:add_entry` and `mantle:replace_item` are
    // `tconstruct:add_entry` and `tconstruct:replace_item` here, and the port's files name those.
    for kind in ["add_entry", "replace_item", "modifier_hook"] {
        r.register_global_loot_modifier_serializer(kind, Serializer(kind));
    }
}

// ---- Rolling

fn uniform_i(min: f32, max: f32) -> i32 {
    // UniformGenerator.getInt: Mth.nextInt(random, round(min), round(max)), inclusive.
    let (lo, hi) = (min.round() as i32, max.round() as i32);
    if hi <= lo { lo } else { lo + (rand_f32() * (hi - lo + 1) as f32) as i32 }
}

fn apply_formula(f: Formula, count: i32, level: i32) -> i32 {
    match f {
        // ApplyBonusCount.OreDrops: count * (max(nextInt(level + 2) - 1, 0) + 1).
        Formula::OreDrops => count * ((((rand_f32() * (level + 2) as f32) as i32) - 1).max(0) + 1),
        // UniformBonusCount: count + nextInt(multiplier * level + 1).
        Formula::UniformBonus(m) => count + (rand_f32() * (m * level + 1) as f32) as i32,
        // BinomialWithBonusCount: count + successes in (level + extra) trials.
        Formula::Binomial { extra, probability } => count + (0..level + extra).filter(|_| rand_f32() < probability).count() as i32,
    }
}

/// What a roll knows: the tool, the block or the mob, the player's chrysophilite and looting.
struct Roll<'a> {
    tool: Option<&'a ItemStack>,
    block: bool,
    entity: bool,
    in_block_tag: &'a dyn Fn(&str) -> bool,
    entity_in_tag: &'a dyn Fn(&str) -> bool,
    table: Option<String>,
    chrysophilite: i32,
    looting: i32,
    preferred: &'a dyn Fn(&str) -> Option<String>,
}

fn holds(roll: &Roll, c: &Cond) -> bool {
    match c {
        Cond::BlockTag(t) => roll.block && (roll.in_block_tag)(t),
        Cond::TagFilled(t) => (roll.preferred)(t).is_some(),
        Cond::HasModifier(m) => roll.tool.is_some_and(|t| modifier_level(t, m) > 0),
        Cond::EntityRoll => roll.entity,
        Cond::BlockRoll => roll.block,
        Cond::EntityTypeTag(t) => roll.entity && (roll.entity_in_tag)(t),
        Cond::LootTable(id) => roll.table.as_deref() == Some(id.as_str()),
        Cond::Config => true,
        Cond::BlockOrEntity => roll.block || roll.entity,
        Cond::Chrysophilite => roll.chrysophilite >= 1,
    }
}

/// Applies every rule to `drops` (item ids and counts), as the loot modifiers would.
fn apply_rules(roll: &Roll, drops: &mut Vec<(String, i32)>, item_in_tag: &dyn Fn(&str, &str) -> bool) {
    for rule in rules() {
        match &*rule {
            Rule::Add { conds, entry, funcs, absent } => {
                if !conds.iter().all(|c| holds(roll, c)) || absent.iter().any(|t| drops.iter().any(|(i, _)| item_in_tag(i, t))) {
                    continue;
                }
                let item = match entry {
                    Entry::Item(i) => Some(i.clone()),
                    Entry::TagPreference(t) => (roll.preferred)(t),
                };
                let Some(item) = item else { continue };
                let mut count = 1;
                for f in funcs {
                    count = match f {
                        Func::SetCount { min, max, add } => uniform_i(*min, *max) + if *add { count } else { 0 },
                        Func::ModifierBonus { modifier, formula, include_base } => {
                            let level = roll.tool.map_or(0, |t| modifier_level(t, modifier)) - i32::from(!include_base);
                            if level > 0 { apply_formula(*formula, count, level) } else { count }
                        }
                        Func::ChrysophiliteBonus { formula, include_base } => {
                            let level = roll.chrysophilite - i32::from(!include_base);
                            if level > 0 { apply_formula(*formula, count, level) } else { count }
                        }
                        Func::Looting { min, max } => count + (0..roll.looting).map(|_| uniform_i(*min, *max)).sum::<i32>(),
                    };
                }
                if count > 0 {
                    drops.push((item, count));
                }
            }
            Rule::Replace { conds, original, replacement } => {
                if conds.iter().all(|c| holds(roll, c)) {
                    for (i, _) in drops.iter_mut().filter(|(i, _)| i == original) {
                        *i = replacement.clone();
                    }
                }
            }
            // The tool's processLoot modifiers run from the drop hooks below (severing).
            Rule::Hook { .. } => {}
        }
    }
}

fn hook_on(roll: &Roll) -> bool {
    rules().iter().any(|r| matches!(&**r, Rule::Hook { conds } if conds.iter().all(|c| holds(roll, c))))
}

// ---- The facts a block drop reads, from a Host

/// The server tick: refresh the tag preferences and the players' chrysophilite (once a second, or after a reload).
pub(crate) fn server_tick(host: &dyn ServerHost) {
    static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tick = TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stale = FACTS.lock().unwrap_or_else(|e| e.into_inner()).is_none();
    if !stale && tick % 20 != 0 {
        return;
    }
    let chrysophilite = FACTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|f| f.chrysophilite.clone()).unwrap_or_default();
    let mut facts = Facts { chrysophilite, ..Facts::default() };
    for rule in rules() {
        let mut tags = Vec::new();
        if let Rule::Add { conds, entry, absent, .. } = &*rule {
            for a in absent {
                let items = tag_items(host, a);
                facts.tag_items.insert(a.clone(), items);
            }
            tags.extend(conds.iter().filter_map(|c| if let Cond::TagFilled(t) = c { Some(t.clone()) } else { None }));
            if let Entry::TagPreference(t) = entry {
                tags.push(t.clone());
            }
        }
        for t in tags {
            let item = preferred_item(&*host, &t);
            facts.preferred.insert(t, item);
        }
    }
    *FACTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(facts);
}

/// The player tick: the acting player's chrysophilite level (the attribute their armor gives).
pub(crate) fn player_tick(host: &dyn ServerHost) {
    let Some(p) = host.players().acting_player() else { return };
    let level = LivingRef::new(p.entity()).attribute_value(host, crate::tools::combat::CHRYSOPHILITE).unwrap_or(0.0) as i32;
    let mut facts = FACTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = facts.as_mut() {
        f.chrysophilite.insert(p.uuid(), level);
    }
}

/// Mantle's tag preference: the tag's item from the first mod in its preference order (`minecraft`, then
/// `tconstruct`, then the rest by name).
fn preferred_item(host: &dyn Host, tag_id: &str) -> Option<String> {
    let id = Identifier::parse(tag_id).ok()?;
    let tags = host.tags();
    let key = tags.key(&id)?;
    let TagMembers::Members(members) = tags.items(key) else { return None };
    let mut items: Vec<String> = members.into_iter().map(|k| key_id(k).to_owned()).collect();
    let rank = |i: &str| match i.split(':').next() {
        Some("minecraft") => 0,
        Some("tconstruct") => 1,
        _ => 2,
    };
    items.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
    items.into_iter().next()
}

fn tag_items(host: &dyn Host, tag_id: &str) -> Vec<String> {
    let Ok(id) = Identifier::parse(tag_id) else { return Vec::new() };
    let tags = host.tags();
    match tags.key(&id).map(|k| tags.items(k)) {
        Some(TagMembers::Members(v)) => v.into_iter().map(|k| key_id(k).to_owned()).collect(),
        _ => Vec::new(),
    }
}

fn item_in_tag(host: &dyn Host, item: &str, tag_id: &str) -> bool {
    let (Ok(tag), Ok(item)) = (Identifier::parse(tag_id), Identifier::parse(item)) else { return false };
    let tags = host.tags();
    match (tags.key(&tag), host.item_key(&item)) {
        (Some(t), Some(i)) => tags.is_in_tag(i, t).unwrap_or(false),
        _ => false,
    }
}

// ---- The drop hooks

/// A block broken with a tool: the rules' nuggets (lustrous, chrysophilite).
pub(crate) fn block_drops(level: &dyn LevelHost, event: &mut BlockDropsEvent<'_>) {
    if event.breaker.is_none() || rules().is_empty() {
        return;
    }
    let facts = FACTS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(facts) = facts.as_ref() else { return };
    let key = event.state.block();
    let in_block_tag = |t: &str| level.block_in_tag(key, t).unwrap_or(false);
    let no_entity = |_: &str| false;
    let preferred = |t: &str| facts.preferred.get(t).cloned().flatten();
    let tool = event.tool.as_ref().and_then(|t| t.project());
    let roll = Roll {
        tool: tool.as_ref(),
        block: true,
        entity: false,
        in_block_tag: &in_block_tag,
        entity_in_tag: &no_entity,
        table: level.state_block(event.state).map(|b| format!("{}:blocks/{}", b.split(':').next().unwrap_or("minecraft"), b.split(':').nth(1).unwrap_or(b))),
        chrysophilite: event.breaker.and_then(|u| facts.chrysophilite.get(&u).copied()).unwrap_or(0),
        looting: 0,
        preferred: &preferred,
    };
    let mut drops: Vec<(String, i32)> =
        event.drops.iter().filter_map(|d| d.stack().map(|s| (key_id(s.item()).to_owned(), s.count() as i32))).collect();
    let before = drops.len();
    let ore_item = |item: &str, t: &str| facts.tag_items.get(t).is_some_and(|v| v.iter().any(|i| i == item));
    apply_rules(&roll, &mut drops, &ore_item);
    let at = [event.pos.x as f64 + 0.5, event.pos.y as f64 + 0.5, event.pos.z as f64 + 0.5];
    for (item, count) in drops.into_iter().skip(before) {
        if let Some(stack) = stack_of(&item, count as u32) {
            let _ = event.drops.push(ItemDrop::new(stack, at, [0.0, 0.1, 0.0], 10));
        }
    }
}

/// A mob killed: the rules (tasty bacon, necrotic bones) and, with the modifier hook on, the tool's severing.
pub(crate) fn living_drops(host: &mut dyn ServerHost, event: &mut LivingDropsEvent<'_>) {
    if rules().is_empty() {
        return;
    }
    let attacker = (event.source.causing > 0).then(|| host.entity(event.source.causing as u32)).flatten();
    let tool = attacker.and_then(|a| LivingRef::new(a).hand_item(&*host, Hand::Main));
    // The luck modifier grants vanilla looting into the enchantments component (EnchantmentModifierHook).
    let looting = tool.as_ref().map_or(0, |t| match component(t, "minecraft:enchantments") {
        Some(Tag::Compound(c)) => gi(&c, "minecraft:looting", 0),
        _ => 0,
    });
    let kind = event.kind.to_owned();
    let entity_in_tag = |t: &str| host.entity_type_in_tag(&kind, t);
    let no_block = |_: &str| false;
    let preferred = |t: &str| preferred_item(&*host, t);
    let (ns, path) = kind.split_once(':').unwrap_or(("minecraft", &kind));
    let roll = Roll {
        tool: tool.as_ref(),
        block: false,
        entity: true,
        in_block_tag: &no_block,
        entity_in_tag: &entity_in_tag,
        table: Some(format!("{ns}:entities/{path}")),
        chrysophilite: 0,
        looting,
        preferred: &preferred,
    };
    let mut drops: Vec<(String, i32)> = event.drops.iter().map(|d| (sid(&d.stack).to_owned(), d.stack.count() as i32)).collect();
    let item_tag = |i: &str, t: &str| item_in_tag(&*host, i, t);
    apply_rules(&roll, &mut drops, &item_tag);
    let hook = hook_on(&roll);
    // Severing (SeveringModule.processLoot): a head when none dropped yet.
    if hook && let Some(t) = tool.as_ref() {
        let level = modifier_level(t, "tconstruct:severing");
        if level > 0 && !drops.iter().any(|(i, _)| item_in_tag(&*host, i, "c:heads")) {
            drops.extend(severing(host, &kind, event.entity, level as f32, looting as f32));
        }
    }
    let at = event.drops.first().map(|d| d.position).or_else(|| host.entity(event.entity).and_then(|e| e.position(&*host))).unwrap_or([0.0; 3]);
    // The drops as the rules left them: replaced items keep their stacks' other data.
    let mut out = Vec::with_capacity(drops.len());
    for (n, (item, count)) in drops.into_iter().enumerate() {
        match event.drops.get(n) {
            Some(d) if sid(&d.stack) == item => out.push(d.clone()),
            Some(d) => {
                if let Some(stack) = stack_of(&item, d.stack.count()) {
                    out.push(ItemDrop::new(stack, d.position, d.velocity, d.pickup_delay));
                }
            }
            None => {
                if let Some(stack) = stack_of(&item, count as u32) {
                    out.push(ItemDrop::new(stack, at, [0.0, 0.1, 0.0], 10));
                }
            }
        }
    }
    *event.drops = out;
}

/// SeveringModule.processLoot: each matching severing recipe at `level * (per_level_chance + looting_bonus *
/// looting)` (doubled for `tconstruct:rare_mobs`), its result (a count over 1 is 1 to count), the child's result
/// for a baby of an ageable recipe.
fn severing(host: &mut dyn ServerHost, kind: &str, entity: u32, level: f32, looting: f32) -> Vec<(String, i32)> {
    let mut out = Vec::new();
    let rare = if host.entity_type_in_tag(kind, "tconstruct:rare_mobs") { 2.0 } else { 1.0 };
    let baby = host.entity(entity).and_then(|e| LivingRef::new(e).baby(&*host)).unwrap_or(false);
    for recipe_type in ["tconstruct:severing", "tconstruct:ageable_severing"] {
        for r in host.recipes_of_type(recipe_type).iter() {
            let Some(json) = crate::smeltery::recipes::chosen(&*host, &r.json) else { continue };
            if !entity_matches(&*host, &json["entity"], kind) {
                continue;
            }
            let per_level = json["per_level_chance"].as_f64().unwrap_or(0.05) as f32;
            let looting_bonus = json["looting_bonus"].as_f64().unwrap_or(0.01) as f32;
            if rand_f32() >= level * (per_level + looting_bonus * looting) * rare {
                continue;
            }
            let result = if baby && recipe_type == "tconstruct:ageable_severing" { &json["child_result"] } else { &json["result"] };
            let (item, count) = match result {
                Value::String(i) => (i.clone(), 1),
                v => (v["item"].as_str().or_else(|| v["id"].as_str()).unwrap_or_default().to_owned(), v["count"].as_i64().unwrap_or(1) as i32),
            };
            if item.is_empty() {
                continue;
            }
            let count = if count > 1 { 1 + (rand_f32() * count as f32) as i32 } else { count };
            out.push((item, count));
        }
    }
    out
}

fn stack_of(item: &str, count: u32) -> Option<ItemStack> {
    ItemStack::new(item_key(item)?, count)
}

/// An `EntityIngredient`: `{"type": id}`, `{"types": [...]}`, `{"tag": tag}` or a list of those.
fn entity_matches(host: &dyn Host, v: &Value, kind: &str) -> bool {
    match v {
        Value::Array(list) => list.iter().any(|e| entity_matches(host, e, kind)),
        Value::Object(_) => {
            if let Some(t) = v["type"].as_str() {
                return t == kind;
            }
            if let Some(ts) = v["types"].as_array() {
                return ts.iter().any(|t| t.as_str() == Some(kind));
            }
            if let Some(t) = v["tag"].as_str() {
                return host.entity_type_in_tag(kind, t);
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tinkers_own_files_decode() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/tconstruct/loot_modifiers");
        let mut files = vec![dir.join("tasty_bacon.json"), dir.join("wither_bone.json"), dir.join("modifier_hook.json"), dir.join("chrysophilite_modifier.json")];
        files.extend(std::fs::read_dir(dir.join("lustrous")).unwrap().map(|e| e.unwrap().path()));
        for f in files {
            let v: Value = mod_api::serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
            let kind = v["type"].as_str().unwrap().split(':').nth(1).unwrap().to_owned();
            decode(&kind, &v).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        }
    }

    #[test]
    fn lustrous_decodes_to_nuggets_from_ores_without_the_ore() {
        let v: Value = mod_api::serde_json::from_str(include_str!("../../data/tconstruct/loot_modifiers/lustrous/iron.json")).unwrap();
        let Rule::Add { conds, entry, funcs, absent } = decode("add_entry", &v).unwrap() else { panic!() };
        assert!(conds.contains(&Cond::BlockTag("c:ores/iron".into())));
        assert!(conds.contains(&Cond::HasModifier("tconstruct:lustrous".into())));
        assert_eq!(entry, Entry::TagPreference("c:nuggets/iron".into()));
        assert!(matches!(funcs[0], Func::SetCount { min, max, add: false } if min == 2.0 && max == 4.0));
        assert_eq!(absent, vec!["c:ores/iron".to_owned()]);
    }

    #[test]
    fn ore_drops_multiplies_by_one_to_level_plus_one() {
        for _ in 0..200 {
            let n = apply_formula(Formula::OreDrops, 3, 2);
            assert!(n % 3 == 0 && (3..=9).contains(&n), "{n}");
        }
    }
}
