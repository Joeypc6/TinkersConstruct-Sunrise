//! Stage 3D: the launchers (ModifiableBowItem for the longbow, ModifiableCrossbowItem) and the shields' blocking.
//! A launcher is used over time (`use_duration` 72000, the bow or crossbow pose); the draw time is
//! `ceil(20 / draw speed)` (GeneralInteractionModifierHook.getDrawtime) and the charge `(c² + 2c) / 3` of the drawn
//! fraction, at most 1. The longbow shoots on release at `charge · velocity · 3` with `3 · (1 / accuracy - 1)`
//! inaccuracy, a critical arrow at full charge, the arrow's damage the tool's projectile damage; the crossbow loads
//! on a full draw (the ammo kept in its persistent data, `tconstruct:crossbow_ammo`) and fires on the next use at
//! `3 · velocity`, always critical. Ammo is vanilla's (`#minecraft:arrows`): the off hand, then the inventory, one
//! taken unless the player is in creative (then an arrow, picked up in creative only). Each shot wears the tool 1.
//! Shields raise and block with `minecraft:blocks_attacks` (content: the definition's block amount, half its angle).

use super::modifier as md;
use super::tool::*;
use crate::smeltery::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) const LONGBOW: &str = "tconstruct:longbow";
pub(crate) const CROSSBOW: &str = "tconstruct:crossbow";
const KEY_AMMO: &str = "tconstruct:crossbow_ammo";
const DURATION: i32 = 72000;

fn launcher(stack: &ItemStack) -> bool {
    matches!(sid(stack), LONGBOW | CROSSBOW) && tool_initialized(stack)
}

fn loaded(stack: &ItemStack) -> Option<Compound> {
    let Some(Tag::Compound(c)) = component(stack, md::C_PERSISTENT) else { return None };
    gc(&c, KEY_AMMO).cloned()
}

pub(crate) fn use_duration(event: &mut UseDurationEvent<'_>) {
    if launcher(event.stack) && !tool_broken(event.stack) && loaded(event.stack).is_none() {
        event.duration = DURATION;
    }
    if !tool_broken(event.stack) && !slings(event.stack).is_empty() {
        event.duration = DURATION;
    }
}

pub(crate) fn use_animation(event: &mut UseAnimationEvent<'_>) {
    if launcher(event.stack) || !slings(event.stack).is_empty() {
        *event.animation = if sid(event.stack) == CROSSBOW { UseAnimation::Crossbow } else { UseAnimation::Bow };
    }
}

/// GeneralInteractionModifierHook.getToolCharge.
fn charge(time: i32, drawtime: f32) -> f32 {
    if time < 0 {
        return 0.0;
    }
    let c = time as f32 / drawtime;
    ((c * c + c * 2.0) / 3.0).min(1.0)
}

fn drawtime(stack: &ItemStack) -> f32 {
    (20.0 / tool_stat(stack, "tconstruct:draw_speed").max(0.01)).ceil().max(1.0)
}

/// Vanilla's projectile search: the off hand, then the main inventory.
fn ammo_places(player: u128) -> impl Iterator<Item = ItemPlace> {
    std::iter::once(ItemPlace::PlayerHand { player, hand: Hand::Off }).chain((0..36).map(move |slot| ItemPlace::PlayerSlot { player, slot }))
}

/// BowAmmoModifierHook.consumeAmmo for one arrow: the stack taken (one of it), or in creative the found arrow's copy
/// or a plain arrow. None: no ammo.
fn take_ammo(host: &mut dyn ServerHost, player: u128, creative: bool) -> Option<ItemStack> {
    let arrows = crate::smeltery::casting_recipes::tag_items(&*host, "minecraft:arrows");
    let found = ammo_places(player).find_map(|p| p.stack(host).filter(|s| arrows.contains(sid(s))).map(|s| (p, s)));
    if creative {
        return found.and_then(|(_, s)| with_count(&s, 1)).or_else(|| new_stack("minecraft:arrow", 1));
    }
    let (place, stack) = found?;
    let res = ItemResource::of(&stack);
    let took = host.transaction(|host, tx| if place.extract(host, tx, &res, 1) == Ok(1) { Ok(()) } else { Err(()) });
    took.ok().and_then(|_| with_count(&stack, 1))
}

