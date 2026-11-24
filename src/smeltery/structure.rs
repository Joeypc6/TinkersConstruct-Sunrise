//! The smeltery and the foundry (TinkerSmeltery, stage 2C): the controllers (SmelteryBlockEntity, FoundryBlockEntity
//! over HeatingStructureBlockEntity with MultiblockCuboid / HeatingStructureMultiblock, SmelteryTank,
//! MultitankFuelModule, MeltingModuleInventory, MultiAlloyingModule, EntityMeltingModule), the drains, ducts and chutes
//! (SmelteryInputOutputBlockEntity, DrainBlockEntity, DuctBlockEntity, ChuteBlockEntity), and the walls as one
//! structure.
//!
//! On the fork:
//! - five block entity types, as Java's: `tconstruct:smeltery`, `tconstruct:foundry`, `tconstruct:drain`,
//!   `tconstruct:duct`, `tconstruct:chute`;
//! - the shared tank (`tank` {fluids [{FluidName, Amount}] bottom first, capacity}, Java's NBT) answers `FLUIDS` on the
//!   controller, the drains (any fluid in, the bottom out) and the ducts (their filter's fluid only), so every mod's
//!   pipes, Tinkers' faucets and buckets reach it; the chutes answer `ITEMS` with their master's;
//! - the melting inventory is up to 10,920 slots, a block entity's container one fixed size: the container has 43 slots
//!   (0, 1 the bucket slots; 2..41 a window of 40 melting slots, the most the side inventory shows; 42 an intake for
//!   hoppers and chutes) and the slots outside the window live in the saved list `hz_inv` (README, gaps);
//! - re-checks from `block_changed` (any block id change in the box or the layer above), the inside sweep and the
//!   expansion check as Java's ticks; `in_structure` on the controller and the IO blocks (`masterOffset`).

use super::containers::{self, CapTank, TankAccess, Way};
use super::melter::{self, Module, can_heat_item, set_stack, stack_key};
use super::recipes::{self, Rate, find_melting};
use super::util::*;
use super::{Fields, casting};
use mod_api::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub(crate) const SMELTERY: &str = "tconstruct:smeltery_controller";
pub(crate) const FOUNDRY: &str = "tconstruct:foundry_controller";
pub(crate) const STRUCTURE_MENU: &str = "tconstruct:smeltery";
pub(crate) const DUCT_MENU: &str = "tconstruct:duct";
pub(crate) const DRAINS: [&str; 2] = ["tconstruct:seared_drain", "tconstruct:scorched_drain"];
pub(crate) const DUCTS: [&str; 2] = ["tconstruct:seared_duct", "tconstruct:scorched_duct"];
pub(crate) const CHUTES: [&str; 2] = ["tconstruct:seared_chute", "tconstruct:scorched_chute"];
const SMELTERY_TYPE: &str = "tconstruct:smeltery";
const FOUNDRY_TYPE: &str = "tconstruct:foundry";
const DRAIN_TYPE: &str = "tconstruct:drain";
const DUCT_TYPE: &str = "tconstruct:duct";
const CHUTE_TYPE: &str = "tconstruct:chute";
/// MeltingModuleInventory.MAX_SLOTS.
pub(crate) const MAX_SLOTS: usize = 10920;
/// MultiblockCuboid: inner X and Z at most 14; 63 layers of wall above the floor.
const MAX_INNER: i32 = 14;
const MAX_LAYERS: i32 = 63;
pub(crate) const WINDOW: usize = 40;
pub(crate) const WIN0: usize = 2;
pub(crate) const INTAKE: usize = 42;
pub(crate) const STRUCTURE_SLOTS: usize = 43;
/// The menu's data: the fuel's 4 ints and the last fuel tank's offset, each window slot's time, required time and
/// temperature, then the window's first slot, the slot count and the bucket mode.
pub(crate) const D_WINDOW: usize = 7;
pub(crate) const D_OFFSET: usize = 127;
pub(crate) const D_SIZE: usize = 128;
pub(crate) const D_MODE: usize = 129;
pub(crate) const STRUCTURE_DATA: usize = 130;
/// Menu button ids from here scroll to the row `id - SCROLL_BUTTON` (Java's slider is the client's own).
pub(crate) const SCROLL_BUTTON: i32 = 1 << 14;
/// Config.smelteryOreRate / foundryOreRate / foundryByproductRate.
const SMELTERY_RATES: ((i64, i64), (i64, i64)) = ((12, 9), (8, 4));
const FOUNDRY_RATES: ((i64, i64), (i64, i64)) = ((9, 9), (4, 4));
const BYPRODUCT_RATES: ((i64, i64), (i64, i64)) = ((3, 9), (4, 4));
/// The items the controller draws inside (maxSmelteryItemQuads, about one a slot of the first ones).
pub(crate) const RENDER_ITEMS: usize = 256;
pub(crate) const SEARED_BLOCKS: [&str; 7] = [
    "tconstruct:seared_stone",
    "tconstruct:seared_cracked_bricks",
    "tconstruct:seared_cobble",
    "tconstruct:seared_paver",
    "tconstruct:seared_bricks",
    "tconstruct:seared_fancy_bricks",
    "tconstruct:seared_triangle_bricks",
];
pub(crate) const SCORCHED_BLOCKS: [&str; 5] = ["tconstruct:scorched_stone", "tconstruct:polished_scorched_stone", "tconstruct:scorched_bricks", "tconstruct:scorched_road", "tconstruct:chiseled_scorched_bricks"];
/// The other wall blocks of either kind (`#tconstruct:smeltery/wall`, `#tconstruct:foundry/wall`).
const WALL_EXTRAS: [&str; 5] = ["glass", "soul_glass", "tinted_glass", "ladder", "lamp"];
/// `#minecraft:inverted_healing_and_harm` as the jar knows it, for where an entity's tags can't be read.
const UNDEAD: [&str; 14] = [
    "minecraft:zombie",
    "minecraft:husk",
    "minecraft:drowned",
    "minecraft:zombie_villager",
    "minecraft:zombified_piglin",
    "minecraft:zoglin",
    "minecraft:skeleton",
    "minecraft:stray",
    "minecraft:wither_skeleton",
    "minecraft:bogged",
    "minecraft:wither",
    "minecraft:phantom",
    "minecraft:skeleton_horse",
    "minecraft:zombie_horse",
];

pub(crate) fn is_controller(block: &str) -> bool {
    block == SMELTERY || block == FOUNDRY
}
pub(crate) fn is_drain(block: &str) -> bool {
    DRAINS.contains(&block)
}
pub(crate) fn is_duct(block: &str) -> bool {
    DUCTS.contains(&block)
}
pub(crate) fn is_chute(block: &str) -> bool {
    CHUTES.contains(&block)
}
pub(crate) fn is_io(block: &str) -> bool {
    is_drain(block) || is_duct(block) || is_chute(block)
}

/// Every wall, floor and window block of either structure (the ones a click on opens the controller).
pub(crate) fn wall_blocks() -> Vec<String> {
    let mut out: Vec<String> = SEARED_BLOCKS.iter().chain(SCORCHED_BLOCKS.iter()).map(|s| (*s).to_owned()).collect();
    for kind in ["seared", "scorched"] {
        for e in WALL_EXTRAS {
            out.push(format!("tconstruct:{kind}_{e}"));
        }
    }
    out
}

