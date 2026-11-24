//! Stage 3C, modifier recipes and the modifier worktable (spec-modifiers.md 3-5; upstream `tconstruct.rs`
//! 14237-15970):
//! - the tinker station's and anvils' modifier recipes (`tconstruct:modifier`, `incremental_modifier`,
//!   `multilevel_modifier`, `multilevel_incremental_modifier`, `swappable_modifier`, `overslime_modifier`,
//!   `modifier_repair`), the modifier crystal, slots and levels (SlotType, ToolStack.tryValidate) and their errors;
//! - the Modifier Worktable's recipes (`remove_modifier`, `extract_modifier`, `modifier_sorting`,
//!   `modifier_set_worktable`, `toggle_interaction`) and salvage (`modifier_salvage`), its buttons and screen.
//! Both go through the stations' virtual result (`station.rs`): taking the result charges its plan (`Use`) atomically.
//! A screen gets no item stacks from the fork, so the station's error and the worktable's buttons reach it in data
//! slots: modifiers by their index in the registry's sorted ids, errors by code (`Err`).

use super::data::*;
use super::modifier::{self as md, ModEntry, ModModule, Modifiers, ToolMods};
use super::station::Use;
use super::tool::*;
use crate::smeltery::casting_recipes::tag_items;
use crate::smeltery::recipes::{chosen, ingredient_items};
use crate::smeltery::util::*;
use mod_api::serde_json::Value;
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub(crate) const MODIFIER_CRYSTAL: &str = "tconstruct:modifier_crystal";
pub(crate) const WT_RESULT: usize = 3;
/// The worktable's data slots: selection, button count, recipe index, message code and its two arguments, then
/// (modifier index, level) per button (tools/make_fork_content.py WT_DATA).
pub(crate) const WT_HEAD: usize = 6;
pub(crate) const WT_BUTTONS: usize = 24;
/// SlotType's registered types, in order.
pub(crate) const SLOT_TYPES: [&str; 4] = ["upgrades", "defense", "abilities", "souls"];

// ---- Errors (the recipes' RecipeResult.failure texts), as codes for the screens

/// A recipe's error: what the server finds, sent to the screen as [code, a, b, c].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Err {
    Slots { slot: String, count: i32 },
    MinLevel { id: String, level: i32, traits: bool },
    MaxLevel { id: String, max: i32, traits: bool },
    ValidateSlots { slot: String },
    Requirement { id: String },
    OverslimeFull,
    Swapped { id: String },
    TooFew,
    SortTwo,
}

fn slot_index(slot: &str) -> i32 {
    SLOT_TYPES.iter().position(|s| *s == slot).map_or(0, |i| i as i32)
}

impl Err {
    pub(crate) fn encode(&self, reg: &Modifiers) -> [i32; 4] {
        match self {
            Err::Slots { slot, count } => [1, slot_index(slot), *count, 0],
            Err::MinLevel { id, level, traits } => [2, reg.index(id), *level, i32::from(*traits)],
            Err::MaxLevel { id, max, traits } => [3, reg.index(id), *max, i32::from(*traits)],
            Err::ValidateSlots { slot } => [4, slot_index(slot), 0, 0],
            Err::Requirement { id } => [5, reg.index(id), 0, 0],
            Err::OverslimeFull => [6, 0, 0, 0],
            Err::Swapped { id } => [7, reg.index(id), 0, 0],
            Err::TooFew => [8, 0, 0, 0],
            Err::SortTwo => [9, 0, 0, 0],
        }
    }
}

fn fill(text: String, args: &[String]) -> String {
    let mut out = text;
    for a in args {
        out = out.replacen("%s", a, 1);
    }
    out
}

/// The error text the screen shows for a code (the recipes' translation keys, English fallbacks).
pub(crate) fn error_text(host: &dyn ClientHost, code: [i32; 4]) -> Option<String> {
    let reg = md::registry(host);
    let name = |i: i32| usize::try_from(i).ok().and_then(|i| reg.ids.get(i)).cloned().unwrap_or_default();
    let slot = |i: i32| {
        let s = SLOT_TYPES.get(i.max(0) as usize).copied().unwrap_or("upgrades");
        lang(host, &format!("stat.tconstruct.slot.display.{s}"), &words(s))
    };
    let modname = |i: i32| {
        let id = name(i);
        lang(host, &md::modifier_key(Some(&reg), &id), &words(id.split(':').nth(1).unwrap_or("")))
    };
    let [c, a, b, t] = code;
    Some(match c {
        1 if b == 1 => fill(lang(host, "recipe.tconstruct.modifier.not_enough_slot", "Need at least 1 %s slot to add this modifier"), &[slot(a)]),
        1 => fill(lang(host, "recipe.tconstruct.modifier.not_enough_slots", "Need at least %s %s slots to add this modifier"), &[b.to_string(), slot(a)]),
        2 => {
            let (key, en) = if t != 0 { ("recipe.tconstruct.modifier.min_level.traits", "This recipe requires the tool to have at least %s to apply (including traits)") } else { ("recipe.tconstruct.modifier.min_level", "This recipe requires the tool to have at least %s to apply (ignoring traits)") };
            fill(lang(host, key, en), &[md::level_name(host, &reg, &name(a), b).0])
        }
        3 => {
            let (key, en) = if t != 0 { ("recipe.tconstruct.modifier.max_level.traits", "This recipe cannot increase %s above level %s (including traits)") } else { ("recipe.tconstruct.modifier.max_level", "This recipe cannot increase %s above level %s (ignoring traits)") };
            fill(lang(host, key, en), &[modname(a), b.to_string()])
        }
        4 => fill(lang(host, "recipe.tconstruct.modifier.validate_slots", "This change leaves the tool with too few %s slots"), &[slot(a)]),
        5 => {
            let key = reg.get(&name(a)).and_then(|m| m.modules.iter().find_map(|x| if let ModModule::Requirements { key, .. } = x { Some(key.clone()) } else { None })).unwrap_or_default();
            lang(host, &key, "The tool does not meet this modifier's requirements")
        }
        6 => lang(host, "recipe.tconstruct.overslime.at_capacity", "Tool has no more space for overslime"),
        7 => fill(lang(host, "recipe.tconstruct.swappable.already_present", "Tool already contains %s with variant %s"), &[modname(a), String::new()]),
        8 => lang(host, "recipe.tconstruct.enchantment_converting.too_few", "Need to provide more inputs to convert an enchantment of a higher level"),
        9 => lang(host, "recipe.tconstruct.modifier_sorting.not_enough_modifiers", "Tool needs at least two modifiers to sort"),
        _ => return None,
    })
}

// ---- Slots and validation (SlotType, ToolStack.tryValidate)

