//! Casting (TinkerSmeltery, stage 2B): the seared and scorched faucets (FaucetBlock, FaucetBlockEntity), casting tables
//! and basins (AbstractCastingBlock, CastingBlockEntity.Table/Basin with CastingFluidHandler), channels (ChannelBlock,
//! ChannelBlockEntity with ChannelTank) and the casting tank (CastingTankBlock, CastingTankBlockEntity).
//!
//! On the fork:
//! - five block entity types, as Java's: `tconstruct:faucet`, `tconstruct:table`, `tconstruct:basin`,
//!   `tconstruct:channel`, `tconstruct:casting_tank`;
//! - every fluid move goes through `FLUIDS`: the faucet drains any mod's block behind it into any below, a channel
//!   fills its neighbours; the tables, basins and channels answer `FLUIDS` with Java's handlers (CastingFluidHandler,
//!   the channel's fill-only top and `in` sides, its empty `out` sides), so other mods' pipes reach them too; the
//!   casting tank's tank is the game's (`ModFluidTanks`, the melter's tanks' way);
//! - their two slots are the block entities' containers, reached through our `ITEMS` handlers with Java's
//!   SidedInvWrapper rules (hoppers put a cast in, take the result out);
//! - redstone from `neighbor_changed` (a rising edge schedules the block tick 2 ticks later, as Java's
//!   `scheduleTick`); `tick_block` and `neighbor_changed` can't reach a block entity, so what they decide waits in
//!   `Runtime` for the block entity's tick of the same game tick (block ticks run first, as in Java);
//! - drawing in `casting_render`, the recipes in `casting_recipes`, the recipe viewer in `casting_jei`.

use super::casting_recipes::{self as recipes, cast_view, find_casting, find_molding};
use super::containers::{self, CapTank, NbtTank, TankAccess, Way, fluid_id, fluid_res};
use super::util::*;
use super::{Fields, melter};
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub(crate) const FAUCETS: [&str; 2] = ["tconstruct:seared_faucet", "tconstruct:scorched_faucet"];
pub(crate) const TABLES: [&str; 2] = ["tconstruct:seared_table", "tconstruct:scorched_table"];
pub(crate) const BASINS: [&str; 2] = ["tconstruct:seared_basin", "tconstruct:scorched_basin"];
pub(crate) const CHANNELS: [&str; 2] = ["tconstruct:seared_channel", "tconstruct:scorched_channel"];
pub(crate) const CASTING_TANK: &str = "tconstruct:seared_casting_tank";
/// CastingTankBlock.getCapacity.
pub(crate) const CASTING_TANK_CAPACITY: i32 = 4000;

pub(crate) const FAUCET_TYPE: &str = "tconstruct:faucet";
pub(crate) const TABLE_TYPE: &str = "tconstruct:table";
pub(crate) const BASIN_TYPE: &str = "tconstruct:basin";
pub(crate) const CHANNEL_TYPE: &str = "tconstruct:channel";
pub(crate) const CASTING_TANK_TYPE: &str = "tconstruct:casting_tank";

/// FaucetBlockEntity.PACKET_SIZE and MB_PER_TICK.
const FAUCET_PACKET: i32 = 90;
const MB_PER_TICK: i32 = 10;
/// ChannelBlockEntity: one ChannelTank of 40 mB.
pub(crate) const CHANNEL_CAPACITY: i32 = 40;
/// FaucetState ordinals.
pub(crate) const OFF: i32 = 0;
pub(crate) const POURING: i32 = 1;
pub(crate) const POWERED: i32 = 2;

/// Directions by index: down, up, north, south, west, east (Direction's 3D data values).
pub(crate) const DIRS: [Direction; 6] = [Direction::Down, Direction::Up, Direction::North, Direction::South, Direction::West, Direction::East];
pub(crate) const DIR_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];
/// The horizontal sides in Plane.HORIZONTAL's order (NORTH, EAST, SOUTH, WEST).
pub(crate) const CHANNEL_SIDES: [usize; 4] = [2, 5, 3, 4];

pub(crate) fn is_faucet(block: &str) -> bool {
    FAUCETS.contains(&block)
}
pub(crate) fn is_table(block: &str) -> bool {
    TABLES.contains(&block)
}
pub(crate) fn is_basin(block: &str) -> bool {
    BASINS.contains(&block)
}
pub(crate) fn is_casting(block: &str) -> bool {
    is_table(block) || is_basin(block)
}
pub(crate) fn is_channel(block: &str) -> bool {
    CHANNELS.contains(&block)
}
pub(crate) fn is_casting_block(block: &str) -> bool {
    is_faucet(block) || is_casting(block) || is_channel(block) || block == CASTING_TANK
}

pub(crate) fn step(pos: Pos, d: usize) -> Pos {
    let v = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]][d];
    [pos[0] + v[0], pos[1] + v[1], pos[2] + v[2]]
}

pub(crate) fn opposite(d: usize) -> usize {
    [1, 0, 3, 2, 5, 4][d]
}

pub(crate) fn dir_index(name: &str) -> Option<usize> {
    DIR_NAMES.iter().position(|n| *n == name)
}

fn dir_of(d: Direction) -> usize {
    DIRS.iter().position(|x| *x == d).unwrap_or(0)
}

fn dir_of_normal(face: [i32; 3]) -> Option<usize> {
    (0..6).find(|&d| step([0, 0, 0], d) == face)
}

/// A property of `state` as LevelHost names it (`block[a=b,...]`).
pub(crate) fn state_prop(level: &dyn LevelHost, state: BlockState, key: &str) -> Option<String> {
    let s = level.state_string(state)?;
    let props = s.split_once('[')?.1.trim_end_matches(']');
    props.split(',').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v.to_owned()))
}

fn is_air(block: &str) -> bool {
    matches!(block, "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
}

// ---- What hooks without the block entity decide, for its next tick

pub(crate) type Key = (String, Pos);

#[derive(Default)]
pub(crate) struct Runtime {
    /// The last redstone signal seen (`lastRedstone`), seeded from the saved data by the first tick.
    redstone: HashMap<Key, bool>,
    /// The scheduled ticks that fired (FaucetBlock.tick → activate, the casting blocks' → swap).
    activate: HashSet<Key>,
    /// A faucet's falling edge (handleRedstone: a waiting faucet turns off).
    falling: HashSet<Key>,
    /// A channel's new `powered` (ChannelBlock.neighborChanged), applied with its `down` by its tick.
    channel_power: HashMap<Key, bool>,
    /// The tables' and basins' comparator signals (getAnalogSignal).
    signals: HashMap<Key, i32>,
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

fn with_rt<T>(f: impl FnOnce(&mut Runtime) -> T) -> T {
    let mut g = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Runtime::default))
}

fn key(host: &dyn Host, pos: Pos) -> Key {
    (host.dimension().to_owned(), pos)
}

pub(crate) fn signal_at(key: &Key) -> i32 {
    with_rt(|r| r.signals.get(key).copied().unwrap_or(0))
}

/// The last redstone state of a block, seeded from its saved flag.
fn redstone_of(k: &Key, saved: bool) -> bool {
    with_rt(|r| *r.redstone.entry(k.clone()).or_insert(saved))
}

// ---- Registration

/// The part every casting block, its block entity and its item carry.
#[derive(Default)]
pub(crate) struct CastingPart;

pub(crate) fn register(r: &mut dyn Registrar) {
    let types: [(&str, Vec<&str>); 5] = [
        (FAUCET_TYPE, FAUCETS.to_vec()),
        (TABLE_TYPE, TABLES.to_vec()),
        (BASIN_TYPE, BASINS.to_vec()),
        (CHANNEL_TYPE, CHANNELS.to_vec()),
        (CASTING_TANK_TYPE, vec![CASTING_TANK]),
    ];
    for (ty, blocks) in types {
        let key = r.block_entity_type(ty).blocks(&blocks).build();
        r.block_entity_part(&key, CastingPart).expect("the casting block entity types are native");
        for b in &blocks {
            let k = r.item_key(b);
            r.item_part(k, CastingPart);
        }
        match ty {
            TABLE_TYPE | BASIN_TYPE => {
                r.block_entity_capability_before_game(&FLUIDS, &key, casting_fluids);
                r.block_entity_capability_before_game(&ITEMS, &key, casting_items);
            }
            CHANNEL_TYPE => r.block_entity_capability_before_game(&FLUIDS, &key, channel_fluids),
            CASTING_TANK_TYPE => r.block_entity_capability_before_game(&ITEMS, &key, casting_tank_items),
            _ => {}
        }
    }
}

