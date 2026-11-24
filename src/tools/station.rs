//! The stations (spec-tools.md 3; upstream `tconstruct.rs` 8217-10911): the crafting station, the part builder, the
//! tinker station and the two tinkers' anvils, their menus with Java's slot positions, and their screens.
//!
//! - The crafting station's 3x3 grid is the fork's menu crafting grid (`crafting` in its menu type): it crafts as
//!   vanilla's crafting table, result slot and shift-clicks included.
//! - The part builder's and the tinker station's result slot is a take-only menu slot (`results`) that the server
//!   fills while a player has the menu open. The result is VIRTUAL: what taking it uses up (`Use`) is kept beside it
//!   (`PLACED`, and its key in the block entity's `hz_res`). Each settle (`settle`) looks first: a result slot that no
//!   longer holds what the server put there was taken, so the plan runs ONCE on the inputs (an item left over from a
//!   part taken is the player's own: `hz_real`, no result goes over it). It runs after every change of the station's
//!   container (`contents_changed`, so after each click), in its tick, before each shift-click and button. Hoppers
//!   reach no slot (no faces). Breaking the station or blowing it up takes the virtual result out first.
//! - The part builder's buttons and selection reach its screen in data slots (`PB_BUTTONS`).

use super::data::*;
use super::tool::*;
use crate::smeltery::util::*;
use crate::smeltery::Fields;
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub(crate) const CRAFTING_STATION: &str = "tconstruct:crafting_station";
pub(crate) const PART_BUILDER: &str = "tconstruct:part_builder";
pub(crate) const TINKER_STATION: &str = "tconstruct:tinker_station";
pub(crate) const TINKERS_ANVIL: &str = "tconstruct:tinkers_anvil";
pub(crate) const SCORCHED_ANVIL: &str = "tconstruct:scorched_anvil";
pub(crate) const MODIFIER_WORKTABLE: &str = "tconstruct:modifier_worktable";
pub(crate) const STATIONS: [&str; 6] = [CRAFTING_STATION, PART_BUILDER, TINKER_STATION, TINKERS_ANVIL, SCORCHED_ANVIL, MODIFIER_WORKTABLE];

/// PartBuilderContainerMenu's slots: material, pattern, output, then 54 for the side inventory.
const PB_MATERIAL: usize = 0;
const PB_PATTERN: usize = 1;
const PB_OUTPUT: usize = 2;
/// The buttons sent to the part builder's screen (tools/make_fork_content.py PB_BUTTONS).
pub(crate) const PB_BUTTONS: usize = 40;

pub(crate) fn is_station(block: &str) -> bool {
    STATIONS.contains(&block)
}

pub(crate) fn tinker_inputs(block: &str) -> usize {
    if block == TINKER_STATION { 3 } else { 5 }
}

fn result_slot(block: &str) -> Option<usize> {
    match block {
        CRAFTING_STATION => None,
        PART_BUILDER => Some(PB_OUTPUT),
        MODIFIER_WORKTABLE => Some(super::modrecipe::WT_RESULT),
        _ => Some(tinker_inputs(block) + 1),
    }
}

fn station_slots(block: &str) -> usize {
    match block {
        CRAFTING_STATION => 64,
        PART_BUILDER => 57,
        MODIFIER_WORKTABLE => super::modrecipe::WT_RESULT + 1,
        _ => tinker_inputs(block) + 2,
    }
}

fn menu_of(block: &str) -> &'static str {
    match block {
        CRAFTING_STATION => CRAFTING_STATION,
        PART_BUILDER => PART_BUILDER,
        TINKER_STATION => TINKER_STATION,
        MODIFIER_WORKTABLE => MODIFIER_WORKTABLE,
        _ => TINKERS_ANVIL,
    }
}

pub(crate) fn container(block: &str) -> Option<ModContainer> {
    if !is_station(block) {
        return None;
    }
    // No faces: hoppers reach no slot, as Java's ConfigurableInvWrapperCapability(this, false, false) on every table
    // (CraftingStation/PartBuilder/TinkerStation/ModifierWorktableBlockEntity) refuses inserts and extracts. Every slot
    // is an input and an output: on main `Container.canPlaceItem`/`canTakeItem` read these lists.
    let n = station_slots(block);
    Some(ModContainer { slots: n, faces: std::array::from_fn(|_| Vec::new()), inputs: (0..n).collect(), outputs: (0..n).collect(), insert_rules: Vec::new(), menu: Some(menu_of(block).to_owned()) })
}

// ---- What a result uses

#[derive(Clone, Debug)]
pub(crate) enum Use {
    /// `amount` of the slot's item go (PartBuilderBlockEntity.shrinkSlot, ITinkerableContainer.shrinkInput).
    Shrink { slot: usize, item: ItemStack, amount: u32 },
    /// A leftover for the player (a material recipe's).
    Give(ItemStack),
}

#[derive(Clone)]
pub(crate) struct Placed {
    key: String,
    /// The result as the server put it in the slot (what a refused charge takes back from the player).
    stack: ItemStack,
    uses: Vec<Use>,
}

/// A taken result whose plan no longer fits the inputs (two clicks between settles): the caller takes the result back
/// from the player or charges what is missing from them (`reclaim`), never letting it go uncharged.
pub(crate) struct Reclaim {
    stack: ItemStack,
    uses: Vec<Use>,
}

/// What a settle did: the leftovers for the player, the menu's data, a taken result that couldn't be charged.
pub(crate) struct Settled {
    gives: Vec<ItemStack>,
    data: Option<Vec<i32>>,
    reclaim: Option<Reclaim>,
}