fn add(a: Pos, b: Pos) -> Pos {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: Pos, b: Pos) -> Pos {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn get_pos(nbt: &Compound, key: &str) -> Option<Pos> {
    let v = gia(nbt, key);
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}
fn offset_tag(d: Pos) -> Tag {
    let mut c = Compound::new();
    si(&mut c, "X", d[0]);
    si(&mut c, "Y", d[1]);
    si(&mut c, "Z", d[2]);
    Tag::Compound(c)
}
fn read_offset(nbt: &Compound, key: &str) -> Option<Pos> {
    let c = gc(nbt, key)?;
    Some([gi(c, "X", 0), gi(c, "Y", 0), gi(c, "Z", 0)])
}

// ---- The shared tank (SmelteryTank): fluids bottom first, one capacity for all

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct STank {
    pub fluids: Vec<(String, i32)>,
    pub capacity: i32,
}

impl STank {
    pub(crate) fn contained(&self) -> i64 {
        self.fluids.iter().map(|(_, a)| i64::from(*a)).sum()
    }
    /// getRemainingSpace: none when full or over full.
    pub(crate) fn remaining(&self) -> i32 {
        (i64::from(self.capacity) - self.contained()).clamp(0, i64::from(i32::MAX)) as i32
    }
    /// SmelteryTank.fill: what fits, merged into the same fluid or added on top.
    pub(crate) fn fill(&mut self, fluid: &str, amount: i32, execute: bool) -> i32 {
        let usable = amount.min(self.remaining());
        if usable <= 0 {
            return 0;
        }
        if execute {
            match self.fluids.iter_mut().find(|(f, _)| f == fluid) {
                Some(f) => f.1 += usable,
                None => self.fluids.push((fluid.to_owned(), usable)),
            }
        }
        usable
    }
    /// drain(FluidStack): the first entry of that fluid, wherever it sits.
    pub(crate) fn drain_fluid(&mut self, fluid: &str, max: i32, execute: bool) -> i32 {
        let Some(i) = self.fluids.iter().position(|(f, _)| f == fluid) else { return 0 };
        let n = max.min(self.fluids[i].1);
        if n <= 0 {
            return 0;
        }
        if execute {
            if self.fluids[i].1 - n <= 0 {
                self.fluids.remove(i);
            } else {
                self.fluids[i].1 -= n;
            }
        }
        n
    }
}

/// The controller's `tank` as Java writes it.
pub(crate) fn read_tank(nbt: &Compound) -> STank {
    let Some(t) = gc(nbt, "tank") else { return STank::default() };
    let fluids = match t.get(&js("fluids")) {
        Some(Tag::List(l)) => l
            .iter()
            .filter_map(|f| match f {
                Tag::Compound(f) => {
                    let name = gs(f, "FluidName")?;
                    let amount = gi(f, "Amount", 0);
                    (amount > 0 && name != "minecraft:empty").then_some((name, amount))
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    STank { fluids, capacity: gi(t, "capacity", 0).max(0) }
}

pub(crate) fn tank_compound(tank: &STank) -> Compound {
    let mut t = Compound::new();
    let list: Vec<Tag> = tank
        .fluids
        .iter()
        .map(|(f, a)| {
            let mut c = Compound::new();
            ss(&mut c, "FluidName", f);
            si(&mut c, "Amount", *a);
            Tag::Compound(c)
        })
        .collect();
    t.insert(js("fluids"), Tag::List(list.into_iter().collect()));
    si(&mut t, "capacity", tank.capacity);
    t
}

fn write_tank(nbt: &mut Compound, tank: &STank) {
    nbt.insert(js("tank"), Tag::Compound(tank_compound(tank)));
}

/// The shared tank as Mantle's helpers see a tank: its bottom fluid (or a chosen one) to fill items from, any fluid in.
pub(crate) struct STankAccess<'a> {
    pub tank: &'a mut STank,
    pub prefer: Option<usize>,
}

impl TankAccess for STankAccess<'_> {
    fn contents(&mut self, _host: &mut dyn ServerHost) -> Tank {
        let i = self.prefer.unwrap_or(0);
        match self.tank.fluids.get(i) {
            Some((f, a)) => (Some(f.clone()), *a),
            None => (None, 0),
        }
    }
    fn fill(&mut self, _host: &mut dyn ServerHost, fluid: &str, amount: i32, execute: bool) -> i32 {
        self.tank.fill(fluid, amount, execute)
    }
    fn drain(&mut self, _host: &mut dyn ServerHost, fluid: &str, amount: i32, execute: bool) -> i32 {
        self.tank.drain_fluid(fluid, amount, execute)
    }
}

// ---- The structure (MultiblockCuboid with the floor; the foundry with its frame)

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shape {
    /// The outer corners: the floor's lowest, the top wall layer's highest.
    pub min: Pos,
    pub max: Pos,
    pub tanks: Vec<Pos>,
}

impl Shape {
    /// MultiblockStructureData: minInside and maxInside (no ceiling).
    pub(crate) fn lo(&self) -> Pos {
        [self.min[0] + 1, self.min[1] + 1, self.min[2] + 1]
    }
    pub(crate) fn hi(&self) -> Pos {
        [self.max[0] - 1, self.max[1], self.max[2] - 1]
    }
    /// innerX, innerY, innerZ.
    pub(crate) fn size(&self) -> [i32; 3] {
        let (l, h) = (self.lo(), self.hi());
        [h[0] - l[0] + 1, h[1] - l[1] + 1, h[2] - l[2] + 1]
    }
}

fn in_box(p: Pos, min: Pos, max: Pos) -> bool {
    (0..3).all(|i| p[i] >= min[i] && p[i] <= max[i])
}

fn is_inside(p: Pos, min: Pos, max: Pos) -> bool {
    in_box(p, [min[0] + 1, min[1] + 1, min[2] + 1], [max[0] - 1, max[1], max[2] - 1])
}

/// MultiblockStructureData.contains: in the box and not inside; the smeltery's corner columns and floor ring are no
/// part of it.
pub(crate) fn contained(p: Pos, min: Pos, max: Pos, foundry: bool) -> bool {
    if !in_box(p, min, max) || is_inside(p, min, max) {
        return false;
    }
    if foundry {
        return true;
    }
    let edges = i32::from(p[0] == min[0] || p[0] == max[0]) + i32::from(p[2] == min[2] || p[2] == max[2]) + i32::from(p[1] == min[1]);
    edges < 2
}

/// SmelteryBlockEntity / FoundryBlockEntity.setStructure: the tank's capacity.
fn structure_capacity(foundry: bool, [x, y, z]: [i32; 3]) -> i32 {
    let (x, y, z) = (i64::from(x), i64::from(y), i64::from(z));
    let c = if foundry { 720 * (x + 2) * (y + 1) * (z + 2) } else { 1080 * x * y * z };
    c.clamp(0, i64::from(i32::MAX)) as i32
}

fn structure_slots([x, y, z]: [i32; 3]) -> usize {
    (i64::from(x) * i64::from(y) * i64::from(z)).clamp(0, MAX_SLOTS as i64) as usize
}

/// The fuel burned each 4-tick cycle (the foundry's formula as coded: (X+2) in two terms, Z in one).
fn structure_fuel_rate(foundry: bool, [x, y, z]: [i32; 3]) -> i32 {
    let (x, y, z) = (i64::from(x), i64::from(y), i64::from(z));
    let r = if foundry { 1 + (2 * (x + 2) * y + 2 * y * z + (x + 2) * (z + 2)) / 18 } else { 1 + (2 * x * y + 2 * y * z + x * z) / 15 };
    r.clamp(0, i64::from(i32::MAX)) as i32
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Wall,
    Floor,
    Tank,
}

/// The jar's `#tconstruct:smeltery|foundry/wall|floor|tanks`, for where the tags can't be read.
fn part_fallback(block: &str, foundry: bool, part: Part) -> bool {
    let prefix = if foundry { "tconstruct:scorched_" } else { "tconstruct:seared_" };
    let blocks: &[&str] = if foundry { &SCORCHED_BLOCKS } else { &SEARED_BLOCKS };
    let named = |names: &[&str]| block.strip_prefix(prefix).is_some_and(|rest| names.contains(&rest));
    let tank = named(&["fuel_tank", "fuel_gauge", "ingot_tank", "ingot_gauge"]);
    let io = named(&["lamp", "drain", "chute", "duct"]);
    match part {
        Part::Tank => tank,
        Part::Floor => blocks.contains(&block) || io,
        Part::Wall => blocks.contains(&block) || io || tank || named(&["glass", "soul_glass", "tinted_glass", "ladder"]),
    }
}

pub(crate) struct Scan<'h> {
    host: &'h dyn Host,
    level: LevelRef,
    dim: String,
    controller: Pos,
    foundry: bool,
    tanks: Vec<Pos>,
}

impl Scan<'_> {
    fn block(&self, p: Pos) -> Option<(BlockKey, String)> {
        let b = self.level.block(self.host, p)?;
        let id = b.id(self.host).unwrap_or_default().to_owned();
        Some((b, id))
    }
    fn part_of(&self, p: Pos, part: Part) -> bool {
        let Some((b, id)) = self.block(p) else { return false };
        let kind = if self.foundry { "foundry" } else { "smeltery" };
        let tag = match part {
            Part::Wall => format!("tconstruct:{kind}/wall"),
            Part::Floor => format!("tconstruct:{kind}/floor"),
            Part::Tank => format!("tconstruct:{kind}/tanks"),
        };
        self.host.block_in_tag(b, &tag).unwrap_or_else(|| part_fallback(&id, self.foundry, part))
    }
    /// MultiblockCuboid.isInnerBlock: air or `#tconstruct:structure_air`.
    fn air(&self, p: Pos) -> bool {
        let Some((b, id)) = self.block(p) else { return false };
        matches!(id.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
            || self.host.block_in_tag(b, "tconstruct:structure_air").unwrap_or_else(|| matches!(id.as_str(), "minecraft:light" | "tconstruct:glow"))
    }
    fn loaded(&self, p: Pos) -> bool {
        self.level.is_loaded(self.host, p)
    }
    /// level.hasChunksAt(from, to).
    fn layer_loaded(&self, from: Pos, to: Pos) -> bool {
        for cx in (from[0] >> 4)..=(to[0] >> 4) {
            for cz in (from[2] >> 4)..=(to[2] >> 4) {
                if !self.loaded([cx << 4, from[1], cz << 4]) {
                    return false;
                }
            }
        }
        true
    }
    /// HeatingStructureMultiblock.isValidBlock.
    fn valid(&mut self, p: Pos, floor: bool, frame: bool) -> bool {
        if p == self.controller {
            return true;
        }
        if owner_of(&self.dim, p).is_some_and(|c| c != self.controller) {
            return false;
        }
        if floor && !frame {
            return self.part_of(p, Part::Floor);
        }
        if self.part_of(p, Part::Tank) {
            self.tanks.push(p);
            return true;
        }
        self.part_of(p, Part::Wall)
    }
    /// MultiblockCuboid.detectLayer: the inside, the frame's corners (foundry), then the walls.
    fn layer(&mut self, from: Pos, to: Pos) -> Result<(), Failure> {
        if !self.layer_loaded(from, to) {
            return Err(failure("not_loaded", None));
        }
        let y = from[1];
        for x in from[0] + 1..to[0] {
            for z in from[2] + 1..to[2] {
                if !self.air([x, y, z]) {
                    return Err(failure("invalid_inner_block", Some([x, y, z])));
                }
            }
        }
        if self.foundry {
            for c in [from, [from[0], y, to[2]], [to[0], y, from[2]], to] {
                if !self.valid(c, false, true) {
                    return Err(failure("invalid_wall_frame", Some(c)));
                }
            }
        }
        for x in from[0] + 1..to[0] {
            for z in [from[2], to[2]] {
                if !self.valid([x, y, z], false, false) {
                    return Err(failure("invalid_wall_block", Some([x, y, z])));
                }
            }
        }
        for z in from[2] + 1..to[2] {
            for x in [from[0], to[0]] {
                if !self.valid([x, y, z], false, false) {
                    return Err(failure("invalid_wall_block", Some([x, y, z])));
                }
            }
        }
        Ok(())
    }
    /// MultiblockCuboid.detectCap for the floor: the frame (foundry), then the inside.
    fn floor(&mut self, from: Pos, to: Pos) -> Result<(), Failure> {
        if !self.layer_loaded(from, to) {
            return Err(failure("not_loaded", None));
        }
        let y = from[1];
        if self.foundry {
            for x in from[0]..=to[0] {
                for z in [from[2], to[2]] {
                    if !self.valid([x, y, z], true, true) {
                        return Err(failure("invalid_floor_frame", Some([x, y, z])));
                    }
                }
            }
            for z in from[2] + 1..to[2] {
                for x in [from[0], to[0]] {
                    if !self.valid([x, y, z], true, true) {
                        return Err(failure("invalid_floor_frame", Some([x, y, z])));
                    }
                }
            }
        }
        for z in from[2] + 1..to[2] {
            for x in from[0] + 1..to[0] {
                if !self.valid([x, y, z], true, false) {
                    return Err(failure("invalid_floor_block", Some([x, y, z])));
                }
            }
        }
        Ok(())
    }
}

/// A MultiblockResult that is not a success: its lang key, where, and the arguments of `too_large`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    key: &'static str,
    pos: Option<Pos>,
    args: Vec<i32>,
}

fn failure(key: &'static str, pos: Option<Pos>) -> Failure {
    Failure { key, pos, args: Vec::new() }
}

/// MultiblockCuboid.detectMultiblock from the controller facing `facing`: the structure or why not, with the last
/// failed layer check (a formed structure's `lastResult`).
fn detect(host: &dyn Host, level: LevelRef, controller: Pos, facing: usize, foundry: bool) -> (Result<Shape, Failure>, Option<Failure>) {
    let mut s = Scan { host, level, dim: host.dimension().to_owned(), controller, foundry, tanks: Vec::new() };
    let mut center = casting::step(controller, casting::opposite(facing));
    let mut floor_first = false;
    if !s.air(center) {
        // Only the foundry's frame lets the controller sit in the floor ring.
        if !foundry {
            return (Err(failure("invalid_inner_block", Some(center))), None);
        }
        floor_first = true;
        center = above(center);
        if !s.air(center) {
            return (Err(failure("invalid_inner_block", Some(center))), None);
        }
    }
    // South, west, north, east: the signed distance to the first block that isn't inside.
    let mut edges = [0i32; 4];
    for (k, d) in [3usize, 4, 2, 5].into_iter().enumerate() {
        let mut p = center;
        let mut i = 0;
        while i < 15 && s.loaded(p) && s.air(p) {
            p = casting::step(p, d);
            i += 1;
        }
        edges[k] = (p[0] - center[0]) + (p[2] - center[2]);
    }
    let z_size = edges[0] - edges[2] - 1;
    let x_size = edges[3] - edges[1] - 1;
    if z_size > MAX_INNER || x_size > MAX_INNER {
        return (Err(Failure { key: "too_large", pos: None, args: vec![z_size, x_size, MAX_INNER, MAX_INNER] }), None);
    }
    let y = center[1];
    let from = [center[0] + edges[1], y, center[2] + edges[2]];
    let to = [center[0] + edges[3], y, center[2] + edges[0]];
    let at = |p: Pos, dy: i32| [p[0], y + dy, p[2]];
    if let Err(f) = s.layer(from, to) {
        return (Err(f), None);
    }
    let mut last = None;
    let mut min_layer = -1;
    let mut remaining = MAX_LAYERS;
    if !floor_first {
        while min_layer > -remaining {
            match s.layer(at(from, min_layer), at(to, min_layer)) {
                Ok(()) => min_layer -= 1,
                Err(f) => {
                    last = Some(f);
                    break;
                }
            }
        }
    }
    remaining += min_layer + 1;
    if let Err(f) = s.floor(at(from, min_layer), at(to, min_layer)) {
        return (Err(f), last);
    }
    let mut max_layer = 1;
    while max_layer < remaining {
        match s.layer(at(from, max_layer), at(to, max_layer)) {
            Ok(()) => max_layer += 1,
            Err(f) => {
                last = Some(f);
                break;
            }
        }
    }
    max_layer -= 1;
    let (min, max) = (at(from, min_layer), at(to, max_layer));
    let mut tanks = std::mem::take(&mut s.tanks);
    // Tanks of a layer that failed are no part of it (HeatingStructureMultiblock.create).
    tanks.retain(|t| in_box(*t, min, max));
    let mut seen = HashSet::new();
    tanks.retain(|t| seen.insert(*t));
    (Ok(Shape { min, max, tanks }), last)
}

/// HeatingStructureMultiblock.canExpand: the layer above the top would be one more.
fn can_expand(host: &dyn Host, level: LevelRef, controller: Pos, foundry: bool, shape: &Shape) -> Result<(), Failure> {
    let mut s = Scan { host, level, dim: host.dimension().to_owned(), controller, foundry, tanks: Vec::new() };
    let y = shape.max[1] + 1;
    s.layer([shape.min[0], y, shape.min[2]], [shape.max[0], y, shape.max[2]])
}

pub(crate) fn read_shape(nbt: &Compound) -> Option<Shape> {
    let (min, max) = (get_pos(nbt, "hz_min")?, get_pos(nbt, "hz_max")?);
    let t = gia(nbt, "hz_tanks");
    Some(Shape { min, max, tanks: t.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect() })
}

fn write_shape(nbt: &mut Compound, shape: Option<&Shape>) {
    match shape {
        Some(s) => {
            sia(nbt, "hz_min", s.min.to_vec());
            sia(nbt, "hz_max", s.max.to_vec());
            sia(nbt, "hz_tanks", s.tanks.iter().flat_map(|t| t.iter().copied()).collect());
        }
        None => {
            for k in ["hz_min", "hz_max", "hz_tanks"] {
                rm(nbt, k);
            }
        }
    }
}

// ---- Runtime state: each controller's counters, the formed structures, the decoded inventories

#[derive(Clone, Copy, Default)]
pub(crate) struct Ctl {
    /// HeatingStructureBlockEntity.tick (0..19) and expandCounter.
    tick: u8,
    expand: u8,
    /// structureUpdateQueued.
    queued: bool,
    /// FuelModule.fuelQuality (not saved).
    quality: i32,
    /// The inside sweep's place.
    inside: usize,
    /// Its first tick here has run (the structure re-checked after a load).
    started: bool,
}

/// The logical slots outside the window (the window's are None here) and every slot's MeltingModule.
#[derive(Clone, Default)]
pub(crate) struct Store {
    generation: i32,
    items: Vec<Option<ItemStack>>,
    modules: Vec<Module>,
}

/// A module whose recipe the controller looks up on its next tick.
const UNSET: i32 = i32::MIN;

fn unset() -> Module {
    Module { current: 0, required: UNSET, temperature: 0 }
}

pub(crate) type Key = (String, Pos);

#[derive(Default)]
pub(crate) struct Runtime {
    controllers: HashMap<Key, Ctl>,
    /// The formed structures: (dimension, controller) to (min, max, foundry).
    formed: HashMap<Key, (Pos, Pos, bool)>,
    stores: HashMap<Key, Store>,
    /// The saved inventories of controllers that were removed, to drop in their level's tick.
    drops: Vec<(String, Pos, Vec<ItemStack>)>,
    /// The comparator signal of each controller (its tank).
    signals: HashMap<Key, i32>,
    /// Server: the slot count of the controller whose menu opens now.
    opening: Option<usize>,
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

fn with_rt<T>(f: impl FnOnce(&mut Runtime) -> T) -> T {
    let mut g = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Runtime::default))
}

