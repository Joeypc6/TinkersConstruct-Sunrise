//! Stage 5: the gadgets (TinkerGadgets), the foods' own consume effects (TinkerFood, CheeseBlockItem), the cheese
//! block and the decorative blocks with behaviour (punji sticks, glow, platforms, cakes). The content (blocks, items,
//! effects, the snowball-copy entity types) is the generator's (`gadget_data.py`, `food_data.py` upstream); this is
//! the logic, ported from the upstream port's "Gadgets" section on the fork's hooks and parts:
//! - throwables (GlowBallItem, EFLNItem, ShurikenItem): `spawn_projectile_from_rotation` of the content thrown types;
//!   their impacts (GlowballEntity, EFLNEntity + EFLNExplosion, ShurikenEntityBase) in `projectile_impact`, the
//!   EFLN's sphere through `explosion_detonate`, its damage rescaled in `living_incoming_damage`;
//! - glow (GlowBlock), punji (PunjiBlock), platforms (PlatformBlock, WeatheringPlatformBlock), cakes (FoodCakeBlock,
//!   InvertedCakeBlock), the cheese block's speed 0.4 and jump 0.5: BlockParts;
//! - the effects acting through the player: antigravity (gravity x -1), returning (back where the first slice was
//!   eaten), carry (PiggyBackPackItem's -5 % speed a level), bouncy (ModifierEvents.bounceOnFall, for the slime
//!   boots' `tconstruct:bouncy` modifier and the earth cake's effect);
//! - the piggyback pack (PiggyBackPackItem.interactLivingEntity: up to 3 mobs stacked on the player, the pack worn);
//! - the consume effects `tconstruct:remove_random_effect`, `tconstruct:set_fire`, `tconstruct:give_item`.

use crate::smeltery::casting::{dir_index, opposite, state_prop, step, DIRS, DIR_NAMES};
use crate::smeltery::util::*;
use crate::tools::modifier as md;
use mod_api::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub(crate) type Pos = [i32; 3];

const GLOW: &str = "tconstruct:glow";
const PUNJI: &str = "tconstruct:punji";
const ICHOR_CAKE: &str = "tconstruct:ichor_cake";
const CHEESE: &str = "tconstruct:cheese_block";
const GLOW_BALL: &str = "tconstruct:glow_ball";
const EFLN_BALL: &str = "tconstruct:efln_ball";
const QUARTZ_SHURIKEN: &str = "tconstruct:quartz_shuriken";
const FLINT_SHURIKEN: &str = "tconstruct:flint_shuriken";
const BACKPACK: &str = "tconstruct:piggy_backpack";
const PLATFORMS: [&str; 11] = [
    "gold_platform", "iron_platform", "cobalt_platform", "copper_platform", "exposed_copper_platform",
    "weathered_copper_platform", "oxidized_copper_platform", "waxed_copper_platform", "waxed_exposed_copper_platform",
    "waxed_weathered_copper_platform", "waxed_oxidized_copper_platform",
];
const CAKES: [&str; 6] = ["earth_cake", "sky_cake", "ichor_cake", "ender_cake", "blood_cake", "magma_cake"];
const COPPER_AGES: [&str; 4] = ["", "exposed_", "weathered_", "oxidized_"];
const EFFECT_IDS: [&str; 10] = [
    "minecraft:jump_boost",
    "tconstruct:bouncy", "tconstruct:double_jump", "tconstruct:antigravity", "tconstruct:returning", "tconstruct:carry",
    "minecraft:regeneration", "minecraft:fire_resistance", "minecraft:slowness", "minecraft:speed",
];

static KEYS: OnceLock<HashMap<&'static str, EffectKey>> = OnceLock::new();

pub(crate) fn effect(id: &str) -> Option<EffectKey> {
    key(id)
}

fn key(id: &str) -> Option<EffectKey> {
    KEYS.get()?.get(id).copied()
}

pub(crate) struct Gadget;
pub(crate) struct Throwable;
pub(crate) struct Backpack;

pub(crate) fn register(r: &mut dyn Registrar) {
    let _ = KEYS.set(EFFECT_IDS.iter().map(|e| (*e, r.effect_key(e))).collect());
    let mut blocks: Vec<String> = vec![GLOW.into(), PUNJI.into(), CHEESE.into()];
    blocks.extend(PLATFORMS.iter().chain(CAKES.iter()).map(|b| format!("tconstruct:{b}")));
    for b in blocks {
        let k = r.block_key(&b);
        r.block_part(k, Gadget);
    }
    for i in [GLOW_BALL, EFLN_BALL, QUARTZ_SHURIKEN, FLINT_SHURIKEN] {
        let k = r.item_key(i);
        r.item_part(k, Throwable);
    }
    let k = r.item_key(BACKPACK);
    r.item_part(k, Backpack);
}

fn b(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

fn is_platform(name: &str) -> bool {
    name.starts_with("tconstruct:") && name.ends_with("_platform")
}

/// A copper platform's (waxed, age).
fn copper_age(name: &str) -> Option<(bool, usize)> {
    let rest = name.strip_prefix("tconstruct:")?.strip_suffix("copper_platform")?;
    let (waxed, rest) = match rest.strip_prefix("waxed_") {
        Some(r) => (true, r),
        None => (false, rest),
    };
    COPPER_AGES.iter().position(|a| *a == rest).map(|age| (waxed, age))
}

fn copper_name(waxed: bool, age: usize) -> String {
    format!("tconstruct:{}{}copper_platform", if waxed { "waxed_" } else { "" }, COPPER_AGES[age])
}

/// PunjiBlock.getLocalNorth / getLocalEast (Direction order down, up, north, south, west, east).
fn punji_north(facing: usize) -> usize {
    match facing {
        0 => 2,
        1 => 3,
        _ => 1,
    }
}

fn punji_east(facing: usize) -> usize {
    if facing < 2 { 5 } else { [0, 0, 5, 4, 2, 3][facing] }
}

// ---- Block views (a part's LevelHost has no face-sturdiness read: see the README's gaps)

pub(crate) struct View<'a> {
    level: &'a dyn LevelHost,
    /// A neighbour as it is now (update_shape's, which the level may not hold yet).
    over: Option<(Pos, BlockState)>,
}

