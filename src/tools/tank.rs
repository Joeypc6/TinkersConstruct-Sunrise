//! The tool's fluid tank (ToolTankHelper, `tconstruct:tank_handler`) and the modifiers that use it: bucketing
//! (BucketModule: pick a fluid source up into the tank, sneak-use a face to place 1000 mB) and spilling
//! (SpillingModule: a hit spends the tank's fluid on the target through its fluid effects, FluidEffectManager).
//!
//! The tank is the persistent data `tconstruct:tank_fluid` ({FluidName, Amount}, Java's FluidStack NBT), its size the
//! stat `tconstruct:tank_capacity` (the modifiers' stat boosts); every tool answers `ITEM_FLUIDS` with it, so the
//! melter, tanks, drains and other mods' machines fill and drain it.

use super::modifier as md;
use super::tool::*;
use crate::smeltery::containers::{fluid_id, fluid_res};
use crate::smeltery::util::*;
use mod_api::*;
use serde_json::Value;
use std::sync::OnceLock;
use std::collections::HashMap;
use std::sync::Mutex;

thread_local! {
    /// The projectile's motion while a ranged spill applies (PushEntityFluidEffect's direction).
    static PUSH_DIR: std::cell::Cell<Option<[f64; 3]>> = const { std::cell::Cell::new(None) };
}

const KEY: &str = "tconstruct:tank_fluid";
const ST_CAPACITY: &str = "tconstruct:tank_capacity";

/// The tank's capacity (0 without a tank modifier).
pub(crate) fn capacity(stack: &ItemStack) -> i32 {
    if !tool_initialized(stack) {
        return 0;
    }
    tool_stat(stack, ST_CAPACITY) as i32
}

/// ToolTankHelper.getFluid.
pub(crate) fn fluid(stack: &ItemStack) -> Option<(String, i32)> {
    let Some(Tag::Compound(p)) = component(stack, md::C_PERSISTENT) else { return None };
    let Some(Tag::Compound(f)) = p.get(&js(KEY)) else { return None };
    let (id, n) = (gs(f, "FluidName")?, gi(f, "Amount", 0));
    (n > 0).then_some((id, n))
}

/// ToolTankHelper.setFluid: clamped to the capacity; empty removes it.
pub(crate) fn set_fluid(stack: &mut ItemStack, f: Option<(&str, i32)>) {
    let cap = capacity(stack);
    match f.filter(|(_, n)| *n > 0) {
        Some((id, n)) => {
            let mut c = Compound::new();
            c.insert(js("FluidName"), Tag::String(js(id)));
            c.insert(js("Amount"), Tag::Int(n.min(cap)));
            md::set_persistent(stack, KEY, Some(Tag::Compound(c)));
        }
        None => md::set_persistent(stack, KEY, None),
    }
}

/// `ITEM_FLUIDS` of a tool (ToolFluidCapability): one tank.
pub(crate) struct ToolTank {
    place: ItemPlace,
}

impl ToolTank {
    fn write(&self, host: &mut dyn ServerHost, tx: &TransactionContext, s: &ItemStack, f: Option<(&str, i32)>) -> bool {
        let mut one = s.clone();
        set_fluid(&mut one, f);
        self.place.exchange(host, tx, &ItemResource::of(&one), 1) == Ok(1)
    }
}