pub(crate) fn opening_slots() -> Option<usize> {
    with_rt(|r| r.opening)
}

pub(crate) fn signal_at(key: &Key) -> i32 {
    with_rt(|r| r.signals.get(key).copied().unwrap_or(0))
}

/// The formed structure another controller of this dimension has `pos` in, if any (a part is never shared).
fn owner_of(dim: &str, pos: Pos) -> Option<Pos> {
    with_rt(|r| r.formed.iter().find(|((d, _), (min, max, foundry))| d == dim && contained(pos, *min, *max, *foundry)).map(|((_, c), _)| *c))
}

fn random(bound: usize) -> usize {
    if bound == 0 { 0 } else { ((melter::rand01() * bound as f64) as usize).min(bound - 1) }
}

fn decode_store(host: &dyn Host, nbt: &Compound, size: usize) -> Store {
    let mut items = vec![None; size];
    if let Some(Tag::List(l)) = nbt.get(&js("hz_inv")) {
        for t in l.iter() {
            let Tag::Compound(c) = t else { continue };
            let slot = gi(c, "Slot", -1);
            if slot < 0 || slot as usize >= size {
                continue;
            }
            if let Some(item) = gc(c, "Item") {
                items[slot as usize] = ItemStack::from_nbt(host, item).filter(|s| s.count() > 0);
            }
        }
    }
    let v = gia(nbt, "hz_melt");
    let modules = (0..size).map(|i| Module { current: v.get(i * 3).copied().unwrap_or(0), required: v.get(i * 3 + 1).copied().unwrap_or(UNSET), temperature: v.get(i * 3 + 2).copied().unwrap_or(0) }).collect();
    Store { generation: gi(nbt, "hz_inv_gen", 0), items, modules }
}

fn take_store(host: &dyn Host, key: &Key, nbt: &Compound, size: usize) -> Store {
    let generation = gi(nbt, "hz_inv_gen", 0);
    let cached = with_rt(|r| r.stores.remove(key));
    let mut s = match cached {
        Some(s) if s.generation == generation && generation != 0 => s,
        _ => decode_store(host, nbt, size),
    };
    s.items.resize(size, None);
    s.modules.resize(size, unset());
    s
}

/// The store back in the cache, and in the NBT: the items when they changed (under a new generation), the modules always.
fn put_store(host: &dyn Host, key: Key, nbt: &mut Compound, mut s: Store, changed: bool) {
    if changed || s.generation == 0 {
        let mut generation = random(i32::MAX as usize) as i32;
        if generation == 0 || generation == s.generation {
            generation = s.generation.wrapping_add(1).max(1);
        }
        s.generation = generation;
        si(nbt, "hz_inv_gen", generation);
        let list: Vec<Tag> = s
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let it = it.as_ref()?;
                let mut c = Compound::new();
                si(&mut c, "Slot", i as i32);
                c.insert(js("Item"), Tag::Compound(it.to_nbt(host)?));
                Some(Tag::Compound(c))
            })
            .collect();
        nbt.insert(js("hz_inv"), Tag::List(list.into_iter().collect()));
    }
    sia(nbt, "hz_melt", s.modules.iter().flat_map(|m| [m.current, m.required, m.temperature]).collect());
    with_rt(|r| r.stores.insert(key, s));
}

/// The inventory of a controller: the window in its container's slots, the rest in `store`.
pub(crate) struct Inventory<'a> {
    items: &'a mut Vec<Option<ItemStack>>,
    store: &'a mut Store,
    offset: usize,
    changed: bool,
}

impl Inventory<'_> {
    fn size(&self) -> usize {
        self.store.items.len()
    }
    fn windowed(&self, i: usize) -> bool {
        i >= self.offset && i < self.offset + WINDOW
    }
    fn get(&self, i: usize) -> Option<&ItemStack> {
        if i >= self.size() {
            return None;
        }
        if self.windowed(i) { self.items.get(WIN0 + i - self.offset).and_then(Option::as_ref) } else { self.store.items[i].as_ref() }
    }
    fn slot(&mut self, i: usize) -> &mut Option<ItemStack> {
        if self.windowed(i) {
            &mut self.items[WIN0 + i - self.offset]
        } else {
            self.changed = true;
            &mut self.store.items[i]
        }
    }
    /// The first empty slot gets one item of `stack` (MeltingModuleInventory.insertItem, limit 1); false when full.
    fn insert_one(&mut self, stack: &ItemStack) -> bool {
        let Some(i) = (0..self.size()).find(|&i| self.get(i).is_none()) else { return false };
        *self.slot(i) = with_count(stack, 1);
        self.store.modules[i] = unset();
        true
    }
    fn free(&self) -> usize {
        (0..self.size()).filter(|&i| self.get(i).is_none()).count()
    }
    /// The window moved: its items back to the store, the new window's out of it. Moved, never copied.
    fn set_offset(&mut self, offset: usize) {
        if offset == self.offset {
            return;
        }
        let size = self.size();
        for w in 0..WINDOW {
            let i = self.offset + w;
            if i < size {
                self.store.items[i] = self.items[WIN0 + w].take();
            }
        }
        self.offset = offset;
        for w in 0..WINDOW {
            let i = offset + w;
            if i < size {
                self.items[WIN0 + w] = self.store.items[i].take();
            }
        }
        self.changed = true;
    }
}

/// HeatingStructureContainerMenu.calcColumns and the side inventory's rows: columns, rows, rows shown.
pub(crate) fn side_layout(slots: usize) -> (usize, usize, usize) {
    let n = slots.max(1);
    let cols = n.div_ceil(7).min(4);
    let rows = n.div_ceil(cols);
    (cols, rows, rows.min(10))
}

/// The first slot of the window for scroll row `row` (clamped to the last row that fills the side inventory).
fn window_offset(slots: usize, row: usize) -> usize {
    let (cols, rows, shown) = side_layout(slots);
    row.min(rows.saturating_sub(shown)) * cols
}

// ---- Recipes: tconstruct:alloy and tconstruct:entity_melting

pub(crate) struct Alloy {
    /// Each input's FluidIngredient and whether it is a catalyst (needed, not used up).
    pub inputs: Vec<(Vec<(bool, String, i32)>, bool)>,
    pub output: (String, i32),
    pub temperature: i32,
}

pub(crate) struct EntityMelt {
    pub(crate) types: HashSet<String>,
    pub(crate) tags: Vec<String>,
    /// None for a result the fork's fluids can't carry (a potion's components): the entity is hurt, nothing comes out.
    pub(crate) result: Option<(String, i32)>,
    pub(crate) damage: i32,
}

fn parse_alloy(host: &dyn Host, json: &Value) -> Option<Alloy> {
    let r = recipes::chosen(host, json)?;
    let inputs: Vec<(Vec<(bool, String, i32)>, bool)> = r["inputs"]
        .as_array()?
        .iter()
        .map(|i| {
            let ingredient = if i.get("match").is_some() { super::casting_recipes::fluid_ingredient(&i["match"]) } else { super::casting_recipes::fluid_ingredient(i) };
            (ingredient, i["catalyst"].as_bool().unwrap_or(false))
        })
        .collect();
    if inputs.len() < 2 || inputs.iter().any(|(i, _)| i.is_empty()) {
        return None;
    }
    let temperature = r["temperature"].as_i64()?.clamp(1, i64::from(i32::MAX)) as i32;
    Some(Alloy { inputs, output: recipes::fluid_output(&r["result"])?, temperature })
}

/// An EntityIngredient: `{type}`, `{types}`, `{tag}` or a list of them.
fn entity_types(v: &Value, types: &mut HashSet<String>, tags: &mut Vec<String>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| entity_types(x, types, tags)),
        Value::Object(o) => {
            if let Some(t) = o.get("type").and_then(Value::as_str) {
                types.insert(t.to_owned());
            }
            if let Some(ts) = o.get("types").and_then(Value::as_array) {
                types.extend(ts.iter().filter_map(|t| t.as_str().map(str::to_owned)));
            }
            if let Some(tag) = o.get("tag").and_then(Value::as_str) {
                tags.push(recipes::c_tag(tag));
            }
        }
        _ => {}
    }
}

fn parse_entity_melt(host: &dyn Host, json: &Value) -> Option<EntityMelt> {
    let r = recipes::chosen(host, json)?;
    let (mut types, mut tags) = (HashSet::new(), Vec::new());
    entity_types(&r["entity"], &mut types, &mut tags);
    if types.is_empty() && tags.is_empty() {
        return None;
    }
    let res = &r["result"];
    let result = if res.get("nbt").is_some() || res.get("components").is_some() { None } else { Some(recipes::fluid_output(res)?) };
    Some(EntityMelt { types, tags, result, damage: r["damage"].as_i64().unwrap_or(2).clamp(1, i64::from(i32::MAX)) as i32 })
}

pub(crate) struct HeatRecipes {
    pub alloys: Vec<Alloy>,
    pub(crate) melts: Vec<EntityMelt>,
}

pub(crate) const ALLOY_KIND: &str = "tconstruct:alloy";
pub(crate) const ENTITY_MELTING_KIND: &str = "tconstruct:entity_melting";
static HEAT_CACHE: Mutex<Option<(recipes::Key, Arc<HeatRecipes>)>> = Mutex::new(None);

pub(crate) fn heat_recipes(host: &dyn Host) -> Arc<HeatRecipes> {
    let key: recipes::Key = crate::smeltery::util::tables_key(host, &[ALLOY_KIND, ENTITY_MELTING_KIND]);
    if let Some(r) = HEAT_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, r)| r.clone()) {
        return r;
    }
    let tables: Vec<Arc<[Recipe]>> = [ALLOY_KIND, ENTITY_MELTING_KIND].iter().map(|k| host.recipes_of_type(k)).collect();
    let alloys = tables[0].iter().filter_map(|r| parse_alloy(host, &r.json)).collect();
    let mut melts: Vec<(String, EntityMelt)> = tables[1].iter().filter_map(|r| Some((r.id.to_string(), parse_entity_melt(host, &r.json)?))).collect();
    // A deterministic order for the first match by entity type.
    melts.sort_by(|a, b| a.0.cmp(&b.0));
    let out = Arc::new(HeatRecipes { alloys, melts: melts.into_iter().map(|(_, m)| m).collect() });
    *HEAT_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, out.clone()));
    out
}

/// AlloyRecipe.findMatch for each input in order: the first unused fluid it takes (with its amount when `sized`).
fn alloy_match(r: &Alloy, tank: &STank, sized: bool) -> Option<Vec<usize>> {
    let mut used = vec![false; tank.fluids.len()];
    let mut out = Vec::new();
    for (ingredient, _) in &r.inputs {
        let i = (0..tank.fluids.len()).find(|&i| !used[i] && super::casting_recipes::fluid_amount(ingredient, &tank.fluids[i].0).is_some_and(|a| !sized || tank.fluids[i].1 >= a))?;
        used[i] = true;
        out.push(i);
    }
    Some(out)
}

/// What the recipe drains of each fluid it uses up (not the catalysts).
fn alloy_drains(r: &Alloy, tank: &STank, matched: &[usize]) -> Vec<(String, i32)> {
    r.inputs
        .iter()
        .zip(matched)
        .filter(|((_, catalyst), _)| !catalyst)
        .filter_map(|((ingredient, _), &i)| Some((tank.fluids[i].0.clone(), super::casting_recipes::fluid_amount(ingredient, &tank.fluids[i].0)?)))
        .collect()
}

/// AlloyRecipe.canPerform.
fn can_alloy_recipe(r: &Alloy, tank: &STank, temperature: i32) -> bool {
    if temperature < r.temperature {
        return false;
    }
    let Some(matched) = alloy_match(r, tank, true) else { return false };
    let drained: i64 = alloy_drains(r, tank, &matched).iter().map(|(_, a)| i64::from(*a)).sum();
    i64::from(r.output.1) - drained <= i64::from(tank.remaining())
}