/// The two slots of the tables, basins and the casting tank (dropped with the block, as InventoryBlock.onRemove).
pub(crate) fn block_entity_container(block: &str) -> Option<ModContainer> {
    if !(is_casting(block) || block == CASTING_TANK) {
        return None;
    }
    Some(ModContainer { slots: 2, faces: std::array::from_fn(|_| vec![0, 1]), inputs: vec![0], outputs: vec![1], insert_rules: Vec::new(), menu: None })
}

// ---- Fluids between blocks

fn cap_at(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, side: Option<Direction>) -> Option<CapTank> {
    level.capability(host, &FLUIDS, pos, side).map(CapTank)
}

/// IFluidHandler.drain(max, SIMULATE) by amount: the first fluid there and how much of `max` comes out.
fn peek_drain(host: &mut dyn ServerHost, t: &mut CapTank, max: i32) -> Option<(String, i32)> {
    let n = t.0.size(host);
    let fluid = (0..n).find_map(|i| if t.0.amount(host, i) > 0 { t.0.resource(host, i).and_then(|r| fluid_id(&*host, &r)) } else { None })?;
    let d = t.drain(host, &fluid, max, false);
    (d > 0).then_some((fluid, d))
}

/// A fluid stack as Forge saves one (`{FluidName, Amount}`).
fn fluid_nbt(fluid: Option<&str>, amount: i32) -> Compound {
    let mut c = Compound::new();
    ss(&mut c, "FluidName", fluid.filter(|_| amount > 0).unwrap_or("minecraft:empty"));
    si(&mut c, "Amount", if fluid.is_some() { amount.max(0) } else { 0 });
    c
}

fn read_fluid(c: Option<&Compound>) -> Option<(String, i32)> {
    let c = c?;
    let f = gs(c, "FluidName").filter(|f| !f.is_empty() && f != "minecraft:empty")?;
    let n = gi(c, "Amount", 0);
    (n > 0).then_some((f, n))
}

fn play(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, sound: &str, volume: f32, pitch: f32) {
    let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
    let _ = level.play_sound(host, sound, at, SoundSource::Blocks, volume, pitch, None);
}

fn sneaking(host: &dyn Host, player: u128) -> bool {
    host.player(player).is_some_and(|p| {
        let e = p.entity();
        e.is_shift_key_down(host).unwrap_or(false) || e.is_crouching(host).unwrap_or(false)
    })
}

/// The slot behind a hand (a creative hand keeps its stack through `PlayerHand`; Java's casting blocks take one anyway).
fn hand_slot(host: &dyn Host, player: u128, hand: Hand) -> ItemPlace {
    let slot = match hand {
        Hand::Off => -106,
        _ => host.player(player).and_then(|p| p.selected_slot(host)).unwrap_or(0) as i32,
    };
    ItemPlace::PlayerSlot { player, slot }
}

/// ItemHandlerHelper.giveItemToPlayer(player, stack, selected): the selected slot first, then the inventory, else
/// dropped at the player.
fn give(host: &mut dyn ServerHost, player: u128, stack: ItemStack) {
    let place = hand_slot(&*host, player, Hand::Main);
    let n = stack.count();
    let res = ItemResource::of(&stack);
    let given = host.transaction(|host, tx| Ok::<u32, ()>(place.insert(host, tx, &res, n).unwrap_or(0))).unwrap_or(0);
    if given < n
        && let Some(rest) = with_count(&stack, n - given)
    {
        containers::give_player(host, player, rest);
    }
}

/// One item of the hand's stack into slot 0, in one transaction with its removal from the hand: the block gets
/// nothing unless the hand loses exactly that item.
fn place_from_hand(host: &mut dyn ServerHost, e: BlockEntityRef, place: &ItemPlace, held: &ItemStack) {
    let res = ItemResource::of(held);
    let one = with_count(held, 1);
    let _ = host.transaction(|host, tx| {
        if e.set_item_slot(host, tx, 0, one.clone()).unwrap_or(false) && place.extract(host, tx, &res, 1) == Ok(1) { Ok(()) } else { Err(()) }
    });
}

fn set_slot(host: &mut dyn ServerHost, e: BlockEntityRef, slot: u32, stack: Option<ItemStack>) -> bool {
    host.transaction(|host, tx| if e.set_item_slot(host, tx, slot, stack).unwrap_or(false) { Ok(()) } else { Err(()) }).is_ok()
}

fn slots_of(host: &dyn Host, e: BlockEntityRef) -> [Option<ItemStack>; 2] {
    [e.item_slot(host, 0).ok().flatten().filter(|s| s.count() > 0), e.item_slot(host, 1).ok().flatten().filter(|s| s.count() > 0)]
}

/// The client's copy of a stack list, for drawing (`hz_items`).
fn items_tag(host: &dyn Host, items: &[Option<ItemStack>]) -> Tag {
    Tag::List(items.iter().map(|s| Tag::Compound(s.as_ref().and_then(|s| s.to_nbt(host)).unwrap_or_default())).collect())
}

/// A slot holds one (`stackSizeLimit` 1): what got past that goes out on top.
fn limit_one(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, e: &mut Fields) {
    if e.items.len() < 2 {
        e.items.resize(2, None);
    }
    for slot in 0..2 {
        if let Some(s) = e.items[slot].clone().filter(|s| s.count() > 1) {
            e.items[slot] = with_count(&s, 1);
            if let Some(x) = with_count(&s, s.count() - 1) {
                let _ = level.spawn_item(host, f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.0, f64::from(pos[2]) + 0.5, x, 10);
            }
        }
    }
}

// ---- The casting table and basin: CastingFluidHandler and the block entity's state

/// The casting tank (`tank`: {capacity, fluid {FluidName, Amount}, filter}).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CastTank {
    pub fluid: Option<String>,
    pub amount: i32,
    /// The recipe's amount, 0 before the first fill.
    pub capacity: i32,
    /// The fluid the first fill set (`isFluidValid`).
    pub filter: Option<String>,
}

pub(crate) fn cast_tank(nbt: &Compound) -> CastTank {
    let Some(t) = gc(nbt, "tank") else { return CastTank::default() };
    let (fluid, amount) = read_fluid(gc(t, "fluid")).map_or((None, 0), |(f, n)| (Some(f), n));
    CastTank { fluid, amount, capacity: gi(t, "capacity", 0).max(0), filter: gs(t, "filter").filter(|f| !f.is_empty()) }
}

fn cast_tank_tag(t: &CastTank) -> Tag {
    let mut c = Compound::new();
    si(&mut c, "capacity", t.capacity);
    c.insert(js("fluid"), Tag::Compound(fluid_nbt(t.fluid.as_deref(), t.amount)));
    ss(&mut c, "filter", t.filter.as_deref().unwrap_or(""));
    Tag::Compound(c)
}

fn set_cast_tank(nbt: &mut Compound, t: &CastTank) {
    let tag = cast_tank_tag(t);
    if nbt.get(&js("tank")) != Some(&tag) {
        nbt.insert(js("tank"), tag);
    }
}

/// CastingBlockEntity.reset (and the handler's): no recipe, no fluid.
fn cast_reset(nbt: &mut Compound) {
    set_cast_tank(nbt, &CastTank::default());
    ss(nbt, "recipe", "");
    si(nbt, "timer", 0);
    si(nbt, "hz_cooling", -1);
}

/// CastingFluidHandler on every side.
pub(crate) struct CastingFluids {
    entity: BlockEntityRef,
    basin: bool,
    require_cast: bool,
}

impl CastingFluids {
    fn data(&self, host: &dyn Host) -> Compound {
        self.entity.data(host).unwrap_or_default()
    }
}

