//! The melter's recipes: `tconstruct:melting` with its ore and damageable kinds (MeltingRecipe, OreMeltingRecipe,
//! DamageableMeltingRecipe) and `tconstruct:melting_fuel` (MeltingFuel), read from the game's recipe table
//! (`recipes_of_type`, the data packs' files merged) and parsed again whenever that table changes (a `/reload`).
//! Item ingredients and tags go through the game's ingredient reader; Tinkers' fluid tags through the fluid table.

use super::util::*;
use crate::fluids;
use mod_api::serde_json::Value;
use mod_api::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub(crate) const MELTING_KINDS: [&str; 3] = ["tconstruct:melting", "tconstruct:ore_melting", "tconstruct:damagable_melting"];
pub(crate) const FUEL_KIND: &str = "tconstruct:melting_fuel";
pub(crate) const MATERIAL_MELTING: &str = "tconstruct:material_melting";

/// Config.melterOreRate (the default `ore_rates.melter`): metal 12 of 9 nuggets, gem 8 of 4 shards.
pub(crate) const ORE_RATE_METAL: (i64, i64) = (12, 9);
pub(crate) const ORE_RATE_GEM: (i64, i64) = (8, 4);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Rate {
    Plain,
    Metal,
    Gem,
}

#[derive(Clone, Debug)]
pub(crate) struct Melting {
    #[allow(dead_code)]
    pub id: String,
    pub items: HashSet<String>,
    /// The items in display order (for the recipe viewer).
    #[allow(dead_code)]
    pub order: Vec<String>,
    pub fluid: String,
    pub amount: i32,
    pub temperature: i32,
    pub time: i32,
    pub rate: Rate,
    /// DamageableMeltingRecipe's `unit_size`.
    pub unit: Option<i32>,
    /// OreMeltingRecipe's byproducts (the foundry's).
    pub byproducts: Vec<(String, i32)>,
    /// MaterialMeltingRecipe (stage 3): the material a part must be of; its amount is per cast cost unit.
    pub material: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Fuel {
    /// FluidIngredient: (is a tag, id); none for the solid fuel.
    pub fluid: Option<(bool, String)>,
    pub amount: i32,
    pub duration: i32,
    pub temperature: i32,
    pub rate: i32,
}

impl Fuel {
    pub(crate) fn matches(&self, fluid: &str) -> bool {
        match &self.fluid {
            Some((false, id)) => id == fluid,
            Some((true, tag)) => fluid_tag(tag).iter().any(|f| f == fluid),
            None => false,
        }
    }

    /// The fluids it takes (a tag read out).
    #[allow(dead_code)]
    pub(crate) fn fluids(&self) -> Vec<String> {
        match &self.fluid {
            Some((false, id)) => vec![id.clone()],
            Some((true, tag)) => fluid_tag(tag).to_vec(),
            None => Vec::new(),
        }
    }
}

pub(crate) struct Recipes {
    pub melting: Vec<Melting>,
    pub fuels: Vec<Fuel>,
}

pub(crate) fn c_tag(tag: &str) -> String {
    match tag.strip_prefix("forge:") {
        Some(rest) => format!("c:{rest}"),
        None => tag.to_owned(),
    }
}

/// A fluid tag's fluids, in the order Mantle's FluidOutput reads them (the first is what a tag output makes).
pub(crate) fn fluid_tag(tag: &str) -> &'static [String] {
    fluids::table().tags.get(&c_tag(tag)).map_or(&[][..], Vec::as_slice)
}

fn ingredient_of(host: &dyn Host, text: &str) -> (HashSet<String>, Vec<String>) {
    let json = Value::String(text.to_owned()).to_string();
    match host.recipe_ingredient(&json) {
        Ok(Some(mut i)) => (std::mem::take(&mut i.items), std::mem::take(&mut i.order)),
        _ => (HashSet::new(), Vec::new()),
    }
}