pub(crate) type Key = (String, Pos);
static PLACED: Mutex<Option<HashMap<Key, Placed>>> = Mutex::new(None);
/// Who has each station open (`player_container_open` / `_close`).
static VIEWERS: Mutex<Option<HashMap<Key, Vec<u128>>>> = Mutex::new(None);

fn placed<R>(f: impl FnOnce(&mut HashMap<Key, Placed>) -> R) -> R {
    f(PLACED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new))
}

/// How many have each station open (`mod_container_viewers`), by position.
static COUNTS: Mutex<Option<HashMap<Pos, u32>>> = Mutex::new(None);

pub(crate) fn viewer_count(pos: Pos, n: u32) {
    let mut g = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if n == 0 { m.remove(&pos); } else { m.insert(pos, n); }
}

fn viewed(k: &Key) -> bool {
    !viewers(k).is_empty() || COUNTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.contains_key(&k.1))
}

/// The viewers of a station by its position (the open/close events and the level hosts name dimensions differently).
fn viewers(k: &Key) -> Vec<u128> {
    VIEWERS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&(String::new(), k.1)).cloned()).unwrap_or_default()
}

pub(crate) fn container_opened(_dim: &str, player: u128, pos: Option<Pos>, open: bool) {
    let Some(pos) = pos else { return };
    let dim = "";
    let mut g = VIEWERS.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    let k = (dim.to_owned(), pos);
    let list = m.entry(k.clone()).or_default();
    list.retain(|p| *p != player);
    if open {
        list.push(player);
    }
    if list.is_empty() {
        m.remove(&k);
    }
}

/// A stack's identity for the placed-result check: item, count, effective damage and components. Not its `Debug`
/// form: a repaired tool's damage of 0 comes back from the slot as no damage at all, the same stack.
fn stack_key(s: &Option<ItemStack>) -> String {
    s.as_ref()
        .map(|s| format!("{} x{} d{} {:?}", sid(s), s.count(), s.damage(), s.components().map(|c| c.as_nbt())))
        .unwrap_or_default()
}

fn same(a: &ItemStack, b: &ItemStack) -> bool {
    a.same_item(b) && a.same_components(b)
}

/// Whether the slots still hold everything the plan takes (each slot's shrinks summed): checked in the same step as
/// the charge, so a plan is carried out whole or not at all.
fn uses_fit(items: &[Option<ItemStack>], uses: &[Use]) -> bool {
    let mut need: HashMap<usize, (ItemStack, u32)> = HashMap::new();
    for u in uses {
        if let Use::Shrink { slot, item, amount } = u {
            let e = need.entry(*slot).or_insert_with(|| (item.clone(), 0));
            if !same(&e.0, item) {
                return false;
            }
            e.1 += amount;
        }
    }
    need.iter().all(|(slot, (item, n))| items.get(*slot).cloned().flatten().is_some_and(|o| same(&o, item) && o.count() >= *n))
}

/// The plan carried out on the slots, once, after `uses_fit` said it fits; the leftovers for the player.
fn apply_uses(items: &mut [Option<ItemStack>], uses: &[Use]) -> Vec<ItemStack> {
    let mut gives = Vec::new();
    for u in uses {
        match u {
            Use::Shrink { slot, amount, .. } => {
                if let Some(stack) = items.get(*slot).cloned().flatten() {
                    items[*slot] = with_count(&stack, stack.count().saturating_sub(*amount));
                }
            }
            Use::Give(s) => gives.push(s.clone()),
        }
    }
    gives
}

/// Takes up to `n` of `want` from the player: the cursor of their open menu first (where the game lends it), then
/// their inventory. With `all`, nothing is taken unless all `n` are there. Returns how many were taken.
fn take_from_player(host: &mut dyn ServerHost, player: u128, want: &ItemStack, n: u32, all: bool) -> u32 {
    let res = ItemResource::of(want);
    let cursor = host.player(player).and_then(|p| p.container_menu(&*host)).map(MenuRef::cursor);
    let r = host.transaction(|host, tx| {
        let mut got = 0;
        for place in cursor.iter().cloned().chain((0..36).map(|slot| ItemPlace::PlayerSlot { player, slot })) {
            if got >= n {
                break;
            }
            got += place.extract(host, tx, &res, n - got).unwrap_or(0);
        }
        if all && got < n { Err(()) } else { Ok(got) }
    });
    r.unwrap_or(0)
}

/// A taken result whose plan no longer fits (the inputs changed between the take and its settle): the result is taken
/// back from the player, the inputs left as they are; where it can't be (already used or dropped), every input still
/// in the station is charged and the rest of the plan is taken from the player's inventory. Nothing is gained for
/// free. Returns the plan's leftovers when it was charged.
fn reclaim(host: &mut dyn ServerHost, k: &Key, items: &mut [Option<ItemStack>], rc: Reclaim) -> Vec<ItemStack> {
    let players = viewers(k);
    for p in &players {
        if take_from_player(host, *p, &rc.stack, rc.stack.count(), true) == rc.stack.count() {
            crate::smeltery::util::tlog(&format!("a station's result at {:?} was taken back: its inputs changed before the charge", k.1));
            return Vec::new();
        }
    }
    let mut gives = Vec::new();
    for u in &rc.uses {
        match u {
            Use::Shrink { slot, item, amount } => {
                let here = items.get(*slot).cloned().flatten().filter(|o| same(o, item)).map_or(0, |o| o.count().min(*amount));
                if let Some(o) = items.get(*slot).cloned().flatten().filter(|_| here > 0) {
                    items[*slot] = with_count(&o, o.count() - here);
                }
                let mut missing = amount - here;
                for p in &players {
                    if missing == 0 {
                        break;
                    }
                    missing -= take_from_player(host, *p, item, missing, false);
                }
                if missing > 0 {
                    crate::smeltery::util::tlog(&format!("a station at {:?} could not charge {missing} of an input", k.1));
                }
            }
            Use::Give(s) => gives.push(s.clone()),
        }
    }
    gives
}

