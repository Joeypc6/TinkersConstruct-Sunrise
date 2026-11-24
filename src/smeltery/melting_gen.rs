//! `/tconstruct generate melting_recipes <recipe_type>` (GenerateMeltingRecipesCommand, 3.12.1.231): a melting recipe
//! for every result of `recipe_type` that doesn't melt yet, from what its ingredients melt into, written into the
//! world's generated data pack (`SlimeKnightsGenerated`) as MeltingRecipeBuilder saves them.
//!
//! - The configuration is `data/tconstruct/command/generate_melting_recipes.json` of the server's data (packs over the
//!   mods'), read with `resource`: Mantle item predicates `melt` (which results), `inputs` (which ingredient items may
//!   be read), `ignore` (items that may melt into nothing) and `skip_recipes`.
//! - An item's fluid (MeltingCache): Mantle's transfer emptying it, a BucketItem's fluid, its `ITEM_FLUIDS` handler
//!   (`containers::contained_fluid`), else the first melting recipe that takes it (MeltingRecipeLookup, rebuilt from
//!   the `tconstruct:melting`, `ore_melting` and `damagable_melting` tables). A container's fluid melts at its fluid
//!   type's temperature less 300 (at least 100): Tinkers' fluids' from TinkerFluids, water's and lava's from vanilla;
//!   no host API gives another mod's fluid type temperature, so theirs is FluidType's default 300.
//! - A recipe's ingredients and result are `recipes_of_type`'s typed ones (shaped, shapeless and cooking recipes,
//!   stonecutting's input and result, smithing's result); a mod type's recipes have none there, so they give nothing,
//!   as a recipe whose getResultItem is empty in Java.
//! - MeltingResult.intersection is kept as the jar has it: after the first pair it never takes the next values, so two
//!   recipes of one result keep the first pair's smaller fluid, once per element of the shorter list, or nothing.
//! - Files go to `data/tinkers_generated/recipe/melting/<namespace>/<path>.json` (26.3's `recipe` folder, 1.20.1's
//!   `recipes`), in Gson's pretty print with MeltingRecipe's fields in their loader order.

use super::containers;
use super::recipes::{MELTING_KINDS, chosen, fluid_tag, ingredient_items};
use super::util::*;
use mod_api::serde_json::{self, Value};
use mod_api::*;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// GenerateMeltingRecipesCommand.MELTING_CONFIGURATION.
pub(crate) const CONFIGURATION: &str = "tconstruct:command/generate_melting_recipes.json";

// ---- MeltingResult

/// GenerateMeltingRecipesCommand.MeltingResult: a fluid stack, the tag it came from (a melting recipe's tag output)
/// and its melting temperature. The empty one is `MeltingResult.EMPTY`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MeltingResult {
    pub fluid: String,
    pub amount: i32,
    pub tag: Option<String>,
    pub temperature: i32,
}

impl MeltingResult {
    pub(crate) fn empty() -> Self {
        MeltingResult { fluid: String::new(), amount: 0, tag: None, temperature: 0 }
    }

    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::empty()
    }

    /// MeltingResult.from(FluidStack): the fluid type's temperature less 300, at least 100.
    pub(crate) fn from_stack(fluid: &str, amount: i32) -> Self {
        MeltingResult { fluid: fluid.to_owned(), amount, tag: None, temperature: (fluid_temperature(fluid) - 300).max(100) }
    }

    /// withAmount: EMPTY for nothing.
    fn with_amount(&self, amount: i32) -> Self {
        if amount <= 0 {
            return Self::empty();
        }
        MeltingResult { amount, ..self.clone() }
    }
}

/// simpler: the first unless it came from a tag.
fn simpler<'a>(first: &'a MeltingResult, second: &'a MeltingResult) -> &'a MeltingResult {
    if first.tag.is_none() { first } else { second }
}

/// matches: the same tag when both have one, else the same fluid (FluidStack.isFluidEqual).
pub(crate) fn matches(first: &MeltingResult, second: &MeltingResult) -> bool {
    match (&first.tag, &second.tag) {
        (Some(a), Some(b)) => a == b,
        _ => first.fluid == second.fluid,
    }
}

/// merge: both amounts on the simpler one.
pub(crate) fn merge(first: &MeltingResult, second: &MeltingResult) -> MeltingResult {
    simpler(first, second).with_amount(first.amount.wrapping_add(second.amount))
}

/// min: the smaller amount, kept as the fluid that came from no tag.
pub(crate) fn min(first: &MeltingResult, second: &MeltingResult) -> MeltingResult {
    let (a, b) = (first.amount, second.amount);
    if a == b {
        simpler(first, second).clone()
    } else if a < b {
        if first.tag.is_some() && second.tag.is_none() { second.with_amount(a) } else { first.clone() }
    } else if second.tag.is_some() && first.tag.is_none() {
        first.with_amount(b)
    } else {
        second.clone()
    }
}