/// A 1.20 ingredient's items (`{"item"}`, `{"tag"}`, a list, Forge's difference and intersection), in order.
pub(crate) fn ingredient_items(host: &dyn Host, v: &Value, order: &mut Vec<String>) -> HashSet<String> {
    let mut out = HashSet::new();
    match v {
        Value::Array(a) => {
            for x in a {
                out.extend(ingredient_items(host, x, order));
            }
        }
        Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("forge:difference") => {
            let less = ingredient_items(host, &o["subtracted"], &mut Vec::new());
            let mut base_order = Vec::new();
            let base = ingredient_items(host, &o["base"], &mut base_order);
            out.extend(base.into_iter().filter(|i| !less.contains(i)));
            order.extend(base_order.into_iter().filter(|i| out.contains(i)));
        }
        Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("forge:intersection") => {
            let children: Vec<&Value> = o.get("children").and_then(Value::as_array).map(|c| c.iter().collect()).unwrap_or_default();
            let mut first_order = Vec::new();
            let mut sets = children.iter().enumerate().map(|(i, c)| {
                let mut scratch = Vec::new();
                let set = ingredient_items(host, c, &mut scratch);
                if i == 0 {
                    first_order = scratch;
                }
                set
            });
            let first = sets.next();
            let rest: Vec<HashSet<String>> = sets.collect();
            if let Some(first) = first {
                out.extend(first.into_iter().filter(|i| rest.iter().all(|s| s.contains(i))));
                order.extend(first_order.into_iter().filter(|i| out.contains(i)));
            }
        }
        // MaterialIngredient with no `material` (the casts' and molding's patterns) and Mantle's potion display
        // ingredient test the item alone.
        Value::Object(o) if o.get("type").is_none_or(|t| matches!(t.as_str(), Some("tconstruct:material" | "mantle:potion_display"))) => {
            let name = if let Some(item) = o.get("item").and_then(Value::as_str) {
                item.to_owned()
            } else if let Some(tag) = o.get("tag").and_then(Value::as_str) {
                format!("#{}", c_tag(tag))
            } else {
                return out;
            };
            let (items, o) = ingredient_of(host, &name);
            out.extend(items);
            order.extend(o);
        }
        Value::String(s) => {
            let (items, o) = ingredient_of(host, s);
            out.extend(items);
            order.extend(o);
        }
        _ => {}
    }
    out
}

fn tag_filled(host: &dyn Host, tag: &str) -> bool {
    !ingredient_of(host, &format!("#{}", c_tag(tag))).0.is_empty()
}

/// Forge's and Mantle's recipe conditions, as the melting recipes use them.
pub(crate) fn condition_holds(host: &dyn Host, c: &Value) -> bool {
    match c["type"].as_str().unwrap_or("") {
        "forge:true" => true,
        "forge:false" => false,
        "forge:not" => !condition_holds(host, &c["value"]),
        "forge:and" => c["values"].as_array().is_none_or(|a| a.iter().all(|x| condition_holds(host, x))),
        "forge:or" => c["values"].as_array().is_some_and(|a| a.iter().any(|x| condition_holds(host, x))),
        "forge:mod_loaded" => matches!(c["modid"].as_str(), Some("minecraft" | "forge" | "mantle" | "tconstruct")),
        "forge:item_exists" => c["item"].as_str().is_some_and(|i| item_key(i).is_some()),
        "mantle:tag_filled" if c["registry"].as_str() == Some("minecraft:fluid") => c["tag"].as_str().is_some_and(|t| !fluid_tag(t).is_empty()),
        "mantle:tag_filled" => c["tag"].as_str().is_some_and(|t| tag_filled(host, t)),
        "mantle:tag_empty" => c["tag"].as_str().is_none_or(|t| !tag_filled(host, t)),
        // ConfigEnabledCondition with Config's defaults: every recipe switch is on but forceIntegrationMaterials.
        "tconstruct:config" => c["prop"].as_str().is_some_and(|p| p != "force_integration_materials"),
        "mantle:tag_combination_filled" => {
            // `match` and `ignore`: a tag or a list of tags.
            let tags = |v: &Value| -> Vec<String> { v.as_str().map(|t| vec![t.to_owned()]).unwrap_or_else(|| v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()) };
            let sets: Vec<HashSet<String>> = tags(&c["match"]).iter().map(|t| ingredient_of(host, &format!("#{}", c_tag(t))).0).collect();
            let ignore: HashSet<String> = tags(&c["ignore"]).iter().flat_map(|t| ingredient_of(host, &format!("#{}", c_tag(t))).0).collect();
            let Some((first, rest)) = sets.split_first() else { return false };
            first.iter().any(|i| rest.iter().all(|s| s.contains(i)) && !ignore.contains(i))
        }
        _ => false,
    }
}

