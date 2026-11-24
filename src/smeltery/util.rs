//! Small helpers of the smeltery's first blocks: the saved compound's typed reads and writes, item ids without a host,
//! a block's tank as Horizon saves it (`fluids`: `[{Fluid, Amount}]`), stack components, text with a fallback.

use mod_api::text::TextContent;
use mod_api::*;
use std::collections::HashMap;
use std::sync::OnceLock;

pub(crate) type Pos = [i32; 3];

// ---- The compound's key type: the SDK's own, under this path for the port's modules

pub(crate) use mod_api::JavaString;

pub(crate) fn js(s: &str) -> JavaString {
    JavaString::from(s)
}

pub(crate) fn gi(n: &Compound, k: &str, d: i32) -> i32 {
    match n.get(&js(k)) {
        Some(Tag::Int(v)) => *v,
        Some(Tag::Short(v)) => i32::from(*v),
        Some(Tag::Byte(v)) => i32::from(*v),
        // A number from a JSON data file can come as any numeric tag (a component's codec reads them all).
        Some(Tag::Long(v)) => i32::try_from(*v).unwrap_or(d),
        Some(Tag::Float(v)) => f32::from_bits(*v) as i32,
        Some(Tag::Double(v)) => f64::from_bits(*v) as i32,
        _ => d,
    }
}

pub(crate) fn si(n: &mut Compound, k: &str, v: i32) {
    if n.get(&js(k)) != Some(&Tag::Int(v)) {
        n.insert(js(k), Tag::Int(v));
    }
}

pub(crate) fn gs(n: &Compound, k: &str) -> Option<String> {
    match n.get(&js(k)) {
        Some(Tag::String(s)) => Some(s.to_string_lossy().to_string()),
        _ => None,
    }
}

pub(crate) fn ss(n: &mut Compound, k: &str, v: &str) {
    let t = Tag::String(js(v));
    if n.get(&js(k)) != Some(&t) {
        n.insert(js(k), t);
    }
}

pub(crate) fn gc<'a>(n: &'a Compound, k: &str) -> Option<&'a Compound> {
    match n.get(&js(k)) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

