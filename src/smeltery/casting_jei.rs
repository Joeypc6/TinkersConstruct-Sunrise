//! JEIPlugin's casting table, casting basin and molding categories (CastingTableCategory, CastingBasinCategory,
//! AbstractCastingCategory, MoldingRecipeCategory) through the JEI fork's Rust API. A casting card (117x54 on
//! `gui/jei/casting`): the fluid's well at 3,3 (32x32 of 810 mB), the faucet's stream at 43,8, the cast at 38,19 (an
//! input when consumed, else kept), the result at 93,18; the cooling time and "cast kept / consumed" as tooltips. A
//! molding card (70x57): the material at 3,24, the pattern at 3,1 (kept: shown again at 51,8), the result at 51,24.
//! Material casting cards come with the material system (stage 3). The recipe function gets no translations, so the
//! lines are worded from a table taken on the client.

use super::casting_recipes::{self as recipes, CastKind, fluid_amount};
use super::recipes::fluid_tag;
use super::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

const TABLE: &str = "tconstruct:casting_table";
const BASIN: &str = "tconstruct:casting_basin";
const MOLDING: &str = "tconstruct:molding";
const BACKGROUND: &str = "tconstruct:gui/jei/casting";

static REGISTERED: Mutex<bool> = Mutex::new(false);
static WORDS: Mutex<Option<HashMap<&'static str, String>>> = Mutex::new(None);

const GRAY: u32 = 0xFF80_8080;

const KEYS: [(&str, &str); 5] = [
    ("jei.tconstruct.time", "%s s"),
    ("jei.tconstruct.casting.time", "Cooling time: %s s"),
    ("jei.tconstruct.casting.cast_kept", "Cast kept"),
    ("jei.tconstruct.casting.cast_consumed", "Cast consumed"),
    ("jei.tconstruct.molding.pattern_consumed", "Pattern consumed"),
];

fn word(key: &str) -> String {
    WORDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|w| w.get(key).cloned())
        .or_else(|| KEYS.iter().find(|(k, _)| *k == key).map(|(_, e)| (*e).to_owned()))
        .unwrap_or_else(|| key.to_owned())
}

/// Registered once, from the client's tick (JEI is installed).
pub(crate) fn register(host: &mut dyn ClientHost) {
    let mut done = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
    if *done {
        return;
    }
    *done = true;
    let mut words: HashMap<&'static str, String> = HashMap::new();
    for (k, english) in KEYS {
        words.insert(k, lang(&*host, k, english));
    }
    *WORDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(words);
    use jei::api::*;
    register_category(CategoryDef::new(TABLE, "jei.tconstruct.casting.table", "tconstruct:seared_table").catalyst("tconstruct:scorched_table"));
    register_category(CategoryDef::new(BASIN, "jei.tconstruct.casting.basin", "tconstruct:seared_basin").catalyst("tconstruct:scorched_basin"));
    // addCastingCatalyst: the tables (and the basins, when the basin has molding recipes) are molding's stations.
    register_category(CategoryDef::new(MOLDING, "jei.tconstruct.molding.title", "tconstruct:blank_sand_cast").catalyst("tconstruct:seared_table").catalyst("tconstruct:scorched_table"));
    register_recipes("tconstruct:casting", cards);
}

/// The fluids a recipe takes (a tag read out) with the amount, the first first.
fn fluids_of(c: &recipes::Casting) -> Vec<(String, i32)> {
    let mut out: Vec<(String, i32)> = Vec::new();
    for (tag, id, _) in &c.fluids {
        let list: Vec<String> = if *tag { fluid_tag(id).to_vec() } else { vec![id.clone()] };
        for f in list {
            if !out.iter().any(|(g, _)| *g == f)
                && let Some(a) = fluid_amount(&c.fluids, &f)
            {
                out.push((f, a));
            }
        }
    }
    out
}