fn has_ammo(host: &mut dyn ServerHost, player: u128) -> bool {
    let arrows = crate::smeltery::casting_recipes::tag_items(&*host, "minecraft:arrows");
    ammo_places(player).any(|p| p.stack(host).is_some_and(|s| arrows.contains(sid(&s))))
}

/// AbstractArrow shot by `living` (Projectile.shootFromRotation): `spawn_projectile_from_rotation` takes thrown item
/// projectiles only at 514ecf9, so the arrow is summoned at the eyes (minus 0.1) with its motion, owner, damage, crit
/// and pickup in its NBT.
#[allow(clippy::too_many_arguments)]
fn shoot(host: &mut dyn ServerHost, living: LivingRef, owner: u128, tool: &ItemStack, ammo: &ItemStack, speed: f32, critical: bool, creative: bool, angle: f32) -> Option<u32> {
    let kind = if sid(ammo) == "minecraft:spectral_arrow" { "minecraft:spectral_arrow" } else { "minecraft:arrow" };
    let accuracy = tool_stat(tool, "tconstruct:accuracy").max(0.1);
    let inaccuracy = 3.0 * (1.0 / accuracy - 1.0);
    let e = living.entity();
    let (Some(eye), Some([yaw, pitch]), Some(dim)) = (e.eye_position(&*host), e.rotation(&*host), host.player(owner).and_then(|p| p.dimension(&*host))) else {
        return None;
    };
    let (y, p) = (f64::from(yaw + angle).to_radians(), f64::from(pitch).to_radians());
    let dir = [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()];
    // Projectile.shoot: the normalized direction plus triangular noise of 0.0172275 x inaccuracy, times the speed.
    let tri = || f64::from(md::rand_f32() - md::rand_f32()) * 0.0172275 * f64::from(inaccuracy);
    let m: Vec<f64> = dir.iter().map(|d| (d + tri()) * f64::from(speed)).collect();
    *LAST_MOTION.lock().unwrap_or_else(|e| e.into_inner()) = [m[0], m[1], m[2]];
    let motion: Vec<Tag> = m.iter().map(|v| Tag::Double(v.to_bits())).collect();
    let mut nbt = Compound::new();
    nbt.insert(js("Motion"), Tag::List(motion));
    nbt.insert(js("Owner"), Tag::IntArray(vec![(owner >> 96) as i32, (owner >> 64) as i32, (owner >> 32) as i32, owner as i32]));
    // AbstractArrow's base damage 2.0 - 2 + PROJECTILE_DAMAGE.
    nbt.insert(js("damage"), Tag::Double(f64::from(tool_stat(tool, "tconstruct:projectile_damage")).to_bits()));
    nbt.insert(js("crit"), Tag::Byte(i8::from(critical)));
    nbt.insert(js("pickup"), Tag::Byte(if creative { 2 } else { 1 }));
    if let Some(item) = with_count(ammo, 1).and_then(|a| a.to_nbt(&*host)) {
        nbt.insert(js("item"), Tag::Compound(item));
    }
    let id = host.summon(&dim.to_string(), kind, [eye[0], eye[1] - 0.1, eye[2]], nbt);
    if id.is_none() {
        tlog(&format!("the arrow of {} could not spawn", sid(tool)));
    }
    id
}

fn sound(host: &mut dyn ServerHost, living: LivingRef, id: &str, pitch: f32) {
    let (Some(level), Some(at)) = (here(&*host), living.entity().position(&*host)) else { return };
    let _ = level.play_sound(host, id, at, SoundSource::Players, 1.0, pitch, None);
}