/// ToolStack.tryValidate on a rebuilt tool: no slot type below 0 free, then each modifier's requirements.
pub(crate) fn try_validate(host: &dyn Host, stack: &ItemStack) -> Option<Err> {
    let mut slots: Vec<String> = SLOT_TYPES.iter().map(|s| (*s).to_owned()).collect();
    for comp in [md::C_PERSISTENT, md::C_VOLATILE] {
        if let Some(Tag::Compound(c)) = component(stack, comp) {
            for (k, v) in c.iter() {
                let k = k.to_string_lossy().to_string();
                if matches!(v, Tag::Int(_)) && !k.contains(':') && !slots.contains(&k) {
                    slots.push(k);
                }
            }
        }
    }
    if let Some(slot) = slots.into_iter().find(|s| md::free_slots(stack, s) < 0) {
        return Some(Err::ValidateSlots { slot });
    }
    let tm = ToolMods::of(host, stack);
    let view = tm.view(sid(stack));
    for (id, _) in &tm.mods {
        let Some(m) = tm.reg.get(id) else { continue };
        for module in &m.modules {
            if let ModModule::Requirements { requirement, .. } = module {
                if !requirement.matches(&tm.reg, &view) {
                    return Some(Err::Requirement { id: id.clone() });
                }
            }
        }
    }
    None
}

// ---- The recipes

/// SizedIngredient: the items it takes and how many a match needs.
#[derive(Clone, Debug, Default)]
pub(crate) struct SizedIng {
    items: HashSet<String>,
    amount: u32,
}

impl SizedIng {
    fn test(&self, s: &ItemStack) -> bool {
        s.count() >= self.amount && self.items.contains(sid(s))
    }
}

fn ing_items(host: &dyn Host, v: &Value) -> HashSet<String> {
    match v {
        Value::Array(a) => a.iter().flat_map(|x| ing_items(host, x)).collect(),
        Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("mantle:fluid_container") => ing_items(host, &v["display"]),
        Value::Object(o) if o.contains_key("ingredient") => ing_items(host, &v["ingredient"]),
        _ => ingredient_items(host, v, &mut Vec::new()),
    }
}

fn sized_ing(host: &dyn Host, v: &Value) -> SizedIng {
    SizedIng { items: ing_items(host, v), amount: v["amount_needed"].as_u64().unwrap_or(1).clamp(1, 64) as u32 }
}

fn sized_list(host: &dyn Host, v: &Value) -> Vec<SizedIng> {
    match v {
        Value::Array(a) => a.iter().map(|x| sized_ing(host, x)).collect(),
        Value::Null => Vec::new(),
        other => vec![sized_ing(host, other)],
    }
}

fn item_out(v: &Value) -> Option<ItemStack> {
    match v {
        Value::String(s) => new_stack(&md::pattern_mc(s), 1),
        Value::Object(_) => new_stack(&md::pattern_mc(v["item"].as_str()?), v["count"].as_u64().unwrap_or(1).clamp(1, 64) as u32),
        _ => None,
    }
}

type SlotCount = Option<(String, i32)>;

fn slot_count(v: &Value) -> SlotCount {
    v.as_object()?.iter().next().map(|(k, n)| (k.clone(), n.as_i64().unwrap_or(1) as i32))
}

#[derive(Clone, Debug)]
pub(crate) enum AddKind {
    Plain(Vec<SizedIng>),
    Incremental { input: HashSet<String>, per_item: i32, needed: i32, leftover: Option<ItemStack> },
    Swappable { inputs: Vec<SizedIng>, value: String },
}

#[derive(Clone, Debug)]
pub(crate) struct AddRecipe {
    /// The recipe's id (`tconstruct:tools/modifiers/upgrade/haste`).
    id: String,
    tools: HashSet<String>,
    max_tool_size: u32,
    result: String,
    level: (i32, i32),
    slots: SlotCount,
    levels: Vec<((i32, i32), SlotCount)>,
    crystal: bool,
    check_trait: bool,
    kind: AddKind,
}

#[derive(Clone, Debug)]
pub(crate) enum StationRecipe {
    Add(AddRecipe),
    Overslime { tools: HashSet<String>, items: HashSet<String>, restore: i32 },
    Repair { modifier: String, items: HashSet<String>, amount: i32 },
}

#[derive(Clone, Debug)]
pub(crate) struct Salvage {
    tools: HashSet<String>,
    max_tool_size: u32,
    modifier: String,
    level: (i32, i32),
    slots: (String, i32),
}

/// ModifierPredicate: single, tag, slot_type, and/or/inverted.
#[derive(Clone, Debug)]
pub(crate) enum ModPred {
    Any,
    Single(String),
    Tag(String),
    Slot(Option<String>),
    And(Vec<ModPred>),
    Or(Vec<ModPred>),
    Not(Box<ModPred>),
    Never,
}

fn parse_mod_pred(v: &Value, key: &str) -> ModPred {
    if v.is_null() {
        return ModPred::Any;
    }
    let kind = v.as_str().or_else(|| v[key].as_str()).unwrap_or("");
    let list = |k: &str| -> Vec<ModPred> { v[k].as_array().into_iter().flatten().map(|p| parse_mod_pred(p, "type")).collect() };
    match kind.trim_start_matches("tconstruct:") {
        "mantle:any" | "mantle:always" | "any" => ModPred::Any,
        "single" => ModPred::Single(md::pattern_tc(v["modifier"].as_str().unwrap_or(""))),
        "tag" => ModPred::Tag(md::pattern_tc(v["tag"].as_str().unwrap_or(""))),
        "slot_type" => ModPred::Slot(v["slot"].as_str().map(str::to_owned)),
        "mantle:and" => ModPred::And(list("predicates")),
        "mantle:or" => ModPred::Or(list("predicates")),
        "mantle:inverted" => ModPred::Not(Box::new(parse_mod_pred(v, "inverted_type"))),
        _ => ModPred::Never,
    }
}

