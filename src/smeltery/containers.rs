//! Fluid containers (Mantle's FluidTransferHelper and the items' fluid handlers):
//! - `ITEM_FLUIDS` for Tinkers' buckets (FluidBucketWrapper: the world fluids' and the tank-only ones'), the empty
//!   bucket (any fluid that has a bucket, as Forge's wrapper fills it), the copper can (CopperCanFluidHandler: one
//!   ingot, whole) and the tanks' and lanterns' items (TankItemFluidHandler: any part, up to their block's capacity),
//!   so every mod's machines and pipes fill and empty them;
//! - Mantle's data-driven transfers (`mantle/fluid_transfer`: bottles, bowls), read by a server reload listener;
//! - one container item against a tank (`transfer`): the transfers first for the items they name, else the item's
//!   `ITEM_FLUIDS`, through a scratch place so the item it becomes is known before it goes anywhere; used by a click on
//!   the melter and the tanks (`interact_with_tank`) and by the melter's tank buttons;
//! - the tank items' and the copper can's tooltip.

use super::util::*;
use super::{LANTERNS, TANK_TYPE, tank_capacity};
use crate::fluids;
use mod_api::serde_json::Value;
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

pub(crate) const COPPER_CAN: &str = "tconstruct:copper_can";
/// CopperCanFluidHandler: FluidValues.INGOT.
pub(crate) const COPPER_CAN_CAPACITY: i32 = 90;
const BUCKET: u32 = 1000;

// ---- Fluids as resources

pub(crate) fn fluid_id(host: &dyn Host, r: &FluidResource) -> Option<String> {
    r.fluid().key().and_then(|k| k.id(host)).map(str::to_owned)
}

pub(crate) fn fluid_res(host: &dyn Host, f: &str, n: u32) -> Option<FluidResource> {
    let k = host.fluid_key(f)?;
    Some(FluidResource::of(&FluidStack::new(k, n.max(1))))
}

/// The bucket that holds `fluid`: the game's (vanilla's and the world fluids'), else Tinkers' tank-only buckets (the
/// potion's needs its potion, which a plain bucket can't carry).
pub(crate) fn bucket_item(host: &dyn Host, fluid: &str) -> Option<ItemKey> {
    if let Some(k) = host.fluid_bucket(fluid) {
        return Some(k);
    }
    if fluid == "tconstruct:potion" {
        return None;
    }
    fluids::table().buckets.get(fluid).and_then(|b| item_key(b))
}

/// A filled bucket's fluid (FluidBucketWrapper), for the buckets this mod answers.
fn tinkers_buckets() -> &'static HashMap<String, String> {
    static B: OnceLock<HashMap<String, String>> = OnceLock::new();
    B.get_or_init(|| {
        fluids::table().buckets.iter().filter(|(f, _)| f.as_str() != "tconstruct:potion").map(|(f, b)| (b.clone(), f.clone())).collect()
    })
}

// ---- A scratch place: one item held for a capability call, so what it becomes is read back before it is placed

const SCRATCH: &str = "tconstruct:scratch";

#[derive(Default)]
pub(crate) struct Scratch {
    slots: Mutex<HashMap<u64, ItemStack>>,
}

static SCRATCH_SLOTS: OnceLock<Arc<Scratch>> = OnceLock::new();
static SCRATCH_NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn scratch() -> &'static Arc<Scratch> {
    SCRATCH_SLOTS.get_or_init(|| Arc::new(Scratch::default()))
}

impl PlaceProvider for Scratch {
    fn stack(&self, _host: &mut dyn ServerHost, key: u64) -> Option<ItemStack> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned()
    }

    fn insert(&self, host: &mut dyn ServerHost, _tx: &TransactionContext, key: u64, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            let mut g = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            match g.get_mut(&key) {
                None => {
                    let Some(s) = resource.to_stack(amount) else { return 0 };
                    g.insert(key, s);
                    amount
                }
                Some(h) if resource.matches(h) => {
                    let room = max_stack(&*host, h).saturating_sub(h.count()).min(amount);
                    if room > 0 && h.grow(room).is_ok() { room } else { 0 }
                }
                _ => 0,
            }
        })())
    }

    fn extract(&self, _host: &mut dyn ServerHost, _tx: &TransactionContext, key: u64, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            let mut g = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = g.get(&key) else { return 0 };
            if !resource.matches(h) {
                return 0;
            }
            let n = h.count().min(amount);
            match h.clone().shrink(n) {
                Some(rest) => {
                    g.insert(key, rest);
                }
                None => {
                    g.remove(&key);
                }
            }
            n
        })())
    }
}

/// Lends `stack` to a scratch place for `f`; what is there after it.
fn with_scratch<R>(host: &mut dyn ServerHost, stack: ItemStack, f: impl FnOnce(&mut dyn ServerHost, &ItemPlace) -> R) -> (R, Option<ItemStack>) {
    let key = SCRATCH_NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    scratch().slots.lock().unwrap_or_else(|e| e.into_inner()).insert(key, stack);
    let place = ItemPlace::Mod { provider: SCRATCH, key };
    let out = f(host, &place);
    let left = scratch().slots.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
    (out, left)
}

// ---- The items' fluid handlers

/// A Tinkers' bucket (FluidBucketWrapper): 1000 mB of its fluid, drained whole into an empty bucket.
pub(crate) struct FilledBucket {
    place: ItemPlace,
    fluid: &'static str,
}