// ---- The part builder (PartBuilderBlockEntity)

fn part_compute(host: &dyn Host, items: &[Option<ItemStack>], nbt: &Compound) -> (Option<ItemStack>, Vec<Use>, Vec<i32>) {
    let material = items.get(PB_MATERIAL).cloned().flatten();
    let pattern = items.get(PB_PATTERN).cloned().flatten();
    let recipes = material_recipes(host);
    let mats = materials(host);
    let buttons = part_builder_buttons(host, pattern.as_ref(), material.as_ref());
    let mut data = vec![-1, buttons.len().min(PB_BUTTONS) as i32];
    data.extend(buttons.iter().take(PB_BUTTONS).map(|&i| i as i32));
    data.resize(2 + PB_BUTTONS, -1);
    let sel = gs(nbt, "hz_sel").unwrap_or_default();
    let Some(index) = buttons.iter().position(|&i| recipes.part_builder[i].pattern == sel) else { return (None, Vec::new(), data) };
    data[0] = index as i32;
    let recipe = &recipes.part_builder[buttons[index]];
    let value = material.as_ref().and_then(|m| material_value(host, m));
    if !recipe.matches(host, &mats, material.as_ref(), value.as_ref()) {
        return (None, Vec::new(), data);
    }
    let result = recipe.assemble(host, &mats, value.as_ref());
    let mut uses = Vec::new();
    if let Some(l) = value.as_ref().and_then(|v| v.leftover(recipe.cost)) {
        uses.push(Use::Give(l));
    }
    let used = recipe.items_used(value.as_ref());
    if let Some(m) = material.as_ref().filter(|_| used > 0) {
        uses.push(Use::Shrink { slot: PB_MATERIAL, item: m.clone(), amount: used as u32 });
    }
    if let Some(p) = pattern.as_ref().filter(|p| !recipes.patterns_reusable.contains(sid(p))) {
        uses.push(Use::Shrink { slot: PB_PATTERN, item: p.clone(), amount: 1 });
    }
    (result, uses, data)
}

// ---- The tinker station and the anvils (TinkerStationBlockEntity): tool building and repair

/// ToolBuildingRecipe.matches: part i in input i, then each extra requirement, the rest empty; one ingredient and no
/// parts: exactly one item anywhere.
fn build_matches(r: &ToolBuild, parts: &[String], inputs: &[Option<ItemStack>]) -> bool {
    let required = parts.len() + r.extra.len();
    if required == 0 || required > inputs.len() {
        return false;
    }
    if required == 1 && parts.is_empty() {
        let mut found = false;
        for s in inputs.iter().flatten() {
            if found || !r.extra[0].contains(sid(s)) {
                return false;
            }
            found = true;
        }
        return found;
    }
    inputs.iter().enumerate().all(|(i, input)| match (parts.get(i), i.checked_sub(parts.len()).and_then(|k| r.extra.get(k))) {
        (Some(part), _) => input.as_ref().is_some_and(|s| sid(s) == part),
        (None, Some(extra)) => input.as_ref().is_some_and(|s| extra.contains(sid(s))),
        _ => input.is_none(),
    })
}

fn tool_building(host: &dyn Host, inputs: &[Option<ItemStack>]) -> (Option<ItemStack>, Vec<Use>) {
    let t = tooling(host);
    let mats = materials(host);
    for r in &t.building {
        let def = t.defs.get(&r.result);
        let parts: &[String] = match &r.parts {
            Some(p) => p,
            None => def.map_or(&[][..], |d| d.parts.as_slice()),
        };
        if !build_matches(r, parts, inputs) {
            continue;
        }
        let stat_count = def.map_or(0, |d| d.stat_types.len());
        let mut list: Vec<String> = (0..parts.len().min(stat_count))
            .map(|i| inputs[i].as_ref().and_then(|s| part_material(&mats, s)).map_or_else(|| UNKNOWN_MATERIAL.to_owned(), |m| material_string(&m)))
            .collect();
        list.extend(r.materials.iter().take(stat_count.saturating_sub(parts.len())).cloned());
        let Some(mut tool) = new_stack(&r.result, 1) else { continue };
        if def.is_some() && !write_tool(host, &mut tool, &list) {
            continue;
        }
        let count = r.count.min(max_stack(host, &tool)).max(1);
        let Some(tool) = with_count(&tool, count) else { continue };
        let uses = inputs.iter().enumerate().filter_map(|(i, s)| Some(Use::Shrink { slot: i + 1, item: s.clone()?, amount: 1 })).collect();
        return (Some(tool), uses);
    }
    (None, Vec::new())
}