impl ResourceHandler<FluidResource> for ToolTank {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, _index: usize) -> Option<FluidResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        let (f, n) = fluid(&self.place.stack(host)?)?;
        fluid_res(&*host, &f, n as u32)
    }
    fn amount(&self, host: &mut dyn ServerHost, _index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.place.stack(host).and_then(|s| fluid(&s)).map_or(0, |(_, n)| n as u32)
    }
    fn capacity(&self, host: &mut dyn ServerHost, _index: usize, _resource: Option<&FluidResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.place.stack(host).map_or(0, |s| capacity(&s).max(0) as u32)
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, _index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            let Some(s) = self.place.stack(host) else { return 0 };
            if s.count() != 1 || resource.to_stack(1).has_components() {
                return 0;
            }
            let cap = capacity(&s);
            let Some(id) = fluid_id(&*host, resource) else { return 0 };
            let (have, n) = fluid(&s).map_or((None, 0), |(f, n)| (Some(f), n));
            if have.as_deref().is_some_and(|f| f != id) {
                return 0;
            }
            let k = (amount.min(i32::MAX as u32) as i32).min(cap - n);
            if k <= 0 {
                return 0;
            }
            if self.write(host, tx, &s, Some((&id, n + k))) { k as u32 } else { 0 }
        })())
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, _index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            let Some(s) = self.place.stack(host) else { return 0 };
            let Some((f, n)) = fluid(&s) else { return 0 };
            if s.count() != 1 || fluid_id(&*host, resource).as_deref() != Some(f.as_str()) {
                return 0;
            }
            let k = (amount.min(i32::MAX as u32) as i32).min(n);
            if k <= 0 {
                return 0;
            }
            if self.write(host, tx, &s, Some((&f, n - k))) { k as u32 } else { 0 }
        })())
    }
}

pub(crate) fn register(r: &mut dyn Registrar) {
    let ids: Vec<String> = super::data::table().json["tool_definitions"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    for id in ids {
        let id: &'static str = Box::leak(id.into_boxed_str());
        r.item_capability_before_game(&ITEM_FLUIDS, id, |_, place: &ItemPlace| Some(Box::new(ToolTank { place: place.clone() }) as Box<dyn ResourceHandler<FluidResource>>));
    }
}

// ---- Fluid effects (FluidEffectManager)

fn effects() -> &'static Vec<Value> {
    static E: OnceLock<Vec<Value>> = OnceLock::new();
    E.get_or_init(|| {
        let v: Value = serde_json::from_str(crate::gen_fluid_effects::FLUID_EFFECTS_JSON).unwrap_or_default();
        v.as_object().map(|o| o.values().cloned().collect()).unwrap_or_default()
    })
}

/// FluidEffectManager.find: the first effects file whose fluid ingredient (a fluid or a fluid tag) holds `fluid`.
fn find(host: &dyn Host, fluid: &str) -> Option<&'static Value> {
    effects().iter().find(|e| {
        let f = &e["fluid"];
        if let Some(id) = f["fluid"].as_str() {
            return id == fluid;
        }
        f["tag"].as_str().is_some_and(|t| host.fluid_in_tag(fluid, t).unwrap_or(false) || host.fluid_in_tag(fluid, &crate::smeltery::recipes::c_tag(t)).unwrap_or(false) || t.rsplit(':').next() == fluid.rsplit(':').next())
    })
}

/// mantle:fire_immune and its inverse, the predicates the effects test the target with (others pass).
fn target_ok(host: &dyn Host, target: EntityRef, pred: &Value) -> bool {
    // `EntityType.fireImmune`, the game's answer for every mob (a mod's too).
    let fire_immune = || target.fire_immune(host).unwrap_or(false);
    match pred["type"].as_str() {
        Some("mantle:fire_immune") => fire_immune(),
        Some("mantle:inverted") if pred["inverted_type"].as_str() == Some("mantle:fire_immune") => !fire_immune(),
        _ => true,
    }
}

