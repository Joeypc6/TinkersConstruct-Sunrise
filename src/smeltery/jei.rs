//! JEIPlugin's melting and melting-fuel categories (MeltingCategory, MeltingFuelCategory) through the JEI fork's Rust
//! API: one card per melting recipe (the item at 24,18, the fluid at 96,4 32x32 with the melter's ore boost, the
//! usable fuels at 4,4, the solid fuel examples under them for recipes the heater reaches, the temperature on top)
//! and one per fuel (a fluid fuel's well, or each fuel example; temperature, speed and how long it lasts).
//! The recipe function gets no translations, so the lines are worded from a table taken on the client.

use super::recipes::{self, ORE_RATE_GEM, ORE_RATE_METAL, Rate};
use super::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

const MELTING: &str = "tconstruct:melting";
const FUEL: &str = "tconstruct:fuel";
const BACKGROUND: &str = "tconstruct:gui/jei/melting";
const GRAY: u32 = 0xFF80_8080;
const WHITE: u32 = 0xFFFF_FFFF;

static REGISTERED: Mutex<bool> = Mutex::new(false);
static WORDS: Mutex<Option<HashMap<&'static str, String>>> = Mutex::new(None);

const KEYS: [(&str, &str); 8] = [
    ("jei.tconstruct.melting.ore", "Recipe output is boosted based on the controller type"),
    ("jei.tconstruct.temperature", "%s°C"),
    ("jei.tconstruct.melting.multiplier", "Speed multiplier: %sx"),
    ("jei.tconstruct.fuel.duration.lasts", "Lasts:"),
    ("jei.tconstruct.fuel.duration.liquid", "%s s / %s mb"),
    ("jei.tconstruct.fuel.duration.solid", "%s s / item"),
    ("jei.tconstruct.fuel.solid", "Solid Fuel"),
    ("jei.tconstruct.melting.time", "Melting time: %s s"),
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
    register_category(CategoryDef::new(MELTING, "jei.tconstruct.melting.title", "tconstruct:seared_melter").catalyst("tconstruct:smeltery_controller"));
    let mut fuel = CategoryDef::new(FUEL, "jei.tconstruct.fuel.title", "tconstruct:seared_melter");
    for c in ["tconstruct:seared_heater", "tconstruct:smeltery_controller", "tconstruct:scorched_alloyer", "tconstruct:foundry_controller"] {
        fuel = fuel.catalyst(c);
    }
    register_category(fuel);
    register_recipes("tconstruct:smeltery_melting", cards);
}

/// Java's Float.toString of `rate / 10`.
fn float(v: f32) -> String {
    format!("{v:?}")
}

/// MeltingFuelHandler.getUsableFuels: the fluid fuels at or above the first fuel temperature that reaches `t`.
fn usable_fuels(all: &recipes::Recipes, t: i32) -> Vec<(String, i32)> {
    let mut sorted: Vec<&recipes::Fuel> = all.fuels.iter().collect();
    sorted.sort_by_key(|f| f.temperature);
    let Some(at) = sorted.iter().map(|f| f.temperature).find(|x| t <= *x) else { return Vec::new() };
    sorted.iter().filter(|f| f.temperature >= at && f.fluid.is_some()).flat_map(|f| f.fluids().into_iter().map(|x| (x, f.amount))).collect()
}

fn fuel_examples(host: &dyn Host) -> Vec<String> {
    let json = mod_api::serde_json::Value::String("#tconstruct:fuel_examples".into()).to_string();
    host.recipe_ingredient(&json).ok().flatten().map(|mut i| std::mem::take(&mut i.order)).unwrap_or_default()
}