/// TinkerStationRepairRecipe: a damaged tool and inputs of one material it repairs with (a repair kit its material,
/// else the item's material value); each item mends `repairAmount × value / 3 / needed` (a kit × 2 / 3), as many
/// items as the damage needs.
fn tool_repair(host: &dyn Host, tool: &ItemStack, inputs: &[Option<ItemStack>]) -> Option<(ItemStack, Vec<Use>)> {
    let tools = tooling(host);
    if !tools.durability.contains(sid(tool)) || !tool_initialized(tool) || tool.damage() == 0 {
        if inputs.iter().any(Option::is_some) {
            crate::smeltery::util::tlog(&format!(
                "repair of {}: none (durability {}, built {}, damage {})",
                sid(tool),
                tools.durability.contains(sid(tool)),
                tool_initialized(tool),
                tool.damage()
            ));
        }
        return None;
    }
    let mats = materials(host);
    let recipes = material_recipes(host);
    let mut material: Option<String> = None;
    let mut per: Vec<(usize, f32)> = Vec::new();
    for (i, s) in inputs.iter().enumerate() {
        let Some(s) = s else { continue };
        let (m, factor) = if sid(s) == "tconstruct:repair_kit" {
            (part_material(&mats, s)?.0, 2.0 / 3.0)
        } else {
            let Some(v) = recipes.values.iter().find(|r| mats.get(&r.material.0).is_some() && r.items.contains(sid(s))) else {
                crate::smeltery::util::tlog(&format!("repair of {}: {} is no material", sid(tool), sid(s)));
                return None;
            };
            (mats.resolve(&v.material.0).to_owned(), v.value as f32 / 3.0 / v.needed as f32)
        };
        match &material {
            Some(prev) if *prev != m => return None,
            _ => material = Some(m),
        }
        per.push((i, factor));
    }
    // RepairFactorModifierHook (the modifiers' `repair` modules) scales every item's repair.
    let Some(material) = material else {
        crate::smeltery::util::tlog(&format!("repair of {}: no repair material among the inputs", sid(tool)));
        return None;
    };
    let amount = repair_amount(host, tool, &material) * super::modifier::repair_factor(host, tool);
    crate::smeltery::util::tlog(&format!("repair of {} with {material}: {amount} a unit", sid(tool)));
    if amount <= 0.0 {
        return None;
    }
    let mut remaining = tool.damage() as i32;
    let mut uses = vec![Use::Shrink { slot: 0, item: tool.clone(), amount: 1 }];
    for (i, factor) in per {
        if remaining <= 0 {
            break;
        }
        let per_item = amount * factor;
        let Some(s) = inputs[i].as_ref().filter(|_| per_item > 0.0) else { continue };
        let applied = s.count().min((remaining as f32 / per_item).ceil() as u32);
        remaining -= (applied as f32 * per_item) as i32;
        uses.push(Use::Shrink { slot: i + 1, item: s.clone(), amount: applied });
    }
    let mut out = with_count(tool, 1)?;
    set_tool_damage(&mut out, remaining.max(0));
    Some((out, uses))
}

fn tinker_compute(host: &dyn Host, block: &str, items: &[Option<ItemStack>]) -> (Option<ItemStack>, Vec<Use>, Vec<i32>) {
    let n = tinker_inputs(block);
    let tool = items.first().cloned().flatten();
    let inputs: Vec<Option<ItemStack>> = (1..=n).map(|i| items.get(i).cloned().flatten()).collect();
    // Tool building without a tool; with one, repair, then the modifier recipes (the first that matches; its error
    // reaches the screen in the data slots).
    let mut data = vec![0; 4];
    let (result, uses) = match &tool {
        None => tool_building(host, &inputs),
        Some(t) => match tool_repair(host, t, &inputs) {
            Some((r, u)) => (Some(r), u),
            None => match super::modrecipe::modifier_station(host, t, &inputs) {
                Some(super::modrecipe::Outcome::Done(r, u)) => (Some(r), u),
                Some(super::modrecipe::Outcome::Fail(e)) => {
                    data = super::modrecipe::station_error(host, &e);
                    (None, Vec::new())
                }
                _ => (None, Vec::new()),
            },
        },
    };
    (result, uses, data)
}

// ---- Settling