/// One entity effect at `level` (FluidEffect.applyToEntity): true when it acted.
fn apply(host: &mut dyn ServerHost, effect: &Value, level: f32, attacker: LivingRef, target: EntityRef) -> bool {
    match effect["type"].as_str().unwrap_or("") {
        "tconstruct:conditional" => target_ok(&*host, target, &effect["target"]) && apply(host, &effect["effect"], level, attacker, target),
        "tconstruct:scaling" => {
            // ScalingFluidEffect: the last step at or under the level.
            let steps = effect["effects"].as_array().cloned().unwrap_or_default();
            let step = steps.iter().filter(|s| s["level"].as_f64().unwrap_or(0.0) as f32 <= level).last();
            step.is_some_and(|s| apply(host, &s["effect"], level, attacker, target))
        }
        "tconstruct:sequence" => {
            let mut any = false;
            for e in effect["effects"].as_array().cloned().unwrap_or_default() {
                any |= apply(host, &e, level, attacker, target);
            }
            any
        }
        "tconstruct:damage" => {
            let ty = effect["damage_type"]["melee"].as_str().or_else(|| effect["damage_type"].as_str()).unwrap_or("minecraft:generic");
            let amount = effect["damage"].as_f64().unwrap_or(0.0) as f32 * level;
            target.hurt(host, ty, amount, Some(attacker.entity()), Some(attacker.entity())).unwrap_or(false)
        }
        "tconstruct:fire" => {
            let ticks = (effect["time"].as_f64().unwrap_or(0.0) as f32 * level * 20.0) as i32;
            let mut c = Compound::new();
            c.insert(js("Fire"), Tag::Short(ticks.min(i32::from(i16::MAX)) as i16));
            ticks > 0 && target.merge_nbt(host, c)
        }
        "tconstruct:mob_effect" => {
            let Some(id) = effect["effect"].as_str() else { return false };
            let time = (effect["time"].as_f64().unwrap_or(0.0) as f32 * level) as i32;
            let amp = effect["level"].as_i64().unwrap_or(1) as i32 - 1;
            let living = LivingRef::new(target);
            if effect["action"].as_str() == Some("remove") {
                return false;
            }
            md::give_effect(host, living, id, time, amp);
            time > 0
        }
        // RestoreHungerFluidEffect: a player who can eat (`canEat(canAlwaysEat)`) eats hunger x level, the saturation
        // modifier as is (FoodData.eat).
        "tconstruct:restore_hunger" => {
            let Some(p) = target.uuid(host).and_then(|u| host.player(u)) else { return false };
            let Some(f) = p.food(&*host) else { return false };
            if f.level >= 20 && !effect["can_always_eat"].as_bool().unwrap_or(false) {
                return false;
            }
            let hunger = (effect["hunger"].as_f64().unwrap_or(0.0) as f32 * level) as i32;
            let sat = effect["saturation"].as_f64().unwrap_or(0.0) as f32;
            let food = (f.level + hunger).min(20);
            let _ = p.set_food_level(host, food);
            let _ = p.set_saturation(host, (f.saturation + hunger as f32 * sat * 2.0).min(food as f32));
            true
        }
        // PushEntityFluidEffect: along the projectile's motion, else from the holder to the target, by the leveling
        // value (`flat + each_level x level`; a flat one only at a full level).
        "tconstruct:push_entity" => {
            let flat = effect["flat"].as_f64().unwrap_or(0.0);
            let each = effect["each_level"].as_f64().unwrap_or(0.0);
            if each == 0.0 && level < 1.0 {
                return false;
            }
            let k = flat + each * f64::from(level);
            let dir = PUSH_DIR.with(|d| d.get()).or_else(|| {
                let (a, t) = (attacker.entity().position(&*host)?, target.position(&*host)?);
                Some([t[0] - a[0], t[1] - a[1], t[2] - a[2]])
            });
            let Some(d) = dir else { return false };
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if len < 1.0e-7 || k == 0.0 {
                return false;
            }
            let push = [d[0] / len * k, d[1] / len * k, d[2] / len * k];
            let ok = target.push(host, push);
            tlog(&format!("fluid push {push:?}: {ok}"));
            ok
        }
        // RemoveEffectFluidEffect / CureEffectsFluidEffect (milk cures every effect in 26.3).
        "tconstruct:remove_effect" => {
            let (Some(id), Some(living)) = (effect["effect"].as_str(), host.living(target)) else { return false };
            md::effect_key(&*host, id).or_else(|| crate::gadgets::effect(id)).is_some_and(|k| living.remove_effect(host, k))
        }
        "tconstruct:cure_effects" => {
            let Some(living) = host.living(target) else { return false };
            let list = living.effects(&*host).unwrap_or_default();
            for e in &list {
                let _ = living.remove_effect(host, e.id);
            }
            !list.is_empty()
        }
        "tconstruct:extinguish" => target.set_remaining_fire_ticks(host, 0),
        other => {
            tlog(&format!("fluid effect {other}: not ported"));
            false
        }
    }
}