impl ResourceHandler<FluidResource> for FilledBucket {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, _index: usize) -> Option<FluidResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.place.stack(host)?;
        fluid_res(&*host, self.fluid, BUCKET)
    }
    fn amount(&self, host: &mut dyn ServerHost, _index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        if self.place.stack(host).is_some() { BUCKET } else { 0 }
    }
    fn capacity(&self, _host: &mut dyn ServerHost, _index: usize, _resource: Option<&FluidResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        BUCKET
    }
    fn insert_at(&self, _host: &mut dyn ServerHost, _tx: &TransactionContext, _index: usize, _resource: &FluidResource, _amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            0
        })())
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, _index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if amount < BUCKET || fluid_id(&*host, resource).as_deref() != Some(self.fluid) {
                return 0;
            }
            let Some(empty) = new_stack("minecraft:bucket", 1) else { return 0 };
            if self.place.exchange(host, tx, &ItemResource::of(&empty), 1) == Ok(1) { BUCKET } else { 0 }
        })())
    }
}

/// The empty bucket (FluidBucketWrapper.fill): a whole bucket of any fluid that has a bucket.
pub(crate) struct EmptyBucket {
    place: ItemPlace,
}

impl ResourceHandler<FluidResource> for EmptyBucket {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        1
    }
    fn resource(&self, _host: &mut dyn ServerHost, _index: usize) -> Option<FluidResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        None
    }
    fn amount(&self, _host: &mut dyn ServerHost, _index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        0
    }
    fn capacity(&self, host: &mut dyn ServerHost, _index: usize, resource: Option<&FluidResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        // The general limit: a bucket's worth.
        let Some(resource) = resource else { return BUCKET };
        let ok = !resource.to_stack(1).has_components() && fluid_id(&*host, resource).is_some_and(|f| bucket_item(&*host, &f).is_some());
        if ok { BUCKET } else { 0 }
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, _index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if amount < BUCKET || resource.to_stack(1).has_components() {
                return 0;
            }
            let Some(full) = fluid_id(&*host, resource).and_then(|f| bucket_item(&*host, &f)) else { return 0 };
            let Some(full) = ItemStack::new(full, 1) else { return 0 };
            if self.place.exchange(host, tx, &ItemResource::of(&full), 1) == Ok(1) { BUCKET } else { 0 }
        })())
    }
    fn extract_at(&self, _host: &mut dyn ServerHost, _tx: &TransactionContext, _index: usize, _resource: &FluidResource, _amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            0
        })())
    }
}

/// CopperCanItem.getFluid: the `fluid` id in its custom data.
pub(crate) fn can_fluid(stack: &ItemStack) -> Option<String> {
    gs(&custom_data(stack), "fluid").filter(|f| !f.is_empty() && f != "minecraft:empty")
}

/// CopperCanItem.setFluid on one can (none: removeFluid). The look (FluidContainerModel: the fluid's still sprite in
/// the can's mask) is the item definition's select on `minecraft:custom_model_data` string 0, so the fluid is
/// mirrored there; an empty can has neither, so it stacks with new ones.
pub(crate) fn with_can_fluid(stack: &ItemStack, fluid: Option<&str>) -> Option<ItemStack> {
    let mut out = with_count(stack, 1)?;
    let mut c = custom_data(&out);
    rm(&mut c, "fluid");
    rm(&mut c, "fluid_tag");
    if let Some(f) = fluid {
        ss(&mut c, "fluid", f);
    }
    set_custom_data(&mut out, c);
    let look = fluid.map(|f| {
        let mut l = Compound::new();
        l.insert(js("strings"), Tag::List(vec![Tag::String(js(f))].into_iter().collect()));
        Tag::Compound(l)
    });
    set_component(&mut out, "minecraft:custom_model_data", look);
    Some(out)
}

pub(crate) struct CopperCan {
    place: ItemPlace,
}

impl ResourceHandler<FluidResource> for CopperCan {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, _index: usize) -> Option<FluidResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        let f = can_fluid(&self.place.stack(host)?)?;
        fluid_res(&*host, &f, COPPER_CAN_CAPACITY as u32)
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        if self.resource(host, index).is_some() { COPPER_CAN_CAPACITY as u32 } else { 0 }
    }
    fn capacity(&self, _host: &mut dyn ServerHost, _index: usize, _resource: Option<&FluidResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        COPPER_CAN_CAPACITY as u32
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, _index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            // CopperCanFluidHandler.fill: an empty can, one whole ingot.
            let Some(s) = self.place.stack(host) else { return 0 };
            if can_fluid(&s).is_some() || amount < COPPER_CAN_CAPACITY as u32 || resource.to_stack(1).has_components() {
                return 0;
            }
            let Some(f) = fluid_id(&*host, resource) else { return 0 };
            let Some(full) = with_can_fluid(&s, Some(&f)) else { return 0 };
            if self.place.exchange(host, tx, &ItemResource::of(&full), 1) == Ok(1) { COPPER_CAN_CAPACITY as u32 } else { 0 }
        })())
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, _index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            let Some(s) = self.place.stack(host) else { return 0 };
            let Some(f) = can_fluid(&s) else { return 0 };
            if amount < COPPER_CAN_CAPACITY as u32 || fluid_id(&*host, resource).as_deref() != Some(f.as_str()) {
                return 0;
            }
            let Some(empty) = with_can_fluid(&s, None) else { return 0 };
            if self.place.exchange(host, tx, &ItemResource::of(&empty), 1) == Ok(1) { COPPER_CAN_CAPACITY as u32 } else { 0 }
        })())
    }
}

