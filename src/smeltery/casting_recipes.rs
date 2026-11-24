//! The casting recipes (TinkerRecipeTypes CASTING_TABLE, CASTING_BASIN, MOLDING_TABLE, MOLDING_BASIN and the
//! serializers registered to them): ItemCastingRecipe and RetexturedCastingRecipe, CastDuplicationRecipe,
//! ContainerFillingRecipe and MoldingRecipe, read from the game's recipe table (`recipes_of_type`) with their
//! Forge/Mantle conditions, read again whenever that table changes.
//!
//! Material casting (MaterialCastingRecipe, CompositeCastingRecipe, ToolCastingRecipe, PartSwapCastingRecipe, the
//! tipping recipes) needs the tool and material data of stage 3: those kinds are read into `CastKind::Material` with
//! their JSON kept, and `material_cast` is the one place stage 3 fills in (it answers nothing until then).
//! PotionCastingRecipe needs a fluid that carries its potion (no fluid on the fork carries one yet: README, gaps).

use super::containers;
use super::recipes::{c_tag, chosen, fluid_tag, ingredient_items};
use super::util::*;
use mod_api::serde_json::Value;
use mod_api::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// The table's kinds in the order they are tried (item recipes before material ones, as the spec's port notes ask:
/// Java's own order on ties is RecipeManager's hash order).
pub(crate) const TABLE_KINDS: [&str; 10] = [
    "tconstruct:casting_table",
    "tconstruct:retextured_casting_table",
    "tconstruct:table_duplication",
    "tconstruct:table_filling",
    "tconstruct:casting_table_potion",
    "tconstruct:table_casting_material",
    "tconstruct:table_casting_composite",
    "tconstruct:table_tool_casting",
    "tconstruct:table_casting_part_swapping",
    "tconstruct:casting_table_tipping",
];
pub(crate) const BASIN_KINDS: [&str; 10] = [
    "tconstruct:casting_basin",
    "tconstruct:retextured_casting_basin",
    "tconstruct:basin_duplication",
    "tconstruct:basin_filling",
    "tconstruct:casting_basin_potion",
    "tconstruct:basin_casting_material",
    "tconstruct:basin_casting_composite",
    "tconstruct:basin_tool_casting",
    "tconstruct:basin_casting_part_swapping",
    "tconstruct:casting_basin_tipping",
];
pub(crate) const MOLDING_TABLE: &str = "tconstruct:molding_table";
pub(crate) const MOLDING_BASIN: &str = "tconstruct:molding_basin";
/// The tipped-clearing kinds (tool data too).
const CLEARING_KINDS: [&str; 2] = ["tconstruct:casting_table_tipped_clearing", "tconstruct:casting_basin_tipped_clearing"];

/// Every recipe type the casting blocks read (sent to clients for the recipe viewer).
pub(crate) fn all_kinds() -> Vec<&'static str> {
    TABLE_KINDS.iter().chain(BASIN_KINDS.iter()).chain(CLEARING_KINDS.iter()).chain([MOLDING_TABLE, MOLDING_BASIN].iter()).copied().collect()
}

#[derive(Clone, Debug)]
pub(crate) enum CastKind {
    /// ItemCastingRecipe and RetexturedCastingRecipe (the result as is: no block texture component yet): the result's
    /// item and count.
    Item(String, u32),
    /// CastDuplicationRecipe: a copy of the cast.
    Duplicate,
    /// ContainerFillingRecipe: the cast (a bucket, a can, a tank's item) filled with `fluid_amount`.
    Fill(i32),
    /// The kinds that need the tool and material data (stage 3): the recipe's kind and JSON text.
    Material { kind: String, json: String },
}