/// FluidEffects.applyToEntity: the level is the fluid there is over the file's amount, at most `scale`; returns the
/// fluid spent (the amount a level, scaled by the level the effects used).
fn apply_to_entity(host: &mut dyn ServerHost, fluid: &str, have: i32, scale: f32, attacker: LivingRef, target: EntityRef) -> i32 {
    let Some(doc) = find(&*host, fluid) else { return 0 };
    let per = doc["fluid"]["amount"].as_i64().unwrap_or(0) as i32;
    let list = doc["entity_effects"].as_array().cloned().unwrap_or_default();
    if per <= 0 || list.is_empty() {
        return 0;
    }
    let level = scale.min(have as f32 / per as f32);
    if level <= 0.0 {
        return 0;
    }
    let mut used = false;
    for e in &list {
        used |= apply(host, e, level, attacker, target);
    }
    if used { ((per as f32) * level).ceil() as i32 } else { 0 }
}

/// SpillingModule.afterMeleeHit: the hit spends the tank on the target (`level` = spilling's level).
pub(crate) fn spill(host: &mut dyn ServerHost, attacker: LivingRef, stack: &ItemStack, target: u32) -> Option<ItemStack> {
    let level = md::modifier_level(stack, "tconstruct:spilling");
    if level <= 0 {
        return None;
    }
    let (f, n) = fluid(stack)?;
    let target = host.entity(target)?;
    let spent = apply_to_entity(host, &f, n, level as f32, attacker, target);
    tlog(&format!("spilling {f} ({n} mB) on {}: {spent} mB", target.id()));
    if spent <= 0 {
        return None;
    }
    // ModifierUtil.consumesResources: creative players keep the fluid.
    if attacker.entity().kind(&*host).is_some_and(|k| k.id(&*host).as_deref() == Some("minecraft:player")) && host.players().acting_player().is_some_and(|p| p.is_creative(&*host).unwrap_or(false)) {
        return None;
    }
    let mut out = stack.clone();
    set_fluid(&mut out, Some((&f, n - spent)));
    Some(out)
}

// ---- Bucketing (BucketModule)

/// The world fluid's block (the fork names it after the fluid; vanilla's water and lava).
fn fluid_block(fluid: &str) -> String {
    fluid.to_owned()
}

/// BucketModule.onToolUse (not sneaking): the first fluid source on the look within reach goes into the tank (a
/// whole 1000 mB, the same fluid or an empty tank with room).
pub(crate) fn bucket_pickup(host: &mut dyn ServerHost, living: LivingRef, hand: Hand, stack: &ItemStack) -> Option<ItemStack> {
    if md::modifier_level(stack, "tconstruct:bucketing") <= 0 {
        return None;
    }
    let shift = living.entity().is_shift_key_down(&*host);
    let (cur, n) = fluid(stack).map_or((None, 0), |(f, n)| (Some(f), n));
    tlog(&format!("bucketing use: shift {shift:?}, capacity {}, tank {cur:?} {n}", capacity(stack)));
    if shift.unwrap_or(false) {
        return None;
    }
    if capacity(stack) - n < 1000 {
        return None;
    }
    let e = living.entity();
    let (eye, look) = (e.eye_position(&*host)?, e.view_vector(&*host)?);
    let level = here(&*host)?;
    let reach = 5.0;
    let end = [eye[0] + look[0] * reach, eye[1] + look[1] * reach, eye[2] + look[2] * reach];
    let solid = level.clip(&*host, eye, end, ClipBlock::Outline, ClipFluid::None).filter(|h| !h.miss);
    let max = solid.map_or(reach, |h| ((h.location[0] - eye[0]).powi(2) + (h.location[1] - eye[1]).powi(2) + (h.location[2] - eye[2]).powi(2)).sqrt());
    tlog(&format!("bucketing ray from {eye:?} along {look:?}, solid at {max:.2}"));
    let mut t = 0.0;
    while t <= max {
        let p = [(eye[0] + look[0] * t).floor() as i32, (eye[1] + look[1] * t).floor() as i32, (eye[2] + look[2] * t).floor() as i32];
        if let Some(fs) = level.fluid_state(&*host, p).filter(|f| f.source) {
            let id = fs.fluid.id(&*host).unwrap_or_default().to_owned();
            if cur.as_deref().is_some_and(|c| c != id) {
                return None;
            }
            let block = level.block_state(&*host, p)?.block().id(&*host).unwrap_or_default().to_owned();
            if block != fluid_block(&id) {
                return None;
            }
            let air = host.block_state_id("minecraft:air")?;
            if !matches!(level.set_block(host, p, air, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS), Ok(true)) {
                return None;
            }
            let mut out = stack.clone();
            set_fluid(&mut out, Some((&id, n + 1000)));
            tlog(&format!("bucketing picked up {id} at {p:?}: {} mB", n + 1000));
            let _ = hand;
            return Some(out);
        }
        t += 0.1;
    }
    None
}