impl ResourceHandler<FluidResource> for CastingFluids {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<FluidResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        let t = cast_tank(&self.data(&*host));
        if index != 0 || t.amount <= 0 {
            return None;
        }
        fluid_res(&*host, t.fluid.as_deref()?, t.amount as u32)
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        if index != 0 {
            return 0;
        }
        cast_tank(&self.data(&*host)).amount.max(0) as u32
    }
    /// getTankCapacity: the recipe's amount, or what is in it before a recipe.
    fn capacity(&self, host: &mut dyn ServerHost, _index: usize, _resource: Option<&FluidResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        let t = cast_tank(&self.data(&*host));
        (if t.capacity == 0 { t.amount } else { t.capacity }).max(0) as u32
    }
    /// CastingFluidHandler.fill with CastingBlockEntity.initNewCasting on the first fill.
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if index != 0 || amount == 0 || resource.to_stack(1).has_components() {
                return 0;
            }
            let Some(fluid) = fluid_id(&*host, resource) else { return 0 };
            let data = self.data(&*host);
            let mut t = cast_tank(&data);
            if t.filter.as_deref().is_some_and(|f| f != fluid) {
                return 0;
            }
            let mut moved = false;
            let mut recipe = None;
            let cap = if t.capacity == 0 {
                if !gs(&data, "recipe").unwrap_or_default().is_empty() {
                    return 0;
                }
                let [slot0, slot1] = slots_of(&*host, self.entity);
                if slot0.is_some() && slot1.is_some() {
                    return 0;
                }
                let found = if slot1.is_none() {
                    if slot0.is_none() && self.require_cast {
                        return 0;
                    }
                    find_casting(host, self.basin, &slot0, &fluid)
                } else {
                    // The previous output acts as the cast.
                    moved = true;
                    find_casting(host, self.basin, &slot1, &fluid)
                };
                let Some((id, cap)) = found.filter(|(_, c)| *c > 0) else { return 0 };
                recipe = Some(id);
                cap
            } else {
                t.capacity
            };
            let want = amount.min(i32::MAX as u32) as i32;
            let filled = match &t.fluid {
                None => want.min(cap),
                Some(f) if *f == fluid => want.min(cap - t.amount).max(0),
                Some(_) => 0,
            };
            if filled <= 0 {
                return 0;
            }
            let mut c = Compound::new();
            if let Some(id) = &recipe {
                ss(&mut c, "recipe", id);
                t.capacity = cap;
                t.filter = Some(fluid.clone());
                if moved {
                    let [_, slot1] = slots_of(&*host, self.entity);
                    if !(self.entity.set_item_slot(host, tx, 0, slot1).unwrap_or(false) && self.entity.set_item_slot(host, tx, 1, None).unwrap_or(false)) {
                        return 0;
                    }
                }
            }
            t.amount += filled;
            t.fluid = Some(fluid);
            c.insert(js("tank"), cast_tank_tag(&t));
            if matches!(self.entity.merge_data(host, c), Ok(true)) { filled as u32 } else { 0 }
        })())
    }
    /// CastingFluidHandler.drain: emptied, the casting resets.
    fn extract_at(&self, host: &mut dyn ServerHost, _tx: &TransactionContext, index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if index != 0 || amount == 0 {
                return 0;
            }
            let Some(fluid) = fluid_id(&*host, resource) else { return 0 };
            let mut t = cast_tank(&self.data(&*host));
            if t.fluid.as_deref() != Some(fluid.as_str()) {
                return 0;
            }
            let n = (amount.min(i32::MAX as u32) as i32).min(t.amount);
            if n <= 0 {
                return 0;
            }
            let mut c = Compound::new();
            if t.amount - n <= 0 {
                cast_reset(&mut c);
            } else {
                t.amount -= n;
                c.insert(js("tank"), cast_tank_tag(&t));
            }
            if matches!(self.entity.merge_data(host, c), Ok(true)) { n as u32 } else { 0 }
        })())
    }
}

pub(crate) fn casting_fluids(level: &dyn LevelHost, at: BlockSide, state: BlockState, entity: BlockEntityRef, offer: &mut BlockCapabilityOffer<'_, dyn ResourceHandler<FluidResource>>) {
    let _ = at;
    let block = view_block_id(level, state);
    if !is_casting(&block) {
        return;
    }
    offer.offer(|| Box::new(CastingFluids { entity, basin: is_basin(&block), require_cast: block.starts_with("tconstruct:scorched_") }));
}

/// The table's and basin's slots through SidedInvWrapper: a cast into slot 0 while the tank is empty and slot 1 is
/// free; the result out of slot 1 while the tank is empty.
pub(crate) struct CastingItems {
    entity: BlockEntityRef,
    tank_empty: bool,
    /// The casting tank's rule instead (slot 0 only with slot 1 free and an item that holds fluid).
    casting_tank: bool,
}

impl ResourceHandler<ItemResource> for CastingItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        2
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
            if index != 0 || amount == 0 {
                return 0;
            }
            let [slot0, slot1] = slots_of(&*host, self.entity);
            if slot0.is_some() || slot1.is_some() {
                return 0;
            }
            if self.casting_tank {
                if !containers::is_container(host, resource.as_stack()) {
                    return 0;
                }
            } else if !self.tank_empty {
                return 0;
            }
            let Some(one) = resource.to_stack(1) else { return 0 };
            if self.entity.set_item_slot(host, tx, 0, Some(one)).unwrap_or(false) { 1 } else { 0 }
        })())
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if index != 1 || amount == 0 || !(self.casting_tank || self.tank_empty) {
                return 0;
            }
            let Some(s) = self.entity.item_slot(&*host, 1).ok().flatten() else { return 0 };
            if !resource.matches(&s) {
                return 0;
            }
            let n = s.count().min(amount);
            let rest = s.clone().shrink(n);
            if self.entity.set_item_slot(host, tx, 1, rest).unwrap_or(false) { n } else { 0 }
        })())
    }
}

pub(crate) fn casting_items(level: &dyn LevelHost, at: BlockSide, state: BlockState, entity: BlockEntityRef, offer: &mut BlockCapabilityOffer<'_, dyn ResourceHandler<ItemResource>>) {
    let _ = (at, state);
    let tank_empty = cast_tank(&level.block_entity_data(entity).unwrap_or_default()).amount <= 0;
    offer.offer(|| Box::new(CastingItems { entity, tank_empty, casting_tank: false }));
}

pub(crate) fn casting_tank_items(level: &dyn LevelHost, at: BlockSide, state: BlockState, entity: BlockEntityRef, offer: &mut BlockCapabilityOffer<'_, dyn ResourceHandler<ItemResource>>) {
    let _ = (level, at, state);
    offer.offer(|| Box::new(CastingItems { entity, tank_empty: true, casting_tank: true }));
}

/// CastingBlockEntity.getAnalogSignal.
fn casting_signal(t: &CastTank, cooling: i32, timer: i32, items: &[Option<ItemStack>]) -> i32 {
    if items.get(1).is_some_and(Option::is_some) {
        15
    } else if cooling > 0 {
        11 + timer * 4 / cooling
    } else if t.capacity > 0 {
        2 + t.amount * 9 / t.capacity
    } else if items.first().is_some_and(Option::is_some) {
        1
    } else {
        0
    }
}

/// CastingBlockEntity's server tick (serverTick, swap, onContentsChanged's cooling time), `has_item`, the client's copy
/// and the comparator.
fn table_tick(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, block: &str, e: &mut Fields) {
    limit_one(host, level, pos, e);
    let k = key(&*host, pos);
    let basin = is_basin(block);
    let redstone = redstone_of(&k, gi(&e.nbt, "redstone", 0) != 0);
    si(&mut e.nbt, "redstone", i32::from(redstone));
    let all = { let _t = crate::perf::t(crate::perf::CAST_RECIPES); recipes::recipes(&*host) };
    let mut id = gs(&e.nbt, "recipe").unwrap_or_default();
    // swap: 2 ticks after a rising edge, with no casting going on.
    if with_rt(|r| r.activate.remove(&k)) && id.is_empty() {
        e.items.swap(0, 1);
        play(host, level, pos, "tconstruct:casting_clicks", 1.0, 1.0);
    }
    let t = cast_tank(&e.nbt);
    // onContentsChanged: full with a recipe, its cooling time (for the cast there now); else -1.
    let view = cast_view(&all, basin, &e.items[0]);
    let recipe = all.by_id(basin, &id).cloned();
    let cooling = match (&t.fluid, &recipe) {
        (Some(f), Some(r)) if t.capacity > 0 && t.amount >= t.capacity => r.cooling_time(host, &view, f).max(0),
        _ => -1,
    };
    let mut timer = gi(&e.nbt, "timer", 0);
    if t.fluid.is_some() && !id.is_empty() && recipe.is_none() {
        // A recipe gone (a reload): the fluid stays and can be drained, nothing is made (loadRecipe finds none).
    }
    let mut done = false;
    if let (Some(current), true) = (recipe, cooling >= 0) {
        timer += 1;
        if timer >= cooling {
            let fluid = t.fluid.clone().unwrap_or_default();
            // The recipe still takes the cast and the fluid, else another one does with what is there.
            let r = if current.amount(host, &view, &fluid).is_some() {
                Some(current)
            } else {
                let mut found = None;
                for r in all.list(basin) {
                    if r.amount(host, &view, &fluid).is_some() {
                        found = Some(r.clone());
                        break;
                    }
                }
                match found {
                    Some(r) if r.amount(host, &view, &fluid).is_some_and(|a| a <= t.amount) => {
                        id = r.id.clone();
                        ss(&mut e.nbt, "recipe", &id);
                        Some(r)
                    }
                    _ => None,
                }
            };
            match r.and_then(|r| r.assemble(host, &view, &fluid).map(|out| (r, out))) {
                Some((r, out)) => {
                    if r.switch != redstone {
                        if !r.consumed {
                            e.items[1] = e.items[0].clone();
                        }
                        e.items[0] = Some(out);
                    } else {
                        if r.consumed {
                            e.items[0] = None;
                        }
                        e.items[1] = Some(out);
                    }
                    if redstone {
                        play(host, level, pos, "tconstruct:casting_clicks", 1.0, 1.0);
                    }
                    play(host, level, pos, "tconstruct:casting_cools", 0.5, 4.0);
                    cast_reset(&mut e.nbt);
                    done = true;
                }
                None => timer = 0,
            }
        }
    }
    if !done {
        si(&mut e.nbt, "timer", if cooling >= 0 { timer } else { 0 });
        si(&mut e.nbt, "hz_cooling", cooling);
        // The client counts the cooling from the game time it started (CastingBlockEntity.clientTick's timer).
        if cooling >= 0 {
            let start = level.game_time(&*host).unwrap_or(0) - i64::from(timer);
            if (gi(&e.nbt, "hz_start", 0) as i64 - start).abs() > 1 || gi(&e.nbt, "hz_start", 0) == 0 {
                si(&mut e.nbt, "hz_start", start as i32);
            }
        }
    }
    // TableBlock's `has_item`.
    let has = if e.items.iter().any(Option::is_some) { "true" } else { "false" };
    if level.block_property(&*host, pos, "has_item").is_some_and(|v| v != has) {
        host.set_block_property_with(pos, "has_item", has, 3);
    }
    table_visuals(host, level, pos, basin, e);
}