/// The player's hand holding `stack`, with their uuid and creative flag (the releasing entity, else the acting
/// player).
fn holder(host: &mut dyn ServerHost, entity: Option<i32>, stack: &ItemStack) -> Option<(LivingRef, Hand, u128, bool)> {
    let e = match entity.and_then(|e| u32::try_from(e).ok()).and_then(|e| host.entity(e)) {
        Some(e) => e,
        None => host.players().acting_player()?.entity(),
    };
    let living = LivingRef::new(e);
    let hand = [Hand::Main, Hand::Off].into_iter().find(|h| living.hand_item(&*host, *h).is_some_and(|s| sid(&s) == sid(stack)));
    let uuid = e.uuid(host);
    let (Some(hand), Some(uuid)) = (hand, uuid) else {
        crate::smeltery::util::tlog(&format!("the releasing entity {entity:?} holds no {} ({:?})", sid(stack), uuid));
        return None;
    };
    let creative = host.player(uuid).and_then(|p| p.is_creative(&*host)).unwrap_or(false);
    Some((living, hand, uuid, creative))
}

/// ModifiableBowItem.releaseUsing / ModifiableCrossbowItem.releaseUsing, on the server.
pub(crate) fn release(host: &mut dyn ServerHost, event: &mut ReleaseUsingEvent<'_>) {
    let stack = event.stack;
    if !launcher(stack) || tool_broken(stack) {
        return;
    }
    let Some((living, hand, uuid, creative)) = holder(host, event.entity, stack) else { return };
    let time = DURATION - event.time_left;
    let draw = drawtime(stack);
    if sid(stack) == CROSSBOW {
        if (time as f32) < draw || loaded(stack).is_some() {
            return;
        }
        let Some(ammo) = take_ammo(host, uuid, creative) else { return };
        let Some(nbt) = ammo.to_nbt(&*host) else { return };
        let mut out = stack.clone();
        md::set_persistent(&mut out, KEY_AMMO, Some(Tag::Compound(nbt)));
        let _ = living.set_hand_item(host, hand, Some(out));
        let pitch = 1.0 / (md::rand_f32() * 0.5 + 1.0) + 0.2;
        sound(host, living, "minecraft:item.crossbow.loading_end", pitch);
        event.used = true;
        return;
    }
    let c = charge(time, draw);
    let power = c * tool_stat(stack, "tconstruct:velocity");
    if power < 0.1 {
        crate::smeltery::util::tlog(&format!("a bow released at {time} of {draw} ticks: too weak ({power})"));
        return;
    }
    let Some(ammo) = take_ammo(host, uuid, creative) else {
        crate::smeltery::util::tlog(&format!("a drawn bow found no ammo"));
        return;
    };
    // ModifiableBowItem.releaseUsing: 1 + multishot arrows, 10 degrees apart around the look; each more one taken.
    let count = 1 + md::volatile_int(stack, "tconstruct:multishot").max(0);
    let start = -5.0 * (count - 1) as f32;
    let mut shot = 0;
    for i in 0..count {
        let a = if i == 0 { Some(ammo.clone()) } else { take_ammo(host, uuid, creative) };
        let Some(a) = a else { break };
        let angle = start + 10.0 * i as f32;
        if let Some(id) = shoot(host, living, uuid, stack, &a, power * 3.0, c >= 1.0, creative || i > 0 && creative, angle) {
            track_arrow(id, stack);
            super::tank::note_arrow(id, living.entity().id(), stack);
            let pitch = 1.0 / (md::rand_f32() * 0.4 + 1.2) + c * 0.5 + angle / 10.0;
            sound(host, living, "minecraft:entity.arrow.shoot", pitch);
            shot += 1;
        }
    }
    wear_hand(host, living, hand, shot);
    event.used = true;
}