/// MeltingResult.intersection as the jar runs it (its iterators' `next()` values are dropped, so the first pair is
/// compared every time): the first pair's min once per element of the shorter list when they match, else nothing.
pub(crate) fn intersection(list1: Vec<MeltingResult>, list2: Vec<MeltingResult>) -> Vec<MeltingResult> {
    if list1.is_empty() {
        return list1;
    }
    if list2.is_empty() {
        return list2;
    }
    if !matches(&list1[0], &list2[0]) {
        return Vec::new();
    }
    vec![min(&list1[0], &list2[0]); list1.len().min(list2.len())]
}

/// ResourceLocation.compareTo: the path first, then the namespace (the empty fluid is `minecraft:empty`).
fn id_order(a: &str, b: &str) -> Ordering {
    let split = |s: &str| {
        let s = if s.is_empty() { "minecraft:empty" } else { s };
        let (ns, path) = s.split_once(':').unwrap_or(("minecraft", s));
        (path.to_owned(), ns.to_owned())
    };
    split(a).cmp(&split(b))
}

/// nameComparator: the fluid's id, reversed.
pub(crate) fn name_order(a: &MeltingResult, b: &MeltingResult) -> Ordering {
    id_order(&b.fluid, &a.fluid)
}

/// resultComparator: the hottest first, then the most.
pub(crate) fn result_order(a: &MeltingResult, b: &MeltingResult) -> Ordering {
    b.temperature.cmp(&a.temperature).then(b.amount.cmp(&a.amount))
}

// ---- One recipe

/// What the command asks about items while it reads a recipe: the `inputs` and `ignore` predicates and the cache.
pub(crate) trait MeltSource {
    fn inputs(&mut self, item: &str) -> bool;
    fn ignore(&mut self, item: &str) -> bool;
    /// MeltingCache.get: what the item melts into, EMPTY for nothing.
    fn fluid(&mut self, item: &str) -> MeltingResult;
}

/// The loop over a recipe's ingredients: each ingredient's stacks (None: one with NBT) give one fluid, the smallest
/// they share; ingredients of the same fluid add up. Empty when the recipe can't melt (`put(result, List.of())`), else
/// the fluids by name, divided among the result's count.
pub(crate) fn recipe_fluids(ingredients: &[Vec<Option<String>>], count: u32, src: &mut dyn MeltSource) -> Vec<MeltingResult> {
    let mut fluids: Vec<MeltingResult> = Vec::new();
    for stacks in ingredients {
        // Ingredient.isEmpty.
        if stacks.is_empty() {
            continue;
        }
        let mut ingredient: Option<MeltingResult> = None;
        let mut did_ignore = false;
        for stack in stacks {
            let Some(item) = stack.as_deref() else { return Vec::new() };
            if !src.inputs(item) {
                return Vec::new();
            }
            let new = src.fluid(item);
            if new.is_empty() {
                if ingredient.is_some() || !src.ignore(item) {
                    return Vec::new();
                }
                did_ignore = true;
            } else {
                if did_ignore {
                    return Vec::new();
                }
                ingredient = Some(match ingredient {
                    None => new,
                    Some(current) => {
                        if !matches(&current, &new) {
                            return Vec::new();
                        }
                        min(&current, &new)
                    }
                });
            }
        }
        let Some(ingredient) = ingredient else { continue };
        match fluids.iter_mut().find(|c| matches(c, &ingredient)) {
            Some(slot) => *slot = merge(slot, &ingredient),
            None => fluids.push(ingredient),
        }
    }
    if fluids.is_empty() {
        return fluids;
    }
    fluids.sort_by(name_order);
    if count > 1 {
        for f in &mut fluids {
            *f = f.with_amount(f.amount / count as i32);
        }
    }
    fluids
}

/// `newRecipes`: each result's fluids, in the order results were first seen.
#[derive(Default)]
pub(crate) struct NewRecipes {
    map: HashMap<String, Vec<MeltingResult>>,
    order: Vec<String>,
}