/// MultiAlloyingModule.doAlloy: every recipe that matches, in a shuffled order, performed once if it can be.
fn do_alloy(host: &dyn Host, tank: &mut STank, temperature: i32) -> bool {
    let all = heat_recipes(host);
    let mut order: Vec<usize> = (0..all.alloys.len()).filter(|&i| alloy_match(&all.alloys[i], tank, false).is_some()).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, random(i + 1));
    }
    let mut changed = false;
    for i in order {
        let r = &all.alloys[i];
        if !can_alloy_recipe(r, tank, temperature) {
            continue;
        }
        let Some(matched) = alloy_match(r, tank, true) else { continue };
        for (fluid, amount) in alloy_drains(r, tank, &matched) {
            tank.drain_fluid(&fluid, amount, true);
        }
        tank.fill(&r.output.0, r.output.1, true);
        changed = true;
    }
    changed
}

fn can_alloy(host: &dyn Host, tank: &STank, temperature: i32) -> bool {
    heat_recipes(host).alloys.iter().any(|r| alloy_match(r, tank, false).is_some() && can_alloy_recipe(r, tank, temperature))
}

// ---- The controller's tick

/// What a controller's tick works on, unpacked from its NBT.
pub(crate) struct Hs {
    foundry: bool,
    shape: Option<Shape>,
    tank: STank,
    tank_changed: bool,
    fuel: i32,
    temperature: i32,
    rate: i32,
    last_fuel: Option<Pos>,
    fuel_rate: i32,
}

fn read_hs(nbt: &Compound, pos: Pos, foundry: bool, formed: bool) -> Hs {
    Hs {
        foundry,
        shape: if formed { read_shape(nbt) } else { None },
        tank: read_tank(nbt),
        tank_changed: false,
        fuel: gi(nbt, "fuel", 0),
        temperature: gi(nbt, "temperature", 0),
        rate: gi(nbt, "rate", 0),
        last_fuel: read_offset(nbt, "last_fuel").map(|d| add(pos, d)),
        fuel_rate: gi(nbt, "hz_fuel_rate", 0),
    }
}

fn write_hs(nbt: &mut Compound, pos: Pos, s: &Hs) {
    let t = Tag::Compound(tank_compound(&s.tank));
    if nbt.get(&js("tank")) != Some(&t) {
        nbt.insert(js("tank"), t);
    }
    si(nbt, "fuel", s.fuel);
    si(nbt, "temperature", s.temperature);
    si(nbt, "rate", s.rate);
    match s.last_fuel {
        Some(p) => {
            nbt.insert(js("last_fuel"), offset_tag(sub(p, pos)));
        }
        None => rm(nbt, "last_fuel"),
    }
    si(nbt, "hz_fuel_rate", s.fuel_rate);
}

/// The fuel tank at `p` as a fuel (FuelModule.tryLiquidFuel through its `FLUIDS`): its temperature, the fuel taken
/// with `consume`.
fn try_tank_fuel(host: &mut dyn ServerHost, level: LevelRef, p: Pos, s: &mut Hs, quality: &mut i32, consume: bool) -> i32 {
    let all = recipes::recipes(&*host);
    let Some(cap) = level.capability(host, &FLUIDS, p, None) else { return 0 };
    let mut tank = CapTank(cap);
    let (Some(fluid), amount) = tank.contents(host) else { return 0 };
    let Some(f) = recipes::fluid_fuel(&all, &fluid) else { return 0 };
    if amount < f.amount || f.temperature <= 0 {
        return 0;
    }
    if consume {
        if tank.drain(host, &fluid, f.amount, true) < f.amount {
            return 0;
        }
        s.fuel += f.duration;
        *quality = f.duration;
        s.temperature = f.temperature;
        s.rate = f.rate;
    }
    f.temperature
}

/// MultitankFuelModule.findFuel: the tank used last, then the others in the scan's order.
fn find_structure_fuel(host: &mut dyn ServerHost, level: LevelRef, s: &mut Hs, quality: &mut i32, consume: bool) -> i32 {
    let tanks = s.shape.as_ref().map(|sh| sh.tanks.clone()).unwrap_or_default();
    let last = s.last_fuel.filter(|l| tanks.contains(l));
    for p in last.into_iter().chain(tanks.iter().copied().filter(|t| Some(*t) != last)) {
        let t = try_tank_fuel(host, level, p, s, quality, consume);
        if t > 0 {
            s.last_fuel = Some(p);
            return t;
        }
    }
    if consume {
        s.temperature = 0;
        s.rate = 0;
    }
    0
}

fn rated(amount: i64, rate: Rate, rates: ((i64, i64), (i64, i64))) -> i32 {
    let a = match rate {
        Rate::Metal => amount * rates.0.0 / rates.0.1,
        Rate::Gem => amount * rates.1.0 / rates.1.1,
        Rate::Plain => amount,
    };
    a.clamp(1, i64::from(i32::MAX)) as i32
}

/// MeltingModule.onItemFinishedHeating into the shared tank, all or nothing (the ore rates of the structure).
fn structure_finish(host: &dyn Host, s: &mut Hs, m: &mut Module, slot: &mut Option<ItemStack>) -> bool {
    let Some(stack) = slot.clone() else { return true };
    let Some((all, i)) = find_melting(host, &stack) else { return true };
    let r = &all.melting[i];
    let rates = if s.foundry { FOUNDRY_RATES } else { SMELTERY_RATES };
    let (fluid, amount) = if r.rate == Rate::Plain { recipes::melting_output(host, r, &stack) } else { (r.fluid.clone(), rated(i64::from(r.amount), r.rate, rates)) };
    if s.tank.fill(&fluid, amount, false) != amount {
        m.current = -1;
        return false;
    }
    s.tank.fill(&fluid, amount, true);
    if s.foundry && r.rate != Rate::Plain {
        // The foundry's byproducts (OreMeltingRecipe.byproducts), as much as fits.
        for (f, a) in recipes::byproducts(r) {
            s.tank.fill(&f, rated(i64::from(a), r.rate, BYPRODUCT_RATES), true);
        }
    }
    s.tank_changed = true;
    *slot = None;
    true
}

fn structure_heat_item(host: &dyn Host, s: &mut Hs, m: &mut Module, slot: &mut Option<ItemStack>, temperature: i32, rate: i32) {
    if m.current == -1 || can_heat_item(m, slot, temperature) {
        if m.current != -1 && m.current < m.required {
            m.current += rate;
        } else if structure_finish(host, s, m, slot) {
            *m = Module::default();
        }
    }
}

fn structure_cool_item(host: &dyn Host, s: &mut Hs, m: &mut Module, slot: &mut Option<ItemStack>) {
    if m.current == -1 {
        if structure_finish(host, s, m, slot) {
            *m = Module::default();
        }
    } else if m.current > 0 && m.required > 0 {
        m.current = (m.current - 5).max(0);
    }
}

/// MeltingModuleInventory.heatItems (`Some`) or coolItems (`None`) over every slot.
fn structure_heat_all(host: &dyn Host, s: &mut Hs, inv: &mut Inventory<'_>, heat: Option<(i32, i32)>) {
    for i in 0..inv.size() {
        if inv.get(i).is_none() && inv.store.modules[i].current != -1 {
            continue;
        }
        let mut m = inv.store.modules[i];
        let mut slot = inv.get(i).cloned();
        let before = slot.is_some();
        match heat {
            Some((t, r)) => structure_heat_item(host, s, &mut m, &mut slot, t, r),
            None => structure_cool_item(host, s, &mut m, &mut slot),
        }
        if before && slot.is_none() {
            *inv.slot(i) = None;
        }
        inv.store.modules[i] = m;
    }
}

fn structure_can_heat(inv: &Inventory<'_>, temperature: i32) -> bool {
    (0..inv.size()).any(|i| {
        let item = inv.get(i).cloned();
        can_heat_item(&inv.store.modules[i], &item, temperature)
    })
}

/// HeatingStructureBlockEntity.cool: unformed (or without tanks), items cool while no fuel burns; fuel burns away.
fn structure_cool(host: &dyn Host, s: &mut Hs, inv: &mut Inventory<'_>, tick: u8) {
    match tick % 4 {
        1 if s.fuel <= 0 => structure_heat_all(host, s, inv, None),
        3 if s.fuel > 0 && s.fuel_rate > 0 => s.fuel = (s.fuel - s.fuel_rate).max(0),
        _ => {}
    }
}

/// EntityMeltingModule.interactWithEntities at tick 12: items inside go into the inventory, living things inside are
/// hurt and give their fluid. Whether one melted.
fn melt_entities(host: &mut dyn ServerHost, level: LevelRef, s: &mut Hs, inv: &mut Inventory<'_>, quality: &mut i32) -> bool {
    let Some(shape) = s.shape.clone() else { return false };
    let (lo, hi) = (shape.lo(), shape.hi());
    let min = lo.map(f64::from);
    let max = [f64::from(hi[0] + 1), f64::from(hi[1] + 1), f64::from(hi[2] + 1)];
    let entities = entities_in(&*host, level, min, max).unwrap_or_default();
    let mut targets: Vec<(EntityRef, String)> = Vec::new();
    for ent in entities {
        let Some(kind) = ent.kind(&*host).and_then(|k| k.id(&*host)).map(|k| k.as_str().to_owned()) else { continue };
        if kind == "minecraft:item" {
            let item = ItemEntityRef::new(ent);
            let Some(stack) = item.stack(&*host) else { continue };
            let mut left = stack.count();
            while left > 0 && inv.insert_one(&stack) {
                left -= 1;
            }
            if left != stack.count() {
                match with_count(&stack, left) {
                    Some(rest) => {
                        let _ = item.set_stack(host, rest);
                    }
                    None => {
                        ent.discard(host);
                    }
                }
            }
            continue;
        }
        if LivingRef::new(ent).health(&*host).is_some_and(|h| h > 0.0) {
            targets.push((ent, kind));
        }
    }
    if targets.is_empty() {
        return false;
    }
    // canMeltEntities: fuel burns, or there is some to light.
    if !(s.fuel > 0 || find_structure_fuel(host, level, s, quality, false) > 0) {
        return false;
    }
    let all = heat_recipes(&*host);
    let mut melted = false;
    for (ent, kind) in targets {
        // The scoreboard tags as text (unpaired surrogates as replacement characters, as the old list had them).
        let tags: Vec<String> = ent.tags(&*host).map(|t| t.iter_utf16().map(String::from_utf16_lossy).collect()).unwrap_or_default();
        let (fluid, damage) = match all.melts.iter().find(|m| m.types.contains(&kind) || m.tags.iter().any(|t| tags.contains(t))) {
            Some(m) => (m.result.clone(), m.damage),
            None => (Some(("tconstruct:liquid_soul".to_owned(), 50)), 2),
        };
        let undead = tags.iter().any(|t| t == "minecraft:inverted_healing_and_harm") || UNDEAD.contains(&kind.as_str());
        let source = if undead { "tconstruct:smeltery_magic" } else { "tconstruct:smeltery_heat" };
        let mut hurt = ent.hurt(host, source, damage as f32, None, None) == Some(true);
        if !hurt && LivingRef::new(ent).health(&*host).is_some() {
            hurt = ent.hurt(host, if undead { "minecraft:magic" } else { "minecraft:in_fire" }, damage as f32, None, None) == Some(true);
        }
        if hurt {
            if let Some((f, a)) = fluid
                && s.tank.fill(&f, a, true) > 0
            {
                s.tank_changed = true;
            }
            melted = true;
        }
    }
    melted
}

/// Drops at the controller's front, rand 0.25..0.75 on each axis (HeatingStructureBlockEntity.dropItem).
fn drop_front(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, facing: usize, stack: ItemStack) {
    let f = casting::step(pos, facing);
    let r = || 0.25 + melter::rand01() * 0.5;
    let _ = level.spawn_item(host, f64::from(f[0]) + r(), f64::from(f[1]) + r(), f64::from(f[2]) + r(), stack, 10);
}

/// A part of the structure: its master set (in_structure true) or cleared (ServantTileEntity).
fn set_master(host: &mut dyn ServerHost, level: LevelRef, p: Pos, master: Option<(Pos, &str)>) {
    if let Some(e) = host.block_entity(level, p) {
        let mut c = Compound::new();
        match master {
            Some((m, block)) => {
                c.insert(js("masterOffset"), offset_tag(sub(m, p)));
                ss(&mut c, "masterBlock", block);
            }
            None => {
                c.insert(js("masterOffset"), offset_tag([0, 0, 0]));
                ss(&mut c, "masterBlock", "");
            }
        }
        let _ = e.merge_data(host, c);
    }
    let value = if master.is_some() { "true" } else { "false" };
    if level.block_property(&*host, p, "in_structure").is_some_and(|v| v != value) {
        host.set_block_property_with(p, "in_structure", value, 2);
    }
    host.block_entity_sync(p);
}

/// Where a part's NBT says its master is (`masterOffset` from it), while it names one.
pub(crate) fn named_master(pos: Pos, nbt: &Compound) -> Option<Pos> {
    gs(nbt, "masterBlock").filter(|b| !b.is_empty())?;
    Some(add(pos, read_offset(nbt, "masterOffset")?))
}