/// TankItem: its fluid, `tank` {FluidName, Amount} in the custom data as Java's item tag.
pub(crate) fn item_tank(stack: &ItemStack) -> Option<(String, i32)> {
    let c = custom_data(stack);
    let t = gc(&c, "tank")?;
    let fluid = gs(t, "FluidName")?;
    let amount = gi(t, "Amount", 0);
    (amount > 0 && fluid != "minecraft:empty" && !fluid.is_empty()).then_some((fluid, amount))
}

/// TankItem.setTank on one item: `tank` in the custom data (what its tooltip and its handler read) and the block entity
/// data the placed block starts with (SearedTankBlock.setPlacedBy), both removed when empty.
pub(crate) fn with_item_tank(stack: &ItemStack, fluid: Option<&str>, amount: i32) -> Option<ItemStack> {
    let mut out = with_count(stack, 1)?;
    let mut c = custom_data(&out);
    rm(&mut c, "tank");
    let filled = fluid.filter(|_| amount > 0);
    if let Some(f) = filled {
        let mut t = Compound::new();
        ss(&mut t, "FluidName", f);
        si(&mut t, "Amount", amount);
        c.insert(js("tank"), Tag::Compound(t));
    }
    set_custom_data(&mut out, c);
    let data = filled.map(|f| {
        let mut d = Compound::new();
        ss(&mut d, "id", if sid(stack) == super::casting::CASTING_TANK { super::casting::CASTING_TANK_TYPE } else { TANK_TYPE });
        set_tank(&mut d, Some(f), amount);
        Tag::Compound(d)
    });
    set_component(&mut out, "minecraft:block_entity_data", data);
    Some(out)
}

/// The capacity of a tank's or lantern's item (TankItemFluidHandler: its block's).
pub(crate) fn tank_item_capacity(item: &str) -> Option<i32> {
    tank_capacity(item).filter(|_| item != super::MELTER)
}

pub(crate) struct TankItem {
    place: ItemPlace,
    capacity: i32,
}

impl TankItem {
    fn write(&self, host: &mut dyn ServerHost, tx: &TransactionContext, s: &ItemStack, f: Option<&str>, amount: i32) -> bool {
        let Some(one) = with_item_tank(s, f, amount) else { return false };
        self.place.exchange(host, tx, &ItemResource::of(&one), 1) == Ok(1)
    }
}

impl ResourceHandler<FluidResource> for TankItem {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, _index: usize) -> Option<FluidResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        let (f, n) = item_tank(&self.place.stack(host)?)?;
        fluid_res(&*host, &f, n.min(self.capacity) as u32)
    }
    fn amount(&self, host: &mut dyn ServerHost, _index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.place.stack(host).and_then(|s| item_tank(&s)).map_or(0, |(_, n)| n.clamp(0, self.capacity) as u32)
    }
    fn capacity(&self, _host: &mut dyn ServerHost, _index: usize, _resource: Option<&FluidResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.capacity as u32
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, _index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            let Some(s) = self.place.stack(host) else { return 0 };
            if resource.to_stack(1).has_components() {
                return 0;
            }
            let Some(id) = fluid_id(&*host, resource) else { return 0 };
            let (have, n) = item_tank(&s).map_or((None, 0), |(f, n)| (Some(f), n.min(self.capacity)));
            if have.as_deref().is_some_and(|f| f != id) {
                return 0;
            }
            let k = (amount.min(i32::MAX as u32) as i32).min(self.capacity - n);
            if k <= 0 {
                return 0;
            }
            if self.write(host, tx, &s, Some(&id), n + k) { k as u32 } else { 0 }
        })())
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, _index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            let Some(s) = self.place.stack(host) else { return 0 };
            let Some((f, n)) = item_tank(&s) else { return 0 };
            if fluid_id(&*host, resource).as_deref() != Some(f.as_str()) {
                return 0;
            }
            let n = n.min(self.capacity);
            let k = (amount.min(i32::MAX as u32) as i32).min(n);
            if k <= 0 {
                return 0;
            }
            if self.write(host, tx, &s, Some(&f), n - k) { k as u32 } else { 0 }
        })())
    }
}

/// The items' handlers: Tinkers' buckets, the empty bucket, the copper can, the tanks' and lanterns' items.
pub(crate) fn register(r: &mut dyn Registrar) {
    r.place_provider(SCRATCH, scratch().clone());
    for (bucket, fluid) in tinkers_buckets() {
        let fluid: &'static str = Box::leak(fluid.clone().into_boxed_str());
        r.item_capability_before_game(&ITEM_FLUIDS, bucket, move |_, place: &ItemPlace| {
            Some(Box::new(FilledBucket { place: place.clone(), fluid }) as Box<dyn ResourceHandler<FluidResource>>)
        });
    }
    r.item_capability_before_game(&ITEM_FLUIDS, "minecraft:bucket", |_, place: &ItemPlace| {
        Some(Box::new(EmptyBucket { place: place.clone() }) as Box<dyn ResourceHandler<FluidResource>>)
    });
    r.item_capability_before_game(&ITEM_FLUIDS, COPPER_CAN, |_, place: &ItemPlace| {
        Some(Box::new(CopperCan { place: place.clone() }) as Box<dyn ResourceHandler<FluidResource>>)
    });
    let casting_tank = [(super::casting::CASTING_TANK, super::casting::CASTING_TANK_CAPACITY)];
    for (item, capacity) in super::TANKS.iter().map(|(b, c)| (*b, *c)).chain(LANTERNS.iter().map(|b| (*b, super::LANTERN_CAPACITY))).chain(casting_tank) {
        r.item_capability_before_game(&ITEM_FLUIDS, item, move |_, place: &ItemPlace| {
            Some(Box::new(TankItem { place: place.clone(), capacity }) as Box<dyn ResourceHandler<FluidResource>>)
        });
    }
}