/// ModifiableCrossbowItem.use: a loaded crossbow fires; one without ammo (out of creative) isn't drawn.
pub(crate) fn use_item(host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
    let stack = event.stack;
    if matches!(sid(stack), SHURIKEN | THROWING_AXE) && !tool_initialized(stack) && !tool_materials(stack).is_empty() {
        // A stack the inventory tick hasn't built yet is thrown as built (IModifiable.verifyTagAfterLoad).
        let mut built = stack.clone();
        let ok = match &host {
            HostSide::Server(h) => rebuild(&**h, &mut built),
            HostSide::Client(h) => rebuild(&**h, &mut built),
        };
        if ok {
            return throw(host, event, &built);
        }
        tlog(&format!("{} could not be built to throw", sid(stack)));
    }
    if thrown_item(stack) {
        let s = stack.clone();
        return throw(host, event, &s);
    }
    if matches!(&host, HostSide::Server(_)) && (sid(stack).ends_with("_staff") || matches!(sid(stack), SHURIKEN | THROWING_AXE)) {
        tlog(&format!("use of {}: initialized {}, slings {}, modifiers {:?}", sid(stack), tool_initialized(stack), slings(stack).len(), md::entries_of(stack, C_MODIFIERS).iter().map(|e| e.id.clone()).collect::<Vec<_>>()));
    }
    if !launcher(stack) {
        return None;
    }
    if tool_broken(stack) {
        return Some(InteractionResult::Fail);
    }
    let HostSide::Server(host) = host else {
        return (sid(stack) == CROSSBOW && loaded(stack).is_some()).then_some(InteractionResult::Consume);
    };
    let p = host.players().acting_player()?;
    let (uuid, living) = (p.uuid(), LivingRef::new(p.entity()));
    let creative = p.is_creative(&*host).unwrap_or(false);
    if sid(stack) == CROSSBOW {
        if let Some(nbt) = loaded(stack) {
            let mut out = stack.clone();
            md::set_persistent(&mut out, KEY_AMMO, None);
            let _ = living.set_hand_item(host, event.hand, Some(out));
            if let Some(ammo) = ItemStack::from_nbt(&*host, &nbt) {
                let speed = 3.0 * tool_stat(stack, "tconstruct:velocity");
                // ModifiableCrossbowItem.fireCrossbow: the multishot arrows fanned as the bow's.
                let count = 1 + md::volatile_int(stack, "tconstruct:multishot").max(0);
                let start = -5.0 * (count - 1) as f32;
                let mut shot = 0;
                for i in 0..count {
                    let angle = start + 10.0 * i as f32;
                    // The extra arrows of a load are creative-only pickups, as Java's.
                    if let Some(id) = shoot(host, living, uuid, stack, &ammo, speed, true, creative || i > 0, angle) {
                        track_arrow(id, stack);
                        super::tank::note_arrow(id, living.entity().id(), stack);
                        sound(host, living, "minecraft:item.crossbow.shoot", 1.0);
                        shot += 1;
                    }
                }
                wear_hand(host, living, event.hand, shot);
            }
            return Some(InteractionResult::Consume);
        }
    }
    if !creative && !has_ammo(host, uuid) {
        return Some(InteractionResult::Fail);
    }
    None
}

// ---- The projectiles' hits (ProjectileHitModifierHook): punch, fiery, and the thrown shurikens and axes

/// What a tracked projectile does when it hits: Punch's knockback, Fiery's burning (ticks), and for a thrown tool its
/// damage (ThrownShuriken: the hit is cancelled and the entity takes this as `minecraft:thrown`).
#[derive(Clone, Copy)]
pub(crate) struct Flying {
    punch: f32,
    fire: i32,
    /// The motion it was shot with (the push's direction).
    motion: [f64; 3],
    thrown: Option<f32>,
    owner: u32,
    done: bool,
}

static LAST_MOTION: Mutex<[f64; 3]> = Mutex::new([0.0; 3]);
static FLYING: Mutex<Option<HashMap<u32, Flying>>> = Mutex::new(None);
/// After the hits: (entity, push, fire ticks; -1: a thrown tool to remove), done in the next level tick.
static AFTER: Mutex<Vec<(u32, [f64; 3], i32)>> = Mutex::new(Vec::new());

fn flying() -> std::sync::MutexGuard<'static, Option<HashMap<u32, Flying>>> {
    FLYING.lock().unwrap_or_else(|e| e.into_inner())
}