fn casting_card(c: &recipes::Casting, category: &str) -> Option<jei::api::RecipeDef> {
    use jei::api::{RecipeDef, SlotRole};
    let fluids = fluids_of(c);
    let results: Vec<(String, u32)> = match &c.kind {
        CastKind::Item(id, n) => vec![(id.clone(), *n)],
        CastKind::Duplicate => c.cast_order.iter().map(|i| (i.clone(), 1)).collect(),
        // ContainerFillingRecipe: the container (its filled look needs the fluid in the stack, which the card's item
        // slots can't carry); the fluid is any that has a bucket, so the card shows water.
        CastKind::Fill(_) => c.container.iter().map(|i| (i.clone(), 1)).collect(),
        CastKind::Material { .. } => return None,
    };
    let fluids = if fluids.is_empty() {
        match c.kind {
            CastKind::Fill(n) => vec![("minecraft:water".to_owned(), n)],
            _ => return None,
        }
    } else {
        fluids
    };
    let (fluid, amount) = fluids.first()?.clone();
    let has_cast = c.cast.is_some();
    let result_items: Vec<(&str, u32)> = results.iter().map(|(i, n)| (i.as_str(), *n)).collect();
    // AbstractCastingCategory.draw: the background (with its own frames), the tank gauge (133,0) over the fluid, the
    // faucet's stream, the table or basin (117,0 / 117,16) at 38,35, the arrow (117,32) filling over the cooling
    // time, the cast hint (141,32 consumed / 141,43 kept) and the cooling time centred on 72 at the top, in grey.
    let basin = category == BASIN;
    let mut card = RecipeDef::new(&format!("{category}/{}", c.id.replace(':', "/")), [117.0, 54.0])
        .background(BACKGROUND, [0.0, 0.0, 117.0, 54.0])
        .frameless()
        .output(93.0, 18.0, &result_items)
        .fluid([3.0, 3.0, 32.0, 32.0], SlotRole::Input, &fluid, amount.max(1) as u32, 810)
        .fluid_bare(Some((BACKGROUND, [133.0, 0.0, 32.0, 32.0])))
        .fluid([43.0, 8.0, 6.0, if has_cast { 11.0 } else { 27.0 }], SlotRole::Other, &fluid, 1, 1)
        .fluid_bare(None)
        .sprite(BACKGROUND, [117.0, if basin { 16.0 } else { 0.0 }, 16.0, 16.0], 38.0, 35.0)
        .animated(BACKGROUND, [117.0, 32.0, 24.0, 17.0], 58.0, 18.0, (c.cooling.max(1) as u32) * 50)
        .hover([58.0, 18.0, 24.0, 17.0], &[&word("jei.tconstruct.casting.time").replacen("%s", &(c.cooling / 20).to_string(), 1)])
        .text_centered(72.0, 2.0, &word("jei.tconstruct.time").replacen("%s", &(c.cooling / 20).to_string(), 1), GRAY);
    if has_cast {
        let casts: Vec<(&str, u32)> = c.cast_order.iter().map(|i| (i.as_str(), 1)).collect();
        card = if c.consumed { card.input(38.0, 19.0, &casts) } else { card.other(38.0, 19.0, &casts) };
        let note = if c.consumed { "jei.tconstruct.casting.cast_consumed" } else { "jei.tconstruct.casting.cast_kept" };
        card = card
            .sprite(BACKGROUND, [141.0, if c.consumed { 32.0 } else { 43.0 }, 13.0, 11.0], 63.0, 39.0)
            .hover([63.0, 39.0, 13.0, 11.0], &[&word(note)]);
    }
    Some(card)
}

fn cards(host: &dyn Host) -> Vec<(String, jei::api::RecipeDef)> {
    use jei::api::RecipeDef;
    ensure_ids(host);
    let all = recipes::recipes(host);
    let mut out = Vec::new();
    for (category, list) in [(TABLE, &all.table), (BASIN, &all.basin)] {
        for c in list.iter() {
            if c.cast.as_ref().is_some_and(|_| c.cast_order.is_empty()) {
                continue;
            }
            if let Some(card) = casting_card(c, category) {
                out.push((category.to_owned(), card));
            }
        }
    }
    for (basin, m) in all.molding_table.iter().map(|m| (false, m)).chain(all.molding_basin.iter().map(|m| (true, m))) {
        let material: Vec<(&str, u32)> = m.material_order.iter().map(|i| (i.as_str(), 1)).collect();
        if material.is_empty() {
            continue;
        }
        // MoldingRecipeCategory: casting.png 0,55 (70x57, its arrow and frames drawn in), the dot that says whether
        // a pattern is used (70,55 / 76,55) at 8,17, the table or basin under the material, and under the result
        // when there is a pattern.
        let block = [117.0, if basin { 16.0 } else { 0.0 }, 16.0, 16.0];
        let mut card = RecipeDef::new(&format!("{MOLDING}/{}", m.id.replace(':', "/")), [70.0, 57.0])
            .background(BACKGROUND, [0.0, 55.0, 70.0, 57.0])
            .frameless()
            .sprite(BACKGROUND, [if m.pattern.is_some() { 70.0 } else { 76.0 }, 55.0, 6.0, 6.0], 8.0, 17.0)
            .sprite(BACKGROUND, block, 3.0, 40.0)
            .input(3.0, 24.0, &material)
            .output(51.0, 24.0, &[(m.result.0.as_str(), m.result.1)]);
        if m.pattern.is_some() {
            card = card.sprite(BACKGROUND, block, 51.0, 40.0);
            let pattern: Vec<(&str, u32)> = m.pattern_order.iter().map(|i| (i.as_str(), 1)).collect();
            if m.pattern_consumed {
                card = card.input(3.0, 1.0, &pattern).hover([3.0, 1.0, 16.0, 16.0], &[&word("jei.tconstruct.molding.pattern_consumed")]);
            } else {
                card = card.other(3.0, 1.0, &pattern).other(51.0, 8.0, &pattern);
            }
        }
        out.push((MOLDING.to_owned(), card));
    }
    out
}