impl NewRecipes {
    /// `put(result, List.of())` for an empty list, else `merge(result, fluids, intersection)`.
    pub(crate) fn add(&mut self, result: &str, fluids: Vec<MeltingResult>) {
        let merged = match self.map.remove(result) {
            None => {
                self.order.push(result.to_owned());
                fluids
            }
            Some(_) if fluids.is_empty() => fluids,
            Some(old) => intersection(old, fluids),
        };
        self.map.insert(result.to_owned(), merged);
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = (&str, &Vec<MeltingResult>)> {
        self.order.iter().map(|k| (k.as_str(), &self.map[k]))
    }
}

// ---- The recipe file (MeltingRecipeBuilder.save, LoadableFinishedRecipe, JsonHelper.DEFAULT_GSON)

/// An ordered JSON value (Gson keeps the order fields were added in).
pub(crate) enum J {
    S(String),
    I(i64),
    O(Vec<(&'static str, J)>),
    A(Vec<J>),
}

/// Gson's JsonWriter string escaping with HTML escaping off.
fn gson_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{c}' => out.push_str("\\f"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Gson's pretty printing: two spaces an indent, `"key": value`.
pub(crate) fn gson_pretty(v: &J) -> String {
    fn write(v: &J, depth: usize, out: &mut String) {
        let pad = |n: usize, out: &mut String| out.extend(std::iter::repeat_n("  ", n));
        match v {
            J::S(s) => gson_string(s, out),
            J::I(n) => out.push_str(&n.to_string()),
            J::O(fields) if fields.is_empty() => out.push_str("{}"),
            J::A(items) if items.is_empty() => out.push_str("[]"),
            J::O(fields) => {
                out.push_str("{\n");
                for (i, (k, x)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    pad(depth + 1, out);
                    gson_string(k, out);
                    out.push_str(": ");
                    write(x, depth + 1, out);
                }
                out.push('\n');
                pad(depth, out);
                out.push('}');
            }
            J::A(items) => {
                out.push_str("[\n");
                for (i, x) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    pad(depth + 1, out);
                    write(x, depth + 1, out);
                }
                out.push('\n');
                pad(depth, out);
                out.push(']');
            }
        }
    }
    let mut out = String::new();
    write(v, 0, &mut out);
    out
}

/// IMeltingRecipe.calcTime: `round(pow(temperature + 300, 0.585F) * factor)`.
pub(crate) fn calc_time(temperature: i32, factor: f32) -> i32 {
    ((f64::from(temperature) + 300.0).powf(f64::from(0.585f32)) * f64::from(factor)).round() as i32
}

/// MeltingResult.toOutput serialized (FluidOutput.OfTagPreference or FluidStackLoadable): None for an empty output,
/// which FluidOutput.Loadable.REQUIRED refuses with an exception.
fn output_json(r: &MeltingResult) -> Option<Vec<(&'static str, J)>> {
    if r.amount <= 0 {
        return None;
    }
    Some(match &r.tag {
        Some(tag) => vec![("tag", J::S(tag.clone())), ("amount", J::I(i64::from(r.amount)))],
        None => vec![("fluid", J::S(r.fluid.clone())), ("amount", J::I(i64::from(r.amount)))],
    })
}

/// The recipe a result's fluids make (sorted hottest first: the first the output, the rest byproducts): a
/// `tconstruct:melting`, or a `tconstruct:damagable_melting` with unit size 10 for a damageable item. None where an
/// output is empty (Java throws there).
pub(crate) fn recipe_json(item: &str, fluids: &[MeltingResult], damageable: bool) -> Option<String> {
    let first = fluids.first()?;
    let mut result = output_json(first)?;
    if damageable {
        // DamageableMeltingRecipe's unit_size merged into `result` (a byproduct's list is empty).
        result.push(("unit_size", J::I(10)));
    }
    let byproducts = fluids[1..].iter().map(|f| output_json(f).map(J::O)).collect::<Option<Vec<J>>>()?;
    let kind = if damageable { "tconstruct:damagable_melting" } else { "tconstruct:melting" };
    let mut fields = vec![
        ("type", J::S(kind.into())),
        ("ingredient", J::O(vec![("item", J::S(item.to_owned()))])),
        ("result", J::O(result)),
        ("temperature", J::I(i64::from(first.temperature))),
        ("time", J::I(i64::from(calc_time(first.temperature, 1.0)))),
    ];
    // The default (no byproducts) isn't written.
    if !byproducts.is_empty() {
        fields.push(("byproducts", J::A(byproducts)));
    }
    Some(gson_pretty(&J::O(fields)))
}

/// `tinkers_generated:melting/<namespace>/<path>`.
pub(crate) fn recipe_id(item: &str) -> String {
    let (ns, path) = item.split_once(':').unwrap_or(("minecraft", item));
    format!("tinkers_generated:melting/{ns}/{path}")
}

// ---- Fluid temperatures (FluidType.getTemperature)

/// TinkerFluids' fluid types' temperatures (`hot` 1000 and `cool` 300 unless set).
const TINKERS_TEMPERATURES: [(&str, i32); 72] = [
    ("tconstruct:venom", 310), ("tconstruct:powdered_snow", 270), ("tconstruct:earth_slime", 350), ("tconstruct:sky_slime", 310),
    ("tconstruct:ender_slime", 370), ("tconstruct:magma", 600), ("tconstruct:ichor", 1000), ("tconstruct:honey", 301),
    ("tconstruct:beetroot_soup", 400), ("tconstruct:mushroom_stew", 400), ("tconstruct:rabbit_stew", 400), ("tconstruct:meat_soup", 400),
    ("tconstruct:potion", 315), ("tconstruct:seared_stone", 900), ("tconstruct:scorched_stone", 800), ("tconstruct:molten_clay", 750),
    ("tconstruct:molten_glass", 1050), ("tconstruct:liquid_soul", 700), ("tconstruct:molten_porcelain", 1000), ("tconstruct:molten_obsidian", 1300),
    ("tconstruct:molten_ender", 777), ("tconstruct:blazing_blood", 1800), ("tconstruct:molten_emerald", 1234), ("tconstruct:molten_quartz", 937),
    ("tconstruct:molten_amethyst", 1250), ("tconstruct:molten_diamond", 1750), ("tconstruct:molten_debris", 1475), ("tconstruct:molten_iron", 1100),
    ("tconstruct:molten_gold", 1000), ("tconstruct:molten_copper", 800), ("tconstruct:molten_cobalt", 1250), ("tconstruct:molten_steel", 1250),
    ("tconstruct:molten_knightmetal", 1600), ("tconstruct:molten_slimesteel", 1200), ("tconstruct:molten_amethyst_bronze", 1120), ("tconstruct:molten_rose_gold", 850),
    ("tconstruct:molten_pig_iron", 1111), ("tconstruct:molten_manyullyn", 1500), ("tconstruct:molten_hepatizon", 1700), ("tconstruct:molten_cinderslime", 1350),
    ("tconstruct:molten_queens_slime", 1450), ("tconstruct:molten_soulsteel", 1500), ("tconstruct:molten_netherite", 1550), ("tconstruct:molten_knightslime", 1425),
    ("tconstruct:molten_tin", 525), ("tconstruct:molten_aluminum", 725), ("tconstruct:molten_lead", 630), ("tconstruct:molten_silver", 1090),
    ("tconstruct:molten_nickel", 1250), ("tconstruct:molten_zinc", 720), ("tconstruct:molten_platinum", 1270), ("tconstruct:molten_tungsten", 1250),
    ("tconstruct:molten_osmium", 1275), ("tconstruct:molten_uranium", 1130), ("tconstruct:molten_chromium", 1200), ("tconstruct:molten_cadmium", 594),
    ("tconstruct:molten_bronze", 1000), ("tconstruct:molten_brass", 905), ("tconstruct:molten_electrum", 1060), ("tconstruct:molten_invar", 1200),
    ("tconstruct:molten_constantan", 1220), ("tconstruct:molten_pewter", 700), ("tconstruct:molten_enderium", 1650), ("tconstruct:molten_lumium", 1350),
    ("tconstruct:molten_signalum", 1299), ("tconstruct:molten_refined_glowstone", 1125), ("tconstruct:molten_refined_obsidian", 1775), ("tconstruct:molten_nicrosil", 1400),
    ("tconstruct:molten_duralumin", 925), ("tconstruct:molten_bendalloy", 400), ("tconstruct:molten_steeleaf", 1234), ("tconstruct:fiery_liquid", 1800),
];

/// FluidType.getTemperature: Tinkers' own, lava's 1300, else the default 300 (water's, and any fluid whose type
/// Sunrise doesn't describe).
pub(crate) fn fluid_temperature(fluid: &str) -> i32 {
    if let Some((_, t)) = TINKERS_TEMPERATURES.iter().find(|(f, _)| *f == fluid) {
        return *t;
    }
    match fluid {
        "minecraft:lava" | "minecraft:flowing_lava" => 1300,
        _ => 300,
    }
}

// ---- MeltingRecipeLookup

struct MeltingFluid {
    items: HashSet<String>,
    /// FluidOutput.get()'s fluid (a tag's preferred one); empty when a tag has none.
    fluid: String,
    tag: Option<String>,
    amount: i32,
    temperature: i32,
}

/// MeltingRecipeLookup: every melting recipe's ingredient, output and temperature, in table order.
pub(crate) struct Lookup(Vec<MeltingFluid>);

impl Lookup {
    pub(crate) fn build(host: &dyn Host) -> Self {
        ensure_ids(host);
        let mut out = Vec::new();
        for kind in MELTING_KINDS {
            for r in host.recipes_of_type(kind).iter() {
                let Some(j) = chosen(host, &r.json) else { continue };
                let items = ingredient_items(host, &j["ingredient"], &mut Vec::new());
                if items.is_empty() {
                    continue;
                }
                let res = &j["result"];
                let amount = res["amount"].as_i64().unwrap_or(0).clamp(0, i64::from(i32::MAX)) as i32;
                let tag = res["tag"].as_str().map(str::to_owned);
                let fluid = match (&tag, res["fluid"].as_str()) {
                    (Some(t), _) => fluid_tag(t).first().cloned().unwrap_or_default(),
                    (None, Some(f)) => f.to_owned(),
                    (None, None) => continue,
                };
                let temperature = j["temperature"].as_i64().unwrap_or(0).clamp(0, i64::from(i32::MAX)) as i32;
                out.push(MeltingFluid { items, fluid, tag, amount, temperature });
            }
        }
        Lookup(out)
    }