#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "jei"), allow(dead_code))]
pub(crate) struct Casting {
    pub id: String,
    /// The cast ingredient's items; none for "no cast" (it then matches only an empty cast slot).
    pub cast: Option<HashSet<String>>,
    /// The cast's items in display order (the recipe viewer).
    pub cast_order: Vec<String>,
    /// The container a filling recipe takes.
    pub container: Option<String>,
    /// FluidIngredient: (is a tag, id, amount), the first entry taking a fluid gives the amount.
    pub fluids: Vec<(bool, String, i32)>,
    pub cooling: i32,
    pub consumed: bool,
    pub switch: bool,
    pub kind: CastKind,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "jei"), allow(dead_code))]
pub(crate) struct Molding {
    pub id: String,
    pub material: HashSet<String>,
    pub material_order: Vec<String>,
    /// None: the empty pattern (only an empty hand, or the "pickup" retry).
    pub pattern: Option<HashSet<String>>,
    pub pattern_order: Vec<String>,
    pub pattern_consumed: bool,
    pub result: (String, u32),
}

pub(crate) struct CastingRecipes {
    pub table: Vec<Casting>,
    pub basin: Vec<Casting>,
    pub molding_table: Vec<Molding>,
    pub molding_basin: Vec<Molding>,
    /// `#tconstruct:casts/empty/table` (gold bars) and `/basin` (gold platform): "no cast" to the recipes.
    pub empty_table: HashSet<String>,
    pub empty_basin: HashSet<String>,
}

impl CastingRecipes {
    pub(crate) fn list(&self, basin: bool) -> &[Casting] {
        if basin { &self.basin } else { &self.table }
    }
    pub(crate) fn molding(&self, basin: bool) -> &[Molding] {
        if basin { &self.molding_basin } else { &self.molding_table }
    }
    pub(crate) fn by_id(&self, basin: bool, id: &str) -> Option<&Casting> {
        self.list(basin).iter().find(|r| r.id == id)
    }
}