impl ModPred {
    fn matches(&self, recipes: &ModRecipes, reg: &Modifiers, id: &str) -> bool {
        match self {
            ModPred::Any => true,
            ModPred::Single(s) => s == id,
            ModPred::Tag(t) => reg.in_tag(id, t),
            ModPred::Slot(s) => recipes.slot_types.get(id).is_some_and(|set| set.contains(s)),
            ModPred::And(l) => l.iter().all(|p| p.matches(recipes, reg, id)),
            ModPred::Or(l) => l.iter().any(|p| p.matches(recipes, reg, id)),
            ModPred::Not(p) => !p.matches(recipes, reg, id),
            ModPred::Never => false,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum WorkKind {
    Remove { name: String, leftovers: Vec<ItemStack>, pred: ModPred, extract: bool },
    Sort,
    Set { key: String, add: bool, traits: bool, pred: ModPred },
    Toggle,
}

#[derive(Clone, Debug)]
pub(crate) struct WorkRecipe {
    tools: SizedIng,
    inputs: Vec<SizedIng>,
    kind: WorkKind,
}

#[derive(Default)]
pub(crate) struct ModRecipes {
    station: Vec<StationRecipe>,
    salvage: Vec<Salvage>,
    work: Vec<WorkRecipe>,
    /// ModifierRecipeLookup: each modifier's recipes' slot types (None: slotless).
    pub(crate) slot_types: HashMap<String, HashSet<Option<String>>>,
}

const STATION_TYPES: [&str; 7] = [
    "tconstruct:modifier",
    "tconstruct:incremental_modifier",
    "tconstruct:multilevel_modifier",
    "tconstruct:multilevel_incremental_modifier",
    "tconstruct:swappable_modifier",
    "tconstruct:overslime_modifier",
    "tconstruct:modifier_repair",
];
const WORK_TYPES: [&str; 5] = ["tconstruct:remove_modifier", "tconstruct:extract_modifier", "tconstruct:modifier_sorting", "tconstruct:modifier_set_worktable", "tconstruct:toggle_interaction"];
const SALVAGE: &str = "tconstruct:modifier_salvage";

fn add_recipe(host: &dyn Host, kind: &str, j: &Value) -> Option<AddRecipe> {
    let multilevel = kind.contains("multilevel");
    let levels: Vec<((i32, i32), SlotCount)> = if multilevel { j["levels"].as_array()?.iter().map(|e| (md::int_range(&e["level"], 1, 32767), slot_count(&e["slots"]))).collect() } else { Vec::new() };
    let (level, slots) = match levels.first() {
        Some((l, s)) => (*l, s.clone()),
        None => (md::int_range(&j["level"], 1, 32767), slot_count(&j["slots"])),
    };
    let result = match &j["result"] {
        Value::String(s) => md::pattern_tc(s),
        other => md::pattern_tc(other["name"].as_str()?),
    };
    let add_kind = if kind.ends_with("incremental_modifier") {
        AddKind::Incremental {
            input: ing_items(host, &j["input"]),
            per_item: j["amount_per_item"].as_i64().unwrap_or(1).max(1) as i32,
            needed: j["needed_per_level"].as_i64().unwrap_or(1).max(1) as i32,
            leftover: item_out(&j["leftover"]),
        }
    } else if kind.ends_with("swappable_modifier") {
        AddKind::Swappable { inputs: sized_list(host, &j["inputs"]), value: j["result"]["value"].as_str().unwrap_or("").to_owned() }
    } else {
        AddKind::Plain(sized_list(host, &j["inputs"]))
    };
    Some(AddRecipe {
        id: String::new(),
        tools: ing_items(host, &j["tools"]),
        max_tool_size: j["max_tool_size"].as_u64().unwrap_or(16).clamp(1, 64) as u32,
        result,
        level,
        slots,
        levels,
        crystal: j["allow_crystal"].as_bool().unwrap_or(true),
        check_trait: j["check_trait_level"].as_bool().unwrap_or(false),
        kind: add_kind,
    })
}

fn work_recipe(host: &dyn Host, kind: &str, j: &Value) -> Option<WorkRecipe> {
    let modifiable = || SizedIng { items: tag_items(host, "tconstruct:modifiable"), amount: 1 };
    let tools = |v: &Value| if v.is_null() { modifiable() } else { sized_ing(host, v) };
    let name = j["name"].as_str().unwrap_or("modifiers").to_owned();
    let (tool, work) = match kind {
        "tconstruct:remove_modifier" | "tconstruct:extract_modifier" => (
            tools(&j["tools"]),
            WorkKind::Remove {
                name,
                leftovers: j["leftovers"].as_array().into_iter().flatten().filter_map(item_out).collect(),
                pred: parse_mod_pred(&j["modifier_predicate"], "type"),
                extract: kind.ends_with("extract_modifier"),
            },
        ),
        "tconstruct:modifier_sorting" => (modifiable(), WorkKind::Sort),
        "tconstruct:modifier_set_worktable" => (
            tools(&j["tools"]),
            WorkKind::Set {
                key: md::pattern_tc(j["data_key"].as_str()?),
                add: j["add_to_set"].as_bool().unwrap_or(true),
                traits: j["allow_traits"].as_bool().unwrap_or(false),
                pred: parse_mod_pred(&j["modifier_predicate"], "type"),
            },
        ),
        _ => (tools(&j["tools"]), WorkKind::Toggle),
    };
    Some(WorkRecipe { tools: tool, inputs: sized_list(host, &j["inputs"]), kind: work })
}

static RECIPES: Mutex<Option<(Key, Arc<ModRecipes>)>> = Mutex::new(None);

/// The modifier recipes of this data load, by id, the salvages, and the slot types each modifier's recipes use.
pub(crate) fn mod_recipes(host: &dyn Host) -> Arc<ModRecipes> {
    let kinds: Vec<&str> = STATION_TYPES.iter().chain(WORK_TYPES.iter()).copied().chain([SALVAGE]).collect();
    let key = recipes_key(host, &kinds);
    if let Some(r) = RECIPES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, r)| r.clone()) {
        return r;
    }
    let mut out = ModRecipes::default();
    let sorted = |kind: &str| -> Vec<(String, Value)> {
        let mut v: Vec<(String, Value)> = host.recipes_of_type(kind).iter().filter_map(|r| Some((r.id.clone(), chosen(host, &r.json)?.clone()))).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    let mut station: Vec<(String, StationRecipe)> = Vec::new();
    for kind in STATION_TYPES {
        for (id, j) in sorted(kind) {
            let r = match kind {
                "tconstruct:overslime_modifier" => StationRecipe::Overslime {
                    tools: if j["tools"].is_null() { tag_items(host, "tconstruct:modifiable/durability") } else { ing_items(host, &j["tools"]) },
                    items: ing_items(host, &j["ingredient"]),
                    restore: j["restore_amount"].as_i64().unwrap_or(1).max(1) as i32,
                },
                "tconstruct:modifier_repair" => StationRecipe::Repair {
                    modifier: md::pattern_tc(j["modifier"].as_str().unwrap_or("")),
                    items: ing_items(host, &j["ingredient"]),
                    amount: j["repair_amount"].as_i64().unwrap_or(1).max(1) as i32,
                },
                _ => {
                    let Some(mut a) = add_recipe(host, kind, &j) else { continue };
                    a.id.clone_from(&id);
                    out.slot_types.entry(a.result.clone()).or_default().insert(a.slots.as_ref().map(|s| s.0.clone()));
                    StationRecipe::Add(a)
                }
            };
            station.push((id, r));
        }
    }
    station.sort_by(|a, b| a.0.cmp(&b.0));
    out.station = station.into_iter().map(|(_, r)| r).collect();
    let mut work: Vec<(String, WorkRecipe)> = Vec::new();
    for kind in WORK_TYPES {
        for (id, j) in sorted(kind) {
            if let Some(r) = work_recipe(host, kind, &j) {
                work.push((id, r));
            }
        }
    }
    work.sort_by(|a, b| a.0.cmp(&b.0));
    out.work = work.into_iter().map(|(_, r)| r).collect();
    for (_, j) in sorted(SALVAGE) {
        let Some(slots) = slot_count(&j["slots"]) else { continue };
        out.salvage.push(Salvage {
            tools: ing_items(host, &j["tools"]),
            max_tool_size: j["max_tool_size"].as_u64().unwrap_or(16).clamp(1, 64) as u32,
            modifier: md::pattern_tc(j["modifier"].as_str().unwrap_or("")),
            level: md::int_range(&j["level"], 1, 32767),
            slots,
        });
    }
    let out = Arc::new(out);
    *RECIPES.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, out.clone()));
    out
}