pub(crate) fn gia(n: &Compound, k: &str) -> Vec<i32> {
    match n.get(&js(k)) {
        Some(Tag::IntArray(a)) => a.iter().copied().collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn sia(n: &mut Compound, k: &str, v: Vec<i32>) {
    let t = Tag::IntArray(v.into());
    if n.get(&js(k)) != Some(&t) {
        n.insert(js(k), t);
    }
}

pub(crate) fn rm(n: &mut Compound, k: &str) {
    n.remove(&js(k));
}

pub(crate) fn get_strings(n: &Compound, k: &str, len: usize) -> Vec<String> {
    let mut out: Vec<String> = match n.get(&js(k)) {
        Some(Tag::List(l)) => l
            .iter()
            .map(|t| match t {
                Tag::String(s) => s.to_string_lossy().to_string(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    out.resize(len, String::new());
    out
}

pub(crate) fn set_strings(n: &mut Compound, k: &str, v: &[String]) {
    let t = Tag::List(v.iter().map(|s| Tag::String(js(s))).collect());
    if n.get(&js(k)) != Some(&t) {
        n.insert(js(k), t);
    }
}

// ---- Tanks as Horizon saves a mod block's (`ModFluidTanks`: `fluids` = [{Fluid, Amount}], the first is tank 0)

/// One tank: its fluid (none when empty) and amount in mB.
pub(crate) type Tank = (Option<String>, i32);

pub(crate) fn tank_of(n: &Compound) -> Tank {
    let Some(Tag::List(list)) = n.get(&js("fluids")) else { return (None, 0) };
    let Some(Tag::Compound(m)) = list.iter().next() else { return (None, 0) };
    let fluid = gs(m, "Fluid").filter(|f| f != "minecraft:empty" && !f.is_empty());
    let amount = gi(m, "Amount", 0);
    if amount > 0 && fluid.is_some() { (fluid, amount) } else { (None, 0) }
}

pub(crate) fn tank_tag(fluid: Option<&str>, amount: i32) -> Tag {
    let mut m = Compound::new();
    let fluid = if amount > 0 { fluid.unwrap_or("minecraft:empty") } else { "minecraft:empty" };
    m.insert(js("Fluid"), Tag::String(js(fluid)));
    m.insert(js("Amount"), Tag::Int(amount.max(0)));
    Tag::List(vec![Tag::Compound(m)].into_iter().collect())
}

pub(crate) fn set_tank(n: &mut Compound, fluid: Option<&str>, amount: i32) {
    let t = tank_tag(fluid, amount);
    if n.get(&js("fluids")) != Some(&t) {
        n.insert(js("fluids"), t);
    }
}

/// FluidTankBase.fill, simulated: what of `amount` of `fluid` the tank takes (into an empty tank or the same fluid).
pub(crate) fn fill_room(tank: &Tank, capacity: i32, fluid: &str, amount: i32) -> i32 {
    match &tank.0 {
        None => amount.min(capacity).max(0),
        Some(f) if f == fluid => amount.min(capacity - tank.1).max(0),
        Some(_) => 0,
    }
}

/// ITankBlockEntity.getComparatorStrength: 0 empty, else 1 + 14 * amount / capacity.
pub(crate) fn signal(amount: i32, capacity: i32) -> i32 {
    if amount <= 0 || capacity <= 0 { 0 } else { 1 + (14 * i64::from(amount) / i64::from(capacity)) as i32 }
}

// ---- Item ids without a host: the registry is fixed for the process, so the maps are made once.

pub(crate) struct Ids {
    by_key: HashMap<ItemKey, &'static str>,
    by_id: HashMap<&'static str, ItemKey>,
}

static IDS: OnceLock<Ids> = OnceLock::new();

/// Makes the item maps (every hook with a host calls this before reading item ids).
pub(crate) fn ensure_ids(host: &dyn Host) {
    IDS.get_or_init(|| {
        let mut by_key = HashMap::new();
        let mut by_id = HashMap::new();
        for k in host.item_keys() {
            let id: &'static str = Box::leak(k.id(host).unwrap_or_default().to_owned().into_boxed_str());
            by_key.insert(k, id);
            by_id.insert(id, k);
        }
        Ids { by_key, by_id }
    });
}

pub(crate) fn key_id(k: ItemKey) -> &'static str {
    IDS.get().and_then(|i| i.by_key.get(&k).copied()).unwrap_or("")
}

pub(crate) fn sid(s: &ItemStack) -> &'static str {
    key_id(s.item())
}

pub(crate) fn item_key(id: &str) -> Option<ItemKey> {
    IDS.get().and_then(|i| i.by_id.get(id).copied())
}

pub(crate) fn new_stack(id: &str, count: u32) -> Option<ItemStack> {
    ItemStack::new(item_key(id)?, count)
}

pub(crate) fn with_count(stack: &ItemStack, n: u32) -> Option<ItemStack> {
    if n == 0 {
        return None;
    }
    let mut out = stack.clone();
    out.set_count(n).ok()?;
    Some(out)
}

pub(crate) fn max_stack(host: &dyn Host, stack: &ItemStack) -> u32 {
    host.max_stack_size(stack.item()).unwrap_or(64).clamp(1, 99)
}

/// ForgeHooks.getBurnTime.
pub(crate) fn burn_time(host: &dyn Host, stack: &ItemStack) -> i32 {
    host.item_burn_time(stack.item()).unwrap_or(0).max(0)
}

// ---- Stack components

pub(crate) fn component(stack: &ItemStack, id: &str) -> Option<Tag> {
    stack.components()?.as_nbt().get(&js(id)).cloned()
}

pub(crate) fn set_component(stack: &mut ItemStack, id: &str, v: Option<Tag>) {
    let mut patch: Compound = stack.components().map(Compound::from).unwrap_or_default();
    match v {
        Some(t) => {
            patch.insert(js(id), t);
        }
        None => {
            patch.remove(&js(id));
        }
    }
    if let Err(e) = stack.set_components(patch) {
        eprintln!("Tinkers' Construct: component {id} refused: {e:?}");
    }
}

pub(crate) fn custom_data(stack: &ItemStack) -> Compound {
    match component(stack, "minecraft:custom_data") {
        Some(Tag::Compound(c)) => c,
        _ => Compound::new(),
    }
}

pub(crate) fn set_custom_data(stack: &mut ItemStack, data: Compound) {
    set_component(stack, "minecraft:custom_data", (!data.is_empty()).then_some(Tag::Compound(data)));
}

// ---- Places

pub(crate) fn here(host: &dyn Host) -> Option<LevelRef> {
    host.level(host.dimension())
}

pub(crate) fn below(p: Pos) -> Pos {
    [p[0], p[1] - 1, p[2]]
}

pub(crate) fn above(p: Pos) -> Pos {
    [p[0], p[1] + 1, p[2]]
}

pub(crate) fn block_id(host: &dyn Host, level: LevelRef, pos: Pos) -> String {
    level.block(host, pos).and_then(|b| b.id(host)).unwrap_or_default().to_owned()
}

// ---- Text

/// A translatable line with an English fallback (the client shows the fallback where its language has no key, as
/// Mantle's own lang isn't in the content).
pub(crate) fn tr(key: &str, fallback: &str, args: Vec<Text>) -> Text {
    Text::new(TextContent::Translatable { key: key.to_owned(), fallback: Some(fallback.to_owned()), arguments: args })
}

/// FluidStack.getDisplayName: vanilla's by its block, Tinkers' `fluid.tconstruct.<name>`.
pub(crate) fn fluid_text(fluid: &str) -> Text {
    let (ns, name) = fluid.split_once(':').unwrap_or(("minecraft", fluid));
    let key = if ns == "minecraft" { format!("block.minecraft.{name}") } else { format!("fluid.{ns}.{name}") };
    tr(&key, &words(name), Vec::new())
}

/// A path in words: `molten_iron` -> `Molten Iron`.
pub(crate) fn words(path: &str) -> String {
    path.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The client's translation of `key`, else `english`.
pub(crate) fn lang(host: &dyn ClientHost, key: &str, english: &str) -> String {
    host.translate(key).unwrap_or_else(|| english.to_owned())
}

/// The fluid's name on the client.
pub(crate) fn fluid_name(host: &dyn ClientHost, fluid: &str) -> String {
    let (ns, name) = fluid.split_once(':').unwrap_or(("minecraft", fluid));
    let key = if ns == "minecraft" { format!("block.minecraft.{name}") } else { format!("fluid.{ns}.{name}") };
    lang(host, &key, &words(name))
}

/// TranslationHelper.COMMA_FORMAT.
pub(crate) fn commas(v: i64) -> String {
    let s = v.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if v < 0 { format!("-{out}") } else { out }
}

pub(crate) fn shift_down(host: &dyn ClientHost) -> bool {
    host.is_key_down("key.keyboard.left.shift") || host.is_key_down("key.keyboard.right.shift")
}

/// The mod's diagnostic log: each line to stderr (the launcher's `<instance>.log` in the game folder) and appended to
/// `tconstruct-debug.log` in the game's folder (the working directory), so a test can read it back.
/// Debug logging is off by default (release builds don't log every click). On when the environment variable
/// `TCONSTRUCT_DEBUG` is set to anything but `0`, or when `config/tconstruct-debug.flag` exists in the game folder.
static RECIPE_GEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The recipes or tags changed (`tags_updated`, `recipes_received`): the caches made from them are made again.
pub(crate) fn recipes_changed() {
    RECIPE_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The key of the caches made from the recipe tables of `kinds`: the data load they are of (`recipes_changed`). The
/// host answers each `recipes_of_type` with a new copy, so a table's address can't tell a reload, and asking it on
/// every block entity tick cost milliseconds; it is asked once per data load. While every table is empty (outside a
/// world, the data not loaded yet) the key is new each time, so nothing empty is kept.
pub(crate) fn tables_key(host: &dyn Host, kinds: &[&str]) -> Vec<(usize, usize)> {
    use std::hash::{Hash, Hasher};
    thread_local! {
        static SEEN: std::cell::RefCell<HashMap<u64, usize>> = std::cell::RefCell::new(HashMap::new());
    }
    static EMPTY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let load = RECIPE_GEN.load(std::sync::atomic::Ordering::Relaxed);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    kinds.hash(&mut h);
    let h = h.finish();
    if SEEN.with_borrow(|m| m.get(&h) == Some(&load)) {
        return vec![(load, 0)];
    }
    if kinds.iter().all(|k| host.recipes_of_type(k).is_empty()) {
        return vec![(usize::MAX, EMPTY.fetch_add(1, std::sync::atomic::Ordering::Relaxed))];
    }
    SEEN.with_borrow_mut(|m| m.insert(h, load));
    vec![(load, 0)]
}

pub(crate) fn debug_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("TCONSTRUCT_DEBUG").map(|v| !v.is_empty() && v != "0").unwrap_or(false)
            || std::path::Path::new("config/tconstruct-debug.flag").exists()
    })
}

pub(crate) fn tlog(line: &str) {
    use std::io::Write;
    if !debug_on() {
        return;
    }
    eprintln!("Tinkers' Construct: {line}");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("tconstruct-debug.log") {
        let _ = writeln!(f, "{line}");
    }
}

// ---- Held stacks as owned copies

/// The held-stack reads as the ports were written against them (Sunrise 9425651: an owned `ItemStack`). Main lends a
/// view for the call instead (`with_hand_item`, `with_equipment`, `with_stack`); these take its projection, the same
/// owned copy, components included. `None` as before for an empty or unreadable place, and also for a stack the game
/// can't project into the mod API.
pub(crate) trait LivingStacks {
    fn hand_item(self, host: &dyn Host, hand: Hand) -> Option<ItemStack>;
    fn equipment(self, host: &dyn Host, slot: EquipmentSlot) -> Option<ItemStack>;
}

impl LivingStacks for LivingRef {
    fn hand_item(self, host: &dyn Host, hand: Hand) -> Option<ItemStack> {
        self.with_hand_item(host, hand, |s| s.project()).flatten()
    }
    fn equipment(self, host: &dyn Host, slot: EquipmentSlot) -> Option<ItemStack> {
        self.with_equipment(host, slot, |s| s.project()).flatten()
    }
}

/// `ItemPlace::stack` and `ItemEntityRef::stack` as owned copies (see `LivingStacks`).
pub(crate) trait PlaceStack {
    fn stack(&self, host: &mut dyn ServerHost) -> Option<ItemStack>;
}

impl PlaceStack for ItemPlace {
    fn stack(&self, host: &mut dyn ServerHost) -> Option<ItemStack> {
        self.with_stack(host, |s| s.project()).flatten()
    }
}

pub(crate) trait ItemEntityStack {
    fn stack(self, host: &dyn Host) -> Option<ItemStack>;
}

impl ItemEntityStack for ItemEntityRef {
    fn stack(self, host: &dyn Host) -> Option<ItemStack> {
        self.with_stack(host, |s| s.project()).flatten()
    }
}

/// The entities in a box as a list of handles, as `entities_in` gave them: main visits them (`visit_entities_in`), and
/// the callers hurt, move and discard them after the visit. `None` for a stale or unreadable level, as before.
pub(crate) fn entities_in(host: &dyn Host, level: LevelRef, min: [f64; 3], max: [f64; 3]) -> Option<Vec<EntityRef>> {
    let mut out = Vec::new();
    // The visit never breaks: its ControlFlow says nothing more.
    let _ = level
        .visit_entities_in(host, min, max, &mut |e| {
            out.push(e);
            std::ops::ControlFlow::Continue(())
        })
        .ok()?;
    Some(out)
}

// ---- Read-only level views (block capability providers decide on these)

/// The block id of `state` in a read-only level view ("" for none, as `block_id`).
pub(crate) fn view_block_id(level: &dyn LevelHost, state: BlockState) -> String {
    level.state_block(state).unwrap_or_default().to_owned()
}

/// `state`'s property `name` through a read-only level view's tables, as `LevelRef::block_property` reads it.
pub(crate) fn view_property(level: &dyn LevelHost, state: BlockState, name: &str) -> Option<String> {
    state.property(&level.block_states(), name).ok().flatten().map(str::to_owned)
}

/// Workaround for sunrise-org/sunrise#4463 (Sunrise main): a new block entity holds no item slots until one is
/// written, and a menu opened on it is sized by the slots it holds, not by its container's `slots`. A shift-click
/// there is mapped to the wrong inventory slot and the clicked stack stays as it moves (a duplicate). Writing the last
/// declared slot of `block` once, and clearing it, makes the entity hold all of them and leaves its contents as they
/// were. Called just before every `open_menu` on a block entity (melter, heater, controllers, duct, stations).
pub(crate) fn pad_slots(host: &mut dyn ServerHost, entity: BlockEntityRef, block: &str) {
    let Some(c) = crate::smeltery::block_entity_container(block).or_else(|| crate::tools::station::container(block)) else { return };
    let Some(last) = c.slots.checked_sub(1) else { return };
    if !matches!(entity.item_slot(&*host, last as u32), Ok(None)) {
        return;
    }
    let Some(filler) = new_stack("minecraft:stone", 1) else { return };
    let _ = host.transaction(|host, tx| {
        entity.set_item_slot(host, tx, last as u32, Some(filler)).map_err(|_| ())?;
        entity.set_item_slot(host, tx, last as u32, None).map_err(|_| ())?;
        Ok::<(), ()>(())
    });
}

/// `block_entity_on_load` (IBlockEntityExtension.onLoad): the same padding as `pad_slots`, as soon as a block entity
/// joins its level (placed, `/setblock`, or loaded), so hoppers, pipes and commands can insert into a fresh block too
/// (sunrise-org/sunrise#4463: until a slot is written, the container takes nothing).
///
/// The hook lends the entity through its scope: its handle, position and level come from the view, and the padding
/// then runs through the Host as before (no view is live while it does).
pub(crate) fn pad_on_load(subject: &mut BlockEntitySideScope<'_>) {
    let BlockEntitySideScope::Server(scope) = subject else { return };
    let Some((entity, pos, level)) = scope.with_view(|v| (v.handle(), v.position(), v.level())) else { return };
    let host = scope.host();
    let Some(block) = level.block(&*host, pos).and_then(|b| b.id(&*host).map(str::to_owned)) else { return };
    pad_slots(host, entity, &block);
}

/// The placer's yaw as vanilla's `BlockPlaceContext.getRotation`, for the facing rules: a player's view, or for a
/// `DirectionalPlaceContext` (a dispenser) its direction's (`get2DDataValue() * 90`, a vertical one as north,
/// following its `getHorizontalDirection`).
pub(crate) fn placement_yaw(c: &PlacementContext) -> f32 {
    match c.source {
        PlacementSource::Player { yaw, .. } => yaw,
        PlacementSource::Directional { direction } => match direction {
            Direction::South => 0.0,
            Direction::West => 90.0,
            Direction::East => 270.0,
            _ => 180.0,
        },
        _ => 180.0,
    }
}

/// Whether the placer is sneaking (`isSecondaryUseActive`): never for a `DirectionalPlaceContext`.
pub(crate) fn placement_sneaking(c: &PlacementContext) -> bool {
    matches!(c.source, PlacementSource::Player { sneaking: true, .. })
}