/// The check of one station: a result found taken is crafted once, its whole plan checked and charged in this same
/// step (its leftovers returned); a plan that no longer fits comes back as a `Reclaim`, and nothing else happens until
/// the caller settled it (`settle_all`). A viewed station's slot gets the result for its inputs, an unviewed one's is
/// emptied.
fn settle(host: &dyn Host, k: &Key, block: &str, items: &mut Vec<Option<ItemStack>>, nbt: &mut Compound, viewed: bool) -> Settled {
    let Some(r) = result_slot(block) else { return Settled { gives: Vec::new(), data: None, reclaim: None } };
    if items.len() < station_slots(block) {
        items.resize(station_slots(block), None);
    }
    let mut gives = Vec::new();
    let placed_key = gs(nbt, "hz_res").unwrap_or_default();
    if !placed_key.is_empty() && stack_key(&items[r]) != placed_key {
        if items[r].is_some() {
            crate::smeltery::util::tlog(&format!("station result key changed:
  placed {placed_key}
  now    {}", stack_key(&items[r])));
        }
        let plan = placed(|m| m.remove(k)).filter(|p| p.key == placed_key);
        ss(nbt, "hz_res", "");
        si(nbt, "hz_real", i32::from(items[r].is_some()));
        match plan {
            Some(p) if uses_fit(items, &p.uses) => gives = apply_uses(items, &p.uses),
            Some(p) => return Settled { gives, data: None, reclaim: Some(Reclaim { stack: p.stack, uses: p.uses }) },
            None => crate::smeltery::util::tlog(&format!("a station's result at {:?} was taken without its plan", k.1)),
        }
    }
    if items[r].is_none() {
        si(nbt, "hz_real", 0);
    } else if gs(nbt, "hz_res").unwrap_or_default().is_empty() {
        si(nbt, "hz_real", 1);
    }
    if gi(nbt, "hz_real", 0) != 0 {
        trace(k, &format!("{block} at {:?}: a real item in the result slot ({:?}), not computed", k.1, items[r].as_ref().map(|s| sid(s).to_owned())));
        return Settled { gives, data: None, reclaim: None };
    }
    let (want, uses, data) = match (viewed, block) {
        (false, _) => (None, Vec::new(), None),
        (true, PART_BUILDER) => {
            let (w, u, d) = part_compute(host, items, nbt);
            (w, u, Some(d))
        }
        (true, MODIFIER_WORKTABLE) => {
            let (w, u, d) = super::modrecipe::worktable_compute(host, items, nbt);
            (w, u, Some(d))
        }
        (true, _) => {
            let (w, u, d) = tinker_compute(host, block, items);
            (w, u, Some(d))
        }
    };
    let want_key = stack_key(&want);
    if viewed {
        let ids: Vec<String> = items.iter().map(|s| s.as_ref().map_or("-".to_owned(), |s| format!("{}x{}", sid(s), s.count()))).collect();
        trace(k, &format!("{block} at {:?}: inputs {ids:?}, result {:?}, sel {:?}", k.1, want.as_ref().map(|s| sid(s).to_owned()), gs(nbt, "hz_sel")));
    }
    if want_key != gs(nbt, "hz_res").unwrap_or_default() {
        items[r] = want.clone();
        ss(nbt, "hz_res", &want_key);
    }
    placed(|m| match want {
        Some(stack) if !want_key.is_empty() => {
            m.insert(k.clone(), Placed { key: want_key, stack, uses });
        }
        _ => {
            m.remove(k);
        }
    });
    Settled { gives, data, reclaim: None }
}

/// `settle`, with a refused charge settled (`reclaim`) and the station settled again on what is left.
fn settle_all(host: &mut dyn ServerHost, k: &Key, block: &str, items: &mut Vec<Option<ItemStack>>, nbt: &mut Compound, viewed: bool) -> (Vec<ItemStack>, Option<Vec<i32>>) {
    let s = settle(&*host, k, block, items, nbt, viewed);
    let Some(rc) = s.reclaim else { return (s.gives, s.data) };
    let mut gives = s.gives;
    gives.extend(reclaim(host, k, items, rc));
    let again = settle(&*host, k, block, items, nbt, viewed);
    gives.extend(again.gives);
    (gives, again.data)
}

/// Leftovers to the station's first viewer, else on top of it.
fn give(host: &mut dyn ServerHost, k: &Key, gives: Vec<ItemStack>) {
    for g in gives {
        match viewers(k).first() {
            Some(p) => crate::smeltery::containers::give_player(host, *p, g),
            None => {
                if let Some(level) = here(&*host) {
                    let _ = level.spawn_item(host, f64::from(k.1[0]) + 0.5, f64::from(k.1[1]) + 1.0, f64::from(k.1[2]) + 0.5, g, 10);
                }
            }
        }
    }
}

/// A settle on the block entity taken out of the world (its tick, a button, a shift-click).
pub(crate) fn settle_fields(host: &mut dyn ServerHost, pos: Pos, e: &mut Fields) {
    use crate::perf;
    let t0 = perf::t(perf::ST_PREP);
    ensure_ids(&*host);
    let Some(level) = here(&*host) else { return };
    let block = block_id(&*host, level, pos);
    if !is_station(&block) {
        return;
    }
    if e.items.len() < station_slots(&block) {
        e.items.resize(station_slots(&block), None);
    }
    let k = (host.dimension().to_owned(), pos);
    drop(t0);
    let t1 = perf::t(perf::ST_REMEMBER);
    remember_slot_items(&*host);
    drop(t1);
    let t2 = perf::t(perf::ST_SETTLE);
    let (mut items, mut nbt) = (e.items.clone(), e.nbt.clone());
    let (gives, data) = settle_all(host, &k, &block, &mut items, &mut nbt, viewed(&k));
    drop(t2);
    let _t3 = perf::t(perf::ST_WRITE);
    if items != e.items {
        e.items = items;
    }
    if nbt != e.nbt {
        e.nbt = nbt;
    }
    if let Some(d) = data.filter(|d| *d != e.data) {
        e.data = d;
    }
    give(host, &k, gives);
}

/// A settle on the block entity in the world, for `contents_changed` after a click changed its slots. Unused while
/// slots written during a click don't reach the clicking player's open menu (sunrise-org/sunrise#4240): the station
/// settles in its tick instead, one tick later at most.
#[allow(dead_code)]
pub(crate) fn settle_ref(host: &mut dyn ServerHost, level: LevelRef, pos: Pos) {
    ensure_ids(&*host);
    let block = block_id(&*host, level, pos);
    if !is_station(&block) || result_slot(&block).is_none() {
        return;
    }
    let Some(e) = host.block_entity(level, pos) else { return };
    let n = station_slots(&block);
    let before: Vec<Option<ItemStack>> = (0..n).map(|i| e.item_slot(&*host, i as u32).ok().flatten()).collect();
    let nbt0 = e.data(&*host).unwrap_or_default();
    let (mut items, mut nbt) = (before.clone(), nbt0.clone());
    let k = (host.dimension().to_owned(), pos);
    let (gives, data) = settle_all(host, &k, &block, &mut items, &mut nbt, viewed(&k));
    let changed: Vec<usize> = (0..n).filter(|&i| items[i] != before[i]).collect();
    if !changed.is_empty() {
        let ok = host.transaction(|host, tx| {
            for &i in &changed {
                if !e.set_item_slot(host, tx, i as u32, items[i].clone()).unwrap_or(false) {
                    return Err(());
                }
            }
            Ok(())
        });
        if ok.is_err() {
            crate::smeltery::util::tlog(&format!("a station's slots at {pos:?} could not be written"));
            return;
        }
        // BlockEntity.setChanged after the charge, so the open menus send the slots again.
        let _ = e.set_changed(host);
    }
    if nbt != nbt0 {
        let mut m = Compound::new();
        for key in ["hz_res", "hz_real", "hz_sel"] {
            if let Some(t) = nbt.get(&js(key)) {
                m.insert(js(key), t.clone());
            }
        }
        let _ = e.merge_data(host, m);
    }
    if let Some(d) = data {
        let _ = e.set_data_slots(host, d);
    }
    give(host, &k, gives);
}

/// Before the station goes (broken, exploded): the virtual result out, so only the real items drop.
pub(crate) fn clear_result(host: &mut dyn ServerHost, level: LevelRef, pos: Pos) {
    let block = block_id(&*host, level, pos);
    let Some(r) = result_slot(&block) else { return };
    let Some(e) = host.block_entity(level, pos) else { return };
    if e.data(&*host).is_some_and(|d| gi(&d, "hz_real", 0) != 0) {
        return;
    }
    let _ = host.transaction(|host, tx| if e.set_item_slot(host, tx, r as u32, None).unwrap_or(false) { Ok(()) } else { Err(()) });
    let mut m = Compound::new();
    ss(&mut m, "hz_res", "");
    let _ = e.merge_data(host, m);
    placed(|m| m.remove(&(host.dimension().to_owned(), pos)));
}

// ---- Buttons and shift-clicks

/// The part builder's button `id`: its pattern selected (PartBuilderContainerMenu.clickMenuButton).
pub(crate) fn menu_button(host: &mut dyn ServerHost, pos: Pos, id: i32, e: &mut Fields) {
    let Some(level) = here(&*host) else { return };
    let block = block_id(&*host, level, pos);
    if block == MODIFIER_WORKTABLE {
        settle_fields(host, pos, e);
        let data = e.data.clone();
        crate::smeltery::util::tlog(&format!("worktable button {id} at {pos:?}: data {:?}", data.iter().take(8).collect::<Vec<_>>()));
        super::modrecipe::worktable_button(id, &data, &mut e.nbt);
        settle_fields(host, pos, e);
        return;
    }
    if block != PART_BUILDER {
        return;
    }
    settle_fields(host, pos, e);
    let material = e.items.get(PB_MATERIAL).cloned().flatten();
    let pattern = e.items.get(PB_PATTERN).cloned().flatten();
    let buttons = part_builder_buttons(&*host, pattern.as_ref(), material.as_ref());
    if let Some(&i) = usize::try_from(id).ok().and_then(|i| buttons.get(i)) {
        let p = material_recipes(&*host).part_builder[i].pattern.clone();
        ss(&mut e.nbt, "hz_sel", &p);
        settle_fields(host, pos, e);
    }
}

/// `menu_quick_move`: the station settles first (a shift-click on the result takes it, and its plan runs at the next
/// settle). The game's routing then moves the stack as ChestMenu's does, into the first slot whose rules
/// (`slot_rules`) take it: a pattern into the pattern slot, anything else into the material slot; a tool into the
/// tool slot, parts into the inputs. (A mod can't take a shift-click that moves the whole stack: an API stack can't be
/// empty, so what is left can't be "nothing".)
pub(crate) fn quick_move(host: &mut dyn ServerHost, pos: Pos, container_slots: usize, slot: usize, e: &mut Fields, stack: &mut Option<ItemStack>) -> Flow {
    let Some(level) = here(&*host) else { return Flow::Continue };
    let block = block_id(&*host, level, pos);
    if is_station(&block) {
        // A settle that charges here (a result taken by an earlier click, its plan run now) must keep its writes: an
        // unhandled quick move puts `entity.items` back and moves the slot's old stack. So a click whose settle
        // changed any slot but the result is taken (handled) with the charge written and nothing moved; the player's
        // next shift-click moves the stack as it is then. (A new virtual result alone is put again at the next settle.)
        let before = e.items.clone();
        settle_fields(host, pos, e);
        let r = result_slot(&block);
        let charged = e.items.iter().zip(before.iter()).enumerate().any(|(i, (a, b))| Some(i) != r && a != b);
        if charged {
            // A handled move gives its source what `stack` holds: the clicked station slot keeps what the charge left
            // it (`stack` is the copy from before the settle; handing that back would undo the charge's shrink, a
            // duplication). A slot of the player's inventory keeps its stack whole: nothing moved.
            if slot < container_slots && slot < e.items.len() && before.get(slot) != e.items.get(slot) {
                *stack = e.items[slot].clone();
            }
            return Flow::Cancel;
        }
    }
    Flow::Continue
}

/// The pattern slot's items (PatternSlot.mayPlace: the patterns any recipe takes) and the tool slot's
/// (`#tconstruct:modifiable`), for `slot_rules`: made again when the recipes or the tool data change.
static SLOT_ITEMS: Mutex<Option<(HashSet<String>, HashSet<String>)>> = Mutex::new(None);

fn remember_slot_items(host: &dyn Host) {
    static SEEN: Mutex<(usize, usize)> = Mutex::new((0, 0));
    let recipes = material_recipes(host);
    let tools = tooling(host);
    let key = (std::sync::Arc::as_ptr(&recipes) as usize, std::sync::Arc::as_ptr(&tools) as usize);
    {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if *seen == key {
            return;
        }
        *seen = key;
    }
    let patterns: HashSet<String> = recipes.part_builder.iter().flat_map(|r| r.pattern_items.iter().cloned()).collect();
    let modifiable = tools.modifiable.clone();
    *SLOT_ITEMS.lock().unwrap_or_else(|e| e.into_inner()) = Some((patterns, modifiable));
}

/// PatternSlot and TinkerStationSlot.mayPlace: the pattern slot takes patterns, the material slot no pattern, the
/// tool slot a modifiable item (the block's own layout's filter). The results take nothing (the menu's `results`).
pub(crate) fn slot_rules(host: &dyn Host, event: &mut MenuSlotRulesEvent<'_>) {
    // The item ids (`sid`) from the first click on: a world's first click may come before any station settled.
    ensure_ids(host);
    // The slots the station's screen hides (`screen`, for the block at the menu): none on the server's menu.
    if matches!(event.container, MenuContainer::Main) {
        let block = event.context.pos.and_then(|p| here(host).map(|l| block_id(host, l, p)));
        let layout = block.and_then(|b| screen(host, &b));
        crate::refuse_hidden_slot(event, layout.as_ref());
    }
    if !matches!(event.menu, PART_BUILDER | TINKER_STATION | TINKERS_ANVIL) {
        return;
    }
    // The rules get a host now: the lists are made here too, so they hold before the station's first settle.
    remember_slot_items(host);
    let id = sid(event.stack);
    let g = SLOT_ITEMS.lock().unwrap_or_else(|e| e.into_inner());
    let Some((patterns, modifiable)) = g.as_ref() else { return };
    match (event.menu, event.slot) {
        (PART_BUILDER, PB_PATTERN) => event.may_place &= patterns.contains(id),
        (PART_BUILDER, PB_MATERIAL) => event.may_place &= !patterns.contains(id),
        (TINKER_STATION | TINKERS_ANVIL, 0) => {
            event.may_place &= modifiable.contains(id);
            event.max_stack = event.max_stack.min(1);
        }
        _ => {}
    }
}

// ---- Screens

const PART_TEXTURE: &str = "tconstruct:gui/part_builder";
const TINKER_TEXTURE: &str = "tconstruct:gui/tinker";
const ICONS_TEXTURE: &str = "tconstruct:gui/icons";
const CRAFTING_TEXTURE: &str = "minecraft:gui/container/crafting_table";
const LABEL: u32 = 0xFF40_4040;

/// The tinker station's slots from its own layout (StationSlotLayout of the block: the repair layout).
fn tinker_places(host: &dyn Host, block: &str) -> Vec<Option<[f32; 2]>> {
    let n = tinker_inputs(block);
    let t = tooling(host);
    let layout = t.layouts.get(block);
    let mut out = vec![None; n + 2];
    for (i, s) in out.iter_mut().enumerate().take(n + 1) {
        let ls = layout.and_then(|l| if i == 0 { l.tool.as_ref() } else { l.inputs.get(i - 1) }).filter(|s| s.x >= 0 && s.y >= 0);
        *s = Some(ls.map_or([112.0 - 18.0 * i as f32, 62.0], |s| [s.x as f32, s.y as f32]));
    }
    out[n + 1] = Some([114.0, 38.0]);
    out
}

pub(crate) fn screen(host: &dyn Host, block: &str) -> Option<MenuScreenLayout> {
    match block {
        CRAFTING_STATION => {
            let mut slots = vec![None; 64];
            for (i, s) in slots.iter_mut().enumerate().take(9) {
                *s = Some([30.0 + (i % 3) as f32 * 18.0, 17.0 + (i / 3) as f32 * 18.0]);
            }
            slots[9] = Some([124.0, 35.0]);
            Some(MenuScreenLayout { width: 176.0, height: 166.0, slots: crate::screen_slots(slots), inventory_y: 84.0, inventory_x: 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() })
        }
        PART_BUILDER => {
            let mut slots = vec![None; 57];
            slots[PB_MATERIAL] = Some([29.0, 43.0]);
            slots[PB_PATTERN] = Some([8.0, 43.0]);
            slots[PB_OUTPUT] = Some([148.0, 42.0]);
            Some(MenuScreenLayout { width: 176.0, height: 184.0, slots: crate::screen_slots(slots), inventory_y: 102.0, inventory_x: 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() })
        }
        TINKER_STATION | TINKERS_ANVIL | SCORCHED_ANVIL => {
            Some(MenuScreenLayout { width: 176.0, height: 184.0, slots: crate::screen_slots(tinker_places(host, block)), inventory_y: 102.0, inventory_x: 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() })
        }
        MODIFIER_WORKTABLE => Some(super::modrecipe::screen()),
        _ => None,
    }
}

fn sprite(draw: &mut dyn DrawContext, texture: &str, o: [f32; 2], at: [f32; 2], uv: [f32; 4]) {
    draw.sprite(texture, uv, [o[0] + at[0], o[1] + at[1], uv[2], uv[3]], None);
}

fn button_at(i: usize) -> [f32; 2] {
    [51.0 + (i % 4) as f32 * 18.0, 15.0 + (i / 4) as f32 * 18.0]
}

fn hit(at: [f32; 2], p: [f32; 2], w: f32) -> bool {
    at[0] >= p[0] && at[1] >= p[1] && at[0] < p[0] + w && at[1] < p[1] + w
}

/// The part builder's buttons from the data slots: (recipe index) of each, at most 16 shown (4x4).
fn part_buttons_of(data: &[i32]) -> Vec<usize> {
    let n = data_at(data, 1).clamp(0, PB_BUTTONS as i32) as usize;
    (0..n.min(16)).filter_map(|i| usize::try_from(data_at(data, 2 + i)).ok()).collect()
}

fn data_at(data: &[i32], i: usize) -> i32 {
    data.get(i).map_or(-1, |v| i32::from(*v as i16))
}

pub(crate) fn draw(host: &dyn ClientHost, block: &str, data: &[i32], draw: &mut dyn DrawContext, o: [f32; 2]) {
    match block {
        CRAFTING_STATION => {
            sprite(draw, CRAFTING_TEXTURE, o, [0.0, 0.0], [0.0, 0.0, 176.0, 166.0]);
            draw.text(&lang(host, "block.tconstruct.crafting_station", "Crafting Station"), [o[0] + 28.0, o[1] + 6.0], LABEL, false);
            draw.text(&lang(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 72.0], LABEL, false);
        }
        PART_BUILDER => {
            sprite(draw, PART_TEXTURE, o, [0.0, 0.0], [0.0, 0.0, 176.0, 184.0]);
            sprite(draw, PART_TEXTURE, o, [126.0, 15.0], [188.0, 0.0, 12.0, 15.0]);
            let selected = data_at(data, 0);
            let buttons = part_buttons_of(data);
            let recipes = material_recipes(host);
            for (i, &r) in buttons.iter().enumerate() {
                let p = button_at(i);
                let v = if i as i32 == selected { 33.0 } else { 15.0 };
                sprite(draw, PART_TEXTURE, o, p, [176.0, v, 18.0, 18.0]);
                if let Some(rec) = recipes.part_builder.get(r) {
                    let (ns, path) = rec.pattern.split_once(':').unwrap_or(("tconstruct", ""));
                    draw.sprite(&format!("{ns}:gui/tinker_pattern/{path}"), [0.0, 0.0, 16.0, 16.0], [o[0] + p[0] + 1.0, o[1] + p[1] + 1.0, 16.0, 16.0], None);
                }
            }
            draw.text(&lang(host, "block.tconstruct.part_builder", "Part Builder"), [o[0] + 8.0, o[1] + 6.0], LABEL, false);
            draw.text(&lang(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 90.0], LABEL, false);
        }
        TINKER_STATION | TINKERS_ANVIL | SCORCHED_ANVIL => {
            sprite(draw, TINKER_TEXTURE, o, [0.0, 0.0], [0.0, 0.0, 176.0, 184.0]);
            for p in tinker_places(host, block).into_iter().flatten() {
                sprite(draw, TINKER_TEXTURE, o, [p[0] - 1.0, p[1] - 1.0], [194.0, 0.0, 18.0, 18.0]);
            }
            let _ = ICONS_TEXTURE;
            let title = lang(host, &format!("block.{}", block.replace(':', ".")), &words(block.split(':').nth(1).unwrap_or("")));
            draw.text(&title, [o[0] + 8.0, o[1] + 6.0], LABEL, false);
            draw.text(&lang(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 90.0], LABEL, false);
            // TinkerStationScreen's error (the info panel is not drawn: shown below the menu).
            let code = [data_at(data, 0), data_at(data, 1), data_at(data, 2), data_at(data, 3)];
            if let Some(err) = super::modrecipe::error_text(host, code) {
                draw.text(&err, [o[0], o[1] + 186.0], 0xFFFF_5555, true);
            }
        }
        MODIFIER_WORKTABLE => super::modrecipe::draw(host, data, draw, o),
        _ => {}
    }
}

pub(crate) fn click(block: &str, data: &[i32], at: [f32; 2]) -> Option<i32> {
    if block == MODIFIER_WORKTABLE {
        return super::modrecipe::click(data, at);
    }
    if block != PART_BUILDER {
        return None;
    }
    (0..part_buttons_of(data).len()).find(|&i| hit(at, button_at(i), 18.0)).map(|i| i as i32)
}

pub(crate) fn tooltip(host: &dyn ClientHost, block: &str, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) {
    if block == MODIFIER_WORKTABLE {
        super::modrecipe::tooltip(host, data, at, lines);
        return;
    }
    if block != PART_BUILDER {
        return;
    }
    let buttons = part_buttons_of(data);
    let recipes = material_recipes(host);
    if let Some(r) = (0..buttons.len()).find(|&i| hit(at, button_at(i), 18.0)).and_then(|i| recipes.part_builder.get(buttons[i])) {
        let (ns, path) = r.pattern.split_once(':').unwrap_or(("tconstruct", ""));
        lines.push(lang(host, &format!("pattern.{ns}.{path}"), &lang(host, &format!("item.{ns}.{path}"), &words(path))));
    }
}


/// A station's state for the log, once per change (`tlog`): what it computed from which inputs.
fn trace(k: &Key, line: &str) {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static LAST: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
    let mut g = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_default();
    let key = format!("{:?}", k.1);
    if m.get(&key).map(String::as_str) != Some(line) {
        m.insert(key, line.to_owned());
        crate::smeltery::util::tlog(line);
    }
}