fn cards(host: &dyn Host) -> Vec<(String, jei::api::RecipeDef)> {
    use jei::api::{RecipeDef, SlotRole};
    ensure_ids(host);
    let all = recipes::recipes(host);
    let (solid_temp, solid_rate) = recipes::solid_fuel(&all).map_or((800, 8), |f| (f.temperature, f.rate));
    let examples = fuel_examples(host);
    let example_items: Vec<(&str, u32)> = examples.iter().map(|i| (i.as_str(), 1)).collect();
    let mut out = Vec::new();
    for m in &all.melting {
        if m.order.is_empty() {
            continue;
        }
        let items: Vec<(&str, u32)> = m.order.iter().map(|i| (i.as_str(), 1)).collect();
        // MeltingFluidCallback with the ore boost (the melter's rate).
        let boosted = match m.rate {
            Rate::Metal => (i64::from(m.amount) * ORE_RATE_METAL.0 / ORE_RATE_METAL.1) as i32,
            Rate::Gem => (i64::from(m.amount) * ORE_RATE_GEM.0 / ORE_RATE_GEM.1) as i32,
            Rate::Plain => m.amount,
        };
        let temp = word("jei.tconstruct.temperature").replacen("%s", &m.temperature.to_string(), 1);
        // The temperature centred over the left 113 pixels (6 pixels a character, near enough for digits).
        let tx = ((113.0 - temp.chars().count() as f32 * 6.0) / 2.0).floor().max(0.0);
        let mut card = RecipeDef::new(&format!("{MELTING}/{}", m.id.replace(':', "/")), [132.0, 40.0])
            .background(BACKGROUND, [0.0, 0.0, 132.0, 40.0])
            .frameless()
            .input(24.0, 18.0, &items)
            // MeltingCategory's tank gauge (melting.png 132,0) over the fluid, as Java's tank overlay.
            .fluid([96.0, 4.0, 32.0, 32.0], SlotRole::Output, &m.fluid, boosted.max(1) as u32, 810)
            .fluid_bare(Some((BACKGROUND, [132.0, 0.0, 32.0, 32.0])))
            .text(tx, 3.0, &temp)
            // AbstractMeltingCategory: the arrow (150,41) filling over the melting time, its tooltip.
            .animated(BACKGROUND, [150.0, 41.0, 24.0, 17.0], 56.0, 18.0, (m.time.max(1) as u32) * 50)
            .hover([56.0, 18.0, 24.0, 17.0], &[&word("jei.tconstruct.melting.time").replacen("%s", &(m.time / 4).to_string(), 1)]);
        card.texts.last_mut().map(|t| t.2 = GRAY);
        if m.rate != Rate::Plain {
            // The ore boost's plus (132,32) and its tooltip.
            card = card.sprite(BACKGROUND, [132.0, 32.0, 8.0, 8.0], 85.0, 28.0)
                .hover([83.0, 26.0, 12.0, 12.0], &[&word("jei.tconstruct.melting.ore")]);
        }
        let usable = usable_fuels(&all, m.temperature);
        let low = m.temperature <= solid_temp;
        if let Some((f, a)) = usable.first() {
            card = card.fluid([4.0, 4.0, 12.0, if low { 15.0 } else { 32.0 }], SlotRole::Other, f, (*a).max(1) as u32, (*a).max(1) as u32)
                .fluid_bare(None);
        }
        if low {
            // The solid fuel's slot frame (164,0) under the short fuel tank.
            card = card.sprite_under(BACKGROUND, [164.0, 0.0, 18.0, 20.0], 1.0, 19.0);
        }
        if low && !example_items.is_empty() {
            card = card.other(2.0, 22.0, &example_items);
        }
        out.push((MELTING.to_owned(), card));
    }
    // MeltingFuelCategory: the fuels by temperature.
    let mut sorted: Vec<&recipes::Fuel> = all.fuels.iter().collect();
    sorted.sort_by_key(|f| f.temperature);
    let common = |card: jei::api::RecipeDef, title: String, f: &recipes::Fuel| -> jei::api::RecipeDef {
        let mut c = card
            .text(21.0, 0.0, &title)
            .text(21.0, 11.0, &word("jei.tconstruct.temperature").replacen("%s", &f.temperature.to_string(), 1))
            .text(21.0, 21.0, &word("jei.tconstruct.melting.multiplier").replacen("%s", &float(f.rate as f32 / 10.0), 1))
            .text(21.0, 31.0, &word("jei.tconstruct.fuel.duration.lasts"));
        let n = c.texts.len();
        c.texts[n - 4].2 = WHITE;
        for t in &mut c.texts[n - 3..] {
            t.2 = GRAY;
        }
        c
    };
    for (i, f) in sorted.iter().enumerate() {
        match &f.fluid {
            Some(_) => {
                let fluids = f.fluids();
                let Some(first) = fluids.first() else { continue };
                let name = words(first.split_once(':').map_or(first.as_str(), |(_, n)| n));
                let mut card = RecipeDef::new(&format!("{FUEL}/fluid/{i}"), [132.0, 40.0])
                    .background(BACKGROUND, [0.0, 110.0, 132.0, 40.0])
                    .fluid([4.0, 4.0, 12.0, 32.0], SlotRole::Input, first, f.amount.max(1) as u32, 100);
                card = common(card, name, f);
                let d = word("jei.tconstruct.fuel.duration.liquid").replacen("%s", &(f.duration / 5).to_string(), 1).replacen("%s", &f.amount.to_string(), 1);
                let w = d.chars().count() as f32 * 6.0;
                card = card.text(126.0 - w, 31.0, &d);
                card.texts.last_mut().map(|t| t.2 = GRAY);
                out.push((FUEL.to_owned(), card));
            }
            None => {
                for item in &examples {
                    let burn = item_key(item).and_then(|k| host.item_burn_time(k)).unwrap_or(0);
                    let mut card = RecipeDef::new(&format!("{FUEL}/solid/{}", item.replace(':', "/")), [132.0, 40.0])
                        .background(BACKGROUND, [0.0, 110.0, 132.0, 40.0])
                        .input(2.0, 12.0, &[(item.as_str(), 1)]);
                    card = common(card, word("jei.tconstruct.fuel.solid"), f);
                    let d = word("jei.tconstruct.fuel.duration.solid").replacen("%s", &(burn / 20).to_string(), 1);
                    let w = d.chars().count() as f32 * 6.0;
                    card = card.text(126.0 - w, 31.0, &d);
                    card.texts.last_mut().map(|t| t.2 = GRAY);
                    out.push((FUEL.to_owned(), card));
                }
            }
        }
    }
    let _ = (solid_rate, float(0.0));
    out
}