/// The master a part's NBT names, while that controller is there and formed (ServantTileEntity.validateMaster).
fn io_master(host: &dyn Host, level: LevelRef, pos: Pos, nbt: &Compound) -> Option<Pos> {
    let m = named_master(pos, nbt)?;
    let block = gs(nbt, "masterBlock")?;
    (block_id(host, level, m) == block && level.block_property(host, m, "in_structure").as_deref() == Some("true")).then_some(m)
}

/// `io_master` through a provider's read-only level view.
fn view_io_master(level: &dyn LevelHost, pos: Pos, nbt: &Compound) -> Option<Pos> {
    let m = named_master(pos, nbt)?;
    let block = gs(nbt, "masterBlock")?;
    let state = level.block_state(m)?;
    (view_block_id(level, state) == block && view_property(level, state, "in_structure").as_deref() == Some("true")).then_some(m)
}

/// The menus of everyone with the controller's open are closed (the structure went, or its slot count changed).
fn close_viewers(host: &mut dyn ServerHost, level: LevelRef, pos: Pos) {
    let Some(e) = host.block_entity(level, pos) else { return };
    for p in e.viewers(&*host).unwrap_or_default() {
        if let Some(m) = p.container_menu(&*host) {
            m.close_container(host);
        }
    }
}

/// HeatingStructureBlockEntity.checkStructure.
fn check_structure(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, block: &str, e: &mut Fields, s: &mut Hs, inv: &mut Inventory<'_>, facing: usize) {
    let dim = host.dimension().to_owned();
    let (result, last) = detect(&*host, level, pos, facing, s.foundry);
    let formed = result.is_ok();
    let was = level.block_property(&*host, pos, "in_structure").as_deref() == Some("true");
    if formed != was {
        host.set_block_property_with(pos, "in_structure", if formed { "true" } else { "false" }, 3);
        if !formed && level.block_property(&*host, pos, "active").as_deref() == Some("true") {
            host.set_block_property_with(pos, "active", "false", 3);
        }
    }
    let old = s.shape.take().or_else(|| read_shape(&e.nbt).filter(|_| was));
    let foundry = s.foundry;
    let parts = |shape: &Shape| -> Vec<Pos> {
        let mut out = Vec::new();
        for x in shape.min[0]..=shape.max[0] {
            for y in shape.min[1]..=shape.max[1] {
                for z in shape.min[2]..=shape.max[2] {
                    let p = [x, y, z];
                    if contained(p, shape.min, shape.max, foundry) && p != pos {
                        out.push(p);
                    }
                }
            }
        }
        out
    };
    let old_parts: Vec<Pos> = old.as_ref().map(parts).unwrap_or_default();
    let mastered_here = |host: &dyn Host, p: Pos| is_io(&block_id(host, level, p)) && host.block_entity(level, p).and_then(|b| b.data(host)).is_some_and(|d| named_master(p, &d) == Some(pos));
    let error = match result {
        Ok(shape) => {
            let new_parts = parts(&shape);
            let new_set: HashSet<Pos> = new_parts.iter().copied().collect();
            for p in old_parts.iter().filter(|p| !new_set.contains(*p)) {
                if mastered_here(&*host, *p) {
                    set_master(host, level, *p, None);
                }
            }
            for p in new_parts {
                if is_io(&block_id(&*host, level, p)) {
                    set_master(host, level, p, Some((pos, block)));
                }
            }
            with_rt(|r| r.formed.insert((dim.clone(), pos), (shape.min, shape.max, foundry)));
            // setStructure: the capacity, the slots (what no longer fits drops at the front), the fuel rate.
            let size = shape.size();
            s.tank.capacity = structure_capacity(foundry, size);
            let slots = structure_slots(size);
            if slots != inv.size() {
                resize_inventory(host, level, pos, facing, inv, slots);
                close_viewers(host, level, pos);
            }
            s.fuel_rate = structure_fuel_rate(foundry, size);
            write_shape(&mut e.nbt, Some(&shape));
            s.shape = Some(shape);
            last
        }
        Err(f) => {
            for p in old_parts {
                if mastered_here(&*host, p) {
                    set_master(host, level, p, None);
                }
            }
            with_rt(|r| r.formed.remove(&(dim.clone(), pos)));
            write_shape(&mut e.nbt, None);
            if was {
                close_viewers(host, level, pos);
            }
            Some(f)
        }
    };
    set_error(&mut e.nbt, error.as_ref());
    s.tank_changed = true;
}

fn set_error(nbt: &mut Compound, f: Option<&Failure>) {
    match f {
        Some(f) => {
            ss(nbt, "hz_error_key", f.key);
            sia(nbt, "hz_error_args", f.args.clone());
            match f.pos {
                Some(p) => sia(nbt, "hz_error", p.to_vec()),
                None => rm(nbt, "hz_error"),
            }
        }
        None => {
            for k in ["hz_error_key", "hz_error_args", "hz_error"] {
                rm(nbt, k);
            }
        }
    }
}

/// MeltingModuleInventory.resize: the slots past `slots` drop at the front, the window back at the start.
fn resize_inventory(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, facing: usize, inv: &mut Inventory<'_>, slots: usize) {
    let old = inv.size();
    if slots < old {
        for i in slots..old {
            if let Some(st) = inv.slot(i).take() {
                drop_front(host, level, pos, facing, st);
            }
        }
        inv.set_offset(0);
        inv.store.items.truncate(slots);
        inv.store.modules.truncate(slots);
    } else {
        inv.set_offset(0);
        inv.store.items.resize(slots, None);
        inv.store.modules.resize(slots, Module::default());
    }
    inv.changed = true;
}

/// The bucket slots (HeatingStructureContainerMenu.updateBucket): the input item emptied into the tank or filled from
/// its bottom fluid, by the mode (0 auto, 1 empty item, 2 fill item), into the result slot.
fn process_bucket(host: &mut dyn ServerHost, items: &mut [Option<ItemStack>], tank: &mut STank, mode: i32) -> bool {
    let (Some(input), true) = (items[0].clone(), items[1].is_none()) else { return false };
    let way = match mode {
        1 => Way::EmptyItem,
        2 => Way::FillItem,
        _ => Way::Auto,
    };
    let Some(one) = with_count(&input, 1) else { return false };
    let mut access = STankAccess { tank, prefer: None };
    let Some(m) = containers::transfer(host, &one, &mut access, way) else { return false };
    items[0] = with_count(&input, input.count() - 1);
    items[1] = m.result;
    true
}

/// MeltingModule.setStack for every slot whose item is new (its recipe, time and temperature).
fn refresh_modules(host: &dyn Host, inv: &mut Inventory<'_>) {
    for i in 0..inv.size() {
        if inv.store.modules[i].required == UNSET {
            let item = inv.get(i).cloned();
            let mut m = Module::default();
            set_stack(host, &mut m, &item);
            inv.store.modules[i] = m;
        }
    }
}

fn facing_of(host: &dyn Host, level: LevelRef, pos: Pos) -> usize {
    level.block_property(host, pos, "facing").and_then(|f| casting::dir_index(&f)).filter(|&f| f >= 2).unwrap_or(2)
}

fn give_back(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, facing: usize, stack: ItemStack) {
    let viewer = host.block_entity(level, pos).and_then(|e| e.viewers(&*host)).and_then(|v| v.first().copied());
    match viewer {
        Some(p) => {
            let u = p.uuid();
            containers::give_player(host, u, stack);
        }
        None => drop_front(host, level, pos, facing, stack),
    }
}

fn controller_tick(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, block: &str, e: &mut Fields) {
    let dim = host.dimension().to_owned();
    let key = (dim.clone(), pos);
    let foundry = block == FOUNDRY;
    let facing = facing_of(&*host, level, pos);
    if e.items.len() < STRUCTURE_SLOTS {
        e.items.resize(STRUCTURE_SLOTS, None);
    }
    let formed_state = level.block_property(&*host, pos, "in_structure").as_deref() == Some("true");
    let mut c = with_rt(|r| r.controllers.get(&key).copied()).unwrap_or_default();
    let mut s = read_hs(&e.nbt, pos, foundry, formed_state);
    if !c.started {
        c.started = true;
        // After a load: the formed structure is known again, and checked once (its parts' masters with it).
        if let Some(sh) = &s.shape {
            with_rt(|r| r.formed.insert(key.clone(), (sh.min, sh.max, foundry)));
        }
        c.queued = true;
    }
    if gi(&e.nbt, "hz_fluid_dirty", 0) != 0 {
        rm(&mut e.nbt, "hz_fluid_dirty");
        s.tank_changed = true;
    }
    let size = gi(&e.nbt, "hz_size", 0).clamp(0, MAX_SLOTS as i32) as usize;
    let mut store = take_store(&*host, &key, &e.nbt, size);
    let mut items = std::mem::take(&mut e.items);
    let offset = (gi(&e.nbt, "hz_offset", 0).max(0) as usize).min(size);
    let mut seen = get_strings(&e.nbt, "hz_seen", WINDOW);
    let mut inv = Inventory { items: &mut items, store: &mut store, offset, changed: false };
    // The window's slots changed by a menu click or a hopper since the last tick: their modules start again.
    for (w, seen_key) in seen.iter_mut().enumerate() {
        let k = stack_key(&inv.items[WIN0 + w]);
        if k != *seen_key {
            let i = offset + w;
            if i < inv.size() {
                inv.store.modules[i] = unset();
            }
            *seen_key = k;
        }
    }
    // One a slot; a window slot past the inventory holds nothing.
    let size_now = inv.size();
    let mut extras = Vec::new();
    for w in 0..WINDOW {
        let slot = &mut inv.items[WIN0 + w];
        if offset + w >= size_now {
            if let Some(x) = slot.take() {
                extras.push(x);
            }
        } else if let Some(st) = slot.clone().filter(|st| st.count() > 1) {
            *slot = with_count(&st, 1);
            if let Some(x) = with_count(&st, st.count() - 1) {
                extras.push(x);
            }
        }
    }
    // The intake (hoppers, chutes): into the first empty slot.
    if let Some(st) = inv.items[INTAKE].clone() {
        let mut left = st.count();
        while left > 0 && inv.insert_one(&st) {
            left -= 1;
        }
        inv.items[INTAKE] = with_count(&st, left);
    }
    // The bucket slots.
    let mode = gi(&e.nbt, "hz_mode", 0).rem_euclid(3);
    if process_bucket(host, &mut inv.items[..], &mut s.tank, mode) {
        s.tank_changed = true;
    }

    // HeatingStructureBlockEntity.serverTick.
    if c.queued {
        c.queued = false;
        check_structure(host, level, pos, block, e, &mut s, &mut inv, facing);
        c.expand = 0;
    }
    let formed = s.shape.is_some() && level.block_property(&*host, pos, "in_structure").as_deref() == Some("true");
    refresh_modules(&*host, &mut inv);
    if formed {
        let shape = s.shape.clone().unwrap_or(Shape { min: pos, max: pos, tanks: Vec::new() });
        if c.tick == 0 {
            c.expand += 1;
            if c.expand >= 10 && shape.size()[1] < 64 {
                c.expand = 0;
                match can_expand(&*host, level, pos, foundry, &shape) {
                    Ok(()) => c.queued = true,
                    Err(f) => set_error(&mut e.nbt, Some(&f)),
                }
            }
        } else if c.tick % 4 == 0 {
            // The inside sweep: z fastest, then x, then y.
            let [sx, sy, sz] = shape.size().map(|v| v.max(1) as usize);
            let total = sx * sy * sz;
            let i = c.inside % total;
            let (z, x, y) = (i % sz, (i / sz) % sx, i / (sz * sx));
            let lo = shape.lo();
            let p = [lo[0] + x as i32, lo[1] + y as i32, lo[2] + z as i32];
            c.inside = (i + 1) % total;
            let scan = Scan { host: &*host, level, dim: dim.clone(), controller: pos, foundry, tanks: Vec::new() };
            if !scan.air(p) {
                c.queued = true;
            }
        }
        // SmelteryBlockEntity.heat / FoundryBlockEntity.heat.
        if shape.tanks.is_empty() {
            structure_cool(&*host, &mut s, &mut inv, c.tick);
        } else {
            let melted = c.tick == 12 && melt_entities(host, level, &mut s, &mut inv, &mut c.quality);
            match c.tick % 4 {
                0 => {
                    if s.fuel <= 0 {
                        if melted {
                            find_structure_fuel(host, level, &mut s, &mut c.quality, true);
                        } else {
                            let t = find_structure_fuel(host, level, &mut s, &mut c.quality, false);
                            if structure_can_heat(&inv, t) || (!foundry && can_alloy(&*host, &s.tank, t)) {
                                find_structure_fuel(host, level, &mut s, &mut c.quality, true);
                            }
                        }
                    }
                }
                1 => {
                    let heat = (s.fuel > 0).then_some((s.temperature, s.rate));
                    structure_heat_all(&*host, &mut s, &mut inv, heat);
                }
                2 => {
                    if !foundry && s.fuel > 0 {
                        let t = s.temperature;
                        if do_alloy(&*host, &mut s.tank, t) {
                            s.tank_changed = true;
                        }
                    }
                }
                _ => {
                    let lit = if s.fuel > 0 { "true" } else { "false" };
                    if level.block_property(&*host, pos, "active").is_some_and(|v| v != lit) {
                        host.set_block_property_with(pos, "active", lit, 3);
                    }
                    s.fuel = (s.fuel - s.fuel_rate).max(0);
                }
            }
        }
    } else {
        if c.tick == 0 {
            c.queued = true;
        }
        structure_cool(&*host, &mut s, &mut inv, c.tick);
    }
    c.tick = (c.tick + 1) % 20;
    refresh_modules(&*host, &mut inv);

    // Back into the NBT, and what the menu and the clients read.
    let free = inv.free();
    let size = inv.size();
    let offset = inv.offset;
    let changed = inv.changed;
    for (w, seen_key) in seen.iter_mut().enumerate() {
        *seen_key = stack_key(&inv.items[WIN0 + w]);
    }
    let mut data = vec![0; STRUCTURE_DATA];
    data[0] = s.fuel;
    data[1] = c.quality;
    data[2] = s.temperature;
    data[3] = s.rate;
    let last = s.last_fuel.map_or([0, -1000, 0], |l| sub(l, pos));
    data[4..7].copy_from_slice(&last);
    for w in 0..WINDOW {
        let i = offset + w;
        let m = if i < size && inv.items[WIN0 + w].is_some() { inv.store.modules[i] } else { Module { current: 0, required: 0, temperature: -1 } };
        data[D_WINDOW + w * 3] = m.current;
        data[D_WINDOW + w * 3 + 1] = m.required;
        data[D_WINDOW + w * 3 + 2] = m.temperature;
    }
    data[D_OFFSET] = offset as i32;
    data[D_SIZE] = size as i32;
    data[D_MODE] = mode;
    let render: Vec<String> = (0..size.min(RENDER_ITEMS)).map(|i| inv.get(i).map_or(String::new(), |s| sid(s).to_owned())).collect();
    let render_len = render.iter().rposition(|r| !r.is_empty()).map_or(0, |i| i + 1);
    let _ = inv;
    e.items = items;
    for x in extras {
        give_back(host, level, pos, facing, x);
    }
    put_store(&*host, key.clone(), &mut e.nbt, store, changed);
    if e.data != data {
        e.data = data;
    }
    si(&mut e.nbt, "hz_size", size as i32);
    si(&mut e.nbt, "hz_offset", offset as i32);
    si(&mut e.nbt, "hz_free", free as i32);
    set_strings(&mut e.nbt, "hz_seen", &seen);
    set_strings(&mut e.nbt, "hz_items", &render[..render_len]);
    write_hs(&mut e.nbt, pos, &s);
    with_rt(|r| r.controllers.insert(key.clone(), c));
    // What clients draw: the tank, the structure, the items inside, the error.
    let vis = format!("{:?}|{}|{:?}|{:?}|{:?}|{}|{:?}", s.tank.fluids, s.tank.capacity, get_pos(&e.nbt, "hz_min"), get_pos(&e.nbt, "hz_max"), gia(&e.nbt, "hz_tanks"), render[..render_len].join(","), get_pos(&e.nbt, "hz_error"));
    if s.tank_changed || gs(&e.nbt, "hz_vis").as_deref() != Some(vis.as_str()) {
        ss(&mut e.nbt, "hz_vis", &vis);
        host.block_entity_sync(pos);
    }
    // getComparatorInputOverride: the tank's fill.
    let strength = signal(s.tank.contained().min(i64::from(i32::MAX)) as i32, s.tank.capacity);
    if with_rt(|r| r.signals.insert(key, strength)) != Some(strength)
        && let Some(state) = level.block_state(&*host, pos)
    {
        let _ = level.update_neighbors(host, pos, state, NeighbourUpdate::OutputSignal);
    }
}

