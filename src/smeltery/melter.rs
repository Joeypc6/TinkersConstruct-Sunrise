//! The melter's and the heater's behaviour: MelterBlockEntity.tick (the 4-tick cycle), MeltingModuleInventory and
//! MeltingModule (one per input), FuelModule / SolidFuelModule (liquid fuel from the tank below through its `FLUIDS`,
//! solid fuel from the heater below through its `ITEMS`), the block's `active` / `in_structure` (TinyMultiblockController
//! and HeaterBlock.updateShape), the menus (MelterContainerMenu's slots and its four tank buttons), the tanks' clients
//! and comparators, and the heater's slot shown in the melter's menu.
//!
//! The heater's slot in the melter's menu (MelterContainerMenu adds the heater's own slot): a mod menu on the fork shows
//! one block entity's container (content menus have `links`, but `native_menu_links`, which names the containers behind
//! them, has no API at 514ecf9), so while a player has the melter's menu open and nobody the heater's, the heater
//! LENDS its item to the melter: it is moved, not copied, into the melter's slot 3. The item exists in one place at
//! every moment, so whatever removes either block drops it at most once. While lent the fuel burns from slot 3,
//! hoppers can't fill the heater and its menu doesn't open; the last viewer closing (or either block going) puts it
//! back. With no heater below, slot 3 is hidden (the screen leaves it out, so the server's menu has no such slot,
//! as Java's).

use super::containers::{self, CapTank, NbtTank, TankAccess, Way};
use super::recipes::{self, find_melting, melting_output};
use super::util::*;
use super::{Fields, HEATER, HEATER_MENU, MELTER, MELTER_CAPACITY, MELTER_MENU, tank_capacity};
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// MeltingModuleInventory(3).
pub(crate) const INPUTS: usize = 3;
pub(crate) const HEATER_SLOT: usize = 3;
/// MelterContainerMenu: the fuel's four ints, then each input's time, required time and temperature.
pub(crate) const DATA_SLOTS: usize = 13;
/// HeaterItemHandler: SingleItemHandler(64).
const HEATER_MAX: u32 = 64;

pub(crate) type Key = (String, Pos);

/// What isn't saved, as Java's own isn't (the tick counter, FuelModule.fuelQuality), and what hooks without the
/// block entity need: viewers, comparator signals, the tanks' contents for their drops.
#[derive(Default)]
pub(crate) struct Runtime {
    melters: HashMap<Key, (u8, i32)>,
    viewers: HashMap<Key, u32>,
    openers: HashMap<Key, Vec<u128>>,
    signals: HashMap<Key, i32>,
    tanks: HashMap<Key, Tank>,
    /// The melter whose menu the server is opening (its screen hook hides slot 3 with no heater below).
    pub opening: Option<Pos>,
    pub opening_heater: bool,
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

pub(crate) fn with_rt<T>(f: impl FnOnce(&mut Runtime) -> T) -> T {
    let mut g = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Runtime::default))
}

fn key(host: &dyn Host, pos: Pos) -> Key {
    (host.dimension().to_owned(), pos)
}

pub(crate) fn signal_at(key: &Key) -> i32 {
    with_rt(|r| r.signals.get(key).copied().unwrap_or(0))
}

/// The tank's contents a broken tank's item keeps (taken once).
pub(crate) fn removed_tank(key: &Key) -> Option<Tank> {
    with_rt(|r| r.tanks.remove(key))
}

// ---- Fuels: the items that burn (HeaterItemHandler.isItemValid), for the slot rules, which get no host

static FUELS: OnceLock<HashSet<ItemKey>> = OnceLock::new();

pub(crate) fn ensure_fuels(host: &dyn Host) {
    FUELS.get_or_init(|| host.item_keys().into_iter().filter(|k| host.item_burn_time(*k).unwrap_or(0) > 3).collect());
}

fn is_fuel(stack: &ItemStack) -> bool {
    FUELS.get().is_none_or(|f| f.contains(&stack.item()))
}

/// `is_fuel` for the host-less world insertion (`can_place_item`): refused until the list is built (the melter's and
/// heater's ticks build it), so nothing that doesn't burn gets in before.
pub(crate) fn is_known_fuel(stack: &ItemStack) -> bool {
    FUELS.get().is_some_and(|f| f.contains(&stack.item()))
}

pub(crate) fn slot_rules(event: &mut MenuSlotRulesEvent<'_>) {
    if event.menu == MELTER_MENU {
        if event.slot < INPUTS {
            // SmartItemHandlerSlot over MeltingModuleInventory: any item, one.
            event.max_stack = event.max_stack.min(1);
        } else {
            // The heater's slot: HeaterItemHandler (burnTime > 3, 64).
            event.may_place = is_fuel(event.stack);
            event.max_stack = event.max_stack.min(HEATER_MAX);
        }
    } else if event.menu == HEATER_MENU {
        event.may_place = is_fuel(event.stack);
        event.max_stack = event.max_stack.min(HEATER_MAX);
    }
}