/// The client's copy (`hz_items`, the cooling result `hz_preview`) sent again when it changes; the comparator.
fn table_visuals(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, basin: bool, e: &mut Fields) {
    let items_now = items_tag(&*host, &e.items);
    if e.nbt.get(&js("hz_items")) != Some(&items_now) {
        e.nbt.insert(js("hz_items"), items_now);
    }
    // CastingBlockEntity.getRecipeOutput: what the cooling makes, worked out once per recipe, fluid and cast.
    let t = cast_tank(&e.nbt);
    let id = gs(&e.nbt, "recipe").unwrap_or_default();
    let pkey = format!("{id}|{:?}|{}", t.fluid, e.items[0].as_ref().map_or("", |s| sid(s)));
    if gs(&e.nbt, "hz_pkey").as_deref() != Some(pkey.as_str()) {
        ss(&mut e.nbt, "hz_pkey", &pkey);
        let all = recipes::recipes(&*host);
        let view = cast_view(&all, basin, &e.items[0]);
        let preview = match (all.by_id(basin, &id).cloned(), &t.fluid) {
            (Some(r), Some(f)) => r.assemble(host, &view, f).and_then(|p| p.to_nbt(&*host)),
            _ => None,
        };
        match preview {
            Some(p) => {
                e.nbt.insert(js("hz_preview"), Tag::Compound(p));
            }
            None => rm(&mut e.nbt, "hz_preview"),
        }
    }
    let cooling = gi(&e.nbt, "hz_cooling", -1);
    let timer = gi(&e.nbt, "timer", 0);
    let vis = format!("{:?}{:?}{}/{}c{cooling}{}", e.nbt.get(&js("hz_items")), t.fluid, t.amount, t.capacity, gs(&e.nbt, "hz_pkey").unwrap_or_default());
    if gs(&e.nbt, "hz_vis").as_deref() != Some(vis.as_str()) {
        ss(&mut e.nbt, "hz_vis", &vis);
        host.block_entity_sync(pos);
    }
    let strength = casting_signal(&t, cooling, timer, &e.items);
    let k = key(&*host, pos);
    if with_rt(|r| r.signals.insert(k, strength) != Some(strength))
        && let Some(state) = level.block_state(&*host, pos)
    {
        let _ = level.update_neighbors(host, pos, state, NeighbourUpdate::OutputSignal);
    }
}

/// CastingBlockEntity.interact (AbstractCastingBlock.use): a fluid container first; with an empty tank, molding,
/// taking out or putting one in. Not while a casting cools.
fn table_use(host: &mut dyn ServerHost, click: &BlockClick<'_>, level: LevelRef, block: &str) -> Option<InteractionResult> {
    let pos = click.pos;
    let entity = host.block_entity(level, pos)?;
    let basin = is_basin(block);
    let data = entity.data(&*host).unwrap_or_default();
    let (cooling, timer) = (gi(&data, "hz_cooling", -1), gi(&data, "timer", 0));
    if !(cooling < 0 || timer <= 0) {
        return Some(InteractionResult::Success);
    }
    let held = click.stack.cloned().filter(|s| s.count() > 0);
    if held.is_some()
        && let Some(cap) = cap_at(host, level, pos, None)
        && containers::interact_with_container(host, click, level, &mut { cap })
    {
        return Some(InteractionResult::Success);
    }
    if cast_tank(&entity.data(&*host).unwrap_or_default()).amount > 0 {
        return Some(InteractionResult::Success);
    }
    let hand = hand_slot(&*host, click.player, click.hand);
    let [input, output] = slots_of(&*host, entity);
    // Molding: slot 0 the material, the held item the pattern.
    if let (Some(material), None) = (&input, &output) {
        if let Some(m) = find_molding(&*host, basin, material, held.as_ref()) {
            let Some(result) = new_stack(&m.result.0, m.result.1) else { return Some(InteractionResult::Success) };
            match &held {
                None => {
                    if set_slot(host, entity, 0, None) {
                        let res = ItemResource::of(&result);
                        let n = result.count();
                        let _ = host.transaction(|host, tx| Ok::<u32, ()>(hand.insert(host, tx, &res, n).unwrap_or(0)));
                    }
                }
                Some(h) => {
                    // The result in, the pattern kept (unless consumed) and one item off the hand in one step: the
                    // table gets nothing unless the hand loses that item.
                    let pattern = (!m.pattern_consumed).then(|| with_count(h, 1)).flatten();
                    let res = ItemResource::of(h);
                    let _ = host.transaction(|host, tx| {
                        let placed = entity.set_item_slot(host, tx, 0, Some(result.clone())).unwrap_or(false)
                            && (pattern.is_none() || entity.set_item_slot(host, tx, 1, pattern.clone()).unwrap_or(false));
                        if placed && hand.extract(host, tx, &res, 1) == Ok(1) { Ok(()) } else { Err(()) }
                    });
                }
            }
            return Some(InteractionResult::Success);
        }
        if held.is_some()
            && let Some(m) = find_molding(&*host, basin, material, None)
        {
            if set_slot(host, entity, 0, None)
                && let Some(result) = new_stack(&m.result.0, m.result.1)
            {
                give(host, click.player, result);
            }
            return Some(InteractionResult::Success);
        }
    }
    if input.is_some() || output.is_some() {
        let slot = if output.is_none() { 0 } else { 1 };
        let stack = if slot == 0 { input } else { output };
        if let Some(s) = stack
            && set_slot(host, entity, slot, None)
        {
            give(host, click.player, s);
        }
    } else if let Some(h) = &held {
        place_from_hand(host, entity, &hand, h);
    }
    Some(InteractionResult::Success)
}

// ---- The faucet

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Faucet {
    pub state: i32,
    pub stop: bool,
    pub drained: Option<(String, i32)>,
    pub render: Option<String>,
    pub last_redstone: bool,
}

pub(crate) fn read_faucet(nbt: &Compound) -> Faucet {
    Faucet {
        state: gi(nbt, "state", OFF),
        stop: gi(nbt, "stop", 0) != 0,
        drained: read_fluid(gc(nbt, "drained")),
        render: read_fluid(gc(nbt, "render_fluid")).map(|(f, _)| f),
        last_redstone: gi(nbt, "lastRedstone", 0) != 0,
    }
}

fn write_faucet(nbt: &mut Compound, f: &Faucet) {
    si(nbt, "state", f.state);
    si(nbt, "stop", i32::from(f.stop));
    si(nbt, "lastRedstone", i32::from(f.last_redstone));
    let d = Tag::Compound(fluid_nbt(f.drained.as_ref().map(|(x, _)| x.as_str()), f.drained.as_ref().map_or(0, |(_, n)| *n)));
    if nbt.get(&js("drained")) != Some(&d) {
        nbt.insert(js("drained"), d);
    }
    let r = Tag::Compound(fluid_nbt(f.render.as_deref(), i32::from(f.render.is_some())));
    if nbt.get(&js("render_fluid")) != Some(&r) {
        nbt.insert(js("render_fluid"), r);
    }
}

/// The faucet's input (behind it: `facing.opposite`, reached on its `facing` side; facing down, above) and output
/// (below, reached on its top).
fn faucet_facing(host: &dyn Host, level: LevelRef, pos: Pos) -> usize {
    level.block_property(host, pos, "facing").and_then(|f| dir_index(&f)).unwrap_or(2)
}

fn faucet_reset(f: &mut Faucet) {
    f.stop = false;
    f.drained = None;
    f.state = OFF;
    f.render = None;
}