/// A FluidIngredient: `{fluid|name|tag, amount}` or a list of them.
pub(crate) fn fluid_ingredient(v: &Value) -> Vec<(bool, String, i32)> {
    match v {
        Value::Array(a) => a.iter().flat_map(fluid_ingredient).collect(),
        Value::Object(o) => {
            let amount = o.get("amount").and_then(Value::as_i64).unwrap_or(0).clamp(0, i64::from(i32::MAX)) as i32;
            let id = o.get("fluid").or_else(|| o.get("name")).and_then(Value::as_str);
            match (id, o.get("tag").and_then(Value::as_str)) {
                (Some(f), _) => vec![(false, f.to_owned(), amount)],
                (None, Some(t)) => vec![(true, t.to_owned(), amount)],
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

/// The amount a fluid ingredient asks of `fluid`, if it takes it.
pub(crate) fn fluid_amount(fluids: &[(bool, String, i32)], fluid: &str) -> Option<i32> {
    fluids.iter().find(|(tag, id, _)| if *tag { fluid_tag(id).iter().any(|f| f == fluid) } else { id == fluid }).map(|(_, _, a)| *a)
}

/// The fluids a fluid ingredient takes, in order (a tag read out), with their amounts.
#[allow(dead_code)]
pub(crate) fn ingredient_fluids(fluids: &[(bool, String, i32)]) -> Vec<(String, i32)> {
    let mut out: Vec<(String, i32)> = Vec::new();
    for (tag, id, a) in fluids {
        let list: Vec<String> = if *tag { fluid_tag(id).to_vec() } else { vec![id.clone()] };
        for f in list {
            if !out.iter().any(|(g, _)| *g == f) {
                out.push((f, *a));
            }
        }
    }
    out
}

/// Mantle's TagPreference: a tag's item from the first namespace of the preference list, then by name.
fn preferred(items: &[String]) -> Option<String> {
    const ORDER: [&str; 9] = ["minecraft", "tconstruct", "tmechworks", "metalborn", "embers", "create", "immersiveengineering", "mekanism", "thermal"];
    let rank = |id: &str| {
        let ns = id.split_once(':').map_or("minecraft", |(n, _)| n);
        ORDER.iter().position(|o| *o == ns).unwrap_or(ORDER.len())
    };
    items.iter().min_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))).cloned()
}

/// The items of a tag, in order.
pub(crate) fn tag_order(host: &dyn Host, tag: &str) -> Vec<String> {
    let json = Value::String(format!("#{}", c_tag(tag))).to_string();
    host.recipe_ingredient(&json).ok().flatten().map(|mut i| std::mem::take(&mut i.order)).unwrap_or_default()
}

/// ItemOutput: an id, `{item, count}` or `{tag, count}` (the preferred item); one that names no item here, none.
pub(crate) fn item_output(host: &dyn Host, v: &Value) -> Option<(String, u32)> {
    let count = v["count"].as_u64().unwrap_or(1).clamp(1, 64) as u32;
    let id = match (v.as_str(), v["item"].as_str().or_else(|| v["id"].as_str()), v["tag"].as_str()) {
        (Some(s), _, _) | (None, Some(s), _) => s.to_owned(),
        (None, None, Some(t)) => preferred(&tag_order(host, t))?,
        _ => return None,
    };
    item_key(&id)?;
    Some((id, count))
}

/// An ingredient that may be empty: `Some(None)` when absent, none when it names nothing here.
fn optional_ingredient(host: &dyn Host, v: &Value, order: &mut Vec<String>) -> Option<Option<HashSet<String>>> {
    if v.is_null() {
        return Some(None);
    }
    let items = ingredient_items(host, v, order);
    let mut seen = HashSet::new();
    order.retain(|i| seen.insert(i.clone()));
    (!items.is_empty()).then_some(Some(items))
}

fn parse_casting(host: &dyn Host, id: &str, json: &Value) -> Option<Casting> {
    let r = chosen(host, json)?;
    let kind = r["type"].as_str().unwrap_or("");
    let cooling = r["cooling_time"].as_i64().unwrap_or(1).clamp(0, i64::from(i32::MAX)) as i32;
    let fluids = fluid_ingredient(&r["fluid"]);
    let base = |kind: CastKind, cast: Option<HashSet<String>>, cast_order: Vec<String>| Casting {
        id: id.to_owned(),
        cast,
        cast_order,
        container: None,
        fluids: fluids.clone(),
        cooling,
        consumed: r["cast_consumed"].as_bool().unwrap_or(false),
        switch: r["switch_slots"].as_bool().unwrap_or(false),
        kind,
    };
    match kind {
        "tconstruct:casting_table" | "tconstruct:casting_basin" | "tconstruct:retextured_casting_table" | "tconstruct:retextured_casting_basin" => {
            let mut order = Vec::new();
            let cast = optional_ingredient(host, &r["cast"], &mut order)?;
            let (item, count) = item_output(host, &r["result"])?;
            Some(base(CastKind::Item(item, count), cast, order))
        }
        "tconstruct:table_duplication" | "tconstruct:basin_duplication" => {
            let mut order = Vec::new();
            let cast = optional_ingredient(host, &r["cast"], &mut order)??;
            let mut c = base(CastKind::Duplicate, Some(cast), order);
            c.consumed = false;
            c.switch = false;
            Some(c)
        }
        "tconstruct:table_filling" | "tconstruct:basin_filling" => {
            let container = r["container"].as_str()?.to_owned();
            item_key(&container)?;
            let amount = r["fluid_amount"].as_i64().unwrap_or(0).clamp(1, i64::from(i32::MAX)) as i32;
            Some(Casting {
                id: id.to_owned(),
                cast: Some(std::iter::once(container.clone()).collect()),
                cast_order: vec![container.clone()],
                container: Some(container),
                fluids: Vec::new(),
                cooling: 5,
                consumed: true,
                switch: false,
                kind: CastKind::Fill(amount),
            })
        }
        // PotionCastingRecipe: the result carries the fluid's potion, and no fluid here carries one (README, gaps).
        "tconstruct:casting_table_potion" | "tconstruct:casting_basin_potion" => None,
        k if TABLE_KINDS.contains(&k) || BASIN_KINDS.contains(&k) || CLEARING_KINDS.contains(&k) => {
            let mut order = Vec::new();
            let cast = optional_ingredient(host, &r["cast"], &mut order).unwrap_or(None);
            let mut c = base(CastKind::Material { kind: k.to_owned(), json: r.to_string() }, cast, order);
            // ToolCastingRecipe always uses up its cast (the part becomes the tool).
            c.consumed |= k.ends_with("_tool_casting");
            Some(c)
        }
        _ => None,
    }
}

fn parse_molding(host: &dyn Host, id: &str, json: &Value) -> Option<Molding> {
    let r = chosen(host, json)?;
    let mut material_order = Vec::new();
    let material = ingredient_items(host, &r["material"], &mut material_order);
    if material.is_empty() {
        return None;
    }
    let mut pattern_order = Vec::new();
    let pattern = optional_ingredient(host, &r["pattern"], &mut pattern_order)?;
    Some(Molding {
        id: id.to_owned(),
        pattern_consumed: pattern.is_some() && r["pattern_consumed"].as_bool().unwrap_or(false),
        material,
        material_order,
        pattern,
        pattern_order,
        result: item_output(host, &r["result"])?,
    })
}

pub(crate) fn tag_items(host: &dyn Host, tag: &str) -> HashSet<String> {
    tag_order(host, tag).into_iter().collect()
}

pub(crate) type Key = Vec<(usize, usize)>;
static CACHE: Mutex<Option<(Key, Arc<CastingRecipes>)>> = Mutex::new(None);

/// The casting and molding recipes, read again whenever the game's recipe tables change.
pub(crate) fn recipes(host: &dyn Host) -> Arc<CastingRecipes> {
    ensure_ids(host);
    let kinds = all_kinds();
    let key: Key = crate::smeltery::util::tables_key(host, &kinds);
    if let Some(r) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, r)| r.clone()) {
        return r;
    }
    let tables: Vec<Arc<[Recipe]>> = kinds.iter().map(|k| host.recipes_of_type(k)).collect();
    let of = |k: &str| tables[kinds.iter().position(|x| *x == k).unwrap_or(0)].clone();
    let read = |list: &[&str]| -> Vec<Casting> { list.iter().flat_map(|k| of(k).iter().filter_map(|r| parse_casting(host, &r.id, &r.json)).collect::<Vec<_>>()).collect() };
    let mut table = read(&TABLE_KINDS);
    let mut basin = read(&BASIN_KINDS);
    table.extend(read(&CLEARING_KINDS[..1]));
    basin.extend(read(&CLEARING_KINDS[1..]));
    let molding = |k: &str| -> Vec<Molding> { of(k).iter().filter_map(|r| parse_molding(host, &r.id, &r.json)).collect() };
    let out = Arc::new(CastingRecipes {
        table,
        basin,
        molding_table: molding(MOLDING_TABLE),
        molding_basin: molding(MOLDING_BASIN),
        empty_table: tag_items(host, "tconstruct:casts/empty/table"),
        empty_basin: tag_items(host, "tconstruct:casts/empty/basin"),
    });
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, out.clone()));
    out
}