    fn find(&self, item: &str) -> Option<&MeltingFluid> {
        self.0.iter().find(|m| m.items.contains(item))
    }

    /// canMelt: the first recipe that takes it has an output.
    pub(crate) fn can_melt(&self, item: &str) -> bool {
        self.find(item).is_some_and(|m| m.amount > 0)
    }

    /// MeltingResult.from(findFluid(item)).
    pub(crate) fn result(&self, item: &str) -> MeltingResult {
        match self.find(item) {
            Some(m) if m.amount > 0 && !m.fluid.is_empty() => {
                MeltingResult { fluid: m.fluid.clone(), amount: m.amount, tag: m.tag.clone(), temperature: m.temperature }
            }
            _ => MeltingResult::empty(),
        }
    }
}

// ---- Mantle's item predicates (ItemPredicate.LOADER)

pub(crate) enum ItemPred {
    Any,
    None,
    Not(Box<ItemPred>),
    And(Vec<ItemPred>),
    Or(Vec<ItemPred>),
    Set(HashSet<String>),
    Tag(HashSet<String>),
    HasContainer,
    MayHaveTransfer,
    Arrow(HashSet<String>),
    Bucket,
    Map,
    CanMelt,
    Castable,
}

/// A ResourceLocation from JSON: `minecraft:` when it has no namespace.
fn location(s: &str) -> String {
    if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") }
}

impl ItemPred {
    /// DefaultingLoaderRegistry.getOrDefault: a missing or null field is `mantle:any`.
    pub(crate) fn field(host: &dyn Host, parent: &Value, key: &str) -> Result<Self, String> {
        match parent.get(key) {
            None | Some(Value::Null) => Ok(ItemPred::Any),
            Some(v) => Self::parse(host, v, key),
        }
    }