// ---- Mantle's data-driven transfers (FluidContainerTransferManager, `mantle/fluid_transfer`)

#[derive(Clone)]
pub(crate) enum TransferKind {
    /// mantle:empty_item: exactly this fluid and amount into the tank; the result (none: used up).
    Empty { fluid: String, amount: i32, result: Option<(String, u32)> },
    /// mantle:fill_item: a fluid of these, at least `amount` of it, out of the tank; the result.
    Fill { fluids: HashSet<String>, amount: i32, result: Option<(String, u32)> },
    /// mantle:fill_nbt, mantle:empty_potion, or an NBT result: they need fluid or item data a plain stack can't carry;
    /// they still claim their items.
    Unsupported,
}

#[derive(Clone)]
pub(crate) struct FluidTransfer {
    input: HashSet<String>,
    kind: TransferKind,
}

/// The server's files of the last load, with a generation number.
static FILES: RwLock<Option<(u64, Arc<Vec<Value>>)>> = RwLock::new(None);
/// The parsed transfers of a generation.
static PARSED: Mutex<Option<(u64, Arc<Vec<FluidTransfer>>)>> = Mutex::new(None);

pub(crate) struct TransferListener;

impl ServerReloadListener for TransferListener {
    fn prepare(&self, context: &ServerReloadContext) -> Result<Box<dyn FnOnce() + Send + 'static>, ReloadResourceError> {
        let mut files = Vec::new();
        for id in context.list() {
            if let Some(v) = context.read(id).and_then(|b| mod_api::serde_json::from_slice::<Value>(b).ok()) {
                files.push(v);
            }
        }
        Ok(Box::new(move || {
            let mut g = FILES.write().unwrap_or_else(|e| e.into_inner());
            let n = g.as_ref().map_or(0, |(n, _)| *n);
            *g = Some((n + 1, Arc::new(files)));
        }))
    }
}

pub(crate) fn add_reload_listener(event: &mut AddServerReloadListenersEvent<'_>) {
    let (Ok(id), Ok(conv)) = (Identifier::parse("tconstruct:fluid_transfer"), FileToIdConverter::json("mantle/fluid_transfer")) else { return };
    let _ = event.listeners.add_listener(id, conv, Arc::new(TransferListener));
}

/// ItemOutput: an id, `{item, count}` or `{tag}` (its first item); one with `nbt` can't be made here.
fn transfer_output(host: &dyn Host, v: &Value) -> Result<Option<(String, u32)>, ()> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some((s.clone(), 1))),
        Value::Object(o) if o.contains_key("nbt") => Err(()),
        Value::Object(o) => {
            let count = o.get("count").and_then(Value::as_i64).unwrap_or(1).clamp(1, 64) as u32;
            if let Some(i) = o.get("item").and_then(Value::as_str) {
                return Ok(Some((i.to_owned(), count)));
            }
            let tag = o.get("tag").and_then(Value::as_str).ok_or(())?;
            let tag = tag.strip_prefix("forge:").map_or(tag.to_owned(), |t| format!("c:{t}"));
            let json = Value::String(format!("#{tag}")).to_string();
            let first = host.recipe_ingredient(&json).ok().flatten().and_then(|i| i.order.first().cloned()).ok_or(())?;
            Ok(Some((first, count)))
        }
        _ => Err(()),
    }
}

/// FluidIngredient's fluids: `{fluid}`, `{tag}` or a list of them, with the (first) amount.
fn transfer_fluids(v: &Value) -> Option<(HashSet<String>, i32)> {
    if let Some(a) = v.as_array() {
        let mut out = HashSet::new();
        let mut amount = None;
        for x in a {
            let (f, n) = transfer_fluids(x)?;
            out.extend(f);
            amount.get_or_insert(n);
        }
        return Some((out, amount?));
    }
    let amount = v["amount"].as_i64()?.clamp(1, i64::from(i32::MAX)) as i32;
    let fluids = match (v["fluid"].as_str(), v["tag"].as_str()) {
        (Some(f), _) => HashSet::from([f.to_owned()]),
        (None, Some(t)) => super::recipes::fluid_tag(t).iter().cloned().collect(),
        _ => return None,
    };
    Some((fluids, amount))
}

fn parse_transfer(host: &dyn Host, json: &Value) -> Option<FluidTransfer> {
    if !json["conditions"].as_array().is_none_or(|c| c.iter().all(|x| super::recipes::condition_holds(host, x))) {
        return None;
    }
    let mut order = Vec::new();
    let input = super::recipes::ingredient_items(host, &json["input"], &mut order);
    let result = transfer_output(host, json.get("result").or(json.get("filled")).unwrap_or(&Value::Null));
    let kind = match (json["type"].as_str()?, result) {
        ("mantle:empty_item", Ok(result)) => match super::recipes::fluid_output(&json["fluid"]) {
            Some((fluid, amount)) => TransferKind::Empty { fluid, amount, result },
            None => TransferKind::Unsupported,
        },
        ("mantle:fill_item", Ok(result)) => match transfer_fluids(&json["fluid"]) {
            Some((fluids, amount)) => TransferKind::Fill { fluids, amount, result },
            None => TransferKind::Unsupported,
        },
        _ => TransferKind::Unsupported,
    };
    Some(FluidTransfer { input, kind })
}