/// What a material recipe makes of the cast and the fluid (stage 3: MaterialCastingLookup, the material fluids, the
/// tool data): the amount it takes, its cooling time and its result (`tools::data::material_cast`).
pub(crate) fn material_cast(host: &mut dyn ServerHost, kind: &str, json: &str, cast: &Option<ItemStack>, fluid: &str) -> Option<(i32, i32, ItemStack)> {
    crate::tools::data::material_cast(&*host, kind, json, cast, fluid)
}

/// CastingContainerWrapper.getStack: the cast slot's stack, an empty cast (gold bars, a gold platform) as none.
pub(crate) fn cast_view(all: &CastingRecipes, basin: bool, stack: &Option<ItemStack>) -> Option<ItemStack> {
    let empty = if basin { &all.empty_basin } else { &all.empty_table };
    stack.clone().filter(|s| s.count() > 0 && !empty.contains(sid(s)))
}

impl Casting {
    /// ICastingRecipe.matches with the cast view and the fluid, and getFluidAmount.
    pub(crate) fn amount(&self, host: &mut dyn ServerHost, cast: &Option<ItemStack>, fluid: &str) -> Option<i32> {
        if let CastKind::Fill(want) = self.kind {
            // ContainerFillingRecipe: the cast is the container and its handler takes some of `fluid_amount`.
            let c = cast.as_ref().filter(|c| Some(sid(c)) == self.container.as_deref())?;
            return containers::fill_item(host, c, fluid, want).map(|(n, _)| n);
        }
        if let CastKind::Material { kind, json } = &self.kind {
            return material_cast(host, kind, json, cast, fluid).map(|(n, _, _)| n);
        }
        let ok = match (&self.cast, cast) {
            (None, None) => true,
            (Some(items), Some(c)) => items.contains(sid(c)),
            _ => false,
        };
        if !ok {
            return None;
        }
        fluid_amount(&self.fluids, fluid).filter(|a| *a > 0)
    }

