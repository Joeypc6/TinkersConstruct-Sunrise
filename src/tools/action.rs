//! Stage 3C, the area of effect, tool actions and area attacks (spec-tools.md 4 AOE; spec-modifiers.md 6; upstream
//! `tconstruct.rs` 11020-11440, 12230-12275 and 15972-16455):
//! - AOE mining (ToolHarvestLogic.handleBlockBreak): from the first block, when the tool is effective, the
//!   definition's box, circle, vein or tree (each grown by `tconstruct:expanded`) breaks too, as the player's tool
//!   breaks blocks (its drops at the tool's tier and enchantments, its wear); the box's and the circle's blocks are
//!   checked as they come (not air, at most 3x the first block's hardness, effective for the tool as it is then).
//! - Tool actions (BlockTransformModule, ToolActionTransformModule: `use_on`): a right click strips, scrapes and unwaxes
//!   (axe), tills (hoe) or flattens (shovel) the clicked block and the AOE's blocks, 1 durability each.
//! - Area attacks after a melee hit (SweepWeaponAttack: sword 1, cleaver 2; CircleWeaponAttack: kama 1, scythe 2;
//!   + expanded): the mobs around the target take the hit too.

use super::data::*;
use super::modifier::{self as md, ModModule, ToolMods};
use super::tool::*;
use crate::smeltery::util::*;
use mod_api::serde_json::Value;
use mod_api::*;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

/// A box's size (BoxAOEIterator's BoxSize).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct BoxSize {
    width: i32,
    height: i32,
    depth: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Expansion {
    SideHit,
    Pitch,
    Height,
}

/// The AOE modules of a tool definition.
#[derive(Clone, Debug)]
pub(crate) enum Aoe {
    Box { base: BoxSize, expansions: Vec<BoxSize>, direction: Expansion },
    Conditional { condition: BlockPred, if_true: Box<Aoe>, if_false: Box<Aoe> },
    Circle { diameter: i32, three_d: bool },
    Vein { max_distance: i32 },
    Tree { width: i32, depth: i32 },
}

fn parse_box(v: &Value) -> BoxSize {
    let int = |k: &str| v[k].as_i64().unwrap_or(0).clamp(0, 64) as i32;
    BoxSize { width: int("width"), height: int("height"), depth: int("depth") }
}

fn parse_aoe(v: &Value) -> Option<Aoe> {
    Some(match v["type"].as_str().unwrap_or("") {
        "tconstruct:box_aoe" => Aoe::Box {
            base: parse_box(&v["bonus"]),
            expansions: v["expansions"].as_array().into_iter().flatten().map(parse_box).collect(),
            direction: match v["expansion_direction"].as_str().unwrap_or("") {
                "tconstruct:pitch" => Expansion::Pitch,
                "tconstruct:height" => Expansion::Height,
                _ => Expansion::SideHit,
            },
        },
        "tconstruct:conditional_aoe" => Aoe::Conditional {
            condition: parse_block_pred(&v["condition"], "type"),
            if_true: Box::new(parse_aoe(&v["if_true"])?),
            if_false: Box::new(parse_aoe(&v["if_false"])?),
        },
        "tconstruct:circle_aoe" => Aoe::Circle { diameter: v["diameter"].as_i64().unwrap_or(1).clamp(1, 64) as i32, three_d: v["3D"].as_bool().unwrap_or(false) },
        "tconstruct:vein_aoe" => Aoe::Vein { max_distance: v["max_distance"].as_i64().unwrap_or(0).clamp(0, 64) as i32 },
        "tconstruct:tree_aoe" => Aoe::Tree { width: v["width_bonus"].as_i64().unwrap_or(0).clamp(0, 64) as i32, depth: v["depth_bonus"].as_i64().unwrap_or(0).clamp(0, 64) as i32 },
        _ => return None,
    })
}

/// The tool definition's AOE module.
fn aoe_of(item: &str) -> Option<Aoe> {
    let id = if item == "tconstruct:slimy_chestplate" { "tconstruct:slime_chestplate" } else { item };
    table().json["tool_definitions"][id]["modules"].as_array()?.iter().find_map(parse_aoe)
}

/// BoxAOEIterator.sizeFor: the base, whole cycles of every expansion, then the first `level % n`.
fn box_size_for(base: BoxSize, expansions: &[BoxSize], level: i32) -> BoxSize {
    let n = expansions.len() as i32;
    if level == 0 || n == 0 {
        return base;
    }
    let mut s = base;
    let add = |s: &mut BoxSize, e: &BoxSize, k: i32| {
        s.width += e.width * k;
        s.height += e.height * k;
        s.depth += e.depth * k;
    };
    if level >= n {
        for e in expansions {
            add(&mut s, e, level / n);
        }
    }
    for e in expansions.iter().take((level % n) as usize) {
        add(&mut s, e, 1);
    }
    s
}