fn transfers(host: &dyn Host) -> Arc<Vec<FluidTransfer>> {
    let Some((generation, files)) = FILES.read().ok().and_then(|f| f.clone()) else { return Arc::new(Vec::new()) };
    let mut g = PARSED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((n, t)) = g.as_ref()
        && *n == generation
    {
        return t.clone();
    }
    let parsed: Arc<Vec<FluidTransfer>> = Arc::new(files.iter().filter_map(|j| parse_transfer(host, j)).collect());
    *g = Some((generation, parsed.clone()));
    parsed
}

// ---- One container item against a tank

/// IFluidContainerTransfer.TransferDirection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Way {
    Auto,
    EmptyItem,
    FillItem,
}

impl Way {
    fn can_empty(self) -> bool {
        self != Way::FillItem
    }
    fn can_fill(self) -> bool {
        self != Way::EmptyItem
    }
}

/// A tank a container works on: a block's tank through `FLUIDS`, or a block entity's saved tank while it is out of the
/// world (its own tick, its menu's button).
pub(crate) trait TankAccess {
    fn contents(&mut self, host: &mut dyn ServerHost) -> Tank;
    /// IFluidHandler.fill: what of `amount` of `fluid` goes in (done with `execute`).
    fn fill(&mut self, host: &mut dyn ServerHost, fluid: &str, amount: i32, execute: bool) -> i32;
    /// IFluidHandler.drain of `fluid`: what comes out.
    fn drain(&mut self, host: &mut dyn ServerHost, fluid: &str, amount: i32, execute: bool) -> i32;
}

/// A block entity's saved tank (`fluids`).
pub(crate) struct NbtTank<'a> {
    pub nbt: &'a mut Compound,
    pub capacity: i32,
}

impl TankAccess for NbtTank<'_> {
    fn contents(&mut self, _host: &mut dyn ServerHost) -> Tank {
        tank_of(self.nbt)
    }
    fn fill(&mut self, _host: &mut dyn ServerHost, fluid: &str, amount: i32, execute: bool) -> i32 {
        let t = tank_of(self.nbt);
        let k = fill_room(&t, self.capacity, fluid, amount);
        if execute && k > 0 {
            set_tank(self.nbt, Some(fluid), t.1 + k);
        }
        k
    }
    fn drain(&mut self, _host: &mut dyn ServerHost, fluid: &str, amount: i32, execute: bool) -> i32 {
        let t = tank_of(self.nbt);
        if t.0.as_deref() != Some(fluid) {
            return 0;
        }
        let k = amount.min(t.1).max(0);
        if execute && k > 0 {
            set_tank(self.nbt, t.0.as_deref(), t.1 - k);
        }
        k
    }
}

/// A block's tank through its `FLUIDS` (tank 0, as Mantle's helpers read IFluidHandler).
pub(crate) struct CapTank(pub Capability<dyn ResourceHandler<FluidResource>>);

impl TankAccess for CapTank {
    fn contents(&mut self, host: &mut dyn ServerHost) -> Tank {
        if self.0.size(host) == 0 {
            return (None, 0);
        }
        let n = self.0.amount(host, 0) as i32;
        match self.0.resource(host, 0).and_then(|r| fluid_id(&*host, &r)) {
            Some(f) if n > 0 => (Some(f), n),
            _ => (None, 0),
        }
    }
    fn fill(&mut self, host: &mut dyn ServerHost, fluid: &str, amount: i32, execute: bool) -> i32 {
        let Some(res) = fluid_res(&*host, fluid, 1) else { return 0 };
        if amount <= 0 {
            return 0;
        }
        let cap = &self.0;
        let r = host.transaction(|host, tx| {
            let n = cap.insert(host, tx, &res, amount as u32).unwrap_or(0).min(amount as u32) as i32;
            if execute { Ok(n) } else { Err(n) }
        });
        // A dry run aborts with its amount; a transaction that couldn't run or commit moved nothing.
        match r {
            Ok(n) | Err(TransactionError::Aborted(n)) => n,
            Err(_) => 0,
        }
    }
    fn drain(&mut self, host: &mut dyn ServerHost, fluid: &str, amount: i32, execute: bool) -> i32 {
        let Some(res) = fluid_res(&*host, fluid, 1) else { return 0 };
        if amount <= 0 {
            return 0;
        }
        let cap = &self.0;
        let r = host.transaction(|host, tx| {
            let size = cap.size(host);
            let mut n = 0u32;
            for i in 0..size {
                if n >= amount as u32 {
                    break;
                }
                n += cap.extract_at(host, tx, i, &res, amount as u32 - n).unwrap_or(0);
            }
            let n = n.min(amount as u32) as i32;
            if execute { Ok(n) } else { Err(n) }
        });
        // A dry run aborts with its amount; a transaction that couldn't run or commit moved nothing.
        match r {
            Ok(n) | Err(TransactionError::Aborted(n)) => n,
            Err(_) => 0,
        }
    }
}

/// What a container did with a tank: the fluid and amount moved, whether the item was filled, the item it became.
pub(crate) struct Moved {
    pub fluid: String,
    pub amount: i32,
    pub filled_item: bool,
    pub result: Option<ItemStack>,
}