/// FaucetBlockEntity.pour: up to 10 mB of the packet into the block below; with nothing there the packet is lost.
fn faucet_pour(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, f: &mut Faucet) {
    let Some((fluid, amount)) = f.drained.clone() else { return };
    let Some(mut out) = cap_at(host, level, below(pos), Some(Direction::Up)) else {
        faucet_reset(f);
        return;
    };
    let n = out.fill(host, &fluid, amount.min(MB_PER_TICK), false);
    if n > 0 {
        f.render = Some(fluid.clone());
        let done = out.fill(host, &fluid, n, true);
        let left = amount - done;
        f.drained = (left > 0).then_some((fluid, left));
    }
}

/// FaucetBlockEntity.doTransfer: a packet of up to 90 mB that the block below takes (it must take a 10 mB pour),
/// poured from now.
fn faucet_transfer(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, f: &mut Faucet, execute: bool) -> bool {
    let facing = faucet_facing(&*host, level, pos);
    let input = step(pos, opposite(facing));
    let inp = cap_at(host, level, input, Some(DIRS[facing]));
    let out = cap_at(host, level, below(pos), Some(Direction::Up));
    if let (Some(mut inp), Some(mut out)) = (inp, out) {
        if let Some((fluid, d)) = peek_drain(host, &mut inp, FAUCET_PACKET) {
            let filled = out.fill(host, &fluid, d, false);
            if filled > 0 && (filled <= MB_PER_TICK || out.fill(host, &fluid, MB_PER_TICK, false) > 0) {
                if execute {
                    let n = inp.drain(host, &fluid, filled, true);
                    f.render = Some(fluid.clone());
                    f.drained = (n > 0).then_some((fluid, n));
                    f.state = POURING;
                    faucet_pour(host, level, pos, f);
                }
                return true;
            }
        }
        if f.last_redstone {
            if execute {
                f.render = None;
            }
            f.state = POWERED;
            return false;
        }
    }
    if execute {
        faucet_reset(f);
    }
    false
}

/// FaucetBlockEntity.activate (a click, or 2 ticks after a rising redstone edge).
fn faucet_activate(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, f: &mut Faucet) {
    match f.state {
        OFF => {
            f.stop = false;
            faucet_transfer(host, level, pos, f, true);
        }
        POURING => f.stop = true,
        _ => {
            f.state = OFF;
            f.render = None;
        }
    }
}

fn faucet_tick(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, e: &mut Fields) {
    let k = key(&*host, pos);
    let mut f = read_faucet(&e.nbt);
    f.last_redstone = redstone_of(&k, f.last_redstone);
    let (activate, falling) = with_rt(|r| (r.activate.remove(&k), r.falling.remove(&k)));
    // handleRedstone: a falling edge ends waiting.
    if falling && f.state == POWERED {
        f.state = OFF;
        f.render = None;
    }
    if activate {
        faucet_activate(host, level, pos, &mut f);
    }
    // FaucetBlockEntity.tick.
    if f.state != OFF {
        if f.state == POWERED && faucet_transfer(host, level, pos, &mut f, false) {
            f.state = POURING;
        } else if f.drained.is_some() {
            faucet_pour(host, level, pos, &mut f);
        } else if f.stop {
            faucet_reset(&mut f);
        } else {
            faucet_transfer(host, level, pos, &mut f, true);
        }
    }
    write_faucet(&mut e.nbt, &f);
    // FaucetActivationPacket: the clients know pouring or not and the fluid.
    let vis = format!("{}{:?}", f.state == POURING || f.state == POWERED, f.render);
    if gs(&e.nbt, "hz_vis").as_deref() != Some(vis.as_str()) {
        ss(&mut e.nbt, "hz_vis", &vis);
        host.block_entity_sync(pos);
    }
}

/// FaucetBlock.use: sneaking passes; else the faucet activates at once.
fn faucet_use(host: &mut dyn ServerHost, click: &BlockClick<'_>, level: LevelRef) -> Option<InteractionResult> {
    let pos = click.pos;
    let entity = host.block_entity(level, pos)?;
    let k = key(&*host, pos);
    let mut f = read_faucet(&entity.data(&*host).unwrap_or_default());
    f.last_redstone = redstone_of(&k, f.last_redstone);
    faucet_activate(host, level, pos, &mut f);
    let mut c = Compound::new();
    write_faucet(&mut c, &f);
    let _ = entity.merge_data(host, c);
    Some(InteractionResult::Success)
}

// ---- The channel

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Channel {
    pub fluid: Option<String>,
    pub amount: i32,
    pub locked: i32,
    /// isFlowing: down, north, south, west, east (2 just flowed, counting down each tick).
    pub flow: [i32; 5],
}

pub(crate) fn read_channel(nbt: &Compound) -> Channel {
    let t = gc(nbt, "tank");
    let (fluid, amount) = read_fluid(t).map_or((None, 0), |(f, n)| (Some(f), n));
    let flow = gia(nbt, "is_flowing");
    Channel { fluid, amount, locked: t.map_or(0, |t| gi(t, "locked", 0)).max(0), flow: std::array::from_fn(|i| flow.get(i).copied().unwrap_or(0).clamp(0, 2)) }
}

fn channel_tags(c: &Channel) -> (Tag, Tag) {
    let mut t = fluid_nbt(c.fluid.as_deref(), c.amount);
    si(&mut t, "locked", c.locked);
    (Tag::Compound(t), Tag::IntArray(c.flow.to_vec().into()))
}

fn write_channel(nbt: &mut Compound, c: &Channel) {
    let (t, f) = channel_tags(c);
    if nbt.get(&js("tank")) != Some(&t) {
        nbt.insert(js("tank"), t);
    }
    if nbt.get(&js("is_flowing")) != Some(&f) {
        nbt.insert(js("is_flowing"), f);
    }
}

/// The flow flag of a side (down 0, north 1, south 2, west 3, east 4).
pub(crate) fn flow_index(side: usize) -> usize {
    if side == 0 { 0 } else { side - 1 }
}

/// The channel's handlers: FillOnlyFluidHandler on top (and for no side), ChannelSideTank on an `in` side (it marks the
/// side flowing), EmptyFluidHandler on an `out` side.
pub(crate) struct ChannelFluids {
    entity: BlockEntityRef,
    /// The side's flow flag to set on a fill (an `in` side); none on top.
    flow: Option<usize>,
    /// An `out` side: there, but takes nothing.
    empty: bool,
}

impl ResourceHandler<FluidResource> for ChannelFluids {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, _index: usize) -> Option<FluidResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        if self.empty {
            return None;
        }
        let c = read_channel(&self.entity.data(&*host)?);
        fluid_res(&*host, c.fluid.as_deref()?, c.amount.max(1) as u32)
    }
    fn amount(&self, host: &mut dyn ServerHost, _index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        if self.empty {
            return 0;
        }
        self.entity.data(&*host).map_or(0, |d| read_channel(&d).amount.max(0) as u32)
    }
    fn capacity(&self, _host: &mut dyn ServerHost, _index: usize, _resource: Option<&FluidResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        if self.empty { 0 } else { CHANNEL_CAPACITY as u32 }
    }
    fn insert_at(&self, host: &mut dyn ServerHost, _tx: &TransactionContext, _index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if self.empty || amount == 0 || resource.to_stack(1).has_components() {
                return 0;
            }
            let Some(fluid) = fluid_id(&*host, resource) else { return 0 };
            let Some(data) = self.entity.data(&*host) else { return 0 };
            let mut c = read_channel(&data);
            let want = amount.min(i32::MAX as u32) as i32;
            let n = match &c.fluid {
                None => want.min(CHANNEL_CAPACITY),
                Some(f) if *f == fluid => want.min(CHANNEL_CAPACITY - c.amount).max(0),
                Some(_) => 0,
            };
            if n <= 0 {
                return 0;
            }
            c.fluid = Some(fluid);
            c.amount += n;
            c.locked += n;
            if let Some(i) = self.flow {
                c.flow[i] = 2;
            }
            let mut out = Compound::new();
            let (t, f) = channel_tags(&c);
            out.insert(js("tank"), t);
            out.insert(js("is_flowing"), f);
            if matches!(self.entity.merge_data(host, out), Ok(true)) { n as u32 } else { 0 }
        })())
    }
    /// Nothing drains a channel.
    fn extract_at(&self, _host: &mut dyn ServerHost, _tx: &TransactionContext, _index: usize, _resource: &FluidResource, _amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            0
        })())
    }
}