fn clockwise(d: Pos) -> Pos {
    [-d[2], d[1], d[0]]
}

fn counter_clockwise(d: Pos) -> Pos {
    [d[2], d[1], -d[0]]
}

fn reverse(d: Pos) -> Pos {
    [-d[0], -d[1], -d[2]]
}

/// Entity.getDirection: the horizontal facing from the yaw.
fn facing_from_yaw(yaw: f32) -> Pos {
    match ((f64::from(yaw) / 90.0 + 0.5).floor() as i64).rem_euclid(4) {
        0 => [0, 0, 1],
        1 => [-1, 0, 0],
        2 => [0, 0, -1],
        _ => [1, 0, 0],
    }
}

/// IBoxExpansion.getDirections: width, height, depth and whether the height runs both ways.
fn box_directions(direction: Expansion, side: Pos, facing: Pos, pitch: f32) -> (Pos, Pos, Pos, bool) {
    match direction {
        Expansion::SideHit => {
            let depth = reverse(side);
            if side[1] != 0 { (clockwise(facing), facing, depth, true) } else { (counter_clockwise(side), [0, 1, 0], depth, true) }
        }
        Expansion::Pitch => {
            let width = clockwise(facing);
            if pitch < -60.0 {
                (width, facing, [0, 1, 0], true)
            } else if pitch > 60.0 {
                (width, facing, [0, -1, 0], true)
            } else {
                (width, [0, 1, 0], facing, true)
            }
        }
        Expansion::Height => {
            let depth = if side[1] != 0 { facing } else { reverse(side) };
            (clockwise(depth), [0, 1, 0], depth, false)
        }
    }
}