// ---- The tinker station's modifier recipes (ITinkerStationRecipe.matches / getValidatedResult / updateInputs)

/// RecipeResult: pass, failure (its error), success (the result and what taking it uses).
pub(crate) enum Outcome {
    Pass,
    Fail(Err),
    Done(ItemStack, Vec<Use>),
}

/// ModifierCrystalItem.getModifier.
fn crystal_modifier(stack: &ItemStack) -> Option<String> {
    (sid(stack) == MODIFIER_CRYSTAL).then(|| gs(&custom_data(stack), "modifier")).flatten()
}

fn crystal_of(id: &str, count: u32) -> Option<ItemStack> {
    let mut s = new_stack(MODIFIER_CRYSTAL, count.max(1))?;
    let mut c = Compound::new();
    ss(&mut c, "modifier", id);
    set_custom_data(&mut s, c);
    Some(s)
}

/// AbstractModifierRecipe.matchesCrystal: exactly one input, a crystal of the modifier.
fn crystal_matches(inputs: &[Option<ItemStack>], id: &str) -> bool {
    let mut found = false;
    for s in inputs.iter().flatten() {
        if found || crystal_modifier(s).as_deref() != Some(id) {
            return false;
        }
        found = true;
    }
    found
}

/// ModifierRecipe.checkMatch: each ingredient claims the first unused input it takes; nothing left over.
fn claim(ings: &[SizedIng], inputs: &[Option<ItemStack>]) -> Option<Vec<usize>> {
    if ings.is_empty() {
        return None;
    }
    let mut used: Vec<bool> = inputs.iter().map(Option::is_none).collect();
    let mut out = Vec::with_capacity(ings.len());
    for ing in ings {
        let i = (0..inputs.len()).find(|&i| !used[i] && inputs[i].as_ref().is_some_and(|s| ing.test(s)))?;
        used[i] = true;
        out.push(i);
    }
    used.iter().all(|u| *u).then_some(out)
}

fn only_ingredient(items: &HashSet<String>, inputs: &[Option<ItemStack>]) -> bool {
    let mut found = false;
    for s in inputs.iter().flatten() {
        if !items.contains(sid(s)) {
            return false;
        }
        found = true;
    }
    found
}

fn available(items: &HashSet<String>, inputs: &[Option<ItemStack>], per: i32) -> i32 {
    inputs.iter().flatten().filter(|s| items.contains(sid(s))).map(|s| s.count() as i32 * per).sum()
}

/// IncrementalModifierRecipe.updateInputs: ⌈amount / per⌉ items from the inputs in order, the leftover for the part of
/// an item not used. Station slot = input + 1.
fn incremental_uses(items: &HashSet<String>, inputs: &[Option<ItemStack>], amount: i32, per: i32, leftover: Option<&ItemStack>, uses: &mut Vec<Use>) {
    if amount <= 0 {
        return;
    }
    let mut need = amount / per;
    let rest = amount % per;
    if rest > 0 {
        need += 1;
        if let Some(l) = leftover.and_then(|l| with_count(l, (per - rest) as u32 * l.count())) {
            uses.push(Use::Give(l));
        }
    }
    for (i, s) in inputs.iter().enumerate() {
        let Some(s) = s.as_ref().filter(|s| items.contains(sid(s))) else { continue };
        if need <= 0 {
            break;
        }
        let take = (s.count() as i32).min(need);
        uses.push(Use::Shrink { slot: i + 1, item: s.clone(), amount: take as u32 });
        need -= take;
    }
}

fn check_level(id: &str, range: (i32, i32), level: i32, traits: bool) -> Option<Err> {
    if level < range.0 {
        Some(Err::MinLevel { id: id.to_owned(), level: range.0 - 1, traits })
    } else if level > range.1 {
        Some(Err::MaxLevel { id: id.to_owned(), max: range.1, traits })
    } else {
        None
    }
}

fn check_slots(tool: &ItemStack, slots: &SlotCount) -> Option<Err> {
    let (slot, count) = slots.as_ref()?;
    (md::free_slots(tool, slot) < *count).then(|| Err::Slots { slot: slot.clone(), count: *count })
}

fn level_entry(r: &AddRecipe, level: i32) -> Result<SlotCount, Err> {
    if let Some((_, s)) = r.levels.iter().find(|((lo, hi), _)| (*lo..=*hi).contains(&level)) {
        return Ok(s.clone());
    }
    let min = r.levels.first().map_or(1, |e| e.0 .0);
    let max = r.levels.last().map_or(32767, |e| e.0 .1);
    Err(check_level(&r.result, (min, max), level, r.check_trait).unwrap_or(Err::TooFew))
}

/// ModifierNBT.withModifier.
pub(crate) fn add_modifier(list: &mut Vec<ModEntry>, id: &str, n: i32) {
    match list.iter_mut().find(|e| e.id == id) {
        Some(e) => e.level += n,
        None => list.push(ModEntry { id: id.to_owned(), level: n, amount: 0, needed: 0 }),
    }
}

fn gcd(a: i32, b: i32) -> i32 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// ModifierNBT.addAmount (IncrementalModifierEntry.addAmount).
fn add_modifier_amount(list: &mut Vec<ModEntry>, id: &str, amount: i32, needed: i32) {
    match list.iter_mut().find(|e| e.id == id) {
        Some(e) if e.needed == 0 => *e = ModEntry::of(id, e.level + 1, amount, needed),
        Some(e) if e.needed == needed => *e = ModEntry::of(id, e.level, e.amount + amount, needed),
        Some(e) => {
            let g = gcd(needed, e.needed);
            let total = e.amount * (needed / g) + amount * (e.needed / g);
            *e = ModEntry::of(id, e.level, total, needed * e.needed / g);
        }
        None => list.push(ModEntry::of(id, 1, amount, needed)),
    }
}