// ---- The drains, ducts and chutes

/// DuctItemHandler.isItemValid: a filled bucket (`#tconstruct:duct_containers`): its fluid.
fn duct_filter(host: &dyn Host, stack: &ItemStack) -> Option<String> {
    let _ = host;
    duct_fluid(stack)
}

/// `duct_filter` without a host (`can_place_item`, the slot rules); nothing before the item ids are built.
fn duct_fluid(stack: &ItemStack) -> Option<String> {
    let id = sid(stack);
    if id.is_empty() {
        return None;
    }
    if id == "minecraft:water_bucket" {
        return Some("minecraft:water".into());
    }
    if id == "minecraft:lava_bucket" {
        return Some("minecraft:lava".into());
    }
    crate::fluids::table().buckets.iter().find(|(_, b)| b.as_str() == id).map(|(f, _)| f.clone())
}

fn io_tick(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, block: &str, e: &mut Fields) {
    // ServantTileEntity.validateMaster: a master gone or unformed is forgotten, and the part leaves the structure.
    let master = io_master(&*host, level, pos, &e.nbt);
    if master.is_none() && (named_master(pos, &e.nbt).is_some() || level.block_property(&*host, pos, "in_structure").as_deref() == Some("true")) {
        e.nbt.insert(js("masterOffset"), offset_tag([0, 0, 0]));
        ss(&mut e.nbt, "masterBlock", "");
        if level.block_property(&*host, pos, "in_structure").as_deref() == Some("true") {
            host.set_block_property_with(pos, "in_structure", "false", 2);
        }
    }
    if is_duct(block) {
        if e.items.is_empty() {
            e.items.resize(1, None);
        }
        // The filter slot holds one valid container; anything else goes back out on top.
        if let Some(st) = e.items[0].clone() {
            let extra = if duct_filter(&*host, &st).is_some() {
                e.items[0] = with_count(&st, 1);
                with_count(&st, st.count() - 1)
            } else {
                e.items[0] = None;
                Some(st)
            };
            if let Some(x) = extra {
                let _ = level.spawn_item(host, f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.0, f64::from(pos[2]) + 0.5, x, 10);
            }
        }
        let filter = e.items[0].as_ref().and_then(|s| duct_filter(&*host, s)).unwrap_or_default();
        if gs(&e.nbt, "hz_filter").as_deref() != Some(filter.as_str()) {
            ss(&mut e.nbt, "hz_filter", &filter);
        }
    }
    // The client draws the master's bottom fluid in a drain's spouts.
    let bottom = master
        .filter(|_| is_drain(block))
        .and_then(|m| host.block_entity(level, m))
        .and_then(|c| c.data(&*host))
        .and_then(|d| read_tank(&d).fluids.first().map(|f| f.0.clone()))
        .unwrap_or_default();
    if gs(&e.nbt, "hz_bottom").as_deref() != Some(bottom.as_str()) {
        ss(&mut e.nbt, "hz_bottom", &bottom);
    }
    let vis = format!("{master:?}{:?}{bottom}", gs(&e.nbt, "hz_filter"));
    if gs(&e.nbt, "hz_vis").as_deref() != Some(vis.as_str()) {
        ss(&mut e.nbt, "hz_vis", &vis);
        host.block_entity_sync(pos);
    }
}

/// The shared tank through `FLUIDS`: on the controller and the drains each fluid then the room left (SmelteryTank's
/// tanks), on a duct only its filter's fluid (DuctTankWrapper). Fill on top or merged; drain a chosen fluid, or the
/// bottom one through index 0.
pub(crate) struct StructureFluids {
    controller: BlockEntityRef,
    filter: Option<Option<String>>,
}

impl StructureFluids {
    fn tank(&self, host: &dyn Host) -> STank {
        self.controller.data(host).map(|d| read_tank(&d)).unwrap_or_default()
    }
    /// The tank's fluids as this side shows them: (index in the tank, fluid, amount).
    fn views(&self, host: &dyn Host) -> (STank, Vec<usize>) {
        let t = self.tank(host);
        let shown: Vec<usize> = (0..t.fluids.len()).filter(|&i| self.filter.as_ref().is_none_or(|f| f.as_deref() == Some(t.fluids[i].0.as_str()))).collect();
        (t, shown)
    }
    fn write(&self, host: &mut dyn ServerHost, t: &STank) -> bool {
        let mut c = Compound::new();
        c.insert(js("tank"), Tag::Compound(tank_compound(t)));
        si(&mut c, "hz_fluid_dirty", 1);
        matches!(self.controller.merge_data(host, c), Ok(true))
    }
}

impl ResourceHandler<FluidResource> for StructureFluids {
    fn size(&self, host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        let (t, shown) = self.views(&*host);
        let room = t.remaining() > 0 && self.filter.as_ref().is_none_or(Option::is_some);
        shown.len() + usize::from(room)
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<FluidResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        let (t, shown) = self.views(&*host);
        let (f, a) = t.fluids.get(*shown.get(index)?)?;
        containers::fluid_res(&*host, f, (*a).max(1) as u32)
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        let (t, shown) = self.views(&*host);
        shown.get(index).and_then(|&i| t.fluids.get(i)).map_or(0, |(_, a)| (*a).max(0) as u32)
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, _resource: Option<&FluidResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        let (t, shown) = self.views(&*host);
        let room = t.remaining();
        match shown.get(index).and_then(|&i| t.fluids.get(i)) {
            Some((_, a)) => a.saturating_add(room).max(0) as u32,
            None => room.max(0) as u32,
        }
    }
    fn insert_at(&self, host: &mut dyn ServerHost, _tx: &TransactionContext, _index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if amount == 0 || resource.to_stack(1).has_components() {
                return 0;
            }
            let Some(fluid) = containers::fluid_id(&*host, resource) else { return 0 };
            if let Some(f) = &self.filter
                && f.as_deref() != Some(fluid.as_str())
            {
                return 0;
            }
            let mut t = self.tank(&*host);
            let n = t.fill(&fluid, amount.min(i32::MAX as u32) as i32, true);
            if n > 0 && self.write(host, &t) { n as u32 } else { 0 }
        })())
    }
    fn extract_at(&self, host: &mut dyn ServerHost, _tx: &TransactionContext, index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            if amount == 0 {
                return 0;
            }
            let Some(fluid) = containers::fluid_id(&*host, resource) else { return 0 };
            let (mut t, shown) = self.views(&*host);
            let Some(&i) = shown.get(index) else { return 0 };
            if t.fluids[i].0 != fluid {
                return 0;
            }
            let n = t.drain_fluid(&fluid, amount.min(i32::MAX as u32) as i32, true);
            if n > 0 && self.write(host, &t) { n as u32 } else { 0 }
        })())
    }
}

pub(crate) fn structure_fluids(level: &dyn LevelHost, at: BlockSide, state: BlockState, entity: BlockEntityRef, offer: &mut BlockCapabilityOffer<'_, dyn ResourceHandler<FluidResource>>) {
    let block = view_block_id(level, state);
    if is_controller(&block) {
        offer.offer(|| Box::new(StructureFluids { controller: entity, filter: None }));
        return;
    }
    if !(is_drain(&block) || is_duct(&block)) {
        return;
    }
    let Some(data) = level.block_entity_data(entity) else { return };
    let Some(m) = view_io_master(level, at.pos, &data) else { return };
    let Some(controller) = level.block_entity(m) else { return };
    let filter = is_duct(&block).then(|| gs(&data, "hz_filter").filter(|f| !f.is_empty()));
    offer.offer(|| Box::new(StructureFluids { controller, filter }));
}

/// A chute's `ITEMS`: its master's (the controller's intake), while formed. A provider only reads the level now, so
/// the master's handler is looked up by each call with the server host (none there: an empty handler).
pub(crate) struct ChuteItems {
    level: LevelRef,
    master: Pos,
}

impl ChuteItems {
    fn cap(&self, host: &mut dyn ServerHost) -> Option<Capability<dyn ResourceHandler<ItemResource>>> {
        self.level.capability(host, &ITEMS, self.master, None)
    }
}

impl ResourceHandler<ItemResource> for ChuteItems {
    fn size(&self, host: &mut dyn ServerHost) -> usize {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.cap(host).map_or(0, |c| c.size(host))
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.cap(host)?.resource(host, index)
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.cap(host).map_or(0, |c| c.amount(host, index))
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, resource: Option<&ItemResource>) -> u32 {
        let _t = crate::perf::t(crate::perf::CAPS);
        self.cap(host).map_or(0, |c| c.capacity(host, index, resource))
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            self.cap(host).map_or(0, |c| c.insert_at(host, tx, index, resource, amount).unwrap_or(0))
        })())
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        // The f8aa80e handler answer: the old u32 body, unchanged, as Ok.
        Ok((|| {
            let _t = crate::perf::t(crate::perf::CAPS);
            self.cap(host).map_or(0, |c| c.extract_at(host, tx, index, resource, amount).unwrap_or(0))
        })())
    }
}