// ---- The structure: `in_structure` from the neighbour (TinyMultiblockControllerBlock, HeaterBlock)

/// The melter's block below is in `#tconstruct:fuel_tanks`; the heater's above in `#tconstruct:heater_controllers`.
pub(crate) fn structure_neighbour(level: &dyn LevelHost, block: &str, neighbour: BlockState) -> bool {
    let (tag, list): (&str, &[&str]) =
        if block == MELTER { ("tconstruct:fuel_tanks", &super::FUEL_TANKS) } else { ("tconstruct:heater_controllers", &super::HEATER_CONTROLLERS) };
    match level.block_in_tag(neighbour.block(), tag) {
        Some(v) => v,
        None => level.state_block(neighbour).is_some_and(|n| list.contains(&n)),
    }
}

/// updateShape: the melter follows the block below (losing it turns `active` off), the heater the block above.
pub(crate) fn update_shape(level: &mut dyn LevelTicksHost, event: &mut UpdateShapeEvent) -> Option<BlockState> {
    let block = level.state_block(event.state)?.to_owned();
    let want = if block == MELTER {
        Direction::Down
    } else if block == HEATER {
        Direction::Up
    } else {
        return None;
    };
    if event.direction != want {
        return None;
    }
    let formed = structure_neighbour(&*level, &block, event.neighbour);
    let mut s = level.state_with_property(event.state, "in_structure", if formed { "true" } else { "false" })?;
    if block == MELTER && !formed {
        s = level.state_with_property(s, "active", "false")?;
    }
    (s != event.state).then_some(s)
}

// ---- The melter's inputs and the heater's slot as other blocks reach them (IItemHandler)

/// MeltingModuleInventory on every side: one item into an empty input, any input taken out whole at any time.
pub(crate) struct MelterItems {
    entity: BlockEntityRef,
}

impl ResourceHandler<ItemResource> for MelterItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        INPUTS
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        let s = self.entity.item_slot(&*host, index as u32).ok().flatten()?;
        Some(ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.entity.item_slot(&*host, index as u32).ok().flatten().map_or(0, |s| s.count())
    }
    fn capacity(&self, _host: &mut dyn ServerHost, _index: usize, _resource: Option<&ItemResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        1
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if index >= INPUTS || amount == 0 || self.amount(host, index) > 0 {
                return 0;
            }
            let Some(one) = resource.to_stack(1) else { return 0 };
            if self.entity.set_item_slot(host, tx, index as u32, Some(one)).unwrap_or(false) { 1 } else { 0 }
        })())
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if index >= INPUTS || amount == 0 {
                return 0;
            }
            let Some(s) = self.entity.item_slot(&*host, index as u32).ok().flatten() else { return 0 };
            if !resource.matches(&s) {
                return 0;
            }
            let n = s.count();
            if self.entity.set_item_slot(host, tx, index as u32, None).unwrap_or(false) { n } else { 0 }
        })())
    }
}

pub(crate) fn melter_items(level: &dyn LevelHost, at: BlockSide, state: BlockState, entity: BlockEntityRef, offer: &mut BlockCapabilityOffer<'_, dyn ResourceHandler<ItemResource>>) {
    let _ = (level, at, state);
    offer.offer(|| Box::new(MelterItems { entity }));
}

/// HeaterItemHandler on every side: fuels only, up to 64; nothing while its item is lent to the melter's menu.
pub(crate) struct HeaterItems {
    entity: BlockEntityRef,
    lent: bool,
}

impl ResourceHandler<ItemResource> for HeaterItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        let s = self.entity.item_slot(&*host, index as u32).ok().flatten()?;
        Some(ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.entity.item_slot(&*host, index as u32).ok().flatten().map_or(0, |s| s.count())
    }
    fn capacity(&self, host: &mut dyn ServerHost, _index: usize, resource: Option<&ItemResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        // The general limit: the slot's.
        let Some(resource) = resource else { return HEATER_MAX };
        max_stack(&*host, resource.as_stack()).min(HEATER_MAX)
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            let stack = resource.as_stack();
            if index != 0 || amount == 0 || self.lent || burn_time(&*host, stack) <= 3 {
                return 0;
            }
            let max = max_stack(&*host, stack).min(HEATER_MAX);
            let have = self.entity.item_slot(&*host, 0).ok().flatten();
            let (now, k) = match &have {
                None => (0, amount.min(max)),
                Some(h) if resource.matches(h) => (h.count(), amount.min(max.saturating_sub(h.count()))),
                Some(_) => return 0,
            };
            if k == 0 {
                return 0;
            }
            let Some(new) = resource.to_stack(now + k) else { return 0 };
            if self.entity.set_item_slot(host, tx, 0, Some(new)).unwrap_or(false) { k } else { 0 }
        })())
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if index != 0 || amount == 0 {
                return 0;
            }
            let Some(s) = self.entity.item_slot(&*host, 0).ok().flatten() else { return 0 };
            if !resource.matches(&s) {
                return 0;
            }
            let n = s.count().min(amount);
            let rest = s.clone().shrink(n);
            if self.entity.set_item_slot(host, tx, 0, rest).unwrap_or(false) { n } else { 0 }
        })())
    }
}