/// ModifierNBT.without: `n` levels off the matching entries, an entry at 0 dropped.
pub(crate) fn remove_modifier(list: &mut Vec<ModEntry>, id: &str, mut n: i32) {
    for e in list.iter_mut().filter(|e| e.id == id) {
        if n <= 0 {
            break;
        }
        let take = n.min(e.level);
        e.level -= take;
        n -= take;
    }
    list.retain(|e| e.level > 0);
}

/// Shrinks of the claimed inputs (station slot = input + 1).
fn claimed_uses(ings: &[SizedIng], inputs: &[Option<ItemStack>], times: u32, uses: &mut Vec<Use>) {
    let Some(claimed) = claim(ings, inputs) else { return };
    for (ing, i) in ings.iter().zip(claimed) {
        if let Some(s) = &inputs[i] {
            uses.push(Use::Shrink { slot: i + 1, item: s.clone(), amount: (ing.amount * times).min(s.count()) });
        }
    }
}

/// AbstractModifierRecipe and its kinds: the validated result of an adding recipe that matched.
fn add_outcome(host: &dyn Host, r: &AddRecipe, tool: &ItemStack, inputs: &[Option<ItemStack>]) -> Outcome {
    let upgrades = md::entries_of(tool, md::C_UPGRADES);
    let source = if r.check_trait { md::entries_of(tool, C_MODIFIERS) } else { upgrades.clone() };
    let current = source.iter().find(|e| e.id == r.result).cloned();
    let crystal = r.crystal && crystal_matches(inputs, &r.result);
    let count = tool.count().min(r.max_tool_size).max(1);
    let Some(mut out) = with_count(tool, count) else { return Outcome::Pass };
    let mut ups = upgrades.clone();
    let mut uses = vec![Use::Shrink { slot: 0, item: tool.clone(), amount: count }];
    let crystal_uses = |uses: &mut Vec<Use>| {
        for (i, s) in inputs.iter().enumerate() {
            if let Some(s) = s {
                uses.push(Use::Shrink { slot: i + 1, item: s.clone(), amount: 1 });
            }
        }
    };
    match &r.kind {
        AddKind::Plain(ings) => {
            let new_level = current.as_ref().map_or(0, |e| e.level) + 1;
            let slots = if r.levels.is_empty() {
                if let Some(e) = check_level(&r.result, r.level, new_level, r.check_trait) {
                    return Outcome::Fail(e);
                }
                r.slots.clone()
            } else {
                match level_entry(r, new_level) {
                    Ok(s) => s,
                    Err(e) => return Outcome::Fail(e),
                }
            };
            if let Some(e) = check_slots(tool, &slots) {
                return Outcome::Fail(e);
            }
            if let Some((t, n)) = &slots {
                md::add_persistent_slots(&mut out, t, -n);
            }
            add_modifier(&mut ups, &r.result, 1);
            if crystal {
                crystal_uses(&mut uses);
            } else {
                if claim(ings, inputs).is_none() {
                    return Outcome::Pass;
                }
                claimed_uses(ings, inputs, 1, &mut uses);
            }
        }
        AddKind::Swappable { inputs: ings, value } => {
            let level = upgrades.iter().find(|e| e.id == r.result).map_or(0, |e| e.level);
            if level == 0 {
                if let Some(e) = check_slots(tool, &r.slots) {
                    return Outcome::Fail(e);
                }
                if let Some((t, n)) = &r.slots {
                    md::add_persistent_slots(&mut out, t, -n);
                }
                add_modifier(&mut ups, &r.result, 1);
            } else if md::persistent_string(tool, &r.result).as_deref() == Some(value.as_str()) {
                return Outcome::Fail(Err::Swapped { id: r.result.clone() });
            }
            md::set_persistent(&mut out, &r.result, Some(Tag::String(js(value))));
            if crystal {
                crystal_uses(&mut uses);
            } else {
                if claim(ings, inputs).is_none() {
                    return Outcome::Pass;
                }
                claimed_uses(ings, inputs, 1, &mut uses);
            }
        }
        AddKind::Incremental { input, per_item, needed, leftover } => {
            let new_level_due = crystal || current.as_ref().is_none_or(|e| e.amount_or(0) <= 0);
            let level_now = current.as_ref().map_or(0, |e| e.level);
            let check = if new_level_due { level_now + 1 } else { level_now };
            let slots = if r.levels.is_empty() {
                if let Some(e) = check_level(&r.result, r.level, check, r.check_trait) {
                    return Outcome::Fail(e);
                }
                r.slots.clone()
            } else {
                match level_entry(r, check) {
                    Ok(s) => s,
                    Err(e) => return Outcome::Fail(e),
                }
            };
            if new_level_due {
                if let Some(e) = check_slots(tool, &slots) {
                    return Outcome::Fail(e);
                }
                if let Some((t, n)) = &slots {
                    md::add_persistent_slots(&mut out, t, -n);
                }
            }
            let before = upgrades.iter().find(|e| e.id == r.result).cloned();
            if crystal {
                add_modifier(&mut ups, &r.result, 1);
                crystal_uses(&mut uses);
            } else {
                add_modifier_amount(&mut ups, &r.result, available(input, inputs, *per_item), *needed);
                let after = ups.iter().find(|e| e.id == r.result).cloned();
                let in_need = before.as_ref().map_or(0, |e| e.needed);
                let result_amount = after.as_ref().map_or(0, |e| e.amount_or(*needed));
                let input_amount = before.as_ref().map_or(0, |e| e.amount_or(0));
                let used = if in_need != 0 && in_need != *needed {
                    let g = gcd(in_need, *needed);
                    let scale = in_need / g;
                    let u = result_amount * scale - input_amount * *needed / g;
                    (u + scale - 1) / scale
                } else {
                    result_amount - input_amount
                };
                incremental_uses(input, inputs, used, *per_item, leftover.as_ref(), &mut uses);
            }
        }
    }
    md::set_upgrades(&mut out, &ups);
    rebuild(host, &mut out);
    if let Some(e) = try_validate(host, &out) {
        return Outcome::Fail(e);
    }
    Outcome::Done(out, uses)
}