    /// GenericLoaderRegistry.convert: an object with its `type`, or the type's id alone (compact).
    fn parse(host: &dyn Host, v: &Value, key: &str) -> Result<Self, String> {
        match v {
            Value::String(t) => Self::typed(host, &location(t), &Value::Object(Default::default()), key),
            Value::Object(_) => {
                let t = v["type"].as_str().ok_or_else(|| format!("{key}: missing type"))?;
                Self::typed(host, &location(t), v, key)
            }
            _ => Err(format!("{key}: expected an object or a string")),
        }
    }

    fn typed(host: &dyn Host, kind: &str, o: &Value, key: &str) -> Result<Self, String> {
        let list = |k: &str| -> Result<Vec<ItemPred>, String> {
            let a = o[k].as_array().ok_or_else(|| format!("{key}: missing {k}"))?;
            a.iter().enumerate().map(|(i, p)| Self::parse(host, p, &format!("{k}[{i}]"))).collect()
        };
        Ok(match kind {
            "mantle:any" => ItemPred::Any,
            "mantle:none" => ItemPred::None,
            "mantle:inverted" => {
                let inner = o["inverted_type"].as_str().ok_or_else(|| format!("{key}: missing inverted_type"))?;
                ItemPred::Not(Box::new(Self::typed(host, &location(inner), o, key)?))
            }
            "mantle:and" => ItemPred::And(list("predicates")?),
            "mantle:or" => ItemPred::Or(list("predicates")?),
            "mantle:set" => {
                let a = o["items"].as_array().ok_or_else(|| format!("{key}: missing items"))?;
                let mut set = HashSet::new();
                for x in a {
                    let id = location(x.as_str().ok_or_else(|| format!("{key}: an item must be a string"))?);
                    if item_key(&id).is_none() {
                        return Err(format!("{key}: unknown item {id}"));
                    }
                    set.insert(id);
                }
                ItemPred::Set(set)
            }
            "mantle:tag" => {
                let tag = o["tag"].as_str().ok_or_else(|| format!("{key}: missing tag"))?;
                ItemPred::Tag(super::casting_recipes::tag_items(host, &location(tag)))
            }
            "mantle:has_container" => ItemPred::HasContainer,
            "mantle:may_have_transfer" => ItemPred::MayHaveTransfer,
            // ArrowItem: the arrows tag holds vanilla's three and the mods' own.
            "tconstruct:arrow" => ItemPred::Arrow(super::casting_recipes::tag_items(host, "minecraft:arrows")),
            "tconstruct:bucket" => ItemPred::Bucket,
            "tconstruct:map" => ItemPred::Map,
            "tconstruct:can_melt" => ItemPred::CanMelt,
            "tconstruct:castable" => ItemPred::Castable,
            other => return Err(format!("{key}: unknown Item Predicate type {other}")),
        })
    }