pub(crate) fn channel_fluids(level: &dyn LevelHost, at: BlockSide, state: BlockState, entity: BlockEntityRef, offer: &mut BlockCapabilityOffer<'_, dyn ResourceHandler<FluidResource>>) {
    let (flow, empty) = match at.side.map(dir_of) {
        None | Some(1) => (None, false),
        Some(0) => return,
        Some(s) => match view_property(level, state, DIR_NAMES[s]).as_deref() {
            Some("in") => (Some(flow_index(s)), false),
            Some("out") => (None, true),
            _ => return,
        },
    };
    offer.offer(|| Box::new(ChannelFluids { entity, flow, empty }));
}

/// ChannelBlock.canConnect: the same channel there, or a block with a fluid handler on the side facing this one.
fn channel_can_connect(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, side: usize) -> bool {
    let there = step(pos, side);
    block_id(&*host, level, there) == block_id(&*host, level, pos) || level.capability(host, &FLUIDS, there, Some(DIRS[opposite(side)])).is_some()
}

fn channel_side(host: &dyn Host, level: LevelRef, pos: Pos, side: usize) -> String {
    level.block_property(host, pos, DIR_NAMES[side]).unwrap_or_else(|| "none".into())
}

/// ChannelBlockEntity.trySide: up to `rate` of what may leave into the neighbour on `side`.
fn channel_try_side(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, c: &mut Channel, side: usize, rate: i32) -> bool {
    let Some(fluid) = c.fluid.clone().filter(|_| c.amount > 0) else { return false };
    let output = if side == 0 { level.block_property(&*host, pos, "down").as_deref() == Some("true") } else { channel_side(&*host, level, pos, side) == "out" };
    if !output {
        return false;
    }
    let Some(mut target) = cap_at(host, level, step(pos, side), Some(DIRS[opposite(side)])) else { return false };
    let usable = (c.amount - c.locked).max(0).min(rate);
    if usable > 0 {
        let f = target.fill(host, &fluid, usable, false);
        if f > 0 {
            let done = target.fill(host, &fluid, f, true);
            c.amount -= done;
            if c.amount <= 0 {
                c.fluid = None;
                c.amount = 0;
            }
            c.flow[flow_index(side)] = 2;
            return true;
        }
    }
    c.flow[flow_index(side)] = 0;
    false
}

fn channel_tick(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, e: &mut Fields) {
    let k = key(&*host, pos);
    // getStateForPlacement's `powered` (the placement hook can't read redstone), once.
    if gi(&e.nbt, "hz_init", 0) == 0 {
        si(&mut e.nbt, "hz_init", 1);
        let p = level.has_neighbor_signal(&*host, pos);
        if (level.block_property(&*host, pos, "powered").as_deref() == Some("true")) != p {
            with_rt(|r| r.channel_power.insert(k.clone(), p));
        }
    }
    // ChannelBlock.neighborChanged: redstone opens and closes the way down (by level).
    if let Some(p) = with_rt(|r| r.channel_power.remove(&k)) {
        let down = p && channel_can_connect(host, level, pos, 0);
        host.set_block_property_with(pos, "powered", if p { "true" } else { "false" }, 2);
        host.set_block_property_with(pos, "down", if down { "true" } else { "false" }, 2);
    }
    let mut c = read_channel(&e.nbt);
    if c.amount > 0 && c.fluid.is_some() {
        let flown = level.block_property(&*host, pos, "down").as_deref() == Some("true") && channel_try_side(host, level, pos, &mut c, 0, MB_PER_TICK);
        let outs = CHANNEL_SIDES.iter().filter(|&&s| channel_side(&*host, level, pos, s) == "out").count() as i32;
        if !flown && outs > 0 {
            let rate = ((c.amount - c.locked).max(0) / outs).clamp(1, MB_PER_TICK);
            for side in CHANNEL_SIDES {
                channel_try_side(host, level, pos, &mut c, side, rate);
            }
        }
    }
    for v in &mut c.flow {
        if *v > 0 {
            *v -= 1;
        }
    }
    c.locked = 0;
    write_channel(&mut e.nbt, &c);
    // FluidUpdatePacket on empty and not, ChannelFlowPacket on each side's change.
    let vis = format!("{:?}{:?}", c.fluid, c.flow.map(|v| v > 0));
    if gs(&e.nbt, "hz_vis").as_deref() != Some(vis.as_str()) {
        ss(&mut e.nbt, "hz_vis", &vis);
        host.block_entity_sync(pos);
    }
}

/// ChannelBlock.use: a gauge or another channel against an open neighbour passes; else the way down or a side's flow
/// (none, in, out), with the action-bar line.
fn channel_use(host: &mut dyn ServerHost, click: &BlockClick<'_>, level: LevelRef, block: &str) -> Option<InteractionResult> {
    let pos = click.pos;
    let hit_face = dir_of_normal(click.hit.face)?;
    let offset = step(pos, hit_face);
    if replaceable(&*host, level, offset) {
        if let Some(s) = click.stack.filter(|s| s.count() > 0) {
            let id = sid(s);
            if id == block {
                return None;
            }
            if hit_face != 0 && is_attached_gauge(&*host, id) {
                if hit_face != 1 && channel_side(&*host, level, pos, hit_face) == "none" {
                    host.set_block_property_with(pos, DIR_NAMES[hit_face], "in", 3);
                }
                return None;
            }
        }
    }
    let sneak = sneaking(&*host, click.player);
    let mut side = if hit_face == 1 { 0 } else { hit_face };
    if sneak && side != 0 {
        side = opposite(side);
    }
    let (x, z) = (click.hit.point[0] - f64::from(pos[0]), click.hit.point[2] - f64::from(pos[2]));
    if z < 0.25 {
        side = 2;
    } else if z > 0.75 {
        side = 3;
    } else if x < 0.25 {
        side = 4;
    } else if x > 0.75 {
        side = 5;
    }
    let player = host.player(click.player)?;
    let line = if side == 0 {
        let down = level.block_property(&*host, pos, "down").as_deref() == Some("true");
        if !down && channel_can_connect(host, level, pos, 0) {
            host.set_block_property_with(pos, "down", "true", 3);
            ("block.tconstruct.channel.down.out", "Allowed flowing down")
        } else if down {
            host.set_block_property_with(pos, "down", "false", 3);
            ("block.tconstruct.channel.down.none", "Disallowed flowing down")
        } else {
            return None;
        }
    } else {
        let next = |v: &str| match (v, sneak) {
            ("none", false) | ("out", true) => "in",
            ("in", false) | ("none", true) => "out",
            _ => "none",
        };
        let target = step(pos, side);
        let same = block_id(&*host, level, target) == block;
        let mut n = next(&channel_side(&*host, level, pos, side));
        if n == "out" && !same && level.capability(host, &FLUIDS, target, Some(DIRS[opposite(side)])).is_none() {
            n = next(n);
        }
        host.set_block_property_with(pos, DIR_NAMES[side], n, 3);
        match n {
            "in" => ("block.tconstruct.channel.side.in", "Set side to flow inwards"),
            "out" => ("block.tconstruct.channel.side.out", "Set side to flow outwards"),
            _ => ("block.tconstruct.channel.side.none", "Disallowed flowing on side"),
        }
    };
    let _ = player.action_bar(host, tr(line.0, line.1, Vec::new()));
    Some(InteractionResult::Success)
}

/// BlockState.canBeReplaced, as near as names tell: air, or `#minecraft:replaceable`.
fn replaceable(host: &dyn Host, level: LevelRef, pos: Pos) -> bool {
    let Some(b) = level.block(host, pos) else { return false };
    let id = b.id(host).unwrap_or_default().to_owned();
    is_air(&id) || host.block_in_tag(b, "minecraft:replaceable").unwrap_or(false)
}

/// A block item whose block is in `#mantle:gauges/attached`.
fn is_attached_gauge(host: &dyn Host, item: &str) -> bool {
    host.block_state_id(item).is_some_and(|s| host.block_in_tag(s.block(), "mantle:gauges/attached").unwrap_or(false))
}

/// ChannelBlock.updateShape: the way down closes over air; a side mirrors a neighbouring same channel, and closes
/// toward air.
fn channel_update_shape(level: &mut dyn LevelTicksHost, event: &UpdateShapeEvent) -> Option<BlockState> {
    let me = level.state_block(event.state)?.to_owned();
    let d = dir_of(event.direction);
    let neighbour = level.state_block(event.neighbour).unwrap_or("").to_owned();
    let mut s = event.state;
    if d == 0 {
        if state_prop(&*level, s, "down").as_deref() == Some("true") && is_air(&neighbour) {
            s = level.state_with_property(s, "down", "false")?;
        }
    } else if d != 1 {
        if neighbour == me {
            let theirs = state_prop(&*level, event.neighbour, DIR_NAMES[opposite(d)]).unwrap_or_else(|| "none".into());
            let mirror = match theirs.as_str() {
                "in" => "out",
                "out" => "in",
                _ => "none",
            };
            s = level.state_with_property(s, DIR_NAMES[d], mirror)?;
        } else if state_prop(&*level, s, DIR_NAMES[d]).is_some_and(|v| v != "none") && is_air(&neighbour) {
            s = level.state_with_property(s, DIR_NAMES[d], "none")?;
        }
    }
    (s != event.state).then_some(s)
}