pub(crate) fn heater_items(level: &dyn LevelHost, at: BlockSide, state: BlockState, entity: BlockEntityRef, offer: &mut BlockCapabilityOffer<'_, dyn ResourceHandler<ItemResource>>) {
    let _ = state;
    // melter_lent, through the provider's level view.
    let lent = level.block_entity(above(at.pos)).and_then(|e| level.block_entity_data(e)).is_some_and(|d| gi(&d, "hz_lent", 0) != 0);
    offer.offer(|| Box::new(HeaterItems { entity, lent }));
}

/// Whether the melter at `pos` has its heater's item lent to its menu (false for one that is ticking now).
fn melter_lent(host: &dyn Host, level: LevelRef, pos: Pos) -> bool {
    host.block_entity(level, pos).and_then(|e| e.data(host)).is_some_and(|d| gi(&d, "hz_lent", 0) != 0)
}

// ---- The melter

/// One input's MeltingModule: current time, required time, required temperature.
#[derive(Clone, Copy, Default)]
pub(crate) struct Module {
    pub current: i32,
    pub required: i32,
    pub temperature: i32,
}

/// The melter's state for a tick (FuelModule's ints and the modules).
pub(crate) struct State {
    modules: [Module; INPUTS],
    fuel: i32,
    quality: i32,
    pub temperature: i32,
    rate: i32,
}

fn read_modules(nbt: &Compound) -> [Module; INPUTS] {
    let v = gia(nbt, "hz_melt");
    std::array::from_fn(|i| Module { current: v.get(i * 3).copied().unwrap_or(0), required: v.get(i * 3 + 1).copied().unwrap_or(0), temperature: v.get(i * 3 + 2).copied().unwrap_or(0) })
}

pub(crate) fn stack_key(s: &Option<ItemStack>) -> String {
    s.as_ref().filter(|s| s.count() > 0).map_or(String::new(), |s| format!("{s:?}"))
}

/// MeltingModule.setStack for an input whose stack changed: the time starts again and the recipe is looked up.
pub(crate) fn set_stack(host: &dyn Host, m: &mut Module, new: &Option<ItemStack>) {
    match new {
        None => *m = Module::default(),
        Some(n) => {
            m.current = 0;
            let (time, temp) = match find_melting(host, n) {
                Some((all, i)) => (all.melting[i].time.saturating_mul(10), all.melting[i].temperature),
                None => (0, 0),
            };
            m.required = time;
            m.temperature = temp;
        }
    }
}

/// MeltingModule.canHeatItem.
pub(crate) fn can_heat_item(m: &Module, stack: &Option<ItemStack>, temperature: i32) -> bool {
    m.required > 0 && stack.is_some() && m.current != -1 && temperature >= m.temperature
}

/// MeltingModule.onItemFinishedHeating: the recipe's fluid into the tank, all or nothing; true when the module resets.
fn finish_item(host: &mut dyn ServerHost, nbt: &mut Compound, m: &mut Module, slot: &mut Option<ItemStack>) -> bool {
    let Some(stack) = slot.clone() else { return true };
    let Some((all, i)) = find_melting(&*host, &stack) else { return true };
    let (fluid, amount) = melting_output(&*host, &all.melting[i], &stack);
    let mut tank = NbtTank { nbt, capacity: MELTER_CAPACITY };
    if tank.fill(host, &fluid, amount, false) == amount {
        tank.fill(host, &fluid, amount, true);
        *slot = None;
        true
    } else {
        m.current = -1;
        false
    }
}

/// MeltingModule.heatItem.
fn heat_item(host: &mut dyn ServerHost, nbt: &mut Compound, m: &mut Module, slot: &mut Option<ItemStack>, temperature: i32, rate: i32) {
    if m.current == -1 || can_heat_item(m, slot, temperature) {
        if m.current != -1 && m.current < m.required {
            m.current += rate;
        } else if finish_item(host, nbt, m, slot) {
            *m = Module::default();
        }
    }
}

/// MeltingModule.coolItem: a full tank's item tries again (without fuel too); the rest cool by 5 (below 0 too).
fn cool_item(host: &mut dyn ServerHost, nbt: &mut Compound, m: &mut Module, slot: &mut Option<ItemStack>) {
    if m.current == -1 {
        if finish_item(host, nbt, m, slot) {
            *m = Module::default();
        }
    } else if m.current > 0 && m.required > 0 {
        m.current -= 5;
    }
}