/// Whether `stack` is a fluid container (TankItem.mayHaveFluid: a transfer names it, or it has a fluid handler).
pub(crate) fn is_container(host: &mut dyn ServerHost, stack: &ItemStack) -> bool {
    let id = sid(stack);
    if transfers(&*host).iter().any(|t| t.input.contains(id)) {
        return true;
    }
    let Some(one) = with_count(stack, 1) else { return false };
    with_scratch(host, one, |host, place| place.capability(host, &ITEM_FLUIDS).is_some()).0
}

/// TankItem.mayHaveFluid as the client can tell it (no capabilities there): a bucket, the copper can, a tank's or
/// lantern's item, or an item a transfer names (the transfers this process loaded).
pub(crate) fn maybe_container(host: &dyn Host, stack: &ItemStack) -> bool {
    ensure_ids(host);
    let id = sid(stack);
    matches!(id, "minecraft:bucket" | "minecraft:water_bucket" | "minecraft:lava_bucket" | COPPER_CAN)
        || tinkers_buckets().contains_key(id)
        || tank_item_capacity(id).is_some()
        || transfers(host).iter().any(|t| t.input.contains(id))
}

/// FluidTransferHelper.interactWithStack: one item of `held` against `tank`, in `way`'s order; the tank changed and
/// the item it becomes (none: used up). None when nothing moved.
pub(crate) fn transfer(host: &mut dyn ServerHost, held: &ItemStack, tank: &mut dyn TankAccess, way: Way) -> Option<Moved> {
    ensure_ids(&*host);
    let id = sid(held);
    let all = transfers(&*host);
    // An item a transfer names moves by the transfers only (the first that matches).
    if all.iter().any(|t| t.input.contains(id)) {
        let first = tank.contents(host);
        let t = all.iter().find(|t| {
            t.input.contains(id)
                && match &t.kind {
                    TransferKind::Empty { .. } => true,
                    TransferKind::Fill { fluids, amount, .. } => first.0.as_ref().is_some_and(|f| fluids.contains(f) && first.1 >= *amount),
                    TransferKind::Unsupported => false,
                }
        })?;
        let made = |r: &Option<(String, u32)>| r.as_ref().and_then(|(i, n)| new_stack(i, *n));
        return match &t.kind {
            TransferKind::Empty { fluid, amount, result } if way.can_empty() => {
                if tank.fill(host, fluid, *amount, false) != *amount {
                    return None;
                }
                tank.fill(host, fluid, *amount, true);
                Some(Moved { fluid: fluid.clone(), amount: *amount, filled_item: false, result: made(result) })
            }
            TransferKind::Fill { amount, result, .. } if way.can_fill() => {
                let f = first.0?;
                if tank.drain(host, &f, *amount, false) != *amount {
                    return None;
                }
                tank.drain(host, &f, *amount, true);
                Some(Moved { fluid: f, amount: *amount, filled_item: true, result: made(result) })
            }
            _ => None,
        };
    }
    let empty = |host: &mut dyn ServerHost, tank: &mut dyn TankAccess| item_into_tank(host, held, tank);
    match way {
        Way::EmptyItem => empty(host, tank),
        Way::FillItem => tank_into_item(host, held, tank),
        Way::Auto => empty(host, tank).or_else(|| tank_into_item(host, held, tank)),
    }
}

/// FluidContainerTransferManager.mayHaveTransfer: a data-driven transfer names the item.
pub(crate) fn has_transfer(host: &dyn Host, item: &str) -> bool {
    transfers(host).iter().any(|t| t.input.contains(item))
}

/// The `FluidTank(10000)` GenerateMeltingRecipesCommand empties an item into: nothing in it at first.
struct ScratchTank(Tank);

impl TankAccess for ScratchTank {
    fn contents(&mut self, _host: &mut dyn ServerHost) -> Tank {
        self.0.clone()
    }
    fn fill(&mut self, _host: &mut dyn ServerHost, fluid: &str, amount: i32, execute: bool) -> i32 {
        let k = fill_room(&self.0, 10_000, fluid, amount);
        if execute && k > 0 {
            self.0 = (Some(fluid.to_owned()), self.0.1 + k);
        }
        k
    }
    fn drain(&mut self, _host: &mut dyn ServerHost, _fluid: &str, _amount: i32, _execute: bool) -> i32 {
        0
    }
}

/// Vanilla's other BucketItems (MobBucketItem): water.
const MOB_BUCKETS: [&str; 6] = [
    "minecraft:pufferfish_bucket",
    "minecraft:salmon_bucket",
    "minecraft:cod_bucket",
    "minecraft:tropical_fish_bucket",
    "minecraft:axolotl_bucket",
    "minecraft:tadpole_bucket",
];

/// `item instanceof BucketItem` (TinkerPredicate.BUCKET): vanilla's buckets, its mob buckets and Tinkers' own.
pub(crate) fn is_bucket_item(item: &str) -> bool {
    matches!(item, "minecraft:bucket" | "minecraft:water_bucket" | "minecraft:lava_bucket") || MOB_BUCKETS.contains(&item) || tinkers_buckets().contains_key(item)
}