/// The recipe a file stands for: the first `forge:conditional` alternative whose conditions hold (the content keeps
/// them as `hz_conditional`), and its own `conditions`.
pub(crate) fn chosen<'a>(host: &dyn Host, json: &'a Value) -> Option<&'a Value> {
    let recipe = match json["hz_conditional"].as_array() {
        Some(alternatives) => alternatives
            .iter()
            .find(|a| a["conditions"].as_array().is_none_or(|c| c.iter().all(|x| condition_holds(host, x))))
            .map(|a| &a["recipe"])?,
        None => json,
    };
    recipe["conditions"].as_array().is_none_or(|c| c.iter().all(|x| condition_holds(host, x))).then_some(recipe)
}

/// FluidOutput: `{fluid, amount}` or `{tag, amount}` (the tag's first fluid).
pub(crate) fn fluid_output(v: &Value) -> Option<(String, i32)> {
    let amount = v["amount"].as_i64()?.clamp(1, i64::from(i32::MAX)) as i32;
    let fluid = match (v["fluid"].as_str(), v["tag"].as_str()) {
        (Some(f), _) => f.to_owned(),
        (None, Some(t)) => fluid_tag(t).first()?.clone(),
        _ => return None,
    };
    Some((fluid, amount))
}

fn parse_melting(host: &dyn Host, id: &str, json: &Value) -> Option<Melting> {
    let r = chosen(host, json)?;
    let kind = r["type"].as_str().unwrap_or("");
    let mut order = Vec::new();
    let items = ingredient_items(host, &r["ingredient"], &mut order);
    if items.is_empty() {
        return None;
    }
    let mut seen = HashSet::new();
    order.retain(|i| seen.insert(i.clone()));
    let (fluid, amount) = fluid_output(&r["result"])?;
    let rate = match (kind, r["rate"].as_str()) {
        ("tconstruct:ore_melting", Some("metal")) => Rate::Metal,
        ("tconstruct:ore_melting", Some("gem")) => Rate::Gem,
        _ => Rate::Plain,
    };
    let unit = (kind == "tconstruct:damagable_melting").then(|| r["result"]["unit_size"].as_i64().unwrap_or(1).clamp(1, i64::from(i32::MAX)) as i32);
    Some(Melting {
        id: id.to_owned(),
        items,
        order,
        fluid,
        amount,
        temperature: r["temperature"].as_i64().unwrap_or(0).clamp(0, i64::from(i32::MAX)) as i32,
        time: r["time"].as_i64().unwrap_or(1).clamp(1, i64::from(i32::MAX) / 10) as i32,
        rate,
        unit,
        byproducts: r["byproducts"].as_array().map(|a| a.iter().filter_map(fluid_output).collect()).unwrap_or_default(),
        material: None,
    })
}

fn parse_fuel(host: &dyn Host, json: &Value) -> Option<Fuel> {
    let r = chosen(host, json)?;
    let f = &r["fluid"];
    let fluid = if f.is_object() {
        match (f["fluid"].as_str(), f["tag"].as_str()) {
            (Some(id), _) => Some((false, id.to_owned())),
            (None, Some(tag)) => Some((true, tag.to_owned())),
            _ => return None,
        }
    } else {
        None
    };
    let int = |v: &Value, k: &str| v[k].as_i64().unwrap_or(0).clamp(0, i64::from(i32::MAX)) as i32;
    Some(Fuel { amount: int(f, "amount"), fluid, duration: int(r, "duration"), temperature: int(r, "temperature"), rate: int(r, "rate") })
}

pub(crate) type Key = Vec<(usize, usize)>;
static CACHE: Mutex<Option<(Key, Arc<Recipes>)>> = Mutex::new(None);