impl View<'_> {
    fn state(&self, p: Pos) -> Option<BlockState> {
        match self.over {
            Some((q, s)) if q == p => Some(s),
            _ => self.level.block_state(p),
        }
    }
    fn name(&self, p: Pos) -> String {
        self.state(p).and_then(|s| self.level.state_block(s)).unwrap_or("minecraft:air").to_owned()
    }
    fn prop(&self, p: Pos, k: &str) -> Option<String> {
        self.state(p).and_then(|s| state_prop(self.level, s, k))
    }
    /// `isFaceSturdy(FULL)`, approximated: a block that isn't air, a fluid, replaceable or one of the thin or partial
    /// blocks below. Slabs and stairs count by their half or facing where that decides it.
    fn sturdy(&self, p: Pos, face: usize) -> bool {
        let Some(s) = self.state(p) else { return false };
        let name = self.name(p);
        if matches!(name.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:water" | "minecraft:lava")
            || self.level.block_in_tag(s.block(), "minecraft:replaceable").unwrap_or(false)
            || self.level.state_fluid_state(s).is_some_and(|f| !f.is_empty()) && self.level.state_block(s).is_some_and(|n| !n.contains("glass"))
        {
            return false;
        }
        let thin = ["_pane", "_bars", "fence", "_wall", "torch", "button", "lever", "_sign", "pressure_plate", "carpet", "rail", "_door", "trapdoor",
            "ladder", "vine", "flower", "sapling", "_bed", "lantern", "chain", "candle", "_head", "_skull", "punji", "glow", "_platform", "cake",
            "_banner", "redstone", "repeater", "comparator", "snow", "scaffolding", "lily", "_coral", "kelp", "grass", "fern", "mushroom", "roots",
            "_pot", "end_rod", "lightning_rod", "anvil", "bell", "cobweb", "fungus", "sprouts", "bush", "dripleaf", "amethyst_bud", "cluster"];
        if thin.iter().any(|t| name.contains(t)) && !name.ends_with("_block") && !name.contains("pumpkin") {
            return false;
        }
        if name.ends_with("_slab") {
            return match self.prop(p, "type").as_deref() {
                Some("double") => true,
                Some("top") => face == 1,
                _ => face == 0,
            };
        }
        if name.ends_with("_stairs") {
            let half = self.prop(p, "half");
            return (face == 0 && half.as_deref() == Some("bottom")) || (face == 1 && half.as_deref() == Some("top"))
                || self.prop(p, "facing").as_deref() == Some(DIR_NAMES[face]);
        }
        true
    }
    fn punji_survives(&self, p: Pos, facing: usize) -> bool {
        self.sturdy(step(p, facing), opposite(facing))
    }
    fn punji_connected(&self, facing: usize, p: Pos) -> bool {
        self.name(p) == PUNJI && self.prop(p, "facing").as_deref() == Some(DIR_NAMES[facing])
    }
    /// GlowBlock.canSurvive: not in a liquid, and a full face on the facing side.
    fn glow_survives(&self, p: Pos, facing: usize) -> bool {
        !matches!(self.name(p).as_str(), "minecraft:water" | "minecraft:lava") && self.sturdy(step(p, facing), opposite(facing))
    }
    /// InvertedCakeBlock.canSurvive: the block above is solid.
    fn ichor_cake_survives(&self, p: Pos) -> bool {
        self.sturdy(step(p, 1), 0)
    }
    fn platform_vertical(&self, block: &str, other: &str) -> bool {
        if copper_age(block).is_some() { copper_age(other).is_some() } else { other == block }
    }
    /// PlatformBlock.connected.
    fn platform_connected(&self, d: usize, p: Pos) -> bool {
        let Some(s) = self.state(p) else { return false };
        if !self.level.block_in_tag(s.block(), "tconstruct:platform_connections").unwrap_or(false) {
            return false;
        }
        if let Some(v) = self.prop(p, DIR_NAMES[opposite(d)]) {
            return v == "true";
        }
        if self.prop(p, "face").is_some_and(|f| f != "wall") {
            return false;
        }
        ["facing", "horizontal_facing", "vertical_direction"].iter().all(|k| self.prop(p, k).is_none_or(|v| v == DIR_NAMES[d]))
    }
    fn platform_down(&self, block: &str, p: Pos) -> bool {
        let below = step(p, 0);
        self.platform_vertical(block, &self.name(below)) || self.sturdy(below, 1)
    }
}

fn with(level: &dyn LevelHost, state: BlockState, props: &[(&str, &str)]) -> BlockState {
    props.iter().fold(state, |s, (k, v)| level.state_with_property(s, k, v).unwrap_or(s))
}

fn platform_state(v: &View<'_>, block: &str, p: Pos, state: BlockState) -> BlockState {
    let up = b(v.platform_vertical(block, &v.name(step(p, 1))));
    let down = b(v.platform_down(block, p));
    // Plane.HORIZONTAL: north, east, south, west.
    let s: Vec<&str> = [2usize, 5, 3, 4].iter().map(|d| b(v.platform_connected(*d, step(p, *d)))).collect();
    with(v.level, state, &[("up", up), ("down", down), ("north", s[0]), ("east", s[1]), ("south", s[2]), ("west", s[3])])
}

fn punji_state(v: &View<'_>, p: Pos, state: BlockState, facing: usize) -> BlockState {
    let (north, east) = (punji_north(facing), punji_east(facing));
    let np = step(p, north);
    with(
        v.level,
        state,
        &[
            ("facing", DIR_NAMES[facing]),
            ("north", b(v.punji_connected(facing, np))),
            ("east", b(v.punji_connected(facing, step(p, east)))),
            ("northeast", b(v.punji_connected(facing, step(np, east)))),
            ("northwest", b(v.punji_connected(facing, step(np, opposite(east))))),
        ],
    )
}

fn dir_of(d: Direction) -> usize {
    DIRS.iter().position(|x| *x == d).unwrap_or(0)
}

#[mod_api::sunrise_part]
impl BlockPart for Gadget {
    /// PunjiBlock / GlowBlock / PlatformBlock.getStateForPlacement.
    fn state_for_placement(&self, level: LevelSide<'_>, event: &mut StateForPlacementEvent<'_>) -> Option<Option<BlockState>> {
        let (LevelSide::Server(level) | LevelSide::Client(level)) = level;
        let block = level.state_block(event.state)?.to_owned();
        let c = event.context;
        let p = [c.pos.x, c.pos.y, c.pos.z];
        let v = View { level, over: None };
        let water = b(event.fluid.is_source() && level.state_block(event.state).is_some());
        if block == PUNJI || block == GLOW {
            let wanted = opposite(dir_of(c.face));
            let survives = |d: usize| if block == PUNJI { v.punji_survives(p, d) } else { v.glow_survives(p, d) };
            let Some(facing) = std::iter::once(wanted).chain((0..6).filter(|d| *d != wanted)).find(|d| survives(*d)) else {
                return Some(None);
            };
            if block == GLOW {
                return Some(level.state_with_property(event.state, "facing", DIR_NAMES[facing]));
            }
            let s = punji_state(&v, p, event.state, facing);
            return Some(Some(with(level, s, &[("waterlogged", water)])));
        }
        if is_platform(&block) {
            let s = platform_state(&v, &block, p, event.state);
            return Some(Some(with(level, s, &[("waterlogged", water)])));
        }
        if block == ICHOR_CAKE && !v.ichor_cake_survives(p) {
            return Some(None);
        }
        None
    }

    fn update_shape(&self, level: &mut dyn LevelTicksHost, event: &mut UpdateShapeEvent) -> Option<BlockState> {
        let lh: &dyn LevelHost = &*level;
        let block = lh.state_block(event.state)?.to_owned();
        let p = [event.pos.x, event.pos.y, event.pos.z];
        let np = [event.neighbour_pos.x, event.neighbour_pos.y, event.neighbour_pos.z];
        let v = View { level: lh, over: Some((np, event.neighbour)) };
        let air = lh.block_state_id("minecraft:air");
        let d = dir_of(event.direction);
        let facing = state_prop(lh, event.state, "facing").and_then(|f| dir_index(&f)).unwrap_or(0);
        if block == GLOW {
            return if v.glow_survives(p, facing) { None } else { air };
        }
        if block == ICHOR_CAKE {
            return if d == 1 && !v.ichor_cake_survives(p) { air } else { None };
        }
        if block == PUNJI {
            if d == facing && !v.punji_survives(p, facing) {
                return air;
            }
            return Some(punji_state(&v, p, event.state, facing));
        }
        if is_platform(&block) {
            return Some(platform_state(&v, &block, p, event.state));
        }
        None
    }

    fn can_survive(&self, level: LevelSide<'_>, event: &mut CanSurviveEvent) -> Option<bool> {
        let (LevelSide::Server(level) | LevelSide::Client(level)) = level;
        let block = level.state_block(event.state)?.to_owned();
        let p = [event.pos.x, event.pos.y, event.pos.z];
        let v = View { level, over: None };
        let facing = state_prop(level, event.state, "facing").and_then(|f| dir_index(&f)).unwrap_or(0);
        match block.as_str() {
            PUNJI => Some(v.punji_survives(p, facing)),
            GLOW => Some(v.glow_survives(p, facing)),
            ICHOR_CAKE => Some(v.ichor_cake_survives(p)),
            _ => None,
        }
    }