    /// ICastingRecipe.getCoolingTime.
    pub(crate) fn cooling_time(&self, host: &mut dyn ServerHost, cast: &Option<ItemStack>, fluid: &str) -> i32 {
        match &self.kind {
            CastKind::Material { kind, json } => material_cast(host, kind, json, cast, fluid).map_or(1, |(_, t, _)| t),
            _ => self.cooling,
        }
    }

    /// ICastingRecipe.assemble.
    pub(crate) fn assemble(&self, host: &mut dyn ServerHost, cast: &Option<ItemStack>, fluid: &str) -> Option<ItemStack> {
        match &self.kind {
            CastKind::Item(id, n) => new_stack(id, *n),
            CastKind::Duplicate => cast.as_ref().and_then(|c| with_count(c, 1)),
            CastKind::Fill(want) => containers::fill_item(host, cast.as_ref()?, fluid, *want).and_then(|(_, s)| s),
            CastKind::Material { kind, json } => material_cast(host, kind, json, cast, fluid).map(|(_, _, s)| s),
        }
    }
}

/// CastingBlockEntity.findCastingRecipe: the first recipe taking the cast and the fluid, and the amount it takes.
pub(crate) fn find_casting(host: &mut dyn ServerHost, basin: bool, cast: &Option<ItemStack>, fluid: &str) -> Option<(String, i32)> {
    let all = recipes(&*host);
    let view = cast_view(&all, basin, cast);
    for r in all.list(basin) {
        if let Some(a) = r.amount(host, &view, fluid) {
            return Some((r.id.clone(), a));
        }
    }
    // Logged once per cast item and fluid: what the table or basin was offered and refused.
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    if let Some(c) = cast {
        let key = format!("{} {} {fluid}", if basin { "basin" } else { "table" }, sid(c));
        if SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().insert(key.clone()) {
            super::util::tlog(&format!("no casting recipe: {key} (view {:?}, {} recipes)", view.as_ref().map(|v| sid(v).to_owned()), all.list(basin).len()));
        }
    }
    None
}

/// CastingBlockEntity.findMoldingRecipe: slot 0 the material, `pattern` the held item (none: an empty one).
pub(crate) fn find_molding(host: &dyn Host, basin: bool, material: &ItemStack, pattern: Option<&ItemStack>) -> Option<Molding> {
    let all = recipes(host);
    let m = sid(material);
    all.molding(basin)
        .iter()
        .find(|r| {
            r.material.contains(m)
                && match (&r.pattern, pattern) {
                    (None, None) => true,
                    (Some(p), Some(h)) => p.contains(sid(h)),
                    _ => false,
                }
        })
        .cloned()
}