/// The tinker station's modifier recipes on `tool`: the first that matches (by id) and its validated result.
pub(crate) fn modifier_station(host: &dyn Host, tool: &ItemStack, inputs: &[Option<ItemStack>]) -> Option<Outcome> {
    let tools = tooling(host);
    if !tool_initialized(tool) || !tools.defs.contains_key(sid(tool)) {
        return None;
    }
    let recipes = mod_recipes(host);
    let reg = md::registry(host);
    for r in &recipes.station {
        match r {
            StationRecipe::Add(a) => {
                if reg.get(&a.result).is_none() || !a.tools.contains(sid(tool)) {
                    continue;
                }
                let matched = (a.crystal && crystal_matches(inputs, &a.result))
                    || match &a.kind {
                        AddKind::Plain(ings) | AddKind::Swappable { inputs: ings, .. } => claim(ings, inputs).is_some(),
                        AddKind::Incremental { input, .. } => only_ingredient(input, inputs),
                    };
                if matched {
                    return Some(add_outcome(host, a, tool, inputs));
                }
            }
            StationRecipe::Overslime { tools: t, items, restore } => {
                if !t.contains(sid(tool)) || !only_ingredient(items, inputs) {
                    continue;
                }
                let ups = md::entries_of(tool, md::C_UPGRADES);
                let mut out = with_count(tool, 1)?;
                let (pool, cap) = md::overslime(tool);
                if !ups.iter().any(|e| e.id == "tconstruct:overslime") {
                    if md::modifier_level(tool, "tconstruct:overslime") > 0 && pool >= cap {
                        return Some(Outcome::Fail(Err::OverslimeFull));
                    }
                    let mut ups = ups;
                    add_modifier(&mut ups, "tconstruct:overslime", 1);
                    md::set_upgrades(&mut out, &ups);
                    rebuild(host, &mut out);
                } else if pool >= cap {
                    return Some(Outcome::Fail(Err::OverslimeFull));
                }
                md::add_overslime(&mut out, available(items, inputs, *restore));
                let gained = md::overslime(&out).0 - pool;
                let mut uses = vec![Use::Shrink { slot: 0, item: tool.clone(), amount: 1 }];
                incremental_uses(items, inputs, gained, restore * (1 + md::modifier_level(tool, "tconstruct:overworked")), None, &mut uses);
                return Some(Outcome::Done(out, uses));
            }
            StationRecipe::Repair { modifier, items, amount } => {
                if !tools.durability.contains(sid(tool)) || md::modifier_level(tool, modifier) <= 0 || (tool.damage() == 0 && !tool_broken(tool)) || !only_ingredient(items, inputs) {
                    continue;
                }
                let factor = md::repair_factor(host, tool);
                let per = ((md::modifier_level(tool, modifier) * amount) as f32 * factor) as i32;
                if per <= 0 || factor <= 0.0 {
                    return Some(Outcome::Pass);
                }
                let avail = available(items, inputs, per);
                let damage = tool.damage() as i32;
                let left = (damage - avail).max(0);
                let mut out = with_count(tool, 1)?;
                set_tool_damage(&mut out, left);
                let mut uses = vec![Use::Shrink { slot: 0, item: tool.clone(), amount: 1 }];
                incremental_uses(items, inputs, damage - left, per, None, &mut uses);
                return Some(Outcome::Done(out, uses));
            }
        }
    }
    None
}

// ---- The modifier crystal (ModifierCrystalItem)

/// The crystal's tooltip: its modifier's name.
pub(crate) fn crystal_tooltip(host: &dyn ClientHost, stack: &ItemStack, lines: &mut Vec<(Text, u32)>) {
    let Some(id) = crystal_modifier(stack) else { return };
    let reg = md::registry(host);
    let (name, color) = md::level_name(host, &reg, &id, 1);
    lines.push((Text::literal(name), color));
}

// ---- The worktable (ModifierWorktableBlockEntity, IModifierWorktableRecipe)

fn work_matches(r: &WorkRecipe, tool: &ItemStack, inputs: &[Option<ItemStack>]) -> bool {
    r.tools.test(tool) && claim(&r.inputs, inputs).is_some()
}

/// getModifierOptions: the buttons (modifier and level).
fn work_options(recipes: &ModRecipes, reg: &Modifiers, r: &WorkRecipe, tool: &ItemStack) -> Vec<(String, i32)> {
    let ups = md::entries_of(tool, md::C_UPGRADES);
    let all = md::entries_of(tool, C_MODIFIERS);
    match &r.kind {
        WorkKind::Remove { pred, extract, .. } => ups.iter().filter(|e| pred.matches(recipes, reg, &e.id) && (!extract || e.int_effective() > 0)).map(|e| (e.id.clone(), e.level)).collect(),
        WorkKind::Sort => {
            if ups.len() < 2 {
                Vec::new()
            } else {
                ups.iter().map(|e| (e.id.clone(), e.level)).collect()
            }
        }
        WorkKind::Set { key, add, traits, pred } => {
            let existing = persistent_strings(tool, key);
            let list = if *traits { &all } else { &ups };
            list.iter().filter(|e| pred.matches(recipes, reg, &e.id) && existing.contains(&e.id) != *add).map(|e| (e.id.clone(), e.level)).collect()
        }
        WorkKind::Toggle => all.iter().filter(|e| reg.in_tag(&e.id, "tconstruct:dual_interaction")).map(|e| (e.id.clone(), e.level)).collect(),
    }
}