    /// WeatheringCopper.onRandomTick for the unwaxed copper platforms.
    fn random_tick(&self, level: &mut dyn LevelWriteHost, event: &mut RandomTickEvent) -> Flow {
        let Some(block) = level.state_block(event.state).map(str::to_owned) else { return Flow::Continue };
        let Some((false, age)) = copper_age(&block) else { return Flow::Continue };
        if age >= 3 {
            return Flow::Cancel;
        }
        // Both draws of the level's stream, in their order (the scan between them draws nothing).
        let Some((roll, chance_roll)) = level.with_random(|r| (r.next_float(), r.next_float())) else { return Flow::Cancel };
        let p = [event.pos.x, event.pos.y, event.pos.z];
        let (mut same, mut later) = (0u32, 0u32);
        let mut older = false;
        for dx in -4i32..=4 {
            for dy in -4i32..=4 {
                for dz in -4i32..=4 {
                    let m = dx.abs() + dy.abs() + dz.abs();
                    if m == 0 || m > 4 {
                        continue;
                    }
                    let q = [p[0] + dx, p[1] + dy, p[2] + dz];
                    let Some(n) = level.block_state(q).and_then(|s| level.state_block(s)).map(str::to_owned) else { continue };
                    let Some(a) = weathering_age(&n) else { continue };
                    if a < age {
                        older = true;
                    } else if a > age {
                        later += 1;
                    } else {
                        same += 1;
                    }
                }
            }
        }
        if roll >= 0.056_888_89 || older {
            return Flow::Cancel;
        }
        let f = (later + 1) as f32 / (later + same + 1) as f32;
        let chance = f * f * if age == 0 { 0.75 } else { 1.0 };
        if chance_roll < chance {
            let text = level.state_string(event.state).unwrap_or_default();
            let next = text.replacen(&block, &copper_name(false, age + 1), 1);
            if let Some(s) = level.block_state_id(&next) {
                level.set_block(p, s, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS);
            }
        }
        Flow::Cancel
    }

    fn block_speed_factor(&self, _context: &dyn LoopContext, event: &mut BlockSpeedFactorEvent) {
        event.factor = 0.4;
    }

    fn block_jump_factor(&self, _context: &dyn LoopContext, event: &mut BlockJumpFactorEvent) {
        event.factor = 0.5;
    }

    /// PunjiBlock.entityInside. The sweep's host reads no entity and its only damage write is generic
    /// (`EntityWrite::Hurt`), where the sticks hurt as a cactus: the touch is kept and dealt on this level's tick.
    fn entity_inside(&self, _level: &mut dyn LevelRandomHost, event: &mut EntityInsideEvent<'_>) {
        let Ok(id) = u32::try_from(event.entity) else { return };
        let mut q = PUNJI_TOUCHES.lock().unwrap_or_else(|e| e.into_inner());
        let touch = (id, [event.pos.x, event.pos.y, event.pos.z], event.state);
        if !q.iter().any(|(d, t)| d == event.dimension && *t == touch) {
            q.push((event.dimension.to_owned(), touch));
        }
    }

    fn use_without_item(&self, host: &mut dyn ServerHost, event: &mut UseWithoutItemEvent<'_>) -> Option<InteractionResult> {
        cake_click(host, event.click, event.state)
    }

    fn use_item_on(&self, host: &mut dyn ServerHost, event: &mut UseItemOnEvent<'_>) -> Option<InteractionResult> {
        if let Some(r) = platform_click(host, event.click, event.state) {
            return Some(r);
        }
        cake_click(host, event.click, event.state)
    }
}

/// The weathering stage of a block that ages as copper does, None for anything else (waxed included).
fn weathering_age(name: &str) -> Option<usize> {
    if let Some((waxed, age)) = copper_age(name) {
        return (!waxed).then_some(age);
    }
    let rest = name.strip_prefix("minecraft:")?;
    if rest.starts_with("waxed_") || !rest.contains("copper") || rest.contains("ore") || rest.starts_with("raw_") || rest.contains("torch") || rest.ends_with("_ingot") || rest.ends_with("_nugget") {
        return None;
    }
    Some(COPPER_AGES.iter().rposition(|a| !a.is_empty() && rest.starts_with(a)).unwrap_or(0))
}

fn level_of(host: &dyn ServerHost, dim: &LevelDimension) -> Option<LevelRef> {
    host.level(dim.key().id())
}

/// Honeycomb waxes a copper platform; an axe takes the wax off, or a stage (WeatheringPlatformBlock).
fn platform_click(host: &mut dyn ServerHost, click: &BlockClick<'_>, state: BlockState) -> Option<InteractionResult> {
    let text = host.block_state_string(state)?;
    let block = text.split('[').next()?.to_owned();
    let (waxed, age) = copper_age(&block)?;
    let held = click.stack?.clone();
    let axe = crate::smeltery::casting_recipes::tag_items(&*host, "minecraft:axes").contains(sid(&held)) || sid(&held).ends_with("_axe");
    let (to, ev, sound) = if sid(&held) == "minecraft:honeycomb" && !waxed {
        (copper_name(true, age), 3003, "minecraft:item.honeycomb.wax_on")
    } else if axe && waxed {
        (copper_name(false, age), 3004, "minecraft:item.axe.wax_off")
    } else if axe && age > 0 {
        (copper_name(false, age - 1), 3005, "minecraft:item.axe.scrape")
    } else {
        return None;
    };
    let level = level_of(&*host, &click.dimension)?;
    let new = host.block_state_id(&text.replacen(&block, &to, 1))?;
    let p = click.pos;
    let _ = level.set_block(host, p, new, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS);
    let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
    let _ = level.play_sound(host, sound, at, SoundSource::Blocks, 1.0, 1.0, None);
    let _ = level.level_event(host, ev, p, 0);
    let player = host.player(click.player)?;
    let creative = player.is_creative(&*host).unwrap_or(false);
    let living = LivingRef::new(player.entity());
    let slot = if click.hand == Hand::Main { EquipmentSlot::MainHand } else { EquipmentSlot::OffHand };
    if ev == 3003 {
        if !creative {
            let place = ItemPlace::PlayerHand { player: click.player, hand: click.hand };
            let res = ItemResource::of(&held);
            let _ = host.transaction(|host, tx| if place.extract(host, tx, &res, 1) == Ok(1) { Ok(()) } else { Err(()) });
        }
    } else if !creative {
        let _ = living.hurt_and_break(host, slot, 1);
    }
    Some(InteractionResult::Success)
}

// ---- Cakes (FoodCakeBlock.eatSlice)

/// (nutrition, saturation modifier, effect, duration, ADD combination).
fn cake_food(block: &str) -> Option<(i32, f32, &'static str, i32, bool)> {
    Some(match block {
        "tconstruct:earth_cake" => (1, 0.3, "tconstruct:bouncy", 600, false),
        "tconstruct:sky_cake" => (1, 0.2, "tconstruct:double_jump", 600, false),
        "tconstruct:ichor_cake" => (1, 0.3, "tconstruct:antigravity", 600, false),
        "tconstruct:ender_cake" => (1, 0.4, "tconstruct:returning", 600, true),
        "tconstruct:blood_cake" => (2, 0.2, "minecraft:regeneration", 150, false),
        "tconstruct:magma_cake" => (2, 0.2, "minecraft:fire_resistance", 600, false),
        _ => return None,
    })
}

/// ReturningEffect: where each player got it (dimension, block position), by player.
static RETURNING: Mutex<Vec<(u128, String, [f64; 3])>> = Mutex::new(Vec::new());