// ---- The casting tank

/// tryToProcessItem: the input emptied into the tank, else filled from it, the result to slot 1 (each time the input
/// or the tank changes); handleRedstone's swap; then the melter's tanks' sync (clients, light, comparator, drops).
fn casting_tank_tick(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, e: &mut Fields) {
    limit_one(host, level, pos, e);
    let k = key(&*host, pos);
    let redstone = redstone_of(&k, gi(&e.nbt, "redstone", 0) != 0);
    si(&mut e.nbt, "redstone", i32::from(redstone));
    if with_rt(|r| r.activate.remove(&k)) {
        // CastingTankBlockEntity.swap: unconditional.
        e.items.swap(0, 1);
        play(host, level, pos, "tconstruct:casting_clicks", 1.0, 1.0);
        rm(&mut e.nbt, "hz_try");
    }
    let tank = tank_of(&e.nbt);
    let attempt = format!("{:?}{:?}{:?}", e.items[0], e.items[1].is_some(), tank);
    if gs(&e.nbt, "hz_try").as_deref() != Some(attempt.as_str()) {
        if let (Some(input), None) = (e.items[0].clone(), &e.items[1]) {
            let mut t = NbtTank { nbt: &mut e.nbt, capacity: CASTING_TANK_CAPACITY };
            if let Some(m) = containers::transfer(host, &input, &mut t, Way::Auto) {
                e.items[0] = None;
                e.items[1] = m.result;
                play(host, level, pos, containers::bucket_sound(&m.fluid, m.filled_item), 1.0, 1.0);
            }
        }
        let tank = tank_of(&e.nbt);
        let attempt = format!("{:?}{:?}{:?}", e.items[0], e.items[1].is_some(), tank);
        ss(&mut e.nbt, "hz_try", &attempt);
    }
    let items_now = items_tag(&*host, &e.items);
    if e.nbt.get(&js("hz_items")) != Some(&items_now) {
        e.nbt.insert(js("hz_items"), items_now);
        host.block_entity_sync(pos);
    }
    melter::sync(host, level, pos, CASTING_TANK, e, CASTING_TANK_CAPACITY);
}

/// CastingTankBlockEntity.interact: below 11 pixels the tank (a fluid container); above, the output, else the input,
/// back to the player, else the held container in.
fn casting_tank_use(host: &mut dyn ServerHost, click: &BlockClick<'_>, level: LevelRef) -> Option<InteractionResult> {
    let pos = click.pos;
    let entity = host.block_entity(level, pos)?;
    let clicked_tank = click.hit.point[1] - f64::from(pos[1]) < 0.6875;
    if clicked_tank {
        if click.stack.is_some_and(|s| s.count() > 0)
            && let Some(cap) = cap_at(host, level, pos, None)
        {
            containers::interact_with_container(host, click, level, &mut { cap });
        }
        return Some(InteractionResult::Success);
    }
    let [input, output] = slots_of(&*host, entity);
    if let Some(o) = output {
        if set_slot(host, entity, 1, None) {
            give(host, click.player, o);
        }
    } else if let Some(i) = input {
        if set_slot(host, entity, 0, None) {
            give(host, click.player, i);
        }
    } else if let Some(h) = click.stack.cloned().filter(|s| s.count() > 0)
        && containers::is_container(host, &h)
    {
        let hand = hand_slot(&*host, click.player, click.hand);
        place_from_hand(host, entity, &hand, &h);
    }
    Some(InteractionResult::Success)
}

// ---- Hooks

/// Each block entity's tick.
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, entity: &mut Fields) {
    ensure_ids(&*host);
    let Some(level) = here(&*host) else { return };
    let block = block_id(&*host, level, pos);
    let mut e = entity.clone();
    if is_faucet(&block) {
        faucet_tick(host, level, pos, &mut e);
    } else if is_casting(&block) {
        table_tick(host, level, pos, &block, &mut e);
    } else if is_channel(&block) {
        channel_tick(host, level, pos, &mut e);
    } else if block == CASTING_TANK {
        casting_tank_tick(host, level, pos, &mut e);
    }
    if e.nbt != entity.nbt {
        entity.nbt = e.nbt;
    }
    if e.items != entity.items {
        entity.items = e.items;
    }
}

/// `block_entity_update_tag`: what the clients draw.
pub(crate) fn update_tag(nbt: &Compound) -> Compound {
    let mut out = Compound::new();
    for k in ["state", "render_fluid", "tank", "is_flowing", "timer", "hz_cooling", "hz_start", "recipe", "hz_items", "hz_preview", "fluids"] {
        if let Some(v) = nbt.get(&js(k)) {
            out.insert(js(k), v.clone());
        }
    }
    out
}

/// Block clicks (server).
pub(crate) fn use_block(host: &mut dyn ServerHost, click: &BlockClick<'_>, with_item: bool) -> Option<InteractionResult> {
    ensure_ids(&*host);
    let player = host.player(click.player)?;
    let level = player.level(&*host)?;
    let block = block_id(&*host, level, click.pos);
    let held = click.stack.is_some_and(|s| s.count() > 0);
    // With an empty hand the click comes as `use_without_item`.
    if with_item && !held {
        return None;
    }
    if is_channel(&block) {
        return channel_use(host, click, level, &block);
    }
    if block == CASTING_TANK {
        return casting_tank_use(host, click, level);
    }
    // FaucetBlock and AbstractCastingBlock: sneaking passes (to place a block on them).
    if sneaking(&*host, click.player) {
        return None;
    }
    if is_faucet(&block) {
        return faucet_use(host, click, level);
    }
    if is_casting(&block) {
        return table_use(host, click, level, &block);
    }
    None
}

/// The client ends its click as used where Java's `use` answers SUCCESS on the client too, so the held item's own use
/// (a bucket placing its fluid, a block placed) doesn't run there.
pub(crate) fn client_click(host: &dyn ClientHost, block: &str, event: &mut RightClickBlockEvent<'_>) {
    if !is_casting_block(block) {
        return;
    }
    let sneak = host.local_player().and_then(|p| p.entity().is_shift_key_down(host)).unwrap_or(false);
    let take = if is_channel(block) {
        // Holding a channel or a gauge against an open side passes (to place it).
        let passes = event.click.stack.is_some_and(|s| sid(s) == block || is_attached_gauge(host, sid(s)));
        !passes
    } else if block == CASTING_TANK {
        true
    } else {
        !sneak
    };
    if take {
        event.cancelled = true;
        *event.cancellation_result = InteractionResult::Success;
    }
}

/// FaucetBlock.getStateForPlacement (the clicked face, up as down), TableBlock's and CastingTankBlock's (facing the
/// placer), ChannelBlock's (joined to what it was placed against).
pub(crate) fn placement(level: &dyn LevelHost, event: &StateForPlacementEvent<'_>) -> Option<Option<BlockState>> {
    let block = level.state_block(event.state)?.to_owned();
    let c = event.context;
    let face = dir_of(c.face);
    if is_faucet(&block) {
        let facing = if face == 1 { 0 } else { face };
        return Some(level.state_with_property(event.state, "facing", DIR_NAMES[facing]));
    }
    if is_casting(&block) || block == CASTING_TANK {
        let y = placement_yaw(&c).rem_euclid(360.0);
        let facing = if !(45.0..315.0).contains(&y) {
            "north"
        } else if y < 135.0 {
            "east"
        } else if y < 225.0 {
            "south"
        } else {
            "west"
        };
        return Some(level.state_with_property(event.state, "facing", facing));
    }
    if is_channel(&block) {
        let p = [c.pos.x, c.pos.y, c.pos.z];
        let name_at = |q: Pos| level.block_state(q).and_then(|s| level.state_block(s).map(str::to_owned)).unwrap_or_default();
        let mut s = event.state;
        if face == 1 {
            let b = name_at(step(p, 0));
            if b == block || holds_fluid(&b) {
                s = level.state_with_property(s, "down", "true")?;
            }
        } else if face >= 2 {
            let toward = opposite(face);
            let clicked = name_at(step(p, toward));
            let value = if clicked == block {
                if placement_sneaking(&c) { "in" } else { "out" }
            } else if holds_fluid(&clicked) {
                "out"
            } else {
                "none"
            };
            s = level.state_with_property(s, DIR_NAMES[toward], value)?;
        }
        return Some(Some(s));
    }
    None
}