fn persistent_strings(stack: &ItemStack, key: &str) -> Vec<String> {
    match component(stack, md::C_PERSISTENT) {
        Some(Tag::Compound(p)) => match p.get(&js(key)) {
            Some(Tag::List(l)) => l.iter().filter_map(|t| if let Tag::String(s) = t { Some(s.to_string_lossy().to_string()) } else { None }).collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn set_strings_tag(stack: &mut ItemStack, key: &str, list: &[String]) {
    md::set_persistent(stack, key, (!list.is_empty()).then(|| Tag::List(list.iter().map(|s| Tag::String(js(s))).collect())));
}

/// getResult for the selected button, with what taking it uses.
fn work_result(host: &dyn Host, recipes: &ModRecipes, r: &WorkRecipe, tool: &ItemStack, inputs: &[Option<ItemStack>], id: &str, level: i32) -> Outcome {
    let mut out = tool.clone();
    let mut uses = vec![Use::Shrink { slot: 0, item: tool.clone(), amount: tool.count() }];
    match &r.kind {
        WorkKind::Remove { leftovers, extract, .. } => {
            if let Some(s) = recipes.salvage.iter().find(|s| s.modifier == id && (s.level.0..=s.level.1).contains(&level) && s.tools.contains(sid(tool)) && tool.count() <= s.max_tool_size) {
                md::add_persistent_slots(&mut out, &s.slots.0, s.slots.1);
            }
            let mut ups = md::entries_of(tool, md::C_UPGRADES);
            remove_modifier(&mut ups, id, 1);
            md::set_upgrades(&mut out, &ups);
            rebuild(host, &mut out);
            if let Some(e) = try_validate(host, &out) {
                return Outcome::Fail(e);
            }
            if md::modifier_level(&out, "tconstruct:overslime") == 0 {
                md::set_persistent_int(&mut out, md::ST_OVERSLIME, 0);
            }
            claimed_uses(&r.inputs, inputs, 1, &mut uses);
            uses.extend(leftovers.iter().cloned().map(Use::Give));
            if *extract {
                uses.extend(crystal_of(id, 1).map(Use::Give));
            }
        }
        WorkKind::Sort => {
            let mut ups = md::entries_of(tool, md::C_UPGRADES);
            let Some(from) = ups.iter().position(|e| e.id == id) else { return Outcome::Pass };
            let last = ups.len() - 1;
            let to = if inputs.first().is_none_or(Option::is_none) {
                if from == 0 { last } else { from - 1 }
            } else if from == last {
                0
            } else {
                from + 1
            };
            ups.swap(from, to);
            md::set_upgrades(&mut out, &ups);
            rebuild(host, &mut out);
        }
        WorkKind::Set { key, add, .. } => {
            let mut list = persistent_strings(tool, key);
            if !add {
                list.retain(|s| s != id);
            } else if !list.iter().any(|s| s == id) {
                list.push(id.to_owned());
            }
            set_strings_tag(&mut out, key, &list);
            claimed_uses(&r.inputs, inputs, 1, &mut uses);
        }
        WorkKind::Toggle => {
            let (lk, rk) = ("tconstruct:attack_modifiers", "tconstruct:interact_modifiers");
            let mut left = persistent_strings(tool, lk);
            let mut right = persistent_strings(tool, rk);
            if left.iter().chain(right.iter()).any(|s| s == id) {
                left.retain(|s| s != id);
                right.retain(|s| s != id);
            } else {
                left.push(id.to_owned());
            }
            set_strings_tag(&mut out, lk, &left);
            set_strings_tag(&mut out, rk, &right);
            claimed_uses(&r.inputs, inputs, 1, &mut uses);
        }
    }
    Outcome::Done(out, uses)
}

/// What the worktable shows for its slots and selection: the recipe's index, the buttons, the message, the result.
#[derive(Default)]
pub(crate) struct WtState {
    pub recipe: i32,
    pub buttons: Vec<(String, i32)>,
    /// None: the recipe's description; Some: its error.
    pub error: Option<Err>,
    pub result: Option<(ItemStack, Vec<Use>)>,
}

pub(crate) fn worktable_state(host: &dyn Host, items: &[Option<ItemStack>], sel: i32) -> WtState {
    let Some(tool) = items.first().cloned().flatten() else { return WtState { recipe: -1, ..WtState::default() } };
    let inputs: Vec<Option<ItemStack>> = (1..=2).map(|i| items.get(i).cloned().flatten()).collect();
    let recipes = mod_recipes(host);
    let reg = md::registry(host);
    let Some((ri, r)) = recipes.work.iter().enumerate().find(|(_, r)| work_matches(r, &tool, &inputs)) else { return WtState { recipe: -1, ..WtState::default() } };
    let mut st = WtState { recipe: ri as i32, buttons: work_options(&recipes, &reg, r, &tool), error: None, result: None };
    if matches!(r.kind, WorkKind::Sort) && st.buttons.is_empty() {
        st.error = Some(Err::SortTwo);
    }
    if let Some((id, level)) = usize::try_from(sel).ok().and_then(|i| st.buttons.get(i)).cloned() {
        match work_result(host, &recipes, r, &tool, &inputs, &id, level) {
            Outcome::Done(out, uses) => st.result = Some((out, uses)),
            Outcome::Fail(e) => st.error = Some(e),
            Outcome::Pass => {}
        }
    }
    st
}

/// The worktable's result for the selected button, what it uses, and its screen's data. The selection is cleared when
/// the slots change (ModifierWorktableBlockEntity.setItem).
pub(crate) fn worktable_compute(host: &dyn Host, items: &[Option<ItemStack>], nbt: &mut Compound) -> (Option<ItemStack>, Vec<Use>, Vec<i32>) {
    let key: String = (0..WT_RESULT).map(|i| items.get(i).cloned().flatten().map(|s| format!("{s:?}")).unwrap_or_default()).collect::<Vec<_>>().join("|");
    if gs(nbt, "hz_wkey").unwrap_or_default() != key {
        ss(nbt, "hz_wkey", &key);
        ss(nbt, "hz_sel", "-1");
    }
    let sel = gs(nbt, "hz_sel").and_then(|s| s.parse::<i32>().ok()).unwrap_or(-1);
    let st = worktable_state(host, items, sel);
    let reg = md::registry(host);
    let sel = if usize::try_from(sel).is_ok_and(|i| i < st.buttons.len()) { sel } else { -1 };
    let err = st.error.as_ref().map_or([0; 4], |e| e.encode(&reg));
    let mut data = vec![sel, st.buttons.len().min(WT_BUTTONS) as i32, st.recipe, err[0], err[1], err[2]];
    for (id, level) in st.buttons.iter().take(WT_BUTTONS) {
        data.push(reg.index(id));
        data.push(*level);
    }
    data.resize(WT_HEAD + 2 * WT_BUTTONS, -1);
    match st.result {
        Some((r, u)) => (Some(r), u, data),
        None => (None, Vec::new(), data),
    }
}

/// The worktable's button `id`: the selection (ModifierWorktableContainerMenu.clickMenuButton).
pub(crate) fn worktable_button(id: i32, data: &[i32], nbt: &mut Compound) {
    if id >= 0 && id < data.get(1).copied().unwrap_or(0) {
        ss(nbt, "hz_sel", &id.to_string());
    }
}

// ---- The worktable's screen (ModifierWorktableScreen)

const WORKTABLE_TEXTURE: &str = "tconstruct:gui/worktable";
pub(crate) const WT_PLACES: [[f32; 2]; 4] = [[8.0, 21.0], [8.0, 45.0], [8.0, 67.0], [125.0, 42.0]];
const LABEL: u32 = 0xFF40_4040;

fn button_at(at: [f32; 2], count: usize) -> Option<usize> {
    let (x, y) = (at[0] - 28.0, at[1] - 15.0);
    if x < 0.0 || y < 0.0 || x >= 72.0 || y >= 72.0 {
        return None;
    }
    let i = (y / 18.0) as usize * 4 + (x / 18.0) as usize;
    (i < count.min(16)).then_some(i)
}

fn buttons_of(data: &[i32]) -> Vec<(i32, i32)> {
    let n = data.get(1).copied().unwrap_or(0).clamp(0, WT_BUTTONS as i32) as usize;
    (0..n).map(|i| (data.get(WT_HEAD + 2 * i).copied().unwrap_or(-1), data.get(WT_HEAD + 2 * i + 1).copied().unwrap_or(1))).collect()
}

/// ModifierIconManager.renderIcon: the icon's layers (none drawn without one).
fn draw_icon(draw: &mut dyn DrawContext, id: &str, at: [f32; 2]) {
    let icons = &table().json["modifier_icons"][id];
    let layers: Vec<&str> = match icons {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        _ => vec!["tconstruct:gui/modifiers/default_cover"],
    };
    for l in layers {
        draw.sprite(l, [0.0, 0.0, 16.0, 16.0], [at[0], at[1], 16.0, 16.0], None);
    }
}

pub(crate) fn screen() -> MenuScreenLayout {
    let mut slots = vec![None; WT_RESULT + 1];
    for (i, p) in WT_PLACES.iter().enumerate() {
        slots[i] = Some(*p);
    }
    MenuScreenLayout { width: 176.0, height: 184.0, slots: crate::screen_slots(slots), inventory_y: 102.0, inventory_x: 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() }
}

pub(crate) fn draw(host: &dyn ClientHost, data: &[i32], draw: &mut dyn DrawContext, o: [f32; 2]) {
    draw.sprite(WORKTABLE_TEXTURE, [0.0, 0.0, 176.0, 184.0], [o[0], o[1], 176.0, 184.0], None);
    let reg = md::registry(host);
    let sel = data.first().copied().unwrap_or(-1);
    for (k, (idx, _)) in buttons_of(data).iter().enumerate().take(16) {
        let (x, y) = (28.0 + (k % 4) as f32 * 18.0, 15.0 + (k / 4) as f32 * 18.0);
        let v = if k as i32 == sel { 33.0 } else { 15.0 };
        draw.sprite(WORKTABLE_TEXTURE, [176.0, v, 18.0, 18.0], [o[0] + x, o[1] + y, 18.0, 18.0], None);
        if let Some(id) = usize::try_from(*idx).ok().and_then(|i| reg.ids.get(i)) {
            draw_icon(draw, id, [o[0] + x + 1.0, o[1] + y + 1.0]);
        }
    }
    draw.sprite(WORKTABLE_TEXTURE, [188.0, 0.0, 12.0, 15.0], [o[0] + 103.0, o[1] + 15.0, 12.0, 15.0], None);
    draw.text(&lang(host, "block.tconstruct.modifier_worktable", "Modifier Worktable"), [o[0] + 8.0, o[1] + 6.0], LABEL, false);
    draw.text(&lang(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 90.0], LABEL, false);
    // The info panel's message (Java's right-hand panel): the error, else the recipe's title, below the menu.
    let code = [data.get(3).copied().unwrap_or(0), data.get(4).copied().unwrap_or(0), data.get(5).copied().unwrap_or(0), 0];
    let msg = error_text(host, code).map(|t| (t, 0xFFFF_5555)).or_else(|| (data.get(2).copied().unwrap_or(-1) >= 0).then(|| (work_title(host, data), 0xFFFF_FFFF)));
    if let Some((t, c)) = msg {
        draw.text(&t, [o[0], o[1] + 186.0], c, true);
    }
}

/// getTitle by the recipe index the server sent.
fn work_title(host: &dyn ClientHost, data: &[i32]) -> String {
    let recipes = mod_recipes(host);
    match usize::try_from(data.get(2).copied().unwrap_or(-1)).ok().and_then(|i| recipes.work.get(i)).map(|r| &r.kind) {
        Some(WorkKind::Remove { name, extract: false, .. }) => lang(host, &format!("recipe.tconstruct.remove_modifier.{name}"), "Remove Modifier"),
        Some(WorkKind::Remove { name, .. }) => lang(host, &format!("recipe.tconstruct.extract_modifier.{name}"), "Extract Modifier"),
        Some(WorkKind::Sort) => lang(host, "recipe.tconstruct.modifier_sorting.title", "Sort Modifiers"),
        Some(WorkKind::Set { key, add, .. }) => {
            let (ns, path) = key.split_once(':').unwrap_or(("tconstruct", key));
            lang(host, &format!("recipe.{ns}.{}.{}.title", path.replace('/', "."), if *add { "adding" } else { "removing" }), "Modifier Set")
        }
        Some(WorkKind::Toggle) => lang(host, "recipe.tconstruct.toggle_interaction.title", "Toggle Interaction"),
        None => String::new(),
    }
}

pub(crate) fn click(data: &[i32], at: [f32; 2]) -> Option<i32> {
    let b = button_at(at, buttons_of(data).len()).map(|i| i as i32);
    crate::smeltery::util::tlog(&format!("worktable screen click at {at:?}: {} buttons, button {b:?}", buttons_of(data).len()));
    b
}

pub(crate) fn tooltip(host: &dyn ClientHost, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) {
    let buttons = buttons_of(data);
    let Some((idx, level)) = button_at(at, buttons.len()).map(|i| buttons[i]) else { return };
    let reg = md::registry(host);
    if let Some(id) = usize::try_from(idx).ok().and_then(|i| reg.ids.get(i)) {
        lines.push(md::level_name(host, &reg, id, level).0);
    }
}

/// The tinker station's error for its slots, sent in its data slots ([code, a, b, c]).
pub(crate) fn station_error(host: &dyn Host, e: &Err) -> Vec<i32> {
    e.encode(&md::registry(host)).to_vec()
}

/// One modifier recipe as the recipe viewer shows it (ModifierRecipeCategory): the inputs (each slot's items, sorted),
/// the tools it takes (sorted), the modifier, its level and the slot it costs.
// Without the recipe viewer only the transfer reads it (its id and inputs).
#[allow(dead_code)]
pub(crate) struct JeiModifier {
    /// The recipe's id.
    pub(crate) id: String,
    pub(crate) inputs: Vec<Vec<(String, u32)>>,
    pub(crate) tools: Vec<String>,
    pub(crate) result: String,
    pub(crate) level: i32,
    /// The recipe's level range (max 32767: no limit), for Java's level line.
    pub(crate) levels: (i32, i32),
    pub(crate) slots: Option<(String, i32)>,
    pub(crate) incremental: bool,
    /// The result modifier's RequirementsModule translation key, if it has one.
    pub(crate) requirement: Option<String>,
}

pub(crate) fn jei_modifiers(host: &dyn Host) -> Vec<JeiModifier> {
    let all = mod_recipes(host);
    let reg = md::registry(host);
    let sorted = |s: &HashSet<String>| {
        let mut v: Vec<String> = s.iter().cloned().collect();
        v.sort();
        v
    };
    all.station
        .iter()
        .filter_map(|r| {
            let StationRecipe::Add(a) = r else { return None };
            let inputs = match &a.kind {
                AddKind::Plain(list) | AddKind::Swappable { inputs: list, .. } => list.iter().map(|i| sorted(&i.items).into_iter().map(|x| (x, i.amount)).collect()).collect(),
                AddKind::Incremental { input, .. } => vec![sorted(input).into_iter().map(|x| (x, 1)).collect()],
            };
            let requirement = reg.get(&a.result).and_then(|m| m.modules.iter().find_map(|x| if let ModModule::Requirements { key, .. } = x { Some(key.clone()) } else { None }));
            let incremental = matches!(a.kind, AddKind::Incremental { .. });
            Some(JeiModifier { id: a.id.clone(), inputs, tools: sorted(&a.tools), result: a.result.clone(), level: a.level.0.max(1), levels: a.level, slots: a.slots.clone(), incremental, requirement })
        })
        .collect()
}