/// FoodCakeBlock.use: a slice (always edible), its food and effect (BLOCK: refused while it has 100+ ticks; ADD: the
/// time left added), the stat, a bite or the cake gone.
fn cake_click(host: &mut dyn ServerHost, click: &BlockClick<'_>, state: BlockState) -> Option<InteractionResult> {
    let text = host.block_state_string(state)?;
    let block = text.split('[').next()?.to_owned();
    let (food, saturation, effect, duration, add) = cake_food(&block)?;
    let player = host.player(click.player)?;
    let living = LivingRef::new(player.entity());
    let k = key(effect)?;
    let current = living.effect(&*host, k);
    if !add && current.as_ref().is_some_and(|e| e.duration >= 100) {
        return Some(InteractionResult::Consume);
    }
    let _ = player.award_stat(host, StatType::Custom, "minecraft:eat_cake_slice", 1);
    if let Some(f) = player.food(&*host) {
        let level = (f.level + food).min(20);
        let _ = player.set_food_level(host, level);
        let _ = player.set_saturation(host, (f.saturation + food as f32 * saturation * 2.0).min(level as f32));
    }
    let mut time = duration;
    if add && let Some(e) = current.as_ref().filter(|e| e.amplifier == 0) {
        time += e.duration;
    }
    if effect == "tconstruct:returning" && current.is_none()
        && let (Some(at), Some(dim)) = (player.entity().position(&*host), player.dimension(&*host))
    {
        let mut r = RETURNING.lock().unwrap_or_else(|e| e.into_inner());
        r.retain(|(u, _, _)| *u != click.player);
        r.push((click.player, dim.to_string(), at.map(f64::floor)));
    }
    let visible = !effect.starts_with("tconstruct:");
    let _ = living.add_effect(host, &ActiveEffect::new(k, time, 0, false, visible, true));
    let bites: i32 = state_prop_host(&text, "bites").and_then(|b| b.parse().ok()).unwrap_or(0);
    let level = level_of(&*host, &click.dimension)?;
    let next = if bites < 6 { host.block_state_id(&text.replace(&format!("bites={bites}"), &format!("bites={}", bites + 1))) } else { host.block_state_id("minecraft:air") };
    if let Some(n) = next {
        let _ = level.set_block(host, click.pos, n, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS);
    }
    tlog(&format!("cake {block}: slice {bites}, {effect} {time}"));
    Some(InteractionResult::Success)
}

fn state_prop_host(text: &str, key: &str) -> Option<String> {
    let props = text.split_once('[')?.1.trim_end_matches(']');
    props.split(',').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v.to_owned()))
}

// ---- Punji (PunjiBlock.entityInside)

/// The sticks' touches this tick (dimension, (entity id, position, state)), dealt at the level's tick.
static PUNJI_TOUCHES: Mutex<Vec<(String, (u32, Pos, BlockState))>> = Mutex::new(Vec::new());

/// The punji touches in `host`'s level: their damage, as PunjiBlock.entityInside deals it.
fn punji_tick(host: &mut dyn ServerHost) {
    let here: Vec<(u32, Pos, BlockState)> = {
        let mut q = PUNJI_TOUCHES.lock().unwrap_or_else(|e| e.into_inner());
        if q.is_empty() {
            return;
        }
        let dim = host.dimension();
        let mut out = Vec::new();
        q.retain(|(d, t)| {
            if *d == dim {
                out.push(*t);
                false
            } else {
                true
            }
        });
        out
    };
    for (id, pos, state) in here {
        let Some(e) = host.entity(id) else { continue };
        let Some(level) = e.level(&*host) else { continue };
        // Still the sticks there.
        if level.block_state(&*host, pos).and_then(|s| host.block_state_string(s)).is_none_or(|s| !s.starts_with(PUNJI)) {
            continue;
        }
        punji_hurt(host, e, pos, state);
    }
}

/// The last time each entity was hurt by punji sticks (their 1 damage a hit is kept by the hurt cooldown; this only
/// keeps the call count down).
fn punji_hurt(host: &mut dyn ServerHost, entity: EntityRef, pos: Pos, state: BlockState) {
    if host.living(entity).is_none() {
        return;
    }
    let facing = host.block_state_string(state).and_then(|t| state_prop_host(&t, "facing")).and_then(|f| dir_index(&f)).unwrap_or(0);
    let Some(bb) = entity.bounding_box(&*host) else { return };
    let axis = match facing {
        0 | 1 => 1,
        2 | 3 => 2,
        _ => 0,
    };
    let middle = f64::from(pos[axis]) + 0.5;
    let positive = matches!(facing, 1 | 3 | 5);
    if (positive && bb[1][axis] <= middle) || (!positive && bb[0][axis] >= middle) {
        return;
    }
    // fallDistance: no read of it; the fall speed gives it (v^2 / 2g, g 0.08 a tick^2) while falling.
    let vy = entity.velocity(&*host).map_or(0.0, |v| v[1]);
    let fall = if vy < -0.0785 { (vy * vy / 0.16) as f32 } else { 0.0 };
    let damage = if fall > 0.0 { 1.0 + fall + 1.0 } else { 1.0 };
    let _ = entity.hurt(host, "minecraft:cactus", damage, None, None);
}

// ---- Throwables

fn thrown(item: &str) -> Option<(&'static str, i32)> {
    Some(match item {
        GLOW_BALL => ("tconstruct:throw.throwball", 0),
        EFLN_BALL => ("tconstruct:throw.throwball", 0),
        QUARTZ_SHURIKEN => ("tconstruct:throw.shuriken", 4),
        FLINT_SHURIKEN => ("tconstruct:throw.shuriken", 4),
        _ => return None,
    })
}

#[mod_api::sunrise_part]
impl ItemPart for Throwable {
    /// GlowBallItem / EFLNItem / ShurikenItem.use.
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        let id = sid(event.stack);
        let (sound, cooldown) = thrown(id)?;
        let HostSide::Server(host) = host else { return Some(InteractionResult::Success) };
        let p = host.players().acting_player()?;
        let uuid = p.uuid();
        if cooldown > 0 {
            // ShurikenItem.use: `getCooldowns().addCooldown(this, 4)`, the item's own group, drawn on the player's
            // hotbar. An item on cooldown isn't used (vanilla's PASS before `Item.use`).
            if p.has_item_cooldown(&*host, id).unwrap_or(false) {
                return Some(InteractionResult::Pass);
            }
            let _ = p.set_item_cooldown(host, id, cooldown);
        }
        let living = LivingRef::new(p.entity());
        if let (Some(level), Some(at)) = (p.level(&*host), p.entity().position(&*host)) {
            let _ = level.play_sound(host, sound, at, SoundSource::Neutral, 0.5, 0.4 / (md::rand_f32() * 0.4 + 0.8), None);
        }
        let kind = Identifier::parse(id).ok()?;
        let shot = host.spawn_projectile_from_rotation(living, &kind, event.stack, 0.0, 1.5, 1.0);
        tlog(&format!("thrown {id}: {}", shot.is_some()));
        let _ = p.award_stat(host, StatType::Used, id, 1);
        if !p.is_creative(&*host).unwrap_or(false) {
            let place = ItemPlace::PlayerHand { player: uuid, hand: event.hand };
            let res = ItemResource::of(event.stack);
            let _ = host.transaction(|host, tx| if place.extract(host, tx, &res, 1) == Ok(1) { Ok(()) } else { Err(()) });
        }
        Some(InteractionResult::Success)
    }

    /// The dispenser behaviour (AbstractProjectileDispenseBehavior): handled by the game for thrown types.
    fn dispense_item(&self, _host: &mut dyn ServerHost, _event: &mut DispenseItemEvent<'_>) -> Option<Dispensed> {
        None
    }
}