/// RectangleIterator's positions (width fastest, then height, then depth), the origin left out.
fn box_positions(origin: Pos, dirs: (Pos, Pos, Pos, bool), size: BoxSize) -> Vec<Pos> {
    let (w, h, d, down) = dirs;
    let mut out = Vec::new();
    let heights: Vec<i32> = if down { (-size.height..=size.height).collect() } else { (0..=size.height).collect() };
    for k in 0..=size.depth {
        for &j in &heights {
            for i in -size.width..=size.width {
                let p: Pos = std::array::from_fn(|a| origin[a] + w[a] * i + h[a] * j + d[a] * k);
                if p != origin {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Direction.values(): down, up, north, south, west, east.
const ALL_DIRECTIONS: [Pos; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

fn offset(p: Pos, d: Pos) -> Pos {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

/// The AOE's positions for `stack` at `origin` (whose block is `block`), hit on `side`; `checked` says whether each
/// still needs the hardness and effectiveness check (the box's and circle's). The vein's and the tree's are walks over
/// blocks of the origin's block (VeiningAOEIterator; TreeAOEIterator as a walk up and around the trunk and its
/// branches, at most 256 blocks).
fn aoe_positions(host: &dyn Host, level: LevelRef, stack: &ItemStack, origin: Pos, block: (BlockKey, &str), side: Pos, rot: [f32; 2]) -> Option<(Vec<Pos>, bool)> {
    let mut aoe = aoe_of(sid(stack))?;
    while let Aoe::Conditional { condition, if_true, if_false } = aoe {
        aoe = if condition.matches(host, block.0, block.1) { *if_true } else { *if_false };
    }
    let expanded = md::volatile_int(stack, "tconstruct:expanded").max(0);
    let facing = facing_from_yaw(rot[0]);
    let same = |p: Pos| level.block(host, p).is_some_and(|k| k == block.0);
    match aoe {
        Aoe::Box { base, expansions, direction } => {
            let size = box_size_for(base, &expansions, expanded);
            (size != BoxSize::default()).then(|| (box_positions(origin, box_directions(direction, side, facing, rot[1]), size), true))
        }
        Aoe::Circle { diameter, three_d } => {
            let d = diameter + expanded;
            if d == 1 {
                return None;
            }
            let (r, radius_sq) = (d / 2, d * d / 4);
            let size = BoxSize { width: r, height: r, depth: if three_d { r } else { 0 } };
            let list = box_positions(origin, box_directions(Expansion::SideHit, side, facing, rot[1]), size)
                .into_iter()
                .filter(|p| (0..3).map(|a| (p[a] - origin[a]) * (p[a] - origin[a])).sum::<i32>() <= radius_sq)
                .collect();
            Some((list, true))
        }
        Aoe::Vein { max_distance } => {
            let max = max_distance + expanded;
            let mut out = Vec::new();
            let mut seen: HashSet<Pos> = HashSet::from([origin]);
            let mut queue: VecDeque<(Pos, i32)> = VecDeque::from([(origin, 0)]);
            while let Some((p, dist)) = queue.pop_front() {
                if dist >= max {
                    continue;
                }
                for d in ALL_DIRECTIONS {
                    let n = offset(p, d);
                    if seen.insert(n) && same(n) {
                        out.push(n);
                        queue.push_back((n, dist + 1));
                    }
                }
            }
            Some((out, false))
        }
        Aoe::Tree { width, depth } => {
            // TreeAOEIterator: width + (E + 1) / 2, depth + E / 2; the trunk widened toward the player (the facing
            // for a top or bottom face, else into the hit face) and to its clockwise side.
            let (extra_width, extra_depth) = (width + (expanded + 1) / 2, depth + expanded / 2);
            let (width_dir, depth_dir) = if extra_width > 0 || extra_depth > 0 {
                let depth_dir = if side[1] != 0 { facing } else { reverse(side) };
                (clockwise(depth_dir), depth_dir)
            } else {
                (UP, UP)
            };
            let solid = |p: Pos| {
                let b = p.map(f64::from);
                level.has_collisions(host, [b[0] + 0.01, b[1] + 0.01, b[2] + 0.01, b[0] + 0.99, b[1] + 0.99, b[2] + 0.99]) == Some(true)
            };
            let mut walk = TreeWalk::new(&same, origin, width_dir, extra_width, depth_dir, extra_depth);
            let mut out = Vec::new();
            while let Some(p) = walk.next(&same, &solid) {
                out.push(p);
                if out.len() >= 1024 {
                    break;
                }
            }
            Some((out, false))
        }
        Aoe::Conditional { .. } => None,
    }
}

/// The face the player is looking at on `pos` (the side hit), up when the look ray finds another block.
fn side_hit(host: &dyn Host, player: EntityRef, pos: Pos) -> Pos {
    match player.pick(host, 6.0) {
        Some(hit) if !hit.miss && hit.block == pos => match hit.face {
            BlockFace::Down => [0, -1, 0],
            BlockFace::Up => [0, 1, 0],
            BlockFace::North => [0, 0, -1],
            BlockFace::South => [0, 0, 1],
            BlockFace::West => [-1, 0, 0],
            _ => [1, 0, 0],
        },
        _ => [0, 1, 0],
    }
}

static IN_AOE: AtomicBool = AtomicBool::new(false);

/// `break_block` for the acting player's tool (ToolHarvestLogic.mineBlock / handleBlockBreak): its wear (none for an
/// instant-break block), the block break effects (`after_break`), then, from the first block only and when the tool
/// was effective there, the AOE's other blocks broken as its harvest (drops with the tool, unless creative), each
/// costing its wear; air, unbreakable blocks and blocks the player may not change are skipped.
pub(crate) fn block_broken(host: &mut dyn ServerHost, pos: Pos, state: BlockState) {
    if IN_AOE.load(Ordering::Relaxed) {
        return;
    }
    let Some(player) = host.players().acting_player() else { return };
    let living = LivingRef::new(player.entity());
    let Some(stack) = living.hand_item(&*host, Hand::Main) else { return };
    let tools = tooling(&*host);
    let Some(def) = tools.defs.get(sid(&stack)) else { return };
    if !tool_initialized(&stack) {
        return;
    }
    let Some(level) = here(&*host) else { return };
    let key = state.block();
    let name = key.id(&*host).unwrap_or_default().to_owned();
    let hardness = host.block_states().destroy_speed(state).unwrap_or(1.0);
    let effective = tool_effective(&*host, def, &stack, key, &name);
    let harvest = tools.harvest.contains(sid(&stack));
    let cost = if tools.harvest_primary.contains(sid(&stack)) { 1 } else { 2 };
    let creative = player.is_creative(&*host).unwrap_or(false);
    let walk = if effective && stack.count() == 1 {
        let rot = player.entity().rotation(&*host).unwrap_or([0.0, 0.0]);
        let side = side_hit(&*host, player.entity(), pos);
        aoe_positions(&*host, level, &stack, pos, (key, &name), side, rot)
    } else {
        None
    };
    if harvest && hardness != 0.0 {
        wear(host, living, cost);
    }
    if effective && stack.count() == 1 {
        md::after_break(host, living, &stack, hardness, false);
    }
    let Some((list, checked)) = walk else { return };
    tlog(&format!("aoe from {pos:?}: {} positions (checked {checked})", list.len()));
    IN_AOE.store(true, Ordering::Relaxed);
    for p in list {
        let Some(held) = living.hand_item(&*host, Hand::Main).filter(|h| sid(h) == sid(&stack)) else { break };
        let Some(s) = level.block_state(&*host, p) else { continue };
        let k = s.block();
        let other = k.id(&*host).unwrap_or_default().to_owned();
        let h = host.block_states().destroy_speed(s).unwrap_or(-1.0);
        if other.ends_with(":air") || other == "minecraft:cave_air" || other == "minecraft:void_air" || h < 0.0 || !player.may_build(&*host).unwrap_or(false) {
            continue;
        }
        if checked {
            let ok = if hardness == 0.0 { h == 0.0 } else { h / hardness <= 3.0 };
            if !ok || !tool_effective(&*host, def, &held, k, &other) {
                continue;
            }
        } else if tool_broken(&held) {
            break;
        }
        if level.destroy_block(host, p, !creative, Some(&held), false).is_ok() {
            if harvest && h != 0.0 {
                wear(host, living, cost);
            }
            if effective {
                md::after_break(host, living, &held, h, true);
            }
        }
    }
    IN_AOE.store(false, Ordering::Relaxed);
}

// ---- Tool actions (ToolActionTransformModule)

/// The properties a transformed block keeps.
const KEPT: [&str; 8] = ["axis", "facing", "half", "shape", "type", "waterlogged", "hinge", "open"];

/// Forge's getToolModifiedState for the tool actions Tinkers' modules use: the new block id.
fn action_target(action: &str, id: &str) -> Option<String> {
    let path = id.strip_prefix("minecraft:")?;
    match action {
        "axe_strip" => {
            let woody = ["_log", "_wood", "_stem", "_hyphae"].iter().any(|s| path.ends_with(s)) || path == "bamboo_block";
            (woody && !path.starts_with("stripped_")).then(|| format!("minecraft:stripped_{path}"))
        }
        "axe_scrape" => {
            for (from, to) in [("oxidized_", "weathered_"), ("weathered_", "exposed_"), ("exposed_", "")] {
                if let Some(rest) = path.strip_prefix(from).filter(|r| r.contains("copper")) {
                    return Some(format!("minecraft:{to}{rest}"));
                }
            }
            None
        }
        "axe_wax_off" => path.strip_prefix("waxed_").map(|r| format!("minecraft:{r}")),
        "till" => match path {
            "grass_block" | "dirt" | "dirt_path" => Some("minecraft:farmland".to_owned()),
            "coarse_dirt" | "rooted_dirt" => Some("minecraft:dirt".to_owned()),
            _ => None,
        },
        "shovel_flatten" => matches!(path, "grass_block" | "dirt" | "podzol" | "coarse_dirt" | "mycelium" | "rooted_dirt").then(|| "minecraft:dirt_path".to_owned()),
        _ => None,
    }
}

/// ToolActionTransformModule.transform at `p`: the new state with the old one's kept properties, its sound and level
/// event, the block set. Tilling and flattening need air above (`require_ground`).
#[allow(clippy::too_many_arguments)]
fn transform_block(host: &mut dyn ServerHost, level: LevelRef, p: Pos, action: &str, sound: &str, ground: bool, event: i32, with_sound: bool) -> bool {
    let Some(old) = level.block_state(&*host, p) else { return false };
    let id = old.block().id(&*host).unwrap_or_default().to_owned();
    if ground {
        let above = level.block_state(&*host, offset(p, [0, 1, 0]));
        if !above.is_some_and(|a| host.block_states().replaceable(a).unwrap_or(false)) {
            return false;
        }
    }
    let Some(target) = action_target(action, &id) else { return false };
    let Some(mut new) = host.block_state_id(&target) else { return false };
    let states = host.block_states();
    for prop in KEPT {
        if let Ok(Some(v)) = old.property(&states, prop) {
            let v = v.to_owned();
            if let Ok(s) = new.with(&states, prop, &v) {
                new = s;
            }
        }
    }
    if with_sound {
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        let _ = level.play_sound(host, sound, at, SoundSource::Blocks, 1.0, 1.0, None);
        if event != -1 {
            let _ = level.level_event(host, event, p, 0);
        }
    }
    level.set_block(host, p, new, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS).unwrap_or(false)
}

/// `use_on` (ModifiableItem.useOn's afterBlockUse): not sneaking, not broken; each modifier's transform in order, the
/// first that turns something takes the click: the clicked block, then the AOE's blocks (40 sounds at most), 1
/// durability each.
pub(crate) fn use_on(host: &mut dyn ServerHost, event: &UseOnEvent<'_>) -> bool {
    let stack = event.stack;
    let tools = tooling(&*host);
    if !tools.defs.contains_key(sid(stack)) || !tool_initialized(stack) || tool_broken(stack) {
        return false;
    }
    let Some(player) = host.player(event.player) else { return false };
    if player.entity().is_shift_key_down(&*host).unwrap_or(false) || !player.may_build(&*host).unwrap_or(false) {
        return false;
    }
    let Some(level) = here(&*host) else { return false };
    let living = LivingRef::new(player.entity());
    let tm = ToolMods::of(&*host, stack);
    let origin = level.block(&*host, event.pos);
    let origin_name = origin.and_then(|k| k.id(&*host)).unwrap_or_default().to_owned();
    // HarvestModule (the kama's and scythe's right click): mature crops in the tool's area are harvested and replanted.
    if tm.entries().iter().any(|(m, _, _)| m.modules.iter().any(|x| matches!(x, ModModule::Harvest))) && crop(&origin_name).is_some() {
        let rot = player.entity().rotation(&*host).unwrap_or([0.0, 0.0]);
        let mut list = vec![event.pos];
        if let Some((more, _)) = origin.and_then(|k| aoe_positions(&*host, level, stack, event.pos, (k, &origin_name), event.face, rot)) {
            list.extend(more.into_iter().filter(|p| *p != event.pos));
        }
        let mut any = false;
        for p in list {
            let Some(held) = living.hand_item(&*host, Hand::Main).filter(|h| !tool_broken(h)) else { break };
            if harvest(host, level, p, &held) {
                any = true;
                wear(host, living, 1);
            }
        }
        if any {
            let at = [f64::from(event.pos[0]) + 0.5, f64::from(event.pos[1]) + 0.5, f64::from(event.pos[2]) + 0.5];
            let _ = level.play_sound(host, "minecraft:block.crop.break", at, SoundSource::Blocks, 1.0, 1.0, None);
            return true;
        }
    }
    for (m, lv, _) in tm.entries() {
        for module in &m.modules {
            let ModModule::Transform { action, sound, require_ground, event: ev, cond } = module else { continue };
            if !cond.matches(&tm.reg, &tm.view(sid(stack)), lv) || (*require_ground && event.face == [0, -1, 0]) {
                continue;
            }
            let did = transform_block(host, level, event.pos, action, sound, *require_ground, *ev, true);
            if did {
                wear(host, living, 1);
            }
            let mut count = 0;
            if let Some((list, _)) = origin.and_then(|k| {
                let rot = player.entity().rotation(&*host).unwrap_or([0.0, 0.0]);
                aoe_positions(&*host, level, stack, event.pos, (k, &origin_name), event.face, rot)
            }) {
                for p in list {
                    if living.hand_item(&*host, Hand::Main).is_none_or(|h| tool_broken(&h)) {
                        break;
                    }
                    if transform_block(host, level, p, action, sound, *require_ground, *ev, count < 40) {
                        count += 1;
                        wear(host, living, 1);
                    }
                }
            }
            if did || count > 0 {
                return true;
            }
        }
    }
    false
}

/// The crops HarvestModule replants: (block, mature age, the seed taken back from the drops).
fn crop(id: &str) -> Option<(&'static str, &'static str)> {
    Some(match id {
        "minecraft:wheat" => ("7", "minecraft:wheat_seeds"),
        "minecraft:carrots" => ("7", "minecraft:carrot"),
        "minecraft:potatoes" => ("7", "minecraft:potato"),
        "minecraft:beetroots" => ("3", "minecraft:beetroot_seeds"),
        "minecraft:nether_wart" => ("3", "minecraft:nether_wart"),
        "minecraft:cocoa" => ("2", "minecraft:cocoa_beans"),
        _ => return None,
    })
}

/// HarvestModule.harvest on one block: a mature crop drops its loot (with the tool) less one seed, and is set back to
/// age 0.
fn harvest(host: &mut dyn ServerHost, level: LevelRef, p: Pos, tool: &ItemStack) -> bool {
    let Some(old) = level.block_state(&*host, p) else { return false };
    let id = old.block().id(&*host).unwrap_or_default().to_owned();
    let Some((mature, seed)) = crop(&id) else { return false };
    let states = host.block_states();
    if !matches!(old.property(&states, "age"), Ok(Some(a)) if a == mature) {
        return false;
    }
    let Ok(young) = old.with(&states, "age", "0") else { return false };
    let Ok(Some(mut drops)) = level.destroy_block(host, p, true, Some(tool), true) else { return false };
    if let Some(i) = drops.iter().position(|d| sid(d) == seed) {
        match with_count(&drops[i], drops[i].count() - 1) {
            Some(rest) => drops[i] = rest,
            None => {
                drops.remove(i);
            }
        }
    }
    let _ = level.set_block(host, p, young, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS);
    for d in drops {
        let _ = level.spawn_item(host, f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5, d, 10);
    }
    true
}

// ---- Shears (ShearsAbilityModifier on an entity)

/// `entity_interact` with a shearing tool (`tconstruct:shears`, or `tconstruct:silky_shears` with silky) on an adult
/// sheep not yet sheared: it is sheared (its shearing loot, 1 to 3 wool of its colour) and the tool wears 1.
pub(crate) fn shear_entity(host: &mut dyn ServerHost, event: &EntityInteractEvent<'_>) -> bool {
    let Some(stack) = event.stack else { return false };
    const SHEARABLE: [&str; 4] = ["minecraft:sheep", "minecraft:mooshroom", "minecraft:snow_golem", "minecraft:bogged"];
    if event.kind != "minecraft:sheep" && SHEARABLE.contains(&event.kind) {
        tlog(&format!("shears on a {}: tool {}", event.kind, sid(stack)));
    }
    if !SHEARABLE.contains(&event.kind) || !tool_initialized(stack) || tool_broken(stack) {
        return false;
    }
    let shears = md::modifier_level(stack, "tconstruct:shears") > 0
        || (md::modifier_level(stack, "tconstruct:silky_shears") > 0 && md::modifier_level(stack, "tconstruct:silky") > 0);
    if !shears {
        return false;
    }
    let Some(e) = u32::try_from(event.entity).ok().and_then(|id| host.entity(id)) else { return false };
    let Some(nbt) = e.nbt(host) else {
        tlog(&format!("shears: no NBT for the {}", event.kind));
        return false;
    };
    let byte = |k: &str| match nbt.get(&js(k)) {
        Some(Tag::Byte(b)) => i32::from(*b),
        Some(Tag::Int(i)) => *i,
        _ => 0,
    };
    if event.kind != "minecraft:sheep" {
        return shear_other(host, event, e, &nbt);
    }
    if byte("Sheared") != 0 || byte("Age") < 0 {
        return false;
    }
    let (Some(level), Some(at)) = (here(&*host), e.position(&*host)) else { return false };
    // Sheep.shear: the shearing loot (its colour's wool) rolled while it still has its wool, then it is sheared.
    let drops = shearing_loot(host, level, e, at, "minecraft:shearing/sheep", stack);
    let mut set = Compound::new();
    set.insert(js("Sheared"), Tag::Byte(1));
    if !e.merge_nbt(host, set) {
        return false;
    }
    drop_all(host, level, at, drops);
    let _ = level.play_sound(host, "minecraft:entity.sheep.shear", at, SoundSource::Players, 1.0, 1.0, None);
    if let Some(p) = host.players().acting_player() {
        wear_hand(host, LivingRef::new(p.entity()), event.hand, 1);
    }
    true
}

// ---- Area attacks (SweepWeaponAttack, CircleWeaponAttack)

/// After a melee hit on `target`: sword and cleaver sweep (the mobs within the target's box grown by the range and 0.25
/// up, within (2 + range)² of the player, take 1 + sweep percent × the hit), kama and scythe circle (the mobs within
/// the range² of the target take the hit). The hit is the tool's attack damage plus the attribute's base 1.
pub(crate) fn area_attack(host: &mut dyn ServerHost, attacker: LivingRef, stack: &ItemStack, target: u32) {
    let (circle, base) = match sid(stack) {
        "tconstruct:sword" => (false, 1.0),
        "tconstruct:cleaver" => (false, 2.0),
        "tconstruct:kama" => (true, 1.0),
        "tconstruct:scythe" => (true, 2.0),
        _ => return,
    };
    if tool_broken(stack) {
        return;
    }
    let range = base + md::volatile_int(stack, "tconstruct:expanded").max(0) as f64;
    let Some(level) = here(&*host) else { return };
    let Some(t) = host.entity(target) else { return };
    let Some(at) = t.position(&*host) else { return };
    let Some(me) = attacker.entity().position(&*host) else { return };
    let hit = tool_stat(stack, ST_ATTACK_DAMAGE) + 1.0;
    let damage = if circle { hit } else { 1.0 + md::volatile_float(stack, "tconstruct:sweep_percent") * hit };
    let (min, max) = ([at[0] - range, at[1] - 0.25, at[2] - range], [at[0] + range, at[1] + 2.25, at[2] + range]);
    let Some(list) = entities_in(&*host, level, min, max) else { return };
    let me_id = attacker.entity();
    for e in list {
        if e == me_id || e == t {
            continue;
        }
        let Some(p) = e.position(&*host) else { continue };
        let d2 = |a: [f64; 3]| (p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2) + (p[2] - a[2]).powi(2);
        let ok = if circle { d2(at) <= range * range } else { d2(me) <= (2.0 + range).powi(2) };
        if !ok || LivingRef::new(e).health(&*host).is_none() || e.kind(&*host).and_then(|k| k.id(&*host)).is_some_and(|k| k == "minecraft:player" || k == "minecraft:armor_stand") {
            continue;
        }
        let _ = e.hurt(host, "minecraft:player_attack", damage, Some(me_id), Some(me_id));
    }
    let _ = level.play_sound(host, "minecraft:entity.player.attack.sweep", me, SoundSource::Players, 1.0, 1.0, None);
}

/// `Shearable.dropFromShearingLootTable`'s roll: loot table `table` with the sheared entity as THIS_ENTITY and the
/// tool as TOOL (the data packs' table, so a sheep's colour and a mooshroom's variant pick the drops). Empty when the
/// table can't be rolled.
fn shearing_loot(host: &mut dyn ServerHost, level: LevelRef, e: EntityRef, at: [f64; 3], table: &str, tool: &ItemStack) -> Vec<ItemStack> {
    match level.roll_loot(host, table, at, Some(tool), Some(e)) {
        Ok(Some(drops)) => {
            tlog(&format!("shearing loot {table}: {} stacks", drops.len()));
            drops
        }
        other => {
            tlog(&format!("shearing loot {table}: {other:?}"));
            Vec::new()
        }
    }
}

/// The drops at the sheared entity, a block up.
fn drop_all(host: &mut dyn ServerHost, level: LevelRef, at: [f64; 3], drops: Vec<ItemStack>) {
    for s in drops {
        let _ = level.spawn_item(host, at[0], at[1] + 1.0, at[2], s, 10);
    }
}

/// Shearable.shear for the other vanilla shearables (MushroomCow, SnowGolem, Bogged), as `shear_entity`'s sheep:
/// a mooshroom becomes a cow (its health, rotation and name kept), a snow golem loses its pumpkin, a bogged is
/// sheared; each drops its shearing loot (5 of the mooshroom's mushroom, a carved pumpkin, a red and a brown mushroom).
fn shear_other(host: &mut dyn ServerHost, event: &EntityInteractEvent<'_>, e: EntityRef, nbt: &Compound) -> bool {
    let byte = |k: &str| match nbt.get(&js(k)) {
        Some(Tag::Byte(b)) => i32::from(*b),
        Some(Tag::Int(i)) => *i,
        _ => 0,
    };
    let (Some(level), Some(at)) = (here(&*host), e.position(&*host)) else { return false };
    let Some(tool) = event.stack else { return false };
    // Each one's shearing loot table (`minecraft:shearing/<kind>`), rolled while it still has what it sheds.
    let (drops, sound): (Vec<ItemStack>, &str) = match event.kind {
        "minecraft:mooshroom" => {
            if byte("Age") < 0 {
                return false;
            }
            // `shearing/mooshroom` picks `shearing/mooshroom/<variant>` by the entity's `mooshroom/variant`
            // component, which Sunrise's loot roll doesn't match yet: roll the variant's own table (the same drops).
            let variant = match nbt.get(&js("Type")) {
                Some(Tag::String(s)) if s.to_string_lossy() == "brown" => "brown",
                _ => "red",
            };
            let drops = shearing_loot(host, level, e, at, &format!("minecraft:shearing/mooshroom/{variant}"), tool);
            let mut cow = Compound::new();
            for k in ["Rotation", "Health", "CustomName", "PersistenceRequired", "NoAI", "Invulnerable", "Silent"] {
                if let Some(v) = nbt.get(&js(k)) {
                    cow.insert(js(k), v.clone());
                }
            }
            let dim = host.dimension().to_string();
            if host.summon(&dim, "minecraft:cow", at, cow).is_none() {
                return false;
            }
            let _ = e.discard(host);
            (drops, "minecraft:entity.mooshroom.shear")
        }
        "minecraft:snow_golem" => {
            if byte("Pumpkin") == 0 && nbt.get(&js("Pumpkin")).is_some() {
                tlog("a snow golem without its pumpkin");
                return false;
            }
            let drops = shearing_loot(host, level, e, at, "minecraft:shearing/snow_golem", tool);
            let mut set = Compound::new();
            set.insert(js("Pumpkin"), Tag::Byte(0));
            if !e.merge_nbt(host, set) {
                tlog(&format!("{}: Pumpkin refused by merge_nbt (keys {:?})", event.kind, nbt.keys().map(|k| k.to_string_lossy().to_string()).collect::<Vec<_>>()));
                return false;
            }
            (drops, "minecraft:entity.snow_golem.shear")
        }
        _ => {
            if byte("sheared") != 0 {
                return false;
            }
            let drops = shearing_loot(host, level, e, at, "minecraft:shearing/bogged", tool);
            let mut set = Compound::new();
            set.insert(js("sheared"), Tag::Byte(1));
            if !e.merge_nbt(host, set) {
                tlog(&format!("{}: sheared refused by merge_nbt (keys {:?})", event.kind, nbt.keys().map(|k| k.to_string_lossy().to_string()).collect::<Vec<_>>()));
                return false;
            }
            (drops, "minecraft:entity.bogged.shear")
        }
    };
    drop_all(host, level, at, drops);
    let _ = level.play_sound(host, sound, at, SoundSource::Players, 1.0, 1.0, None);
    crate::smeltery::util::tlog(&format!("sheared a {}", event.kind));
    if let Some(p) = host.players().acting_player() {
        wear_hand(host, LivingRef::new(p.entity()), event.hand, 1);
    }
    true
}

// ---- TreeAOEIterator.TreeIterator

const UP: Pos = [0, 1, 0];
const DOWN: Pos = [0, -1, 0];
const HORIZONTAL: [Pos; 4] = [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]];

/// A queue entry: a trunk column climbing (direction up; `checked` when the widened trunk's first block is known to
/// be a log) or a branch heading `dir`.
#[derive(Clone, Copy)]
pub(crate) struct TreePos {
    pos: Pos,
    dir: Pos,
    checked: bool,
}

/// The trunk columns (the origin's and, with width or depth, the widened trunk's) climb while they meet the origin's
/// block; their sides and the trunk's corners start branches, which follow the logs outward up to 10 blocks
/// (Manhattan) from the trunk's bounds.
pub(crate) struct TreeWalk {
    queue: VecDeque<TreePos>,
    visited: HashSet<Pos>,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
}

impl TreeWalk {
    fn new(valid: &dyn Fn(Pos) -> bool, origin: Pos, width_dir: Pos, extra_width: i32, depth_dir: Pos, extra_depth: i32) -> Self {
        let mut queue = VecDeque::from([TreePos { pos: offset(origin, UP), dir: UP, checked: false }]);
        let (mut min_x, mut max_x, mut min_z, mut max_z) = (origin[0], origin[0], origin[2], origin[2]);
        if extra_depth > 0 || extra_width > 0 {
            for d in 0..=extra_depth {
                for w in -extra_width..=extra_width {
                    if d == 0 && w == 0 {
                        continue;
                    }
                    let p: Pos = std::array::from_fn(|a| origin[a] + depth_dir[a] * d + width_dir[a] * w);
                    if valid(p) {
                        queue.push_back(TreePos { pos: p, dir: UP, checked: true });
                        min_x = min_x.min(p[0]);
                        max_x = max_x.max(p[0]);
                        min_z = min_z.min(p[2]);
                        max_z = max_z.max(p[2]);
                    }
                }
            }
        }
        TreeWalk { queue, visited: HashSet::new(), min_x, max_x, min_z, max_z }
    }

    fn outside_trunk(&self, p: Pos) -> bool {
        p[0] < self.min_x || p[0] > self.max_x || p[2] < self.min_z || p[2] > self.max_z
    }

    /// isBranch: outside the trunk, within 10 of it, not tried before (it is marked now), and a log.
    fn is_branch(&mut self, valid: &dyn Fn(Pos) -> bool, p: Pos) -> bool {
        if !self.outside_trunk(p) {
            return false;
        }
        let dx = (p[0] - self.min_x).abs().min((p[0] - self.max_x).abs());
        let dz = (p[2] - self.min_z).abs().min((p[2] - self.max_z).abs());
        if dx + dz > 10 || !self.visited.insert(p) {
            return false;
        }
        valid(p)
    }

    /// tryBranch: a branch, queued only where the block below isn't solid (no walking along a floor of logs).
    fn try_branch(&mut self, valid: &dyn Fn(Pos) -> bool, solid: &dyn Fn(Pos) -> bool, p: Pos, dir: Pos) {
        if self.is_branch(valid, p) && !solid(offset(p, DOWN)) {
            self.queue.push_back(TreePos { pos: p, dir, checked: false });
        }
    }

    /// The first of `tries` (offsets from `p`, in order) that is a branch goes on, heading `dir`.
    fn first_branch(&mut self, valid: &dyn Fn(Pos) -> bool, p: Pos, tries: &[Pos], dir: Pos) {
        for t in tries {
            let q = offset(p, *t);
            if self.is_branch(valid, q) {
                self.queue.push_back(TreePos { pos: q, dir, checked: false });
                return;
            }
        }
    }

    fn next(&mut self, valid: &dyn Fn(Pos) -> bool, solid: &dyn Fn(Pos) -> bool) -> Option<Pos> {
        while let Some(mut e) = self.queue.pop_front() {
            if e.dir == UP {
                let tree_up = e.checked || valid(e.pos);
                for d in HORIZONTAL {
                    self.try_branch(valid, solid, offset(e.pos, d), if tree_up { d } else { UP });
                }
                if !tree_up {
                    continue;
                }
                // The trunk's corners also try their diagonal (headed west at min x, east at max x).
                let (x, z) = (e.pos[0], e.pos[2]);
                for (at_x, sx, dir) in [(x == self.min_x, -1, [-1, 0, 0]), (x == self.max_x, 1, [1, 0, 0])] {
                    if at_x {
                        for (at_z, sz) in [(z == self.min_z, -1), (z == self.max_z, 1)] {
                            if at_z {
                                self.try_branch(valid, solid, offset(e.pos, [sx, 0, sz]), dir);
                            }
                        }
                    }
                }
                let here = e.pos;
                e.pos = offset(e.pos, UP);
                e.checked = false;
                self.queue.push_back(e);
                if self.outside_trunk(e.pos) {
                    self.visited.insert(e.pos);
                }
                return Some(here);
            }
            // A branch: above, ahead, ahead and up; then each side (clockwise, then counter-clockwise): beside,
            // beside and up, beside and ahead, beside, ahead and up.
            let (p, d) = (e.pos, e.dir);
            self.first_branch(valid, p, &[UP, d, offset(d, UP)], d);
            let side = clockwise(d);
            for s in [side, reverse(side)] {
                self.first_branch(valid, p, &[s, offset(s, UP), offset(s, d), offset(offset(s, d), UP)], s);
            }
            return Some(p);
        }
        None
    }
}