    pub(crate) fn matches(&self, host: &dyn Host, lookup: &Lookup, item: &str) -> bool {
        match self {
            ItemPred::Any => true,
            ItemPred::None => false,
            ItemPred::Not(p) => !p.matches(host, lookup, item),
            ItemPred::And(ps) => ps.iter().all(|p| p.matches(host, lookup, item)),
            ItemPred::Or(ps) => ps.iter().any(|p| p.matches(host, lookup, item)),
            ItemPred::Set(s) | ItemPred::Tag(s) | ItemPred::Arrow(s) => s.contains(item),
            // Item.hasCraftingRemainingItem.
            ItemPred::HasContainer => new_stack(item, 1).is_some_and(|s| host.item_crafting_remainder(&s).is_some()),
            ItemPred::MayHaveTransfer => containers::has_transfer(host, item),
            ItemPred::Bucket => containers::is_bucket_item(item),
            ItemPred::Map => item == "minecraft:filled_map",
            ItemPred::CanMelt => lookup.can_melt(item),
            // CastingRecipeLookup.isCastable: an item casting recipe's result.
            ItemPred::Castable => {
                let r = super::casting_recipes::recipes(host);
                r.table.iter().chain(r.basin.iter()).any(|c| matches!(&c.kind, super::casting_recipes::CastKind::Item(i, _) if i == item))
            }
        }
    }
}

// ---- The command's run

pub(crate) struct Configuration {
    melt: ItemPred,
    inputs: ItemPred,
    ignore: ItemPred,
    skip: Vec<String>,
}

/// Why the command stopped: the configuration (`generate.melting_recipes.invalid_config`), or an exception (an empty
/// output saved: Brigadier's `command.failed`).
pub(crate) enum GenError {
    InvalidConfig,
    Failed,
}

fn configuration(host: &dyn Host) -> Result<Configuration, GenError> {
    let id = Identifier::parse(CONFIGURATION).map_err(|_| GenError::InvalidConfig)?;
    let bytes = host.resource(PackType::ServerData, &id).ok_or(GenError::InvalidConfig)?;
    let json: Value = match serde_json::from_slice(&bytes) {
        Ok(v @ Value::Object(_)) => v,
        Ok(_) | Err(_) => {
            eprintln!("[TConstruct] Failed to load JSON from {CONFIGURATION}");
            return Err(GenError::InvalidConfig);
        }
    };
    let parsed = (|| -> Result<Configuration, String> {
        let skip = match json.get("skip_recipes") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a.iter().map(|x| x.as_str().map(location).ok_or_else(|| "skip_recipes: expected strings".to_owned())).collect::<Result<_, _>>()?,
            Some(_) => return Err("skip_recipes: expected a list".into()),
        };
        Ok(Configuration {
            melt: ItemPred::field(host, &json, "melt")?,
            inputs: ItemPred::field(host, &json, "inputs")?,
            ignore: ItemPred::field(host, &json, "ignore")?,
            skip,
        })
    })();
    parsed.map_err(|e| {
        eprintln!("[TConstruct] Failed to parse configuration {CONFIGURATION} from pack: {e}");
        GenError::InvalidConfig
    })
}

/// ItemStack.hasTag: a component patch on the stack.
fn has_patch(stack: &ItemStack) -> bool {
    stack.damage_component().is_some() || !stack.same_components(&ItemStack::of(stack.item()))
}

/// Item.isDamageable: a max damage.
fn damageable(host: &dyn Host, item: &str) -> bool {
    item_key(item).is_some_and(|k| matches!(host.item_default_component(k, "minecraft:max_damage"), Some(Tag::Int(v)) if v > 0))
}

/// An ingredient's stacks (Ingredient.getItems, in order); None for one that carries data (a components rule).
fn stacks(i: &Ingredient) -> Vec<Option<String>> {
    fn ruled(i: &Ingredient) -> bool {
        i.rule.is_some() || i.children.as_ref().is_some_and(|(_, kids)| kids.iter().any(ruled))
    }
    let tagged = ruled(i);
    let ids: Vec<String> = if !i.order.is_empty() {
        i.order.clone()
    } else {
        let mut v: Vec<String> = i.items.iter().cloned().collect();
        v.sort();
        v
    };
    ids.into_iter().map(|id| (!tagged).then_some(id)).collect()
}

/// Recipe.getResultItem (and whether it carries data) and Recipe.getIngredients from the typed parts: None for an
/// empty result.
fn parts(r: &Recipe) -> Option<(ItemStack, bool, Vec<&Ingredient>)> {
    if let Some(res) = r.results.first() {
        return Some((res.clone(), has_patch(res), r.ingredients.iter().collect()));
    }
    match r.rule.as_ref()? {
        RecipeRule::Stonecutting { input, result } => Some((result.clone(), has_patch(result), vec![input])),
        // SmithingTransformRecipe: no getIngredients.
        RecipeRule::SmithingTransform { result, .. } => {
            let stack = new_stack(result.item.as_deref()?, result.count.max(1))?;
            Some((stack, result.components.is_some(), Vec::new()))
        }
        _ => None,
    }
}

struct Source<'a> {
    host: &'a mut dyn ServerHost,
    config: &'a Configuration,
    lookup: &'a Lookup,
    cache: HashMap<String, MeltingResult>,
}