/// After the impacts, on the level's tick: projectiles to remove and EFLN blasts (dimension, at).
static WORK: Mutex<Vec<(u32, String, [f64; 3])>> = Mutex::new(Vec::new());
static DONE: Mutex<Vec<u32>> = Mutex::new(Vec::new());
/// The EFLN blast going off, for `explosion_detonate` and the damage: its centre, the sphere's blocks it breaks, and
/// those of them the game's rays reached.
static BLAST: Mutex<Option<Blast>> = Mutex::new(None);

struct Blast {
    at: [f64; 3],
    sphere: Vec<[i32; 3]>,
    reached: Vec<[i32; 3]>,
}

/// EFLNExplosion's radius.
const EFLN_RADIUS: i32 = 4;

/// EFLNExplosion.explode's blocks: every cell of the sphere `x^2 + y^2 + z^2 <= r^2` round `centre` (floored) that
/// isn't air and whose strength there, `r (1 - d / r^2)`, outlasts its resistance's step `(res + 0.3) 0.3`.
/// `resistance` answers each cell of the box `centre ± r` in `ResistanceCells::Box` order (x, then z, then y).
fn efln_sphere(centre: [i32; 3], resistance: &[ExplosionResistance]) -> Vec<[i32; 3]> {
    let (r, r2) = (EFLN_RADIUS, EFLN_RADIUS * EFLN_RADIUS);
    let side = 2 * r + 1;
    let mut out = Vec::new();
    for dy in -r..=r {
        for dz in -r..=r {
            for dx in -r..=r {
                let d = dx * dx + dy * dy + dz * dz;
                if d > r2 {
                    continue;
                }
                let i = (((dy + r) * side + (dz + r)) * side + (dx + r)) as usize;
                // NoResistance is air with no fluid (`isEmptyBlock`); Unavailable is a cell the game can't answer.
                let Some(ExplosionResistance::Resists(res)) = resistance.get(i) else { continue };
                let strength = r as f32 * (1.0 - d as f32 / r2 as f32) - (res + 0.3) * 0.3;
                if strength > 0.0 {
                    out.push([centre[0] + dx, centre[1] + dy, centre[2] + dz]);
                }
            }
        }
    }
    out
}

/// `projectile_impact` for the gadget throwables; true cancels the hit.
pub(crate) fn impact(host: &mut dyn ServerHost, event: &ProjectileImpactEvent<'_>) -> bool {
    let kind = event.kind;
    if !matches!(kind, GLOW_BALL | EFLN_BALL | QUARTZ_SHURIKEN | FLINT_SHURIKEN) {
        return false;
    }
    if DONE.lock().unwrap_or_else(|e| e.into_inner()).contains(&event.projectile) {
        return true;
    }
    let done = || {
        let mut d = DONE.lock().unwrap_or_else(|e| e.into_inner());
        if d.len() > 4096 {
            d.clear();
        }
        d.push(event.projectile);
    };
    let Some(level) = host.level(event.dimension) else { return false };
    match kind {
        GLOW_BALL => {
            let target = match event.entity.and_then(|e| host.entity(e)) {
                Some(e) => e.position(&*host).map(|p| (p.map(|v| v.floor() as i32), 0)),
                None => (0..6).find(|f| step([0, 0, 0], *f) == event.face).map(|f| ([event.block[0] + event.face[0], event.block[1] + event.face[1], event.block[2] + event.face[2]], opposite(f))),
            };
            if let Some((p, facing)) = target {
                let placed = add_glow(host, level, p, facing);
                tlog(&format!("glow ball at {p:?}: glow placed {placed}"));
            }
            if event.entity.is_some() {
                done();
                WORK.lock().unwrap_or_else(|e| e.into_inner()).push((event.projectile, String::new(), [0.0; 3]));
                return true;
            }
            false
        }
        EFLN_BALL => {
            done();
            WORK.lock().unwrap_or_else(|e| e.into_inner()).push((event.projectile, event.dimension.to_owned(), event.at));
            true
        }
        _ => {
            if event.entity.is_none()
                && let Some(s) = new_stack(kind, 1)
            {
                let _ = level.spawn_item_with_motion(host, event.at[0], event.at[1], event.at[2], s, [0.0, 0.2, 0.0], 10);
            }
            false
        }
    }
}

fn face_of(d: usize) -> BlockFace {
    [BlockFace::Down, BlockFace::Up, BlockFace::North, BlockFace::South, BlockFace::West, BlockFace::East][d]
}

/// GlowBlock.addGlow: glow at `p` stuck to `facing`, else the first side that holds it, where the block is replaceable
/// and isn't glow already.
fn add_glow(host: &mut dyn ServerHost, level: LevelRef, p: Pos, facing: usize) -> bool {
    let Some(here) = level.block_state(&*host, p) else { return false };
    let name = host.block_state_string(here).unwrap_or_default();
    if name.starts_with(GLOW) || !host.block_states().replaceable(here).unwrap_or(false) || name.starts_with("minecraft:water") || name.starts_with("minecraft:lava") {
        return false;
    }
    let holds = |host: &dyn ServerHost, d: usize| level.block_face_sturdy(host, step(p, d), face_of(opposite(d)), SupportType::Full).unwrap_or(false);
    let Some(side) = std::iter::once(facing).chain(0..6).find(|d| holds(&*host, *d)) else { return false };
    let Some(state) = host.block_state_id(&format!("{GLOW}[facing={}]", DIR_NAMES[side])) else { return false };
    level.set_block(host, p, state, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS).unwrap_or(false)
}

/// `explosion_detonate` during an EFLN blast: the game's rays' blocks, only those of EFLNExplosion's sphere
/// (`efln_sphere`, worked out from the cells' explosion resistance before the blast). The sphere's blocks the rays
/// missed are broken after it (`level_tick`).
pub(crate) fn detonate(event: &mut ExplosionDetonateEvent<'_>) {
    let mut g = BLAST.lock().unwrap_or_else(|e| e.into_inner());
    let Some(b) = g.as_mut() else { return };
    let c = b.at;
    if (c[0] - event.center[0]).abs() > 0.01 || (c[1] - event.center[1]).abs() > 0.01 || (c[2] - event.center[2]).abs() > 0.01 {
        return;
    }
    let before = event.blocks.len();
    event.blocks.retain(|p| b.sphere.contains(p));
    b.reached.clone_from(&*event.blocks);
    tlog(&format!("EFLN blast at {c:?}: {} of {before} blocks in the sphere", event.blocks.len()));
}

/// CustomExplosion's damage `(s^2 + s) / 2 x 8 + 1` from vanilla's `(s^2 + s) / 2 x 7 x 2r + 1` (r 4).
pub(crate) fn blast_damage(event: &mut LivingIncomingDamageEvent<'_>) {
    if BLAST.lock().unwrap_or_else(|e| e.into_inner()).is_some() && event.source.damage_type.contains("explosion") {
        let d = event.container.new_damage();
        let _ = event.container.set_new_damage(((d - 1.0) / 7.0 + 1.0).floor().max(0.0));
    }
}

// ---- The level and player ticks