/// Whether a block answers `FLUIDS` (ChannelBlock.isFluidHandler at placement): the placement hook can't ask the
/// capability, so Tinkers' own fluid blocks by name (README, gaps: another mod's tank placed against stays `none`
/// until clicked).
fn holds_fluid(block: &str) -> bool {
    super::tank_capacity(block).is_some() || is_casting(block) || is_channel(block)
}

pub(crate) fn update_shape(level: &mut dyn LevelTicksHost, event: &UpdateShapeEvent) -> Option<BlockState> {
    let block = level.state_block(event.state)?.to_owned();
    if is_channel(&block) { channel_update_shape(level, event) } else { None }
}

/// FaucetBlock / AbstractCastingBlock / CastingTankBlock / ChannelBlock.neighborChanged: redstone.
pub(crate) fn neighbor_changed(level: &mut dyn LevelTicksHost, event: &NeighborChangedEvent) {
    let Some(block) = level.state_block(event.state).map(str::to_owned) else { return };
    if !is_casting_block(&block) {
        return;
    }
    let pos = [event.pos.x, event.pos.y, event.pos.z];
    let k = (event.dimension.key().id().to_owned(), pos);
    // Unknown (the neighbours not loaded): nothing changes.
    let Some(signal) = level.has_neighbor_signal(pos) else { return };
    if is_channel(&block) {
        let powered = state_prop(&*level, event.state, "powered").as_deref() == Some("true");
        if signal != powered {
            with_rt(|r| r.channel_power.insert(k, signal));
        }
        return;
    }
    let last = with_rt(|r| r.redstone.get(&k).copied());
    if last == Some(signal) {
        return;
    }
    with_rt(|r| r.redstone.insert(k.clone(), signal));
    // handleRedstone: a rising edge ticks the block 2 ticks later; a falling one stops a waiting faucet.
    if signal && last != Some(true) {
        level.schedule_block_tick(pos, event.state.block(), 2);
    } else if !signal && is_faucet(&block) {
        with_rt(|r| r.falling.insert(k));
    }
}

/// The scheduled block tick (FaucetBlock.tick → activate; AbstractCastingBlock.tick, CastingTankBlock.tick → swap).
pub(crate) fn tick_block(level: &dyn LevelHost, event: &TickBlockEvent) {
    let Some(block) = level.state_block(event.state) else { return };
    if !(is_faucet(block) || is_casting(block) || block == CASTING_TANK) {
        return;
    }
    let k = (event.dimension.key().id().to_owned(), [event.pos.x, event.pos.y, event.pos.z]);
    with_rt(|r| r.activate.insert(k));
}

/// Runtime state of blocks gone, dropped now and then.
pub(crate) fn level_tick(host: &mut dyn ServerHost) {
    let Some(level) = here(&*host) else { return };
    if level.game_time(&*host).unwrap_or(0) % 600 != 0 {
        return;
    }
    let dim = host.dimension().to_owned();
    let keys: Vec<Pos> = with_rt(|r| r.redstone.keys().chain(r.signals.keys()).filter(|(d, _)| *d == dim).map(|(_, p)| *p).collect());
    for p in keys {
        if level.is_loaded(&*host, p) && !is_casting_block(&block_id(&*host, level, p)) {
            let k = (dim.clone(), p);
            with_rt(|r| {
                r.redstone.remove(&k);
                r.signals.remove(&k);
                r.activate.remove(&k);
                r.falling.remove(&k);
                r.channel_power.remove(&k);
            });
        }
    }
}

// ---- Parts

#[mod_api::sunrise_part]
impl BlockPart for CastingPart {
    fn state_for_placement(&self, level: LevelSide<'_>, event: &mut StateForPlacementEvent<'_>) -> Option<Option<BlockState>> {
        let (LevelSide::Server(level) | LevelSide::Client(level)) = level;
        placement(level, event)
    }

    fn update_shape(&self, level: &mut dyn LevelTicksHost, event: &mut UpdateShapeEvent) -> Option<BlockState> {
        update_shape(level, event)
    }

    fn neighbor_changed(&self, level: &mut dyn LevelWriteHost, event: &mut NeighborChangedEvent) {
        neighbor_changed(level, event);
    }

    fn tick_block(&self, level: &mut dyn LevelWriteHost, event: &mut TickBlockEvent) -> Flow {
        tick_block(&*level, event);
        Flow::Continue
    }

    fn has_analog_output(&self, level: &dyn LevelHost, event: &mut HasAnalogOutputEvent) {
        if level.state_block(event.state).is_some_and(|b| is_casting(b) || b == CASTING_TANK) {
            event.output = true;
        }
    }

    fn analog_output(&self, level: &mut dyn LevelRandomHost, event: &mut AnalogOutputEvent) {
        let level: &dyn LevelHost = &*level;
        let key = (event.dimension.key().id().to_owned(), [event.pos.x, event.pos.y, event.pos.z]);
        let block = level.state_block(event.state).unwrap_or("");
        event.output = if block == CASTING_TANK { melter::signal_at(&key) } else { signal_at(&key) };
    }

    fn use_item_on(&self, host: &mut dyn ServerHost, event: &mut UseItemOnEvent<'_>) -> Option<InteractionResult> {
        use_block(host, event.click, true)
    }

    fn use_without_item(&self, host: &mut dyn ServerHost, event: &mut UseWithoutItemEvent<'_>) -> Option<InteractionResult> {
        use_block(host, event.click, false)
    }
}

#[mod_api::sunrise_part]
impl BlockEntityPart for CastingPart {
    fn block_entity_on_load(&self, event: &mut BlockEntityOnLoadEvent<'_>) {
        crate::smeltery::util::pad_on_load(&mut event.subject);
    }

    fn block_entity(&self, _level: LevelSide, _event: &mut BlockEntityEvent<'_>) -> Option<Compound> {
        Some(Compound::new())
    }

    fn block_entity_ticks(&self, _host: &dyn Host, event: &mut BlockEntityTicksEvent<'_>) {
        event.ticks = true;
    }

    fn block_entity_tick(&self, event: &mut BlockEntityTickEvent<'_>) {
        let _t = crate::perf::t(crate::perf::BE_CASTING);
        crate::tick_fields(event, tick);
    }

    fn block_entity_update_tag(&self, event: &mut BlockEntityUpdateTagEvent<'_>) -> Option<Compound> {
        Some(update_tag(&event.entity.nbt))
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        super::casting_render::submit(host, event);
    }

    /// A waiting faucet's dust and a cooling cast's smoke run on the client, for every loaded copy
    /// (`casting_render::particles`).
    fn block_entity_client_ticks(&self, _host: &dyn ClientHost, event: &mut BlockEntityClientTicksEvent<'_>) {
        if super::casting_render::client_ticks(event.block) {
            event.ticks = true;
        }
    }

    fn block_entity_client_tick(&self, event: &mut BlockEntityClientTickEvent<'_>) {
        super::casting_render::client_tick(event);
    }

    /// Hoppers and droppers through the container's faces, as Java's WorldlyContainer rules (the `ITEMS` providers
    /// keep the same ones): CastingBlockEntity.canPlaceItemThroughFace, slot 0 while the tank is empty and slot 1 holds
    /// nothing, and empty itself (InventoryBlockEntity.canPlaceItem under the stack limit of 1); CastingTankBlockEntity's,
    /// slot 0 while both slots are empty (canPlaceItem).
    fn can_place_item(&self, event: &mut CanPlaceItemEvent<'_>) -> Option<bool> {
        let e: &Fields = event.entity;
        let empty = |i: usize| e.items.get(i).is_none_or(Option::is_none);
        match event.entity.id() {
            TABLE_TYPE | BASIN_TYPE => Some(event.slot == 0 && empty(0) && empty(1) && cast_tank(&e.nbt).amount <= 0),
            CASTING_TANK_TYPE => Some(event.slot == 0 && empty(0) && empty(1)),
            _ => None,
        }
    }
}

#[mod_api::sunrise_part]
impl ItemPart for CastingPart {
    fn tooltip(&self, host: &dyn ClientHost, event: &mut TooltipEvent<'_>) {
        ensure_ids(host);
        let id = sid(event.stack);
        if id == CASTING_TANK {
            containers::tooltip(host, event.stack, event.lines);
        } else if is_casting_block(id) {
            // BlockTooltipItem: the `.tooltip` key's lines.
            let key = format!("block.{}.tooltip", id.replacen(':', ".", 1));
            containers::block_tooltip(host, &key, event.lines);
        }
    }
}

/// Fluid moves for `containers` users that need a tank by capability (unused names kept for stage 3's drains).
#[allow(dead_code)]
pub(crate) fn tank_at(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, side: Option<Direction>) -> Option<Box<dyn TankAccess>> {
    cap_at(host, level, pos, side).map(|c| Box::new(c) as Box<dyn TankAccess>)
}