impl MeltSource for Source<'_> {
    fn inputs(&mut self, item: &str) -> bool {
        self.config.inputs.matches(&*self.host, self.lookup, item)
    }
    fn ignore(&mut self, item: &str) -> bool {
        self.config.ignore.matches(&*self.host, self.lookup, item)
    }
    fn fluid(&mut self, item: &str) -> MeltingResult {
        if let Some(r) = self.cache.get(item) {
            return r.clone();
        }
        let r = match new_stack(item, 1).and_then(|s| containers::contained_fluid(&mut *self.host, &s)) {
            Some((fluid, amount)) => MeltingResult::from_stack(&fluid, amount),
            None => self.lookup.result(item),
        };
        self.cache.insert(item.to_owned(), r.clone());
        r
    }
}

/// The command past the pack's `pack.mcmeta`: the recipes written under `data` (the pack's `data/` folder), counted.
pub(crate) fn generate(host: &mut dyn ServerHost, kind: &str, data: &Path) -> Result<i32, GenError> {
    ensure_ids(&*host);
    let config = configuration(&*host)?;
    let lookup = Lookup::build(&*host);
    let recipes = host.recipes_of_type(kind);
    let mut new_recipes = NewRecipes::default();
    let mut src = Source { host, config: &config, lookup: &lookup, cache: HashMap::new() };
    for r in recipes.iter() {
        if config.skip.iter().any(|s| *s == r.id) {
            continue;
        }
        let Some((result, patched, ingredients)) = parts(r) else { continue };
        let item = sid(&result);
        if patched && !damageable(&*src.host, item) {
            continue;
        }
        if !config.melt.matches(&*src.host, &lookup, item) || lookup.can_melt(item) {
            continue;
        }
        let ingredients: Vec<Vec<Option<String>>> = ingredients.iter().map(|i| stacks(i)).collect();
        let fluids = recipe_fluids(&ingredients, result.count(), &mut src);
        new_recipes.add(item, fluids);
    }
    let host = src.host;
    let mut successes = 0;
    for (item, fluids) in new_recipes.entries() {
        if fluids.is_empty() {
            continue;
        }
        let mut fluids = fluids.clone();
        fluids.sort_by(result_order);
        let Some(text) = recipe_json(item, &fluids, damageable(&*host, item)) else {
            eprintln!("[TConstruct] FluidOutput cannot be empty for this recipe ({})", recipe_id(item));
            return Err(GenError::Failed);
        };
        let id = recipe_id(item);
        let (ns, path) = id.split_once(':').unwrap_or(("tinkers_generated", &id));
        let file = data.join(ns).join("recipe").join(format!("{path}.json"));
        let saved = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, text));
        match saved {
            Ok(()) => successes += 1,
            Err(e) => eprintln!("[Mantle] Couldn't create JSON at {}: {e}", file.display()),
        }
    }
    Ok(successes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fluid(f: &str, amount: i32, temperature: i32) -> MeltingResult {
        MeltingResult { fluid: f.into(), amount, tag: None, temperature }
    }

    /// Ingots melt to 90 mB of their metal at Tinkers' temperatures, sticks melt to nothing.
    struct Mock;

    impl MeltSource for Mock {
        fn inputs(&mut self, _: &str) -> bool {
            true
        }
        fn ignore(&mut self, _: &str) -> bool {
            true
        }
        fn fluid(&mut self, item: &str) -> MeltingResult {
            match item {
                "minecraft:iron_ingot" => fluid("tconstruct:molten_iron", 90, 800),
                "minecraft:gold_ingot" => MeltingResult { tag: Some("forge:molten_gold".into()), ..fluid("tconstruct:molten_gold", 90, 700) },
                "minecraft:water_bucket" => MeltingResult::from_stack("minecraft:water", 1000),
                _ => MeltingResult::empty(),
            }
        }
    }

    fn one(id: &str) -> Vec<Option<String>> {
        vec![Some(id.into())]
    }

    #[test]
    fn calc_time_matches_java() {
        // Math.round(Math.pow(1100, 0.585F) * 1.0F): 60.147...
        assert_eq!(calc_time(800, 1.0), 60);
        assert_eq!(calc_time(1000, 1.0), 66);
        assert_eq!(calc_time(100, 1.0), 33);
    }

    #[test]
    fn bucket_from_three_ingots() {
        let ings = vec![one("minecraft:iron_ingot"), one("minecraft:iron_ingot"), one("minecraft:iron_ingot")];
        let fluids = recipe_fluids(&ings, 1, &mut Mock);
        assert_eq!(fluids, vec![fluid("tconstruct:molten_iron", 270, 800)]);
        let json = recipe_json("minecraft:bucket", &fluids, false).unwrap();
        assert_eq!(
            json,
            "{\n  \"type\": \"tconstruct:melting\",\n  \"ingredient\": {\n    \"item\": \"minecraft:bucket\"\n  },\n  \"result\": {\n    \"fluid\": \"tconstruct:molten_iron\",\n    \"amount\": 270\n  },\n  \"temperature\": 800,\n  \"time\": 60\n}"
        );
        assert_eq!(recipe_id("minecraft:bucket"), "tinkers_generated:melting/minecraft/bucket");
    }

    #[test]
    fn damageable_sword_ignores_the_stick() {
        let ings = vec![one("minecraft:iron_ingot"), one("minecraft:iron_ingot"), one("minecraft:stick")];
        let fluids = recipe_fluids(&ings, 1, &mut Mock);
        let json = recipe_json("minecraft:iron_sword", &fluids, true).unwrap();
        assert_eq!(
            json,
            "{\n  \"type\": \"tconstruct:damagable_melting\",\n  \"ingredient\": {\n    \"item\": \"minecraft:iron_sword\"\n  },\n  \"result\": {\n    \"fluid\": \"tconstruct:molten_iron\",\n    \"amount\": 180,\n    \"unit_size\": 10\n  },\n  \"temperature\": 800,\n  \"time\": 60\n}"
        );
    }

    #[test]
    fn byproducts_hottest_first_and_tags_kept() {
        // Two gold (a tag output), one iron, a water bucket, made 2 at a time.
        let ings = vec![one("minecraft:gold_ingot"), one("minecraft:iron_ingot"), one("minecraft:gold_ingot"), one("minecraft:water_bucket")];
        let mut fluids = recipe_fluids(&ings, 2, &mut Mock);
        // By name reversed: water, molten_iron, molten_gold; halved.
        assert_eq!(fluids.iter().map(|f| (f.fluid.as_str(), f.amount)).collect::<Vec<_>>(), vec![("minecraft:water", 500), ("tconstruct:molten_iron", 45), ("tconstruct:molten_gold", 90)]);
        fluids.sort_by(result_order);
        let json = recipe_json("minecraft:example", &fluids, false).unwrap();
        assert_eq!(
            json,
            "{\n  \"type\": \"tconstruct:melting\",\n  \"ingredient\": {\n    \"item\": \"minecraft:example\"\n  },\n  \"result\": {\n    \"fluid\": \"tconstruct:molten_iron\",\n    \"amount\": 45\n  },\n  \"temperature\": 800,\n  \"time\": 60,\n  \"byproducts\": [\n    {\n      \"tag\": \"forge:molten_gold\",\n      \"amount\": 90\n    },\n    {\n      \"fluid\": \"minecraft:water\",\n      \"amount\": 500\n    }\n  ]\n}"
        );
    }

    #[test]
    fn unmeltable_ingredients_stop_the_recipe() {
        // A stack with NBT, or a melting item after an ignored one in the same ingredient.
        assert!(recipe_fluids(&[vec![None]], 1, &mut Mock).is_empty());
        let mixed = vec![Some("minecraft:stick".into()), Some("minecraft:iron_ingot".into())];
        assert!(recipe_fluids(&[mixed], 1, &mut Mock).is_empty());
        // Different fluids in one ingredient.
        let either = vec![Some("minecraft:iron_ingot".into()), Some("minecraft:gold_ingot".into())];
        assert!(recipe_fluids(&[either], 1, &mut Mock).is_empty());
        // Only ignored items: nothing to melt.
        assert!(recipe_fluids(&[one("minecraft:stick")], 1, &mut Mock).is_empty());
    }

    #[test]
    fn results_merge_as_the_jar_does() {
        let mut n = NewRecipes::default();
        n.add("a", vec![fluid("tconstruct:molten_iron", 90, 800), fluid("minecraft:water", 10, 100)]);
        n.add("a", vec![fluid("tconstruct:molten_iron", 60, 800), fluid("minecraft:water", 20, 100)]);
        // The first pair's min, once per element.
        assert_eq!(n.entries().next().unwrap().1, &vec![fluid("tconstruct:molten_iron", 60, 800); 2]);
        // A recipe that can't melt clears the result for good.
        n.add("a", Vec::new());
        n.add("a", vec![fluid("tconstruct:molten_iron", 90, 800)]);
        assert!(n.entries().next().unwrap().1.is_empty());
        // Different first fluids: nothing.
        assert!(intersection(vec![fluid("a:x", 1, 0)], vec![fluid("a:y", 1, 0)]).is_empty());
    }

    #[test]
    fn container_temperatures() {
        assert_eq!(MeltingResult::from_stack("minecraft:water", 1000).temperature, 100);
        assert_eq!(MeltingResult::from_stack("minecraft:lava", 1000).temperature, 1000);
        assert_eq!(MeltingResult::from_stack("tconstruct:molten_iron", 90).temperature, 800);
    }

    #[test]
    fn empty_output_fails() {
        assert!(recipe_json("minecraft:x", &[fluid("a:b", 1, 0), MeltingResult::empty()], false).is_none());
    }
}