pub(crate) fn level_tick(host: &mut dyn ServerHost) {
    punji_tick(host);
    let work = std::mem::take(&mut *WORK.lock().unwrap_or_else(|e| e.into_inner()));
    for (id, dim, at) in work {
        if let Some(e) = host.entity(id) {
            let _ = e.discard(host);
        }
        if dim.is_empty() {
            continue;
        }
        let Some(level) = host.level(&dim) else { continue };
        let f = at.map(|v| v.floor() as i32);
        let (r, radius) = (EFLN_RADIUS, EFLN_RADIUS as f32);
        let cells = ResistanceCells::Box { min: [f[0] - r, f[1] - r, f[2] - r], max: [f[0] + r, f[1] + r, f[2] + r] };
        let resistance = level.explosion_resistances(host, &cells, None, at, radius, false, ExplosionInteraction::Block).unwrap_or_default();
        let sphere = efln_sphere(f, &resistance);
        *BLAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(Blast { at, sphere, reached: Vec::new() });
        let ok = level.explode(host, None, at, radius, false, ExplosionInteraction::Block, ExplosionSettings::default());
        let blast = BLAST.lock().unwrap_or_else(|e| e.into_inner()).take();
        // The sphere's blocks the rays didn't reach, broken as the same blast breaks them.
        let missed: Vec<[i32; 3]> = blast.map(|b| b.sphere.into_iter().filter(|p| !b.reached.contains(p)).collect()).unwrap_or_default();
        let more = if missed.is_empty() { Ok(0) } else { level.explosion_destroy_blocks(host, &missed, None, at, radius, false, ExplosionInteraction::Block) };
        tlog(&format!("EFLN exploded at {at:?}: {ok:?}, {more:?} more of the sphere"));
    }
    let mut d = DONE.lock().unwrap_or_else(|e| e.into_inner());
    if d.len() > 256 {
        d.drain(..128);
    }
}

/// What the player tick saw of each player, for `living_fall` (whose loop context reads no entity): bouncy (the
/// slime boots' modifier or the effect), sneaking, the step height bonus, the gravity.
#[derive(Clone, Copy)]
pub(crate) struct Seen {
    bouncy: bool,
    sneaking: bool,
    step: f64,
    gravity: f64,
    velocity: [f64; 3],
}

static SEEN: Mutex<Option<HashMap<u32, Seen>>> = Mutex::new(None);
/// Bounces to apply on the next player tick: (entity, velocity).
static BOUNCE: Mutex<Vec<(u32, [f64; 3])>> = Mutex::new(Vec::new());

const ANTIGRAVITY_ID: &str = "tconstruct:antigravity";
const CARRY_ID: &str = "tconstruct:carry";

pub(crate) fn player_tick(host: &mut dyn ServerHost) {
    let Some(p) = host.players().acting_player() else { return };
    let (uuid, entity) = (p.uuid(), p.entity());
    let living = LivingRef::new(entity);
    // Bounces queued by living_fall.
    let bounces: Vec<[f64; 3]> = {
        let mut b = BOUNCE.lock().unwrap_or_else(|e| e.into_inner());
        let mine = b.iter().filter(|(e, _)| *e == entity.id()).map(|(_, v)| *v).collect();
        b.retain(|(e, _)| *e != entity.id());
        mine
    };
    for v in bounces {
        let ok = entity.set_velocity(host, v);
        let _ = entity.play_sound(host, "tconstruct:slimy_bounce", 1.0, 1.0);
        tlog(&format!("bounce {v:?}: {ok}"));
    }
    let effect = |host: &dyn ServerHost, id: &str| key(id).and_then(|k| living.effect(host, k));
    let boots = living.equipment(&*host, EquipmentSlot::Feet);
    let bouncy = boots.as_ref().is_some_and(|s| md::modifier_level(s, "tconstruct:bouncy") > 0) || effect(&*host, "tconstruct:bouncy").is_some();
    let seen = Seen {
        bouncy,
        sneaking: entity.is_shift_key_down(&*host).unwrap_or(false),
        step: living.attribute_value(&*host, "minecraft:step_height").unwrap_or(0.6) - 0.6,
        gravity: living.attribute_value(&*host, "minecraft:gravity").unwrap_or(0.08),
        velocity: entity.velocity(&*host).unwrap_or([0.0; 3]),
    };
    SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().insert(entity.id(), seen);
    double_jump(host, entity, living, boots.as_ref());
    // Antigravity: the gravity reversed while it lasts.
    let anti = effect(&*host, ANTIGRAVITY_ID).is_some();
    if let Ok(id) = Identifier::parse(ANTIGRAVITY_ID) {
        let has = living.attribute_modifiers(&*host, "minecraft:gravity").is_some_and(|m| m.iter().any(|x| x.id == id));
        if anti && !has {
            let m = AttributeModifier::new(id, -2.0, AttributeOperation::AddMultipliedTotal);
            let _ = living.add_attribute_modifier(host, "minecraft:gravity", m, ModifierLifetime::Transient);
        } else if !anti && has {
            let _ = living.remove_attribute_modifier(host, "minecraft:gravity", &id);
        }
    }
    // Returning: back where it was got, on its last tick.
    if let Some(e) = effect(&*host, "tconstruct:returning")
        && e.duration <= 2
    {
        let back = {
            let mut r = RETURNING.lock().unwrap_or_else(|e| e.into_inner());
            r.iter().position(|(u, _, _)| *u == uuid).map(|i| r.remove(i))
        };
        if let Some((_, dim, at)) = back
            && p.dimension(&*host).is_some_and(|d| d.to_string() == dim)
        {
            let ok = entity.teleport_to(host, [at[0] + 0.5, at[1], at[2] + 0.5], None);
            tlog(&format!("returning to {at:?}: {ok}"));
        }
    }
    // The piggyback pack: carry while it carries (amplifier: the count - 1), riders over the pack's count let go;
    // carry's -5 % speed a level (MULTIPLY_TOTAL).
    let chest = living.equipment(&*host, EquipmentSlot::Chest);
    let pack = chest.as_ref().filter(|s| sid(s) == BACKPACK).map(|s| s.count()).unwrap_or(0);
    let mut riders = Vec::new();
    let mut top = entity;
    while let Some(next) = top.passengers(&*host).and_then(|v| v.first().copied()) {
        riders.push(next);
        top = next;
        if riders.len() > 8 {
            break;
        }
    }
    for (i, r) in riders.iter().enumerate() {
        if i as u32 >= pack {
            let _ = r.stop_riding(host);
        }
    }
    let carried = riders.len().min(pack as usize);
    if carried > 0
        && let Some(k) = key(CARRY_ID)
    {
        let _ = living.add_effect(host, &ActiveEffect::new(k, 2, carried as i32 - 1, true, false, true));
    }
    let carry = effect(&*host, CARRY_ID).map(|e| e.amplifier + 1).unwrap_or(0);
    if let Ok(id) = Identifier::parse(CARRY_ID) {
        let current = living.attribute_modifiers(&*host, "minecraft:movement_speed").and_then(|m| m.into_iter().find(|x| x.id == id));
        let want = -0.05 * f64::from(carry);
        if carry > 0 && current.as_ref().is_none_or(|c| (c.amount - want).abs() > 1e-9) {
            let _ = living.remove_attribute_modifier(host, "minecraft:movement_speed", &id);
            let m = AttributeModifier::new(id, want, AttributeOperation::AddMultipliedTotal);
            let _ = living.add_attribute_modifier(host, "minecraft:movement_speed", m, ModifierLifetime::Transient);
        } else if carry == 0 && current.is_some() {
            let _ = living.remove_attribute_modifier(host, "minecraft:movement_speed", &id);
        }
    }
}

/// ModifierEvents.bounceOnFall for a player the tick saw bouncy: no damage (sneaking: half), the fall's speed back up
/// (`sqrt(distance / gravity) x gravity`), the distance 0.
pub(crate) fn living_fall(event: &mut LivingFallEvent<'_>) -> bool {
    let Some(s) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&event.entity).copied()) else { return false };
    let d = event.distance;
    if !s.bouncy || (d < 3.0 && s.velocity[1] > -0.3) || d <= 0.5 + s.step {
        return false;
    }
    if s.sneaking {
        event.multiplier = 0.5;
        return false;
    }
    event.multiplier = 0.0;
    let g = s.gravity.max(1e-4);
    let up = (d / g).sqrt() * g;
    BOUNCE.lock().unwrap_or_else(|e| e.into_inner()).push((event.entity, [s.velocity[0] / 0.975, up, s.velocity[2] / 0.975]));
    event.distance = 0.0;
    tlog(&format!("bouncy fall {d:.2}: up {up:.3}"));
    true
}