/// The launcher's PunchModule (`tconstruct:punch`, 1 a level) and FieryAttackModule (5 s a level) on its arrow.
fn track_arrow(id: u32, tool: &ItemStack) {
    let punch = md::modifier_level(tool, "tconstruct:punch") as f32;
    let fire = md::modifier_level(tool, "tconstruct:fiery") * 5 * 20;
    if punch > 0.0 || fire > 0 {
        let mut f = flying();
        let map = f.get_or_insert_default();
        if map.len() < 4096 {
            let motion = *LAST_MOTION.lock().unwrap_or_else(|e| e.into_inner());
            map.insert(id, Flying { punch, fire, motion, thrown: None, owner: 0, done: false });
        }
    }
}

/// `projectile_impact` for a tracked projectile: an arrow's hit on an entity notes its punch and fire for the next
/// tick; a thrown tool's hit is cancelled (true) and deals its damage, the tool then removed.
pub(crate) fn impact(host: &mut dyn ServerHost, projectile: u32, entity: Option<u32>) -> bool {
    let f = {
        let mut g = flying();
        let Some(map) = g.as_mut() else { return false };
        let Some(f) = map.get(&projectile).copied() else { return false };
        if entity.is_none() || f.thrown.is_none() {
            map.remove(&projectile);
        } else if let Some(x) = map.get_mut(&projectile) {
            x.done = true;
        }
        f
    };
    let Some(target) = entity else { return false };
    if f.done {
        return true;
    }
    let p = host.entity(projectile);
    let Some(e) = host.entity(target) else { return f.thrown.is_some() };
    if let Some(damage) = f.thrown {
        let owner = host.entity(f.owner);
        let took = e.hurt(host, "minecraft:thrown", damage, p, owner);
        tlog(&format!("thrown tool hit {target}: {damage} damage ({took:?})"));
        AFTER.lock().unwrap_or_else(|e| e.into_inner()).push((projectile, [0.0; 3], -1));
        return true;
    }
    // AbstractArrow.doKnockback: the arrow's horizontal motion, normalized, x punch x 0.6, and 0.1 up.
    let v = p.and_then(|p| p.velocity(&*host)).filter(|v| v[0] != 0.0 || v[2] != 0.0).unwrap_or(f.motion);
    let len = (v[0] * v[0] + v[2] * v[2]).sqrt();
    let k = f64::from(f.punch) * 0.6;
    let push = if k > 0.0 && len > 1.0e-4 { [v[0] / len * k, 0.1, v[2] / len * k] } else { [0.0; 3] };
    AFTER.lock().unwrap_or_else(|e| e.into_inner()).push((target, push, f.fire));
    false
}

/// The level tick after the hits: the pushes and fires, and the thrown tools that hit removed.
pub(crate) fn tick(host: &mut dyn ServerHost) {
    let list = std::mem::take(&mut *AFTER.lock().unwrap_or_else(|e| e.into_inner()));
    for (id, push, fire) in list {
        let Some(e) = host.entity(id) else { continue };
        if fire < 0 {
            if let Some(m) = flying().as_mut() {
                m.remove(&id);
            }
            let _ = e.discard(host);
            continue;
        }
        if push != [0.0; 3] {
            let v = e.velocity(&*host).unwrap_or([0.0; 3]);
            let ok = e.set_velocity(host, [v[0] + push[0], v[1] + push[1], v[2] + push[2]]);
            tlog(&format!("punch on {id}: {push:?} ({ok})"));
        }
        if fire > 0 {
            let mut c = Compound::new();
            c.insert(js("Fire"), Tag::Short(fire.min(i32::from(i16::MAX)) as i16));
            let ok = e.merge_nbt(host, c);
            tlog(&format!("fiery arrow on {id}: {fire} ticks ({ok})"));
        }
    }
}

const SHURIKEN: &str = "tconstruct:shuriken";
const THROWING_AXE: &str = "tconstruct:throwing_axe";