pub(crate) fn chute_items(level: &dyn LevelHost, at: BlockSide, state: BlockState, entity: BlockEntityRef, offer: &mut BlockCapabilityOffer<'_, dyn ResourceHandler<ItemResource>>) {
    let _ = state;
    let Some(data) = level.block_entity_data(entity) else { return };
    let Some(master) = view_io_master(level, at.pos, &data) else { return };
    // The master's own ITEMS is there (the old provider asked for it and offered nothing without it).
    if level.has_item_capability(master, None) != Some(true) {
        return;
    }
    offer.offer(|| Box::new(ChuteItems { level: at.level, master }));
}

// ---- Registration and hooks

#[derive(Default)]
pub(crate) struct StructurePart;

/// The walls, floors and windows: a click opens the controller's menu.
#[derive(Default)]
pub(crate) struct WallPart;

pub(crate) fn register(r: &mut dyn Registrar) {
    let types: [(&str, Vec<&str>); 5] =
        [(SMELTERY_TYPE, vec![SMELTERY]), (FOUNDRY_TYPE, vec![FOUNDRY]), (DRAIN_TYPE, DRAINS.to_vec()), (DUCT_TYPE, DUCTS.to_vec()), (CHUTE_TYPE, CHUTES.to_vec())];
    for (ty, blocks) in types {
        let key = r.block_entity_type(ty).blocks(&blocks).build();
        r.block_entity_part(&key, StructurePart).expect("the structure block entity types are native");
        match ty {
            SMELTERY_TYPE | FOUNDRY_TYPE | DRAIN_TYPE | DUCT_TYPE => r.block_entity_capability_before_game(&FLUIDS, &key, structure_fluids),
            _ => r.block_entity_capability_before_game(&ITEMS, &key, chute_items),
        }
    }
    for m in [STRUCTURE_MENU, DUCT_MENU] {
        r.menu_part(m, StructurePart);
        if let Err(e) = r.register_menu_screen(m, std::sync::Arc::new(menu_screen)) {
            tlog(&format!("{m}: no screen ({e})"));
        }
    }
    for b in wall_blocks() {
        let k = r.block_key(&b);
        r.block_part(k, WallPart);
    }
    for (b, _) in super::TANKS {
        let k = r.block_key(b);
        r.block_part(k, WallPart);
    }
}

/// The container of the controllers (43 slots) and the duct's filter slot.
pub(crate) fn block_entity_container(block: &str) -> Option<ModContainer> {
    if is_controller(block) {
        let window: Vec<usize> = (WIN0..WIN0 + WINDOW).collect();
        let mut faces = window.clone();
        faces.push(INTAKE);
        return Some(ModContainer { slots: STRUCTURE_SLOTS, faces: std::array::from_fn(|_| faces.clone()), inputs: vec![INTAKE], outputs: window, insert_rules: Vec::new(), menu: Some(STRUCTURE_MENU.to_owned()) });
    }
    if is_duct(block) {
        return Some(ModContainer { slots: 1, faces: std::array::from_fn(|_| vec![0]), inputs: vec![0], outputs: vec![0], insert_rules: Vec::new(), menu: Some(DUCT_MENU.to_owned()) });
    }
    None
}

pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, entity: &mut Fields) {
    ensure_ids(&*host);
    let Some(level) = here(&*host) else { return };
    let block = block_id(&*host, level, pos);
    let mut e = entity.clone();
    if is_controller(&block) {
        controller_tick(host, level, pos, &block, &mut e);
    } else if is_io(&block) {
        io_tick(host, level, pos, &block, &mut e);
    }
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

pub(crate) fn update_tag(nbt: &Compound) -> Compound {
    let mut out = Compound::new();
    for k in ["tank", "hz_min", "hz_max", "hz_tanks", "hz_items", "hz_size", "hz_error", "hz_error_key", "masterOffset", "masterBlock", "hz_filter", "hz_bottom"] {
        if let Some(v) = nbt.get(&js(k)) {
            out.insert(js(k), v.clone());
        }
    }
    out
}

/// The formed structure `pos` is a part of here (its controller), if any.
fn structure_of(host: &dyn Host, level: LevelRef, pos: Pos) -> Option<Pos> {
    owner_of(host.dimension(), pos).filter(|c| level.block_property(host, *c, "in_structure").as_deref() == Some("true") && is_controller(&block_id(host, level, *c)))
}

fn open_controller(host: &mut dyn ServerHost, player: u128, level: LevelRef, controller: Pos) -> Option<InteractionResult> {
    let p = host.player(player)?;
    let entity = host.block_entity(level, controller)?;
    let slots = entity.data(&*host).map_or(0, |d| gi(&d, "hz_size", 0).max(0) as usize);
    with_rt(|r| r.opening = Some(slots));
    let block = block_id(&*host, level, controller);
    pad_slots(host, entity, &block);
    let r = p.open_menu(host, entity);
    with_rt(|r| r.opening = None);
    matches!(r, Ok(true)).then_some(InteractionResult::Success)
}

/// HeatingControllerBlock.use: formed, the menu; unformed, why (the last result) on the action bar.
fn controller_use(host: &mut dyn ServerHost, player: u128, level: LevelRef, pos: Pos) -> Option<InteractionResult> {
    if level.block_property(&*host, pos, "in_structure").as_deref() == Some("true") {
        return open_controller(host, player, level, pos).or(Some(InteractionResult::Success));
    }
    let p = host.player(player)?;
    let data = host.block_entity(level, pos)?.data(&*host).unwrap_or_default();
    let key = gs(&data, "hz_error_key").unwrap_or_else(|| "no_attempt".to_owned());
    let args: Vec<Text> = gia(&data, "hz_error_args").iter().map(|a| Text::literal(a.to_string())).collect();
    let english = match key.as_str() {
        "not_loaded" => "Structure is outside the loaded area, this is normally a bug with the mod",
        "too_large" => "Structure inside is too large: currently %s×%s, max %s×%s, ",
        "invalid_inner_block" => "Invalid block inside the structure",
        "invalid_wall_block" => "Invalid block in the walls",
        "invalid_floor_block" => "Invalid block in the floor",
        "invalid_wall_frame" => "Invalid block in the wall frame",
        "invalid_floor_frame" => "Invalid block on the edges of the floor",
        _ => "Structure detection has not yet run, this is normally a bug with the mod",
    };
    let _ = p.action_bar(host, tr(&format!("multiblock.tconstruct.generic.{key}"), english, args));
    Some(InteractionResult::Success)
}

fn sneaking(host: &dyn Host, player: u128) -> bool {
    host.player(player).is_some_and(|p| p.entity().is_shift_key_down(host).unwrap_or(false))
}

/// Block clicks (server): the controller, a drain with a container (FluidTransferHelper.interactWithTank on the master's
/// tank), and any other part of a formed structure: the controller's menu (the structure as one block).
fn use_block(host: &mut dyn ServerHost, click: &BlockClick<'_>, with_item: bool) -> Option<InteractionResult> {
    ensure_ids(&*host);
    let level = host.player(click.player)?.level(&*host)?;
    let pos = click.pos;
    let block = block_id(&*host, level, pos);
    let held = click.stack.cloned().filter(|s| s.count() > 0);
    if with_item && held.is_none() {
        return None;
    }
    if is_controller(&block) {
        if with_item && sneaking(&*host, click.player) {
            return None;
        }
        return controller_use(host, click.player, level, pos);
    }
    if is_drain(&block) && held.is_some() && containers::interact_with_tank(host, click, level) {
        return Some(InteractionResult::Success);
    }
    // A tank with a container is the tank's own click (`melter::use_block`).
    if super::is_tank(&block) && held.as_ref().is_some_and(|h| containers::is_container(host, h)) {
        return None;
    }
    if is_duct(&block) && !(with_item && sneaking(&*host, click.player)) {
        // DuctBlock: its own filter menu.
        let entity = host.block_entity(level, pos)?;
        let p = host.player(click.player)?;
        pad_slots(host, entity, &block);
        return matches!(p.open_menu(host, entity), Ok(true)).then_some(InteractionResult::Success);
    }
    if with_item && sneaking(&*host, click.player) {
        return None;
    }
    let controller = structure_of(&*host, level, pos)?;
    open_controller(host, click.player, level, controller)
}

/// The client ends its click as used where the server opens a menu, so the held item's own use doesn't run there.
pub(crate) fn client_click(host: &dyn ClientHost, block: &str, event: &mut RightClickBlockEvent<'_>) {
    let pos = event.click.pos;
    let sneak = host.local_player().and_then(|p| p.entity().is_shift_key_down(host)).unwrap_or(false);
    let part = super::structure_screen::clicked(host, pos, block);
    let held = event.click.stack.is_some_and(|s| s.count() > 0);
    if (is_controller(block) || is_duct(block) || part) && !(held && sneak) && !(held && event.click.stack.is_some_and(|s| containers::maybe_container(host, s)) && !is_controller(block)) {
        event.cancelled = true;
        *event.cancellation_result = InteractionResult::Success;
    }
}

/// `block_changed` (every setBlock): a part of a formed structure replaced, a block put inside or on the layer above,
/// checks its structure next tick (notifyMasterOfChange); a removed controller's saved inventory drops.
pub(crate) fn block_changed(level: &dyn LevelHost, event: &BlockChangedEvent) {
    let (Some(old), Some(new)) = (level.state_block(event.old), level.state_block(event.new)) else { return };
    if old == new {
        return;
    }
    let dim = event.dimension.key().id().to_owned();
    let p = [event.pos.x, event.pos.y, event.pos.z];
    if is_controller(old) {
        let key = (dim.clone(), p);
        with_rt(|r| {
            r.formed.remove(&key);
            r.controllers.remove(&key);
            r.signals.remove(&key);
            // The window's items drop with the container; the rest from the cache of the last tick.
            if let Some(s) = r.stores.remove(&key) {
                let stacks: Vec<ItemStack> = s.items.into_iter().flatten().collect();
                if !stacks.is_empty() {
                    r.drops.push((dim.clone(), p, stacks));
                }
            }
        });
        return;
    }
    with_rt(|r| {
        let hits: Vec<Key> = r.formed.iter().filter(|((d, _), (min, max, _))| *d == dim && in_box(p, *min, [max[0], max[1] + 1, max[2]])).map(|(k, _)| k.clone()).collect();
        for k in hits {
            r.controllers.entry(k).or_default().queued = true;
        }
    });
}

/// `level_tick_post`: removed controllers' inventories drop where they stood.
pub(crate) fn level_tick(host: &mut dyn ServerHost) {
    let dim = host.dimension().to_owned();
    let drops: Vec<(Pos, Vec<ItemStack>)> = with_rt(|r| {
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut r.drops).into_iter().partition(|(d, _, _)| *d == dim);
        r.drops = rest;
        mine.into_iter().map(|(_, p, s)| (p, s)).collect()
    });
    if drops.is_empty() {
        return;
    }
    let Some(level) = here(&*host) else { return };
    for (p, stacks) in drops {
        for st in stacks {
            let r = || 0.25 + melter::rand01() * 0.5;
            let _ = level.spawn_item(host, f64::from(p[0]) + r(), f64::from(p[1]) + r(), f64::from(p[2]) + r(), st, 10);
        }
    }
}

/// FluidTransferHelper.handleUIResult: the cursor's last item becomes the result, else the result goes to the player.
fn cursor_result(host: &mut dyn ServerHost, player: u128, carried: &mut Option<ItemStack>, held: ItemStack, m: containers::Moved) {
    if let Some(p) = host.player(player) {
        p.play_sound(host, containers::bucket_sound(&m.fluid, m.filled_item), SoundSource::Players, 1.0, 1.0);
    }
    if held.count() <= 1 {
        *carried = m.result;
    } else {
        *carried = with_count(&held, held.count() - 1);
        if let Some(r) = m.result {
            containers::give_player(host, player, r);
        }
    }
}