// ---- The piggyback pack

#[mod_api::sunrise_part]
impl ItemPart for Backpack {
    /// PiggyBackPackItem.interactLivingEntity / pickupEntity, on the server (owned and swung: SuccessServer; owned
    /// without a swing: Consume).
    fn item_interact_mob(&self, host: HostSide, event: &mut ItemInteractMobEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else { return None };
        piggyback(host, event).map(|swing| if swing { InteractionResult::SuccessServer } else { InteractionResult::Consume })
    }
}

fn piggyback(host: &mut dyn ServerHost, event: &mut ItemInteractMobEvent<'_>) -> Option<bool> {
    {
        let p = host.players().acting_player()?;
        let player = p.entity();
        let living = LivingRef::new(player);
        let target = host.entity(u32::try_from(event.entity).ok()?)?;
        if host.entity_type_in_tag(event.kind, "tconstruct:piggybackpack_blacklist") {
            return Some(false);
        }
        let chest = living.equipment(&*host, EquipmentSlot::Chest);
        let wearing = chest.as_ref().is_some_and(|s| sid(s) == BACKPACK);
        // The pack goes on the chest: only when worn already, or when the inventory has room for the armour there.
        if !wearing && chest.is_some() && !has_free_slot(host, p.uuid()) {
            return None;
        }
        // Not riding each other already; at most 3 deep.
        let is_vehicle = |host: &dyn ServerHost, e: EntityRef, of: EntityRef| {
            let mut v = e.vehicle(host);
            let mut n = 0;
            while let Some(x) = v {
                if x.id() == of.id() {
                    return true;
                }
                n += 1;
                if n > 16 {
                    break;
                }
                v = x.vehicle(host);
            }
            false
        };
        if is_vehicle(&*host, player, target) || is_vehicle(&*host, target, player) {
            return Some(false);
        }
        let mut top = player;
        let mut count = 0;
        while let Some(next) = top.passengers(&*host).and_then(|v| v.first().copied()) {
            top = next;
            count += 1;
            if count >= 3 {
                break;
            }
            if event.kind == "minecraft:player" && next.kind(&*host).is_some_and(|k| k.id(&*host).as_deref() == Some("minecraft:player")) {
                return Some(false);
            }
        }
        if top.is_vehicle(&*host).unwrap_or(false) || count >= 3 || target.start_riding(host, top, true).is_err() {
            tlog(&format!("piggyback: {} could not ride", event.kind));
            return Some(false);
        }
        let creative = p.is_creative(&*host).unwrap_or(false);
        let one = with_count(event.stack, 1)?;
        if !wearing {
            if let Some(old) = chest {
                give_player(host, p.uuid(), old);
            }
            let _ = living.set_equipment(host, EquipmentSlot::Chest, Some(one));
            take_one(host, p.uuid(), event.stack, creative);
        } else if let Some(c) = chest.filter(|c| c.count() < (count + 1) as u32) {
            if let Some(more) = with_count(&c, c.count() + 1) {
                let _ = living.set_equipment(host, EquipmentSlot::Chest, Some(more));
                take_one(host, p.uuid(), event.stack, creative);
            }
        }
        tlog(&format!("piggyback: {} rides, {} carried", event.kind, count + 1));
        Some(true)
    }
}

fn has_free_slot(host: &mut dyn ServerHost, player: u128) -> bool {
    (0..36).any(|slot| ItemPlace::PlayerSlot { player, slot }.stack(host).is_none())
}

fn take_one(host: &mut dyn ServerHost, player: u128, stack: &ItemStack, creative: bool) {
    if creative {
        return;
    }
    let place = ItemPlace::PlayerHand { player, hand: Hand::Main };
    let res = ItemResource::of(stack);
    let _ = host.transaction(|host, tx| if place.extract(host, tx, &res, 1) == Ok(1) { Ok(()) } else { Err(()) });
}

use crate::smeltery::containers::give_player;

// ---- The consume effects

/// `consume_effect`: `tconstruct:remove_random_effect` (CheeseItem: one random curable effect cured; every effect
/// is milk-curable in 26.3), `tconstruct:set_fire` (MagmaBottle: `setSecondsOnFire`), `tconstruct:give_item`
/// (CheeseBlockItem: the ingots back, none in creative).
/// Workaround for sunrise-org/sunrise#4673 (fix: #4674): Sunrise main hands a mod's own consume effect its small
/// numbers as `Byte(1)`. These are the numbers Tinkers' item files declare (MagmaBottleItem's 15 s, CheeseBlockItem's
/// 3 ingots), used for those items instead of the event's; `declared_numbers_match_the_item_files` keeps them equal.
const DECLARED: &[(&str, &str, &str, i32)] = &[
    ("tconstruct:magma_bottle", "tconstruct:set_fire", "seconds", 15),
    ("tconstruct:cheese_block", "tconstruct:give_item", "count", 3),
];

fn declared(stack: &ItemStack, kind: &str, key: &str) -> Option<i32> {
    let item = sid(stack);
    DECLARED.iter().find(|d| d.0 == item && d.1 == kind && d.2 == key).map(|d| d.3)
}

pub(crate) fn consume_effect(host: &mut dyn ServerHost, event: &ConsumeEffectEvent<'_>) {
    let Some(p) = host.players().acting_player() else { return };
    let living = LivingRef::new(p.entity());
    let data = match event.data {
        Tag::Compound(c) => c.clone(),
        _ => Compound::new(),
    };
    tlog(&format!("consume effect {}: {:?}", event.kind, event.data));
    match event.kind {
        "tconstruct:remove_random_effect" => {
            let list = living.effects(&*host).unwrap_or_default();
            if !list.is_empty() {
                let i = ((md::rand_f32() * list.len() as f32) as usize).min(list.len() - 1);
                let ok = living.remove_effect(host, list[i].id);
                tlog(&format!("cheese: removed effect {i} of {} ({ok})", list.len()));
            }
        }
        "tconstruct:set_fire" => {
            let seconds = declared(event.stack, event.kind, "seconds").unwrap_or_else(|| gi(&data, "seconds", 15));
            let ok = p.entity().set_remaining_fire_ticks(host, seconds * 20);
            tlog(&format!("set fire {seconds} s: {ok}"));
        }
        "tconstruct:give_item" => {
            if p.is_creative(&*host).unwrap_or(false) {
                return;
            }
            let item = gs(&data, "item").unwrap_or_default();
            let count = declared(event.stack, event.kind, "count").unwrap_or_else(|| gi(&data, "count", 1)).max(1) as u32;
            if let Some(s) = new_stack(&item, count) {
                give_player(host, p.uuid(), s);
                tlog(&format!("given {count} {item}"));
            }
        }
        _ => {}
    }
}

// ---- Double jump (DoubleJumpHandler.extraJump)

/// The host's jump key was pressed (set on the client thread, read by the integrated server's player tick).
pub(crate) static JUMP_KEY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The host player's double jump, as the vertical push its client adds (the server cannot set a player's velocity:
/// the window's player moves itself). Written by the server tick, taken by the next client tick.
static JUMP_PUSH: Mutex<Option<f32>> = Mutex::new(None);