fn thrown_item(stack: &ItemStack) -> bool {
    matches!(sid(stack), SHURIKEN | THROWING_AXE) && tool_initialized(stack)
}

/// ModifiableShurikenItem's cooldown after a throw, in ticks (the item's own cooldown group).
const THROW_COOLDOWN: i32 = 10;

/// ModifiableShurikenItem.use: the throw sound, the tool flies at its velocity (an arrow carrying the stack, picked
/// up as it when it sticks: no thrown entity type of our own), one used up outside creative.
fn throw(host: HostSide, event: &mut UseItemEvent<'_>, stack: &ItemStack) -> Option<InteractionResult> {
    let held = event.stack.clone();
    if tool_broken(stack) {
        return Some(InteractionResult::Fail);
    }
    let HostSide::Server(host) = host else { return Some(InteractionResult::Success) };
    let Some(p) = host.players().acting_player() else {
        tlog("a throw with no acting player");
        return None;
    };
    let (uuid, living) = (p.uuid(), LivingRef::new(p.entity()));
    // `getCooldowns().addCooldown(this, 10)`: the player's item cooldown, drawn on the hotbar. An item on cooldown
    // isn't used (vanilla's PASS before `Item.use`).
    let group = sid(stack);
    if p.has_item_cooldown(&*host, group).unwrap_or(false) {
        return Some(InteractionResult::Pass);
    }
    let _ = p.set_item_cooldown(host, group, THROW_COOLDOWN);
    let creative = p.is_creative(&*host).unwrap_or(false);
    let damage = tool_stat(stack, "tconstruct:projectile_damage");
    let velocity = tool_stat(stack, "tconstruct:velocity").max(0.1);
    let Some(one) = with_count(stack, 1) else { return None };
    sound(host, living, "tconstruct:throw.shuriken", 0.4 / (md::rand_f32() * 0.4 + 0.8));
    let id = shoot(host, living, uuid, stack, &one, velocity, false, creative, 0.0)?;
    let owner = p.entity().id();
    flying().get_or_insert_default().insert(id, Flying { punch: 0.0, fire: 0, motion: [0.0; 3], thrown: Some(damage), owner, done: false });
    tlog(&format!("thrown {}: {damage} damage at {velocity}", sid(stack)));
    if !creative {
        let place = ItemPlace::PlayerHand { player: uuid, hand: event.hand };
        let res = ItemResource::of(&held);
        let took = host.transaction(|host, tx| if place.extract(host, tx, &res, 1) == Ok(1) { Ok(()) } else { Err(()) });
        tlog(&format!("thrown {} taken from the hand: {}", sid(stack), took.is_ok()));
    }
    Some(InteractionResult::Success)
}

// ---- The staffs' slings (SlingModule: SlingLeapModule, SlingTeleportModule, SlingKnockbackModule)

#[derive(Clone, Copy)]
pub(crate) struct Sling {
    kind: u8,
    drawtime: f32,
    force: f32,
    height: f32,
    bias_up: bool,
    leave_ground: bool,
    target: u8,
    damage: f32,
}

/// The tool's sling modules, each with its force multiplier at the modifier's level.
fn slings(stack: &ItemStack) -> Vec<Sling> {
    if !tool_initialized(stack) {
        return Vec::new();
    }
    let Some(tm) = md::ToolMods::cached(stack) else { return Vec::new() };
    let mut out = Vec::new();
    for (m, _, l) in tm.entries() {
        for module in &m.modules {
            if let md::ModModule::Sling { kind, drawtime, force, height, bias_up, leave_ground, target, damage } = module {
                out.push(Sling { kind: *kind, drawtime: *drawtime, force: force.at(l), height: *height, bias_up: *bias_up, leave_ground: *leave_ground, target: *target, damage: *damage });
            }
        }
    }
    out
}

/// GeneralInteractionModifierHook.startDrawtime with the sling's multiplier, and getToolCharge.
fn sling_charge(stack: &ItemStack, s: &Sling, time: i32) -> f32 {
    let draw = (20.0 * s.drawtime / tool_stat(stack, "tconstruct:draw_speed").max(0.01)).ceil().max(1.0);
    charge(time, draw)
}