/// HeatingStructureContainerMenu.clickMenuButton, from the server's cursor stack; and the side inventory's scroll.
fn menu_button(host: &mut dyn ServerHost, player: u128, pos: Pos, id: i32, e: &mut Fields, carried: &mut Option<ItemStack>) {
    ensure_ids(&*host);
    let Some(p) = host.player(player) else { return };
    let Some(level) = p.level(&*host) else { return };
    if !is_controller(&block_id(&*host, level, pos)) || p.game_mode(&*host) == Some(GameMode::Spectator) || id < 0 {
        return;
    }
    if e.items.len() < STRUCTURE_SLOTS {
        e.items.resize(STRUCTURE_SLOTS, None);
    }
    let key = (host.dimension().to_owned(), pos);
    if id >= SCROLL_BUTTON {
        let size = gi(&e.nbt, "hz_size", 0).clamp(0, MAX_SLOTS as i32) as usize;
        let offset = (gi(&e.nbt, "hz_offset", 0).max(0) as usize).min(size);
        let new = window_offset(size, (id - SCROLL_BUTTON) as usize);
        let mut store = take_store(&*host, &key, &e.nbt, size);
        let mut items = std::mem::take(&mut e.items);
        let mut inv = Inventory { items: &mut items, store: &mut store, offset, changed: false };
        inv.set_offset(new);
        let changed = inv.changed;
        let _ = inv;
        // The window's new items are no change of the slots (their modules stay).
        let seen: Vec<String> = (0..WINDOW).map(|w| stack_key(&items[WIN0 + w])).collect();
        set_strings(&mut e.nbt, "hz_seen", &seen);
        e.items = items;
        put_store(&*host, key, &mut e.nbt, store, changed);
        si(&mut e.nbt, "hz_offset", new as i32);
        return;
    }
    let mut tank = read_tank(&e.nbt);
    let before = tank.clone();
    let mode = gi(&e.nbt, "hz_mode", 0).rem_euclid(3);
    let tanks: Vec<Pos> = read_shape(&e.nbt).map(|s| s.tanks).unwrap_or_default();
    match id {
        0 => {
            si(&mut e.nbt, "hz_mode", (mode + 1) % 3);
            process_bucket(host, &mut e.items[..], &mut tank, (mode + 1) % 3);
        }
        1 | 2 => {
            // The fuel tanks: fill the cursor's item from the last fuel's tank (else the first with fluid), or empty
            // it into them in order.
            let Some(held) = carried.clone().filter(|c| c.count() > 0) else { return };
            let Some(one) = with_count(&held, 1) else { return };
            let last = read_offset(&e.nbt, "last_fuel").map(|d| add(pos, d)).filter(|l| tanks.contains(l));
            let order: Vec<Pos> = last.into_iter().chain(tanks.iter().copied().filter(|t| Some(*t) != last)).collect();
            let mut moved = None;
            for t in order {
                let Some(cap) = level.capability(host, &FLUIDS, t, None) else { continue };
                let mut ct = CapTank(cap);
                if id == 1 && ct.contents(host).1 <= 0 {
                    continue;
                }
                moved = containers::transfer(host, &one, &mut ct, if id == 1 { Way::FillItem } else { Way::EmptyItem });
                if moved.is_some() {
                    break;
                }
            }
            if let Some(m) = moved {
                cursor_result(host, player, carried, held, m);
            }
        }
        3 => {
            // interactWithStack(the tank, held, EMPTY_ITEM).
            let Some(held) = carried.clone().filter(|c| c.count() > 0) else { return };
            let Some(one) = with_count(&held, 1) else { return };
            let mut access = STankAccess { tank: &mut tank, prefer: None };
            if let Some(m) = containers::transfer(host, &one, &mut access, Way::EmptyItem) {
                cursor_result(host, player, carried, held, m);
            }
        }
        _ => {
            let index = (id - 4) as usize;
            if index >= tank.fluids.len() {
                return;
            }
            match carried.clone().filter(|c| c.count() > 0) {
                Some(held) => {
                    // FluidTransferHelper.fillStack(tank, held, that fluid).
                    let Some(one) = with_count(&held, 1) else { return };
                    let mut access = STankAccess { tank: &mut tank, prefer: Some(index) };
                    if let Some(m) = containers::transfer(host, &one, &mut access, Way::FillItem) {
                        cursor_result(host, player, carried, held, m);
                    }
                }
                None => {
                    // SmelteryTank.moveFluidToBottom.
                    let f = tank.fluids.remove(index);
                    tank.fluids.insert(0, f);
                    process_bucket(host, &mut e.items[..], &mut tank, mode);
                }
            }
        }
    }
    if tank != before {
        write_tank(&mut e.nbt, &tank);
        si(&mut e.nbt, "hz_fluid_dirty", 1);
    }
}

/// MultiModuleContainerMenu.quickMoveStack from the inventory: a fluid container to the bucket slot, then one item into
/// each empty melting slot, the window's and the rest's alike.
/// `e` is the controller's fields, `stack` the clicked slot's stack.
pub(crate) fn quick_move(host: &mut dyn ServerHost, pos: Pos, container_slots: usize, slot: usize, e: &mut Fields, stack_in: &mut Option<ItemStack>) -> Flow {
    ensure_ids(&*host);
    let Some(level) = here(&*host) else { return Flow::Continue };
    if !is_controller(&block_id(&*host, level, pos)) || slot < container_slots {
        return Flow::Continue;
    }
    if e.items.len() < STRUCTURE_SLOTS {
        e.items.resize(STRUCTURE_SLOTS, None);
    }
    // An empty slot: nothing to move.
    let Some(stack) = stack_in.clone() else { return Flow::Continue };
    let mut left = stack.count();
    if containers::maybe_container(&*host, &stack) {
        let max = max_stack(&*host, &stack);
        match &mut e.items[0] {
            None => {
                let n = left.min(max);
                e.items[0] = with_count(&stack, n);
                left -= n;
            }
            Some(h) if h.is_stackable_with(&stack) => {
                let n = left.min(max.saturating_sub(h.count()));
                if n > 0 {
                    let _ = h.grow(n);
                }
                left -= n;
            }
            Some(_) => {}
        }
    }
    let key = (host.dimension().to_owned(), pos);
    let size = gi(&e.nbt, "hz_size", 0).clamp(0, MAX_SLOTS as i32) as usize;
    let offset = (gi(&e.nbt, "hz_offset", 0).max(0) as usize).min(size);
    let mut store = take_store(&*host, &key, &e.nbt, size);
    let mut items = std::mem::take(&mut e.items);
    let mut inv = Inventory { items: &mut items, store: &mut store, offset, changed: false };
    while left > 0 && inv.insert_one(&stack) {
        left -= 1;
    }
    let (changed, free) = (inv.changed, inv.free());
    let _ = inv;
    e.items = items;
    put_store(&*host, key, &mut e.nbt, store, changed);
    si(&mut e.nbt, "hz_free", free as i32);
    if left == stack.count() {
        return Flow::Continue;
    }
    // What is left stays in the slot; nothing left empties it.
    *stack_in = with_count(&stack, left);
    Flow::Cancel
}

/// The menu's slot rules: the duct's filter slot is DuctContainerMenu's SmartItemHandlerSlot over DuctItemHandler, a
/// filled duct container only (isItemValid), one (SingleItemHandler's limit of 1).
pub(crate) fn slot_rules(host: &dyn Host, event: &mut MenuSlotRulesEvent<'_>) {
    if event.menu == DUCT_MENU && matches!(event.container, MenuContainer::Main) && event.slot == 0 {
        ensure_ids(host);
        event.may_place &= duct_fluid(event.stack).is_some();
        event.max_stack = event.max_stack.min(1);
    }
}

// ---- Parts

#[mod_api::sunrise_part]
impl BlockPart for StructurePart {
    fn state_for_placement(&self, level: LevelSide<'_>, event: &mut StateForPlacementEvent<'_>) -> Option<Option<BlockState>> {
        let (LevelSide::Server(level) | LevelSide::Client(level)) = level;
        let block = level.state_block(event.state)?.to_owned();
        if !(is_controller(&block) || is_io(&block)) {
            return None;
        }
        // ControllerBlock / OrientableSmelteryBlock: facing the placer.
        let y = placement_yaw(&event.context).rem_euclid(360.0);
        let facing = if !(45.0..315.0).contains(&y) {
            "north"
        } else if y < 135.0 {
            "east"
        } else if y < 225.0 {
            "south"
        } else {
            "west"
        };
        Some(level.state_with_property(event.state, "facing", facing))
    }

    fn has_analog_output(&self, level: &dyn LevelHost, event: &mut HasAnalogOutputEvent) {
        if level.state_block(event.state).is_some_and(is_controller) {
            event.output = true;
        }
    }

    fn analog_output(&self, _level: &mut dyn LevelRandomHost, event: &mut AnalogOutputEvent) {
        let key = (event.dimension.key().id().to_owned(), [event.pos.x, event.pos.y, event.pos.z]);
        event.output = signal_at(&key);
    }

    fn use_item_on(&self, host: &mut dyn ServerHost, event: &mut UseItemOnEvent<'_>) -> Option<InteractionResult> {
        use_block(host, event.click, true)
    }

    fn use_without_item(&self, host: &mut dyn ServerHost, event: &mut UseWithoutItemEvent<'_>) -> Option<InteractionResult> {
        use_block(host, event.click, false)
    }
}

#[mod_api::sunrise_part]
impl BlockPart for WallPart {
    fn use_item_on(&self, host: &mut dyn ServerHost, event: &mut UseItemOnEvent<'_>) -> Option<InteractionResult> {
        use_block(host, event.click, true)
    }

    fn use_without_item(&self, host: &mut dyn ServerHost, event: &mut UseWithoutItemEvent<'_>) -> Option<InteractionResult> {
        use_block(host, event.click, false)
    }
}

#[mod_api::sunrise_part]
impl BlockEntityPart for StructurePart {
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
        let _t = crate::perf::t(crate::perf::BE_STRUCTURE);
        crate::tick_fields(event, tick);
    }

    fn block_entity_update_tag(&self, event: &mut BlockEntityUpdateTagEvent<'_>) -> Option<Compound> {
        Some(update_tag(&event.entity.nbt))
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        super::structure_screen::submit(host, event);
    }

    fn block_entity_render_bounding_box(&self, _host: &dyn ClientHost, event: &mut BlockEntityRenderBoundingBoxEvent<'_>) {
        super::structure_screen::render_bounds(event.pos, event.entity, event.bounds);
    }

    fn menu_quick_move(&self, event: &mut MenuQuickMoveEvent<'_>) -> Flow {
        // The entity's edits are kept only when the move is handled (an unhandled one's were dropped before too).
        let (container_slots, slot, stack) = (event.container_slots, event.slot, &mut *event.stack);
        crate::scoped_fields(&mut event.subject, false, |host, pos, e| {
            let flow = quick_move(host, pos, container_slots, slot, e, stack);
            (flow, matches!(flow, Flow::Cancel))
        })
        .unwrap_or(Flow::Continue)
    }

    /// Hoppers through the duct's faces: DuctBlockEntity's ITEM_HANDLER, DuctItemHandler (one filled duct container
    /// into the empty slot). The controllers keep their declared faces (the intake in, the window out).
    fn can_place_item(&self, event: &mut CanPlaceItemEvent<'_>) -> Option<bool> {
        (event.entity.id() == DUCT_TYPE).then(|| event.slot == 0 && event.entity.items.first().is_none_or(Option::is_none) && duct_fluid(event.stack).is_some())
    }
}

#[mod_api::sunrise_part]
impl MenuPart for StructurePart {
    /// The controller's buttons (the fuel and tank clicks, the side inventory's scroll), on the server; none without
    /// the block.
    fn menu_button(&self, event: &mut MenuButtonEvent<'_>) {
        let (player, Some(pos), id) = (event.context.player, event.context.pos, event.context.id) else { return };
        let carried = &mut *event.carried;
        crate::scoped_fields(&mut event.subject, false, |host, _, e| (menu_button(host, player, pos, id, e, carried), true));
    }

    /// Every id reaches `menu_button`, as every `clickMenuButton` reached the Java menus: it checks its own.
    fn menu_button_valid(&self, _host: &dyn Host, event: &mut MenuButtonValidEvent<'_>) -> Flow {
        if event.block.is_some() { Flow::Cancel } else { Flow::Continue }
    }

    fn menu_slot_rules(&self, host: &dyn Host, event: &mut MenuSlotRulesEvent<'_>) {
        slot_rules(host, event);
        // The slots the screen hides (the intake, the window past the controller's slots): none on the server's menu.
        if matches!(event.container, MenuContainer::Main) {
            let at = event.context.pos.and_then(|p| Some((here(host)?, p)));
            let layout = at.and_then(|(l, p)| {
                let block = block_id(host, l, p);
                // Only a controller's window depends on its slot count: the (large) saved data is read for those alone.
                let window = is_controller(&block) && (WIN0..WIN0 + WINDOW).contains(&event.slot);
                let size = if window { host.block_entity(l, p)?.data(host).map_or(0, |d| gi(&d, "hz_size", 0).clamp(0, MAX_SLOTS as i32) as usize) } else { 0 };
                super::structure_screen::layout_for(&block, size)
            });
            crate::refuse_hidden_slot(event, layout.as_ref());
        }
    }
}

/// The controllers' and duct's screens, made by the client for each opening of the block's menu (none without it).
fn menu_screen(host: &dyn ClientHost, open: MenuScreenOpen<'_>) -> Option<MenuScreen> {
    let block = open.block?.to_owned();
    let layout = super::structure_screen::layout(host, &block)?;
    let (draw_block, tip_block) = (block.clone(), block.clone());
    Some(
        MenuScreen::new(layout)
            .with_draw(move |host, frame, draw| super::structure_screen::draw(host, &draw_block, frame.data, draw, frame.origin))
            .with_click(move |host, frame, right| super::structure_screen::click(host, &block, frame.data, frame.at, right))
            .with_tooltip(move |host, frame, _slot, lines| {
                crate::tooltip_lines(lines, |out| super::structure_screen::tooltip(host, &tip_block, frame.data, frame.at, out))
            }),
    )
}