/// MeltingModuleInventory.heatItems (`Some((temperature, rate))`) or coolItems.
fn heat_all(host: &mut dyn ServerHost, e: &mut Fields, s: &mut State, heat: Option<(i32, i32)>) {
    for i in 0..INPUTS {
        let mut slot = e.items[i].clone();
        match heat {
            Some((t, r)) => heat_item(host, &mut e.nbt, &mut s.modules[i], &mut slot, t, r),
            None => cool_item(host, &mut e.nbt, &mut s.modules[i], &mut slot),
        }
        if slot != e.items[i] {
            e.items[i] = slot;
        }
    }
}

/// A small random source (the level's random isn't on every host).
pub(crate) fn rand01() -> f64 {
    static SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let mut x = SEED.load(std::sync::atomic::Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    SEED.store(x, std::sync::atomic::Ordering::Relaxed);
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// SolidFuelModule's drop of a container that doesn't go back: at the fuel's block, `rand * 0.5 + 0.25` on each axis.
fn drop_at(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, stack: ItemStack) {
    let x = f64::from(pos[0]) + rand01() * 0.5 + 0.25;
    let y = f64::from(pos[1]) + rand01() * 0.5 + 0.25;
    let z = f64::from(pos[2]) + rand01() * 0.5 + 0.25;
    let _ = level.spawn_item(host, x, y, z, stack, 10);
}

/// FuelModule.findFuel / tryLiquidFuel / SolidFuelModule.trySolidFuel: the fuel the block below offers (its tank 0
/// through `FLUIDS`, else its items through `ITEMS`), its temperature (0 for none); with `consume`, it is taken and
/// burns from now. The heater's item is in the melter's own slot 3 while it is lent to the menu.
fn find_fuel(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, e: &mut Fields, s: &mut State, consume: bool) -> i32 {
    let below = below(pos);
    let all = recipes::recipes(&*host);
    // A liquid fuel: the tank below's fluid, as much as one burn takes.
    if let Some(cap) = level.capability(host, &FLUIDS, below, None) {
        let mut tank = CapTank(cap);
        if let (Some(fluid), amount) = tank.contents(host)
            && let Some(f) = recipes::fluid_fuel(&all, &fluid)
            && amount >= f.amount
        {
            if consume {
                if tank.drain(host, &fluid, f.amount, true) < f.amount {
                    return 0;
                }
                s.fuel += f.duration;
                s.quality = f.duration;
                s.temperature = f.temperature;
                s.rate = f.rate;
            }
            return f.temperature;
        }
    }
    // A solid fuel: the first item below that burns, a quarter of its burn time.
    if let Some(solid) = recipes::solid_fuel(&all) {
        let (temperature, rate) = (solid.temperature, solid.rate);
        let lent = gi(&e.nbt, "hz_lent", 0) != 0;
        if lent {
            if let Some(stack) = e.items[HEATER_SLOT].clone() {
                let time = burn_time(&*host, &stack) / 4;
                if time > 0 {
                    if consume {
                        let remainder = host.item_crafting_remainder(&stack);
                        e.items[HEATER_SLOT] = stack.shrink(1);
                        if let Some(r) = remainder {
                            // insertItem into HeaterItemHandler: only what burns goes back in.
                            let back = burn_time(&*host, &r) > 3 && e.items[HEATER_SLOT].is_none();
                            if back {
                                e.items[HEATER_SLOT] = Some(r);
                            } else {
                                drop_at(host, level, below, r);
                            }
                        }
                        s.fuel += time;
                        s.quality = time;
                        s.temperature = temperature;
                        s.rate = rate;
                    }
                    return temperature;
                }
            }
        } else if let Some(items) = level.capability(host, &ITEMS, below, None) {
            for i in 0..items.size(host) {
                let Some(res) = items.resource(host, i) else { continue };
                let stack = res.as_stack().clone();
                let time = burn_time(&*host, &stack) / 4;
                if time <= 0 {
                    continue;
                }
                if consume {
                    let took = host.transaction(|host, tx| if items.extract_at(host, tx, i, &res, 1) == Ok(1) { Ok(()) } else { Err(()) });
                    if took.is_err() {
                        return 0;
                    }
                    if let Some(r) = host.item_crafting_remainder(&stack) {
                        let n = r.count();
                        let rr = ItemResource::of(&r);
                        let put = host.transaction(|host, tx| Ok::<u32, ()>(items.insert(host, tx, &rr, n).unwrap_or(0))).unwrap_or(0);
                        if put < n
                            && let Some(rest) = with_count(&r, n - put)
                        {
                            drop_at(host, level, below, rest);
                        }
                    }
                    s.fuel += time;
                    s.quality = time;
                    s.temperature = temperature;
                    s.rate = rate;
                }
                return temperature;
            }
        }
    }
    if consume {
        s.temperature = 0;
        s.rate = 0;
    }
    0
}

/// Each block entity's tick: the melter's (MelterBlockEntity.tick) and, for the melter and the tanks, what their
/// clients and comparators see.
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, entity: &mut Fields) {
    ensure_ids(&*host);
    ensure_fuels(&*host);
    let Some(level) = here(&*host) else { return };
    let block = block_id(&*host, level, pos);
    if block == HEATER {
        if entity.items.is_empty() {
            entity.items.resize(1, None);
        }
        return;
    }
    let mut e = entity.clone();
    if block == MELTER {
        melter_tick(host, level, pos, &mut e);
    }
    if let Some(capacity) = tank_capacity(&block) {
        sync(host, level, pos, &block, &mut e, capacity);
    }
    // Written back only where something changed (each write is a revision clients may redraw for).
    if e.nbt != entity.nbt {
        entity.nbt = e.nbt;
    }
    if e.items != entity.items {
        entity.items = e.items;
    }
    if e.data != entity.data {
        entity.data = e.data;
    }
}

fn melter_tick(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, e: &mut Fields) {
    let k = key(&*host, pos);
    if e.items.len() < INPUTS + 1 {
        e.items.resize(INPUTS + 1, None);
    }
    // MelterBlockEntity.isFormed: the state's `in_structure` (kept by updateShape).
    let formed = level.block_property(&*host, pos, "in_structure").as_deref() == Some("true");
    // An input holds one (getSlotLimit): what got past that goes back out on top.
    for slot in 0..INPUTS {
        if let Some(s) = e.items[slot].clone().filter(|s| s.count() > 1) {
            let extra = with_count(&s, s.count() - 1);
            e.items[slot] = with_count(&s, 1);
            if let Some(x) = extra {
                let _ = level.spawn_item(host, f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.0, f64::from(pos[2]) + 0.5, x, 10);
            }
        }
    }
    let (tick, quality) = with_rt(|r| r.melters.get(&k).copied().unwrap_or((0, 0)));
    let mut s = State {
        modules: read_modules(&e.nbt),
        fuel: gi(&e.nbt, "fuel", 0),
        quality,
        temperature: gi(&e.nbt, "temperature", 0),
        rate: gi(&e.nbt, "rate", 0),
    };
    // The inputs' changes since the last tick (clicks, hoppers): MeltingModule.setStack for each.
    let mut seen = get_strings(&e.nbt, "hz_seen", INPUTS);
    for slot in 0..INPUTS {
        let kk = stack_key(&e.items[slot]);
        if kk != seen[slot] {
            set_stack(&*host, &mut s.modules[slot], &e.items[slot]);
            seen[slot] = kk;
        }
    }
    lend_tick(host, level, pos, &k, e);

    if formed {
        match tick {
            0 => {
                if s.fuel <= 0 {
                    let t = find_fuel(host, level, pos, e, &mut s, false);
                    if (0..INPUTS).any(|i| can_heat_item(&s.modules[i], &e.items[i], t)) {
                        find_fuel(host, level, pos, e, &mut s, true);
                    }
                }
            }
            2 => {
                let has_fuel = s.fuel > 0;
                let lit = if has_fuel { "true" } else { "false" };
                if level.block_property(&*host, pos, "active").is_some_and(|v| v != lit) {
                    host.set_block_property_with(pos, "active", lit, 3);
                    // The heater below shows it too (a fuel tank has no `active`).
                    let b = below(pos);
                    if level.block_property(&*host, b, "active").is_some_and(|v| v != lit) {
                        host.set_block_property_with(b, "active", lit, 3);
                    }
                }
                if has_fuel {
                    let heat = Some((s.temperature, s.rate));
                    heat_all(host, e, &mut s, heat);
                    s.fuel = (s.fuel - 1).max(0);
                } else {
                    heat_all(host, e, &mut s, None);
                }
            }
            _ => {}
        }
    } else if tick == 2 {
        if s.fuel > 0 {
            s.fuel = (s.fuel - 1).max(0);
        } else {
            heat_all(host, e, &mut s, None);
        }
    }
    // A melted item left its slot.
    for slot in 0..INPUTS {
        if e.items[slot].is_none() && !seen[slot].is_empty() {
            seen[slot].clear();
            s.modules[slot] = Module::default();
        }
    }
    with_rt(|r| r.melters.insert(k, ((tick + 1) % 4, s.quality)));
    sia(&mut e.nbt, "hz_melt", s.modules.iter().flat_map(|m| [m.current, m.required, m.temperature]).collect());
    set_strings(&mut e.nbt, "hz_seen", &seen);
    si(&mut e.nbt, "fuel", s.fuel);
    si(&mut e.nbt, "temperature", s.temperature);
    si(&mut e.nbt, "rate", s.rate);
    // The menu's data slots (ValidZeroDataSlot): the fuel's four ints, then each input's three.
    let mut data = vec![0; DATA_SLOTS];
    data[0] = s.fuel;
    data[1] = s.quality;
    data[2] = s.temperature;
    data[3] = s.rate;
    for i in 0..INPUTS {
        data[4 + i * 3] = s.modules[i].current;
        data[5 + i * 3] = s.modules[i].required;
        data[6 + i * 3] = s.modules[i].temperature;
    }
    if e.data != data {
        e.data = data;
    }
    // What the screen's heat bars need of the inputs (the client's copy has no item slots).
    let ids: Vec<String> = (0..INPUTS).map(|i| e.items[i].as_ref().map_or(String::new(), |s| sid(s).to_owned())).collect();
    set_strings(&mut e.nbt, "hz_items", &ids);
}

/// What clients draw of a block (its tank, the melter's inputs): when it changes they are sent it again
/// (FluidUpdatePacket, InventorySlotSyncPacket); when its comparator signal changes the comparators read it again
/// (ITankBlockEntity.onTankContentsChanged); the tank as a broken tank's item keeps it.
pub(crate) fn sync(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, block: &str, e: &mut Fields, capacity: i32) {
    let k = key(&*host, pos);
    let tank = tank_of(&e.nbt);
    let mut vis = format!("{:?}={}", tank.0, tank.1);
    if block == MELTER {
        vis.push_str(&get_strings(&e.nbt, "hz_items", INPUTS).join(","));
    }
    if gs(&e.nbt, "hz_vis").as_deref() != Some(vis.as_str()) {
        ss(&mut e.nbt, "hz_vis", &vis);
        host.block_entity_sync(pos);
    }
    // SearedTankBlock.LIGHT: a tank's and a lantern's light is its fluid's (TankBlockEntity.onTankContentsChanged).
    if block != MELTER {
        let light = tank.0.as_deref().map_or(0, |f| fluid_light(&*host, f));
        let want = light.to_string();
        if level.block_property(&*host, pos, "light").is_some_and(|v| v != want) {
            host.set_block_property_with(pos, "light", &want, 3);
        }
    }
    let strength = signal(tank.1, capacity);
    let changed = with_rt(|r| {
        if block != MELTER {
            if r.tanks.get(&k) != Some(&tank) {
                r.tanks.insert(k.clone(), tank.clone());
            }
        }
        r.signals.insert(k.clone(), strength) != Some(strength)
    });
    if changed
        && let Some(state) = level.block_state(&*host, pos)
    {
        let _ = level.update_neighbors(host, pos, state, NeighbourUpdate::OutputSignal);
    }
}

/// FluidType.getLightLevel (`fluids::light`): Tinkers' from its table, lava's and another mod's from the game.
fn fluid_light(host: &dyn Host, fluid: &str) -> u8 {
    crate::fluids::light(host, fluid)
}

/// `block_entity_update_tag`: the tank, the inputs' ids and the lent flag.
pub(crate) fn update_tag(nbt: &Compound) -> Compound {
    let mut out = Compound::new();
    for k in ["fluids", "hz_items", "hz_lent"] {
        if let Some(v) = nbt.get(&js(k)) {
            out.insert(js(k), v.clone());
        }
    }
    out
}

/// `level_tick_post`: runtime state of blocks gone (their tick counters and signals) is dropped now and then.
pub(crate) fn level_tick(host: &mut dyn ServerHost) {
    let Some(level) = here(&*host) else { return };
    let t = level.game_time(&*host).unwrap_or(0);
    if t % 600 != 0 {
        return;
    }
    let dim = host.dimension().to_owned();
    let keys: Vec<Pos> = with_rt(|r| r.melters.keys().chain(r.signals.keys()).filter(|(d, _)| *d == dim).map(|(_, p)| *p).collect());
    for p in keys {
        if level.is_loaded(&*host, p) && tank_capacity(&block_id(&*host, level, p)).is_none() {
            let k = (dim.clone(), p);
            with_rt(|r| {
                r.melters.remove(&k);
                r.signals.remove(&k);
            });
        }
    }
}

// ---- The heater's slot in the melter's menu

fn viewers(k: &Key) -> u32 {
    with_rt(|r| r.viewers.get(k).copied().unwrap_or(0))
}

/// `stack` into `h` up to `max`; what doesn't fit.
fn merge_into(h: &mut Option<ItemStack>, stack: ItemStack, max: u32) -> Option<ItemStack> {
    match h {
        None => {
            let keep = stack.count().min(max);
            let rest = with_count(&stack, stack.count() - keep);
            *h = with_count(&stack, keep);
            rest
        }
        Some(hs) if hs.is_stackable_with(&stack) => {
            let moved = stack.count().min(max.saturating_sub(hs.count()));
            if moved > 0 {
                let _ = hs.grow(moved);
            }
            with_count(&stack, stack.count() - moved)
        }
        Some(_) => Some(stack),
    }
}

/// An item a player put where it can't stay: back to a player who has the menu open, or out on top.
fn give_back(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, k: &Key, stack: ItemStack) {
    match with_rt(|r| r.openers.get(k).and_then(|o| o.first().copied())) {
        Some(p) => containers::give_player(host, p, stack),
        None => {
            let _ = level.spawn_item(host, f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.0, f64::from(pos[2]) + 0.5, stack, 10);
        }
    }
}

/// The heater's slot through its block entity (not its `ITEMS`, which refuse while lent).
fn heater_slot(host: &dyn Host, level: LevelRef, pos: Pos) -> Option<(BlockEntityRef, Option<ItemStack>)> {
    let e = host.block_entity(level, pos)?;
    let s = e.item_slot(host, 0).ok()?;
    Some((e, s))
}

fn set_heater_slot(host: &mut dyn ServerHost, e: BlockEntityRef, stack: Option<ItemStack>) -> bool {
    host.transaction(|host, tx| if e.set_item_slot(host, tx, 0, stack).unwrap_or(false) { Ok(()) } else { Err(()) }).is_ok()
}

/// The lent item back into the heater below (what doesn't merge, or all of it with no heater, out on top).
fn return_lent(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, item: Option<ItemStack>) {
    let Some(item) = item else { return };
    let b = below(pos);
    let mut rest = Some(item.clone());
    if block_id(&*host, level, b) == HEATER
        && let Some((e, mut slot)) = heater_slot(&*host, level, b)
    {
        let max = max_stack(&*host, &item).min(HEATER_MAX);
        let left = merge_into(&mut slot, item.clone(), max);
        if set_heater_slot(host, e, slot) {
            rest = left;
        }
    }
    if let Some(r) = rest {
        let _ = level.spawn_item(host, f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.0, f64::from(pos[2]) + 0.5, r, 10);
    }
}

/// Each tick of the melter (out of the world): lends, keeps or returns the heater's item.
fn lend_tick(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, k: &Key, e: &mut Fields) {
    let b = below(pos);
    let heater = block_id(&*host, level, b) == HEATER;
    let want = heater && viewers(k) > 0 && viewers(&(k.0.clone(), b)) == 0;
    let slot = e.items[HEATER_SLOT].take();
    if gi(&e.nbt, "hz_lent", 0) != 0 {
        if want {
            // Still lent: what a player put in passed the slot rule (fuel, 64); anything else goes back.
            match slot {
                Some(st) if burn_time(&*host, &st) <= 3 => give_back(host, level, pos, k, st),
                Some(st) => {
                    let max = max_stack(&*host, &st).min(HEATER_MAX);
                    let mut keep = None;
                    if let Some(rest) = merge_into(&mut keep, st, max) {
                        give_back(host, level, pos, k, rest);
                    }
                    e.items[HEATER_SLOT] = keep;
                }
                None => {}
            }
            return;
        }
        si(&mut e.nbt, "hz_lent", 0);
        return_lent(host, level, pos, slot);
    } else if let Some(st) = slot {
        // Not lent, so the slot is no one's: a player's item put there goes into the heater if it burns and fits, or
        // back to the player.
        let mut rest = Some(st.clone());
        if heater
            && burn_time(&*host, &st) > 3
            && let Some((he, mut hs)) = heater_slot(&*host, level, b)
        {
            let max = max_stack(&*host, &st).min(HEATER_MAX);
            let left = merge_into(&mut hs, st, max);
            if set_heater_slot(host, he, hs) {
                rest = left;
            }
        }
        if let Some(r) = rest {
            give_back(host, level, pos, k, r);
        }
    }
    if want
        && let Some((he, hs)) = heater_slot(&*host, level, b)
        && set_heater_slot(host, he, None)
    {
        e.items[HEATER_SLOT] = hs;
        si(&mut e.nbt, "hz_lent", 1);
    }
}

/// The melter at `pos`'s lent item back to its heater now (the melter not ticking).
fn end_lend(host: &mut dyn ServerHost, level: LevelRef, pos: Pos) {
    let Some(m) = host.block_entity(level, pos) else { return };
    if block_id(&*host, level, pos) != MELTER || !m.data(&*host).is_some_and(|d| gi(&d, "hz_lent", 0) != 0) {
        return;
    }
    let item = m.item_slot(&*host, HEATER_SLOT as u32).ok().flatten();
    let mut c = Compound::new();
    si(&mut c, "hz_lent", 0);
    let _ = m.merge_data(host, c);
    let cleared = host.transaction(|host, tx| if m.set_item_slot(host, tx, HEATER_SLOT as u32, None).unwrap_or(false) { Ok(()) } else { Err(()) }).is_ok();
    if cleared {
        return_lent(host, level, pos, item);
    }
}

/// `mod_container_viewers`: who has a melter's or a heater's menu open; the melter's last viewer closing returns the
/// heater's item.
pub(crate) fn container_viewers(host: &mut dyn ServerHost, dimension: &str, pos: Pos, n: u32) {
    let Some(level) = host.level(dimension) else { return };
    let block = block_id(&*host, level, pos);
    if block != MELTER && block != HEATER {
        return;
    }
    with_rt(|r| {
        if n == 0 {
            r.viewers.remove(&(dimension.to_owned(), pos));
        } else {
            r.viewers.insert((dimension.to_owned(), pos), n);
        }
    });
    if n == 0 && block == MELTER {
        end_lend(host, level, pos);
    }
}

/// `player_container_open` / `_close`: who has a melter's menu open, to give back what can't stay in slot 3.
pub(crate) fn container_opened(dimension: &str, player: u128, pos: Option<Pos>, open: bool) {
    let Some(pos) = pos else { return };
    with_rt(|r| {
        let k = (dimension.to_owned(), pos);
        let list = r.openers.entry(k.clone()).or_default();
        list.retain(|p| *p != player);
        if open {
            list.push(player);
        }
        if list.is_empty() {
            r.openers.remove(&k);
        }
    });
}

// ---- Clicks

/// MelterBlock.use, SearedTankBlock.use, HeaterBlock (InventoryBlock.use): a fluid container in hand first, then the
/// menu (the melter's only in a structure, else the action bar says why).
pub(crate) fn use_block(host: &mut dyn ServerHost, click: &BlockClick<'_>, with_item: bool) -> Option<InteractionResult> {
    ensure_ids(&*host);
    let player = host.player(click.player)?;
    let level = player.level(&*host)?;
    let pos = click.pos;
    let block = block_id(&*host, level, pos);
    // SearedLanternBlock has no use().
    if super::LANTERNS.contains(&block.as_str()) {
        return None;
    }
    if with_item && block != HEATER && containers::interact_with_tank(host, click, level) {
        return Some(InteractionResult::Success);
    }
    if with_item {
        // Anything else in hand: the block's own use (useWithoutItem), as vanilla falls through.
        return None;
    }
    if block == MELTER {
        if level.block_property(&*host, pos, "in_structure").as_deref() != Some("true") {
            let _ = player.action_bar(host, tr("multiblock.tconstruct.tiny.no_fuel_tank", "Must be placed above a fuel tank or a heater to function", Vec::new()));
            return Some(InteractionResult::Success);
        }
        let entity = host.block_entity(level, pos)?;
        let heater = block_id(&*host, level, below(pos)) == HEATER;
        with_rt(|r| {
            r.opening = Some(pos);
            r.opening_heater = heater;
        });
        pad_slots(host, entity, MELTER);
        let r = player.open_menu(host, entity);
        with_rt(|r| r.opening = None);
        return match r {
            Ok(true) => Some(InteractionResult::Success),
            _ => None,
        };
    }
    if block == HEATER {
        // The heater's menu doesn't open while its item is lent to the melter's (it is in one place at a time).
        if melter_lent(&*host, level, above(pos)) {
            return Some(InteractionResult::Success);
        }
        let entity = host.block_entity(level, pos)?;
        pad_slots(host, entity, HEATER);
        return match player.open_menu(host, entity) {
            Ok(true) => Some(InteractionResult::Success),
            _ => None,
        };
    }
    None
}

/// MelterContainerMenu.clickMenuButton: 0/1 the melter's tank, 2/3 the fuel tank below; even fills the cursor's item
/// from the tank, odd empties it into the tank (ClickableTankModule: the mouse button plus 2 for the fuel's). From the
/// server's own cursor stack; the result as FluidTransferHelper.handleUIResult.
pub(crate) fn menu_button(host: &mut dyn ServerHost, player: u128, pos: Pos, id: i32, e: &mut Fields, carried: &mut Option<ItemStack>) {
    ensure_ids(&*host);
    if !(0..=3).contains(&id) {
        return;
    }
    let Some(p) = host.player(player) else { return };
    let Some(level) = p.level(&*host) else { return };
    if block_id(&*host, level, pos) != MELTER || p.game_mode(&*host) == Some(GameMode::Spectator) {
        return;
    }
    let Some(held) = carried.clone().filter(|c| c.count() > 0) else { return };
    let way = if id & 1 == 0 { Way::FillItem } else { Way::EmptyItem };
    let moved = if id < 2 {
        let mut tank = NbtTank { nbt: &mut e.nbt, capacity: MELTER_CAPACITY };
        containers::transfer(host, &held, &mut tank, way)
    } else {
        let b = below(pos);
        if tank_capacity(&block_id(&*host, level, b)).is_none() {
            return;
        }
        let Some(cap) = level.capability(host, &FLUIDS, b, None) else { return };
        containers::transfer(host, &held, &mut CapTank(cap), way)
    };
    let Some(m) = moved else { return };
    p.play_sound(host, containers::bucket_sound(&m.fluid, m.filled_item), SoundSource::Players, 1.0, 1.0);
    if held.count() <= 1 {
        *carried = m.result;
    } else {
        *carried = with_count(&held, held.count() - 1);
        if let Some(r) = m.result {
            containers::give_player(host, player, r);
        }
    }
}