/// The client tick: the double jump the server granted, as a push on the local player.
pub(crate) fn client_jump_push(host: &mut dyn ClientHost) {
    // The local player's vertical speed now, from the camera's last two ticks (the API reads no client velocity):
    // the push brings it to the jump's speed, as Java's jumpFromGround sets it.
    let y = host.camera().map(|c| c.position[1]);
    let prev = std::mem::replace(&mut *LAST_Y.lock().unwrap_or_else(|e| e.into_inner()), y);
    jump_peak(y, prev);
    if let Some(target) = JUMP_PUSH.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let vy = match (y, prev) {
            (Some(a), Some(b)) => ((a - b - 0.08) * 0.98) as f32,
            _ => 0.0,
        };
        let up = target - vy;
        host.push_player([0.0, up, 0.0]);
        tlog(&format!("double jump push {up:.2} (speed {vy:.2} -> {target:.2})"));
    }
}
static LAST_Y: Mutex<Option<f64>> = Mutex::new(None);
/// Debug only: each airtime's peak above the take-off height (the ground is two ticks without vertical movement).
static PEAK: Mutex<(Option<f64>, f64, u8)> = Mutex::new((None, 0.0, 0));
fn jump_peak(y: Option<f64>, prev: Option<f64>) {
    let (Some(a), Some(b)) = (y, prev) else { return };
    let mut p = PEAK.lock().unwrap_or_else(|e| e.into_inner());
    p.2 = if (a - b).abs() < 1e-6 { p.2.saturating_add(1) } else { 0 };
    if a > p.1 { p.1 = a; }
    if p.2 >= 2 {
        if let Some(base) = p.0 {
            if p.1 - base > 0.3 { tlog(&format!("jump peak {:.2}", p.1 - base)); }
        }
        *p = (Some(a), a, p.2);
    }
}

pub(crate) fn key_mapping(event: &KeyMappingPreEvent<'_>) {
    if event.mapping == "key.jump" && event.pressed {
        JUMP_KEY.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Per player entity: the jump input last tick, the extra jumps used since the ground, on the ground last tick, the
/// ticks since the last extra jump. One count per player, kept by the server's player tick only.
static JUMPS: Mutex<Option<HashMap<u32, (bool, i32, bool, u32)>>> = Mutex::new(None);

/// DoubleJumpHandler: in the air (not climbing, not in water), a new press of jump while the jump count
/// (`tconstruct:generic.jump_count`: 1 + the double jump effect + the boots' `double_jump` levels) allows an extra
/// jump does LivingEntity.jumpFromGround (0.42 x the block's jump factor + jump boost, the sprint push), with the
/// extra jump sound. Java's client asks the server on the key press; here the server reads the player's jump input
/// (the fork has no mod channel from client to server), on its rising edge. Landing resets the count.
fn double_jump(host: &mut dyn ServerHost, entity: EntityRef, living: LivingRef, boots: Option<&ItemStack>) {
    // The press: the server's jump input, or the host's own jump key (`key_mapping_pre` on this process's client).
    let pressed = JUMP_KEY.swap(false, std::sync::atomic::Ordering::Relaxed);
    let ground = entity.on_ground(&*host).unwrap_or(true);
    let extra = key("tconstruct:double_jump").and_then(|k| living.effect(&*host, k)).map_or(0, |e| e.amplifier + 1) + boots.map_or(0, |b| md::modifier_level(b, "tconstruct:double_jump"));
    let mut g = JUMPS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_default();
    let (was, used, was_ground, since) = map.get(&entity.id()).copied().unwrap_or((false, 0, true, u32::MAX));
    let since = since.saturating_add(1);
    // A key press seen the tick after the player left the ground is the ground jump itself (Java's client asks for an
    // extra jump only for a press in the air): it counts only while the player was already airborne the tick before.
    let jumping = living.is_jumping(&*host).unwrap_or(false) || (pressed && !was_ground);
    // Landing resets the count (Java: only on the ground). The server's on_ground of a player its client moves can read
    // true for one tick mid-air right after an extra jump's push (the client's position report still from before it):
    // a single ground tick then is no landing. The ground counts once it held two ticks, or one well after the jump.
    let landed = ground && (was_ground || since > 6);
    if landed && used > 0 {
        tlog(&format!("landed: extra jumps reset ({used} used, {since} ticks after the last)"));
    }
    let used = if landed { 0 } else { used };
    let mut now_used = used;
    if jumping && !was {
        tlog(&format!("jump input: ground {ground}, extra {extra}, used {used}"));
    }
    if jumping && !was && !ground && extra > used {
        let v = entity.velocity(&*host).unwrap_or([0.0; 3]);
        let boost = key("minecraft:jump_boost").and_then(|k| living.effect(&*host, k)).map_or(0.0, |e| 0.1 * f64::from(e.amplifier + 1));
        let mut nv = [v[0], 0.42 + boost, v[2]];
        if host.players().acting_player().is_some_and(|p| p.entity().id() == entity.id()) && v[0].hypot(v[2]) > 0.2 {
            // Sprinting: 0.2 along the facing.
            if let Some([yaw, _]) = entity.rotation(&*host) {
                let r = f64::from(yaw).to_radians();
                nv[0] -= r.sin() * 0.2;
                nv[2] += r.cos() * 0.2;
            }
        }
        let mut ok = entity.set_velocity(host, nv);
        if !ok && host.players().acting_player().is_some_and(|p| p.entity().id() == entity.id()) {
            // The host's own player: its client moves it, so the jump goes there as a push (to 0.42 up from now).
            *JUMP_PUSH.lock().unwrap_or_else(|e| e.into_inner()) = Some(nv[1] as f32);
            ok = true;
        }
        let _ = entity.play_sound(host, "tconstruct:extra_jump", 0.5, 0.5);
        now_used += 1;
        tlog(&format!("double jump {now_used} of {extra}: {ok}"));
    }
    let since = if now_used > used { 0 } else { since };
    map.insert(entity.id(), (jumping, now_used, ground, since));
}

#[cfg(test)]
mod efln_tests {
    use super::*;

    #[test]
    fn efln_sphere_follows_resistance() {
        let side = (2 * EFLN_RADIUS + 1) as usize;
        let at = |dx: i32, dy: i32, dz: i32| (((dy + EFLN_RADIUS) as usize * side + (dz + EFLN_RADIUS) as usize) * side + (dx + EFLN_RADIUS) as usize);
        // Stone (6) everywhere: strength 4 (1 - d / 16) beats (6 + 0.3) 0.3 = 1.89 while d < 8.44.
        let stone = vec![ExplosionResistance::Resists(6.0); side * side * side];
        let hit = efln_sphere([0, 64, 0], &stone);
        assert!(hit.contains(&[0, 64, 0]) && hit.contains(&[2, 66, 0]) && hit.contains(&[2, 64, 2]));
        assert!(!hit.contains(&[3, 64, 0]) && !hit.contains(&[2, 66, 2]));
        // Air and obsidian (1200) are never in it; dirt (0.5) out to the sphere's edge, d <= 15.
        let mut cells = vec![ExplosionResistance::Resists(0.5); side * side * side];
        cells[at(0, 0, 0)] = ExplosionResistance::NoResistance;
        cells[at(1, 0, 0)] = ExplosionResistance::Resists(1200.0);
        let hit = efln_sphere([0, 0, 0], &cells);
        assert!(!hit.contains(&[0, 0, 0]) && !hit.contains(&[1, 0, 0]));
        assert!(hit.contains(&[3, 2, 1]) && !hit.contains(&[4, 0, 0]) && !hit.contains(&[3, 3, 0]));
    }
}

#[cfg(test)]
mod declared_tests {
    #[test]
    fn declared_numbers_match_the_item_files() {
        let magma = include_str!("../data/tconstruct/sunrise/item/magma_bottle.json");
        let cheese = include_str!("../data/tconstruct/sunrise/block/cheese_block.json");
        assert!(magma.contains("\"type\": \"tconstruct:set_fire\"") && magma.contains("\"seconds\": 15"));
        assert!(cheese.contains("\"type\": \"tconstruct:give_item\"") && cheese.contains("\"count\": 3"));
        assert_eq!(super::DECLARED.len(), 2);
    }
}