/// GenerateMeltingRecipesCommand.MeltingCache's container steps for one plain item: Mantle's transfer emptying it
/// into a scratch tank, else a BucketItem's fluid (1000 mB), else what its fluid handler holds.
pub(crate) fn contained_fluid(host: &mut dyn ServerHost, stack: &ItemStack) -> Option<(String, i32)> {
    ensure_ids(&*host);
    let id = sid(stack);
    if has_transfer(&*host, id)
        && let Some(m) = transfer(host, stack, &mut ScratchTank((None, 0)), Way::EmptyItem)
    {
        return Some((m.fluid, m.amount));
    }
    if let Some(f) = tinkers_buckets().get(id) {
        return Some((f.clone(), BUCKET as i32));
    }
    match id {
        "minecraft:water_bucket" => return Some(("minecraft:water".into(), BUCKET as i32)),
        "minecraft:lava_bucket" => return Some(("minecraft:lava".into(), BUCKET as i32)),
        _ if MOB_BUCKETS.contains(&id) => return Some(("minecraft:water".into(), BUCKET as i32)),
        _ => {}
    }
    item_into_tank(host, stack, &mut ScratchTank((None, 0))).map(|m| (m.fluid, m.amount))
}

/// FluidUtil.tryEmptyContainer: the item's fluid into the tank (as much as the item gives of what fits).
fn item_into_tank(host: &mut dyn ServerHost, held: &ItemStack, tank: &mut dyn TankAccess) -> Option<Moved> {
    let one = with_count(held, 1)?;
    let (moved, left) = with_scratch(host, one, |host, place| -> Option<(String, i32)> {
        let cap = place.capability(host, &ITEM_FLUIDS)?;
        let res = cap.resource(host, 0)?;
        let f = fluid_id(&*host, &res)?;
        let have = cap.amount(host, 0) as i32;
        let room = tank.fill(host, &f, have, false);
        if room <= 0 {
            return None;
        }
        let n = host.transaction(|host, tx| Ok::<u32, ()>(cap.extract_at(host, tx, 0, &res, room as u32).unwrap_or(0).min(room as u32))).unwrap_or(0) as i32;
        if n <= 0 {
            return None;
        }
        tank.fill(host, &f, n, true);
        Some((f, n))
    });
    let (fluid, amount) = moved?;
    Some(Moved { fluid, amount, filled_item: false, result: left })
}

/// FluidUtil.tryFillContainer: the tank's fluid into the item.
fn tank_into_item(host: &mut dyn ServerHost, held: &ItemStack, tank: &mut dyn TankAccess) -> Option<Moved> {
    let (Some(f), a) = tank.contents(host) else { return None };
    let one = with_count(held, 1)?;
    let (moved, left) = with_scratch(host, one, |host, place| -> Option<i32> {
        let cap = place.capability(host, &ITEM_FLUIDS)?;
        let res = fluid_res(&*host, &f, 1)?;
        let can = tank.drain(host, &f, a, false);
        if can <= 0 {
            return None;
        }
        let n = host.transaction(|host, tx| Ok::<u32, ()>(cap.insert(host, tx, &res, can as u32).unwrap_or(0).min(can as u32))).unwrap_or(0) as i32;
        if n <= 0 {
            return None;
        }
        tank.drain(host, &f, n, true);
        Some(n)
    });
    let amount = moved?;
    Some(Moved { fluid: f, amount, filled_item: true, result: left })
}

/// ContainerFillingRecipe: one item of `stack` filled with up to `amount` of `fluid` through its `ITEM_FLUIDS` (a bucket
/// whole, the copper can one ingot, a tank's item up to its room): what it took and the item it became.
pub(crate) fn fill_item(host: &mut dyn ServerHost, stack: &ItemStack, fluid: &str, amount: i32) -> Option<(i32, Option<ItemStack>)> {
    let one = with_count(stack, 1)?;
    let res = fluid_res(&*host, fluid, 1)?;
    if amount <= 0 {
        return None;
    }
    let (n, left) = with_scratch(host, one, |host, place| -> Option<i32> {
        let cap = place.capability(host, &ITEM_FLUIDS)?;
        let n = host.transaction(|host, tx| Ok::<u32, ()>(cap.insert(host, tx, &res, amount as u32).unwrap_or(0).min(amount as u32))).unwrap_or(0) as i32;
        (n > 0).then_some(n)
    });
    Some((n?, left))
}

/// The bucket sounds (FluidType's BUCKET_FILL / BUCKET_EMPTY): lava's for lava and Tinkers' hot fluids.
pub(crate) fn bucket_sound(fluid: &str, filled_item: bool) -> &'static str {
    let hot = fluid == "minecraft:lava" || fluids::table().hot.iter().any(|h| h == fluid);
    match (hot, filled_item) {
        (true, true) => "minecraft:item.bucket.fill_lava",
        (true, false) => "minecraft:item.bucket.empty_lava",
        (false, true) => "minecraft:item.bucket.fill",
        (false, false) => "minecraft:item.bucket.empty",
    }
}

/// A stack into a player's inventory, what doesn't fit dropped at them (ItemHandlerHelper.giveItemToPlayer).
pub(crate) fn give_player(host: &mut dyn ServerHost, player: u128, stack: ItemStack) {
    let place = ItemPlace::PlayerSlot { player, slot: 0 };
    let n = stack.count();
    let res = ItemResource::of(&stack);
    let given = host.transaction(|host, tx| Ok::<u32, ()>(place.insert(host, tx, &res, n).unwrap_or(0))).unwrap_or(0);
    if given < n
        && let Some(rest) = with_count(&stack, n - given)
        && let Some(p) = host.player(player)
        && let (Some(level), Some(at)) = (p.level(&*host), p.entity().position(&*host))
    {
        let _ = level.spawn_item(host, at[0], at[1], at[2], rest, 40);
    }
}