/// The melting and fuel recipes, read again whenever the game's recipe tables change.
pub(crate) fn recipes(host: &dyn Host) -> Arc<Recipes> {
    ensure_ids(host);
    let kinds: Vec<&str> = MELTING_KINDS.iter().chain([FUEL_KIND, MATERIAL_MELTING].iter()).copied().collect();
    let key: Key = crate::smeltery::util::tables_key(host, &kinds);
    if let Some(r) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, r)| r.clone()) {
        return r;
    }
    let tables: Vec<Arc<[Recipe]>> = kinds.iter().map(|k| host.recipes_of_type(k)).collect();
    let mut melting = Vec::new();
    for t in &tables[..MELTING_KINDS.len()] {
        for r in t.iter() {
            if let Some(m) = parse_melting(host, &r.id, &r.json) {
                melting.push(m);
            }
        }
    }
    let fuels = tables[MELTING_KINDS.len()].iter().filter_map(|r| parse_fuel(host, &r.json)).collect();
    // MaterialMeltingRecipe: every part with a cast cost, of the recipe's material (crate::tools).
    let parts: HashSet<String> = crate::tools::data::material_recipes(host).costs.keys().cloned().collect();
    for r in tables[MELTING_KINDS.len() + 1].iter() {
        let Some(j) = chosen(host, &r.json) else { continue };
        let (Some(input), Some((fluid, amount))) = (j["input"].as_str(), fluid_output(&j["result"])) else { continue };
        let temperature = j["temperature"].as_i64().unwrap_or(0).clamp(0, i64::from(i32::MAX)) as i32;
        melting.push(Melting {
            id: r.id.clone(),
            items: parts.clone(),
            order: Vec::new(),
            fluid,
            amount,
            temperature,
            time: crate::tools::data::calc_time(temperature, amount).max(1),
            rate: Rate::Plain,
            unit: None,
            byproducts: Vec::new(),
            material: Some(input.to_owned()),
        });
    }
    let out = Arc::new(Recipes { melting, fuels });
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, out.clone()));
    out
}

/// RecipeManager.getRecipeFor(MELTING, module): the first recipe whose ingredient takes the stack.
pub(crate) fn find_melting(host: &dyn Host, stack: &ItemStack) -> Option<(Arc<Recipes>, usize)> {
    let all = recipes(host);
    let id = sid(stack);
    let i = all.melting.iter().position(|m| m.items.contains(id) && m.material.as_deref().is_none_or(|want| crate::tools::part_is(host, stack, want)))?;
    Some((all, i))
}

/// IMeltingRecipe.getOutput(module): the ore rate (OreMeltingRecipe) or the damage left (DamageableMeltingRecipe).
pub(crate) fn melting_output(host: &dyn Host, m: &Melting, stack: &ItemStack) -> (String, i32) {
    let mut amount = i64::from(m.amount);
    if m.material.is_some() {
        amount *= i64::from(crate::tools::data::material_recipes(host).costs.get(sid(stack)).copied().unwrap_or(1));
    }
    match m.rate {
        Rate::Metal => amount = amount * ORE_RATE_METAL.0 / ORE_RATE_METAL.1,
        Rate::Gem => amount = amount * ORE_RATE_GEM.0 / ORE_RATE_GEM.1,
        Rate::Plain => {}
    }
    if let Some(unit) = m.unit {
        let max = match host.item_default_component(stack.item(), "minecraft:max_damage") {
            Some(Tag::Int(v)) => i64::from(v),
            _ => 0,
        };
        if max > 0 {
            let unit = i64::from(unit);
            let left = amount * (max - i64::from(stack.damage()).min(max)) / max;
            amount = if left <= unit { unit.max(1) } else if unit > 1 { left / unit * unit } else { left };
        }
    }
    (m.fluid.clone(), amount.clamp(1, i64::from(i32::MAX)) as i32)
}

/// OreMeltingRecipe.byproducts.
pub(crate) fn byproducts(m: &Melting) -> Vec<(String, i32)> {
    m.byproducts.clone()
}

/// MeltingFuelLookup.getSolid: the fuel without a fluid (the first; Java warns on more).
pub(crate) fn solid_fuel(r: &Recipes) -> Option<&Fuel> {
    r.fuels.iter().find(|f| f.fluid.is_none())
}

/// MeltingFuelLookup.findFuel: the first fluid fuel that takes `fluid`.
pub(crate) fn fluid_fuel<'a>(r: &'a Recipes, fluid: &str) -> Option<&'a Fuel> {
    r.fuels.iter().find(|f| f.matches(fluid))
}