/// SlingModule.getPower: projectile damage / 2 x velocity.
fn sling_power(stack: &ItemStack) -> f32 {
    tool_stat(stack, "tconstruct:projectile_damage") / 2.0 * tool_stat(stack, "tconstruct:velocity")
}

fn gaussian() -> f64 {
    let (u, v) = (f64::from(md::rand_f32()).max(1.0e-6), f64::from(md::rand_f32()));
    (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
}

fn look(yaw: f32, pitch: f32) -> [f64; 3] {
    let (y, p) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()]
}

fn inaccuracy(stack: &ItemStack) -> f64 {
    f64::from(3.0 * (1.0 / tool_stat(stack, "tconstruct:accuracy").max(0.1) - 1.0)) * 0.0075
}

/// SlingLeapModule on the client (the player moves itself): the push along the look, its height divided, upwards
/// for springing, backwards for a negative force (flinging). The server plays the sound and wears the tool.
pub(crate) fn sling_client(host: &mut dyn ClientHost, stack: &ItemStack, time_left: i32) {
    let time = DURATION - time_left;
    let list = slings(stack);
    if !list.is_empty() {
        tlog(&format!("client sling release of {}: {} slings, {time} ticks, camera {}, power {} (damage {}, velocity {}), draw speed {}", sid(stack), list.len(), host.camera().is_some(), sling_power(stack), tool_stat(stack, "tconstruct:projectile_damage"), tool_stat(stack, "tconstruct:velocity"), tool_stat(stack, "tconstruct:draw_speed")));
    }
    for s in list.into_iter().filter(|s| s.kind == 0) {
        let Some(cam) = host.camera() else { return };
        let c = sling_charge(stack, &s, time);
        let force = f64::from(sling_power(stack) * c * s.force.abs());
        let feet = cam.position;
        let below = [feet[0].floor() as i32, (feet[1] - 0.05).floor() as i32, feet[2].floor() as i32];
        let on_ground = !host.block_collision(below).is_empty() && feet[1] - feet[1].floor() < 0.05;
        let ok = s.target != 1 || (on_ground && host.raycast(5.0).is_some());
        if c <= 0.0 || force <= 0.0 || !ok {
            tlog(&format!("sling leap refused: charge {c}, force {force}, on ground {on_ground}, target {}", s.target));
            continue;
        }
        let mut l = look(cam.yaw, cam.pitch);
        if s.bias_up {
            l[1] += (0.5 - force * 0.1 * f64::from(s.force.signum())).max(0.0);
        }
        let len = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt().max(1.0e-4);
        let inacc = inaccuracy(stack);
        let mut d = [l[0] / len + gaussian() * inacc, (l[1] / len + gaussian() * inacc) / f64::from(s.height), l[2] / len + gaussian() * inacc];
        if s.force < 0.0 {
            d = d.map(|v| -v);
        }
        let up = if s.leave_ground && on_ground { 0.05 } else { 0.0 };
        let kick = [(d[0] * force) as f32, (d[1] * force) as f32 + up, (d[2] * force) as f32];
        host.push_player(kick);
        tlog(&format!("sling leap: charge {c:.2}, force {force:.2}, kick {kick:?}"));
    }
}