/// BucketModule.afterBlockUse (sneaking): 1000 mB placed in the clicked block or the one beside the face.
pub(crate) fn bucket_place(host: &mut dyn ServerHost, living: LivingRef, stack: &ItemStack, pos: [i32; 3], face: [i32; 3]) -> Option<ItemStack> {
    if md::modifier_level(stack, "tconstruct:bucketing") <= 0 || !living.entity().is_shift_key_down(&*host).unwrap_or(false) {
        return None;
    }
    let (f, n) = fluid(stack)?;
    if n < 1000 {
        return None;
    }
    let level = here(&*host)?;
    let target = [pos[0] + face[0], pos[1] + face[1], pos[2] + face[2]];
    let existing = level.block_state(&*host, target)?;
    let replaceable = host.block_in_tag(existing.block(), "minecraft:replaceable").unwrap_or(false) || existing.block().id(&*host).is_some_and(|b| b.ends_with(":air"));
    if !replaceable {
        return None;
    }
    let state = host.block_state_id(&fluid_block(&f))?;
    if !matches!(level.set_block(host, target, state, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS), Ok(true)) {
        return None;
    }
    let mut out = stack.clone();
    set_fluid(&mut out, Some((&f, n - 1000)));
    tlog(&format!("bucketing placed {f} at {target:?}"));
    Some(out)
}

// ---- Spilling on ranged hits (SpillingModule.onLauncherHitEntity)

/// The launcher's tank on each arrow it shot: (owner, fluid, mB, spilling level). Java's arrow carries the launcher's
/// tool data, so a hit spends the arrow's copy, not the launcher's tank.
static ARROWS: Mutex<Option<HashMap<u32, (u32, String, i32, i32)>>> = Mutex::new(None);

pub(crate) fn note_arrow(id: u32, owner: u32, tool: &ItemStack) {
    let level = md::modifier_level(tool, "tconstruct:spilling");
    let Some((f, n)) = fluid(tool).filter(|_| level > 0) else { return };
    let mut a = ARROWS.lock().unwrap_or_else(|e| e.into_inner());
    let map = a.get_or_insert_default();
    if map.len() > 2048 {
        map.clear();
    }
    map.insert(id, (owner, f, n, level));
}

/// `projectile_impact` on an entity for an arrow a spilling launcher shot.
pub(crate) fn arrow_hit(host: &mut dyn ServerHost, projectile: u32, target: Option<u32>) {
    let Some((owner, f, n, level)) = ARROWS.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(&projectile)) else { return };
    let (Some(t), Some(o)) = (target.and_then(|t| host.entity(t)), host.entity(owner)) else { return };
    let dir = host.entity(projectile).and_then(|p| p.velocity(&*host));
    PUSH_DIR.with(|d| d.set(dir));
    let spent = apply_to_entity(host, &f, n, level as f32, LivingRef::new(o), t);
    PUSH_DIR.with(|d| d.set(None));
    tlog(&format!("ranged spilling {f} ({n} mB) on {}: {spent} mB", t.id()));
}