/// FluidTransferHelper.interactWithTank on the block at `pos` (its `FLUIDS` from the face clicked): a container in
/// the hand empties into the tank, else fills from it; sound and action-bar message; the item it becomes as
/// ItemUtils.createFilledResult. True when the held item is a fluid container (the click is then the container's,
/// even if nothing moved).
pub(crate) fn interact_with_tank(host: &mut dyn ServerHost, click: &BlockClick<'_>, level: LevelRef) -> bool {
    let Some(held) = click.stack.cloned().filter(|s| s.count() > 0) else { return false };
    if !is_container(host, &held) {
        return false;
    }
    let face = [Direction::Down, Direction::Up, Direction::North, Direction::South, Direction::West, Direction::East]
        .into_iter()
        .find(|d| {
            let n = click.hit.face;
            let v = match d {
                Direction::Down => [0, -1, 0],
                Direction::Up => [0, 1, 0],
                Direction::North => [0, 0, -1],
                Direction::South => [0, 0, 1],
                Direction::West => [-1, 0, 0],
                _ => [1, 0, 0],
            };
            n == v
        });
    let Some(cap) = level.capability(host, &FLUIDS, click.pos, face).or_else(|| level.capability(host, &FLUIDS, click.pos, None)) else { return true };
    interact_with_container(host, click, level, &mut CapTank(cap));
    true
}

/// FluidTransferHelper.interactWithContainer: the held item (a fluid container) against `tank`, empty into it first;
/// sound and action-bar message; the held item exchanged for what it becomes. True when fluid moved (`didTransfer`).
pub(crate) fn interact_with_container(host: &mut dyn ServerHost, click: &BlockClick<'_>, level: LevelRef, tank: &mut dyn TankAccess) -> bool {
    let Some(held) = click.stack.cloned().filter(|s| s.count() > 0) else { return false };
    let Some(m) = transfer(host, &held, tank, Way::Auto) else { return false };
    let pos = click.pos;
    let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
    level.play_sound(host, bucket_sound(&m.fluid, m.filled_item), at, SoundSource::Blocks, 1.0, 1.0, None);
    if let Some(player) = host.player(click.player) {
        let (key, english) = if m.filled_item { ("block.mantle.tank.drained", "Drained %s mb %s") } else { ("block.mantle.tank.filled", "Filled %s mb %s") };
        let _ = player.action_bar(host, tr(key, english, vec![Text::literal(commas(i64::from(m.amount))), fluid_text(&m.fluid)]));
    }
    // ItemUtils.createFilledResult through the hand's place: the held item exchanged for the result (from a stack of
    // more, the result goes to the inventory; a creative hand keeps its item and is given the result if it has none).
    let hand = ItemPlace::PlayerHand { player: click.player, hand: click.hand };
    match m.result {
        Some(r) => {
            let res = ItemResource::of(&r);
            let n = r.count();
            let _ = host.transaction(|host, tx| if hand.exchange(host, tx, &res, n.min(1).max(1)) == Ok(1) { Ok(()) } else { Err(()) });
            if n > 1
                && let Some(extra) = with_count(&r, n - 1)
            {
                give_player(host, click.player, extra);
            }
        }
        None => {
            let res = ItemResource::of(&held);
            let _ = host.transaction(|host, tx| if hand.extract(host, tx, &res, 1) == Ok(1) { Ok(()) } else { Err(()) });
        }
    }
    true
}

// ---- Tooltips (TankItem.appendHoverText, CopperCanItem.appendHoverText; Mantle's FluidTooltipHandler)

/// The item's lines (gray), or none for an item that isn't one of these.
pub(crate) fn tooltip(host: &dyn ClientHost, stack: &ItemStack, lines: &mut Vec<(Text, u32)>) {
    ensure_ids(host);
    let id = sid(stack);
    let gray = 0xFFAA_AAAA;
    if id == COPPER_CAN {
        match can_fluid(stack) {
            // "%s: 1 ingot", then F3+H's fluid id.
            Some(f) => {
                lines.push((Text::literal(lang(host, "item.tconstruct.copper_can.contents", "%s: 1 ingot").replacen("%s", &fluid_name(host, &f), 1)), gray));
            }
            None => block_tooltip(host, "item.tconstruct.copper_can.tooltip", lines),
        }
        return;
    }
    let block = id;
    if tank_item_capacity(block).is_none() && block != super::MELTER && block != super::HEATER {
        return;
    }
    match item_tank(stack).filter(|_| tank_item_capacity(block).is_some()) {
        Some((f, n)) => {
            let shift = shift_down(host);
            for l in super::screen::fluid_lines(host, &f, n, shift, false) {
                lines.push((Text::literal(l), gray));
            }
        }
        None => {
            let key = format!("block.{}.tooltip", block.replacen(':', ".", 1));
            block_tooltip(host, &key, lines);
        }
    }
}

/// Mantle's TooltipItem: the `.tooltip` key's lines, gray.
pub(crate) fn block_tooltip(host: &dyn ClientHost, key: &str, lines: &mut Vec<(Text, u32)>) {
    if let Some(t) = host.translate(key) {
        for l in t.split('\n') {
            lines.push((Text::literal(l.to_owned()), 0xFFAA_AAAA));
        }
    }
}