/// SlingModule.beforeReleaseUsing on the server: the leap's sound and wear, warping's teleport, bonking's hit.
pub(crate) fn sling_release(host: &mut dyn ServerHost, event: &mut ReleaseUsingEvent<'_>) {
    let stack = event.stack;
    let list = slings(stack);
    if list.is_empty() || tool_broken(stack) {
        return;
    }
    tlog(&format!("sling release of {}: {} slings, {} ticks", sid(stack), list.len(), DURATION - event.time_left));
    let Some((living, hand, _, _)) = holder(host, event.entity, stack) else { return };
    let time = DURATION - event.time_left;
    let e = living.entity();
    let (Some(pos), Some([yaw, pitch]), Some(eye)) = (e.position(&*host), e.rotation(&*host), e.eye_position(&*host)) else { return };
    let inacc = inaccuracy(stack);
    let l = look(yaw, pitch);
    let mut used = false;
    for s in list {
        let c = sling_charge(stack, &s, time);
        let force = sling_power(stack) * c * s.force.abs();
        if c <= 0.0 || force <= 0.0 {
            sound(host, living, "tconstruct:slime_sling", 0.5);
            continue;
        }
        let Some(level) = here(&*host) else { continue };
        match s.kind {
            0 => {
                sound(host, living, "tconstruct:slime_sling", 1.0);
                used = true;
            }
            1 => {
                // SlingTeleportModule: up to `force` blocks along the look and 1 up, stepping back 0.25 a time until
                // the two blocks there are free of collisions.
                let mut off: [f64; 3] = std::array::from_fn(|i| (l[i] + gaussian() * inacc) * f64::from(force));
                off[1] += 1.0;
                let step = |v: f64| if v.abs() > 0.25 { v - 0.25 * v.signum() } else { 0.0 };
                loop {
                    let at = [pos[0] + off[0], pos[1] + off[1], pos[2] + off[2]];
                    let b = [at[0].floor(), at[1].floor(), at[2].floor()];
                    let free = level.has_collisions(&*host, [b[0] + 0.01, b[1] + 0.01, b[2] + 0.01, b[0] + 0.99, b[1] + 1.99, b[2] + 0.99]) == Some(false);
                    if free {
                        if off.iter().any(|v| v.abs() > 0.5) && e.teleport_to(host, at, None) {
                            sound(host, living, "tconstruct:slime_sling.teleport", 1.0);
                            tlog(&format!("sling teleport: {pos:?} -> {at:?}"));
                            used = true;
                        }
                        break;
                    }
                    if off.iter().all(|v| v.abs() <= 0.5) {
                        break;
                    }
                    off = off.map(step);
                }
            }
            _ => {
                // SlingKnockbackModule: the first mob on the look within 5 blocks takes the staff's hit (x the
                // damage multiplier and the charge's cooldown factor) and is knocked away.
                let end = [eye[0] + l[0] * 5.0, eye[1] + l[1] * 5.0, eye[2] + l[2] * 5.0];
                let hits = level.ray_trace_entities(&*host, eye, end, 0.0, Some(e)).unwrap_or_default();
                let Some(t) = hits.into_iter().find(|h| host.living(h.entity).is_some()) else {
                    tlog(&format!("bonk missed: eye {eye:?} look {l:?}"));
                    sound(host, living, "tconstruct:bonk", 1.0);
                    continue;
                };
                let attack = tool_stat(stack, "tconstruct:attack_damage");
                if s.damage > 0.0 && attack > 0.0 {
                    let cd = c.min(1.0);
                    let amount = (1.0 + attack) * s.damage * (0.2 + cd * cd * 0.8);
                    let _ = t.entity.hurt(host, "minecraft:player_attack", amount, Some(e), Some(e));
                }
                let k = f64::from(force);
                let (ax, az) = (-l[0] + gaussian() * inacc, -l[2] + gaussian() * inacc);
                let n = (ax * ax + az * az).sqrt().max(1.0e-4);
                let v = t.entity.velocity(&*host).unwrap_or([0.0; 3]);
                // LivingEntity.knockback(force, ax, az): half the motion minus the direction x force, up to 0.4 up.
                let nv = [v[0] / 2.0 - ax / n * k, (v[1] / 2.0 + k).min(0.4), v[2] / 2.0 - az / n * k];
                let ok = t.entity.set_velocity(host, nv);
                tlog(&format!("bonk: force {k:.2}, velocity {nv:?} ({ok})"));
                sound(host, living, "tconstruct:bonk", 0.5);
                used = true;
            }
        }
    }
    if used {
        wear_hand(host, living, hand, 1);
        event.used = true;
    }
}
