//! Slime islands (IslandStructure, IslandPlacement, IslandPiece) as the fork's structure type `tconstruct:island`
//! (514ecf9's `register_structure_type`). The six `worldgen/structure/*.json` (`"type": "tconstruct:island"`) and the
//! four structure sets are the game's: it picks the start chunks by the seed, checks the biome and calls the piece in
//! each chunk its box covers.
//!
//! The worker does what Java's generatePieces does (template, rotation, height, mirror and tree, in Java's order of
//! draws from the chunk's random). The templates are server data (`structure_template` is a server host call, as AE2's
//! meteorites found: no worker context reads a template), so each covered chunk is noted and on the server's next
//! ticks gets its part of the template (BlockIgnoreProcessor.STRUCTURE_BLOCK: structure blocks are not placed), then
//! the data markers in it (IslandPiece.handleDataMarker): slime vines, slime trees (at most 3 an island) and grasses.

use crate::smeltery::util::{js, tlog};
use mod_api::*;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// Chunks placed a server tick.
const CHUNKS_PER_TICK: usize = 2;

/// Java's `java.util.Random` LCG, for the markers' draws (a piece's RandomSource is the chunk's in Java).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Random(u64);
impl Random {
    const MULTIPLIER: u64 = 0x5_DEEC_E66D;
    const MASK: u64 = (1 << 48) - 1;
    pub(crate) fn new(seed: i64) -> Self {
        Self((seed as u64 ^ Self::MULTIPLIER) & Self::MASK)
    }
    fn next(&mut self, bits: u32) -> i32 {
        self.0 = (self.0.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB)) & Self::MASK;
        (self.0 >> (48 - bits)) as i32
    }
    pub(crate) fn next_int(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
        if bound & -bound == bound {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            if bits - val + (bound - 1) >= 0 {
                return val;
            }
        }
    }
    pub(crate) fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }
}

/// IslandStructure (the type).
pub(crate) struct Island;

/// One island: what IslandPiece keeps (template, position, rotation, mirror, tree) and its structure's vines and
/// grasses (Java reads them from the structure; the piece saves them here).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Start {
    template: String,
    pos: [i32; 3],
    rotation: u8,
    mirror: u8,
    tree: String,
    vines: String,
    grasses: Vec<(String, i32)>,
}

#[derive(Debug)]
pub(crate) struct Piece(Arc<Start>);

#[derive(Default)]
pub(crate) struct Pending {
    chunks: Vec<([i32; 2], Arc<Start>)>,
    /// The world whose saved list was read (`data/tconstruct_island_pending.txt`), and the list as last written.
    loaded: Option<std::path::PathBuf>,
    written: String,
    /// The list as last written, by chunk and start (its `Arc`): the text is built again only when this differs.
    written_ids: Vec<([i32; 2], usize)>,
    templates: HashMap<Arc<Start>, Arc<StructureTemplateSnapshot>>,
    trees: HashMap<Arc<Start>, u32>,
}
static PENDING: LazyLock<Mutex<Pending>> = LazyLock::new(Default::default);
pub(crate) fn pending_len() -> usize {
    pending(|p| p.chunks.len())
}
fn pending<R>(f: impl FnOnce(&mut Pending) -> R) -> R {
    f(&mut PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

pub(crate) fn register(r: &mut dyn Registrar) {
    r.register_structure_type("island", Island);
}

fn text(t: Option<&Tag>) -> String {
    match t {
        Some(Tag::String(s)) => s.to_string_lossy().to_string(),
        _ => String::new(),
    }
}

fn int(t: Option<&Tag>) -> i32 {
    match t {
        Some(Tag::Int(i)) => *i,
        Some(Tag::Byte(i)) => i32::from(*i),
        Some(Tag::Short(i)) => i32::from(*i),
        Some(Tag::Long(i)) => *i as i32,
        _ => 0,
    }
}

/// A `SimpleWeightedRandomList` codec's entries (`[{data, weight}]`).
fn weighted(c: &Compound, key: &str) -> Vec<(String, i32)> {
    match c.get(&js(key)) {
        Some(Tag::List(l)) => l
            .iter()
            .filter_map(|e| match e {
                Tag::Compound(e) => Some((text(e.get(&js("data"))), int(e.get(&js("weight"))).max(1))),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// WeightedRandom.getRandomItem: `nextInt(total)`, then the entry it falls in.
fn pick(list: &[(String, i32)], random: &mut dyn RandomSource) -> Option<String> {
    let total: i32 = list.iter().map(|e| e.1).sum();
    if total <= 0 {
        return None;
    }
    let mut n = random.next_int_bound(total);
    for (v, w) in list {
        n -= w;
        if n < 0 {
            return Some(v.clone());
        }
    }
    None
}

/// The templates' sizes (`size` in each `.nbt`): the same for all five island families.
fn template_size(template: &str) -> [i32; 3] {
    match template.rsplit('/').next().unwrap_or("") {
        "0x1x0" => [21, 10, 21],
        "2x2x4" | "4x1x6" => [23, 12, 25],
        "8x1x11" => [29, 13, 32],
        _ => [32, 13, 32],
    }
}

/// StructureTemplate.transform: mirror, then rotate about the origin (pivot 0).
fn transform(p: [i32; 3], rotation: u8, mirror: u8) -> [i32; 3] {
    let (mut x, y, mut z) = (p[0], p[1], p[2]);
    match mirror {
        1 => z = -z,
        2 => x = -x,
        _ => {}
    }
    match rotation {
        1 => [-z, y, x],
        2 => [-x, y, -z],
        3 => [z, y, -x],
        _ => [x, y, z],
    }
}

impl StructureType for Island {
    /// Structure.onTopOfChunkCenter(WORLD_SURFACE) with IslandStructure.generatePieces.
    fn find_generation_point(&self, context: &mut dyn GenerationContext) -> Option<FoundStructure> {
        let config = context.config();
        let Tag::Compound(c) = &*config else { return None };
        let chunk = context.chunk();
        let (mx, mz) = (chunk.x * 16 + 7, chunk.z * 16 + 7);
        let stub_y = context.base_height(mx, mz, HeightmapType::WorldSurfaceWg);
        if !context.column_has_valid_biome(mx, mz, stub_y, stub_y) {
            return None;
        }
        let templates = weighted(c, "templates");
        let template = pick(&templates, context.random())?;
        // Rotation.getRandom.
        let rotation = context.random().next_int_bound(4) as u8;
        let height = match text(c.get(&js("placement"))).as_str() {
            "sky" => {
                let (dx, dz) = match rotation {
                    1 => (-5, 5),
                    2 => (-5, -5),
                    3 => (5, -5),
                    _ => (5, 5),
                };
                let h = |ctx: &mut dyn GenerationContext, x, z| ctx.base_height(x, z, HeightmapType::WorldSurfaceWg);
                let low = h(context, mx, mz).min(h(context, mx, mz + dz)).min(h(context, mx + dx, mz)).min(h(context, mx + dx, mz + dz));
                let gen_depth = context.max_y() - context.min_y() + 1;
                (low + 60 + context.random().next_int_bound(50)).min(gen_depth - 20)
            }
            _ => (context.sea_level() - 7).max(0),
        };
        // Util.getRandom(Mirror.values()).
        let mirror = context.random().next_int_bound(3) as u8;
        let tree = pick(&weighted(c, "trees"), context.random()).unwrap_or_default();
        let start = Start {
            template,
            pos: [mx, height, mz],
            rotation,
            mirror,
            tree,
            vines: text(c.get(&js("vines"))),
            grasses: weighted(c, "grasses"),
        };
        Some(FoundStructure { position: [mx, stub_y, mz], pieces: vec![Arc::new(Piece(Arc::new(start)))], pool_pieces: Vec::new() })
    }

    fn load_piece(&self, _metadata: &dyn BlockMetadata, tag: &Compound) -> Result<Arc<dyn StructurePiece>, StructureError> {
        let pos = match tag.get(&js("Pos")) {
            Some(Tag::IntArray(v)) if v.len() == 3 => [v[0], v[1], v[2]],
            _ => return Err(StructureError("island piece: no saved position".into())),
        };
        let grasses = text(tag.get(&js("Grasses")))
            .split(',')
            .filter_map(|e| e.split_once('*').map(|(id, w)| (id.to_owned(), w.parse().unwrap_or(1))))
            .collect();
        Ok(Arc::new(Piece(Arc::new(Start {
            template: text(tag.get(&js("Template"))),
            pos,
            rotation: int(tag.get(&js("Rot"))) as u8,
            mirror: int(tag.get(&js("Mi"))) as u8,
            tree: text(tag.get(&js("Tree"))),
            vines: text(tag.get(&js("Vines"))),
            grasses,
        }))))
    }
}

impl StructurePiece for Piece {
    /// TemplateStructurePiece's box: the template's size, mirrored and rotated, at the position.
    fn bounding_box(&self) -> BoundingBox {
        let s = &self.0;
        let size = template_size(&s.template);
        let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
        for corner in [[0, 0, 0], [size[0] - 1, size[1] - 1, size[2] - 1], [size[0] - 1, 0, 0], [0, 0, size[2] - 1]] {
            let t = transform(corner, s.rotation, s.mirror);
            for i in 0..3 {
                lo[i] = lo[i].min(t[i] + s.pos[i]);
                hi[i] = hi[i].max(t[i] + s.pos[i]);
            }
        }
        lo[1] = s.pos[1];
        hi[1] = s.pos[1] + size[1] - 1;
        BoundingBox::new(lo, hi)
    }

    /// The chunk is noted; its part is placed on the server's next ticks.
    fn post_process(&self, context: &mut dyn PieceContext) -> Result<(), StructureError> {
        let c = context.chunk();
        pending(|p| {
            if p.chunks.len() < 8192 {
                p.chunks.push(([c.x, c.z], self.0.clone()));
            }
        });
        Ok(())
    }

    fn add_additional_save_data(&self, tag: &mut Compound) {
        let s = &self.0;
        tag.insert(js("Template"), Tag::String(js(&s.template)));
        tag.insert(js("Pos"), Tag::IntArray(s.pos.to_vec()));
        tag.insert(js("Rot"), Tag::Int(i32::from(s.rotation)));
        tag.insert(js("Mi"), Tag::Int(i32::from(s.mirror)));
        tag.insert(js("Tree"), Tag::String(js(&s.tree)));
        tag.insert(js("Vines"), Tag::String(js(&s.vines)));
        let grasses: Vec<String> = s.grasses.iter().map(|(id, w)| format!("{id}*{w}")).collect();
        tag.insert(js("Grasses"), Tag::String(js(&grasses.join(","))));
    }
}

// ---- The server's half

fn rotation_of(r: u8) -> StructureRotation {
    match r {
        1 => StructureRotation::Clockwise90,
        2 => StructureRotation::Clockwise180,
        3 => StructureRotation::Counterclockwise90,
        _ => StructureRotation::None,
    }
}

fn mirror_of(m: u8) -> StructureMirror {
    match m {
        1 => StructureMirror::LeftRight,
        2 => StructureMirror::FrontBack,
        _ => StructureMirror::None,
    }
}

/// The dimension an island's structure set places it in (the pieces know no dimension): the ender islands the End's,
/// the blood islands the nether's, the rest the overworld's.
fn dimension_of(s: &Start) -> &'static str {
    if s.template.contains("/ender/") {
        "minecraft:the_end"
    } else if s.template.contains("/blood/") {
        "minecraft:the_nether"
    } else {
        "minecraft:overworld"
    }
}

fn line_of(chunk: [i32; 2], s: &Start) -> String {
    let grasses: Vec<String> = s.grasses.iter().map(|(id, w)| format!("{id}*{w}")).collect();
    format!(
        "{} {}|{}|{} {} {}|{} {}|{}|{}|{}",
        chunk[0], chunk[1], s.template, s.pos[0], s.pos[1], s.pos[2], s.rotation, s.mirror, s.tree, s.vines, grasses.join(",")
    )
}

fn parse_line(l: &str) -> Option<([i32; 2], Start)> {
    let f: Vec<&str> = l.split('|').collect();
    if f.len() != 7 {
        return None;
    }
    let nums = |t: &str| t.split(' ').map(|n| n.parse::<i32>().ok()).collect::<Option<Vec<i32>>>();
    let (c, p, rm) = (nums(f[0])?, nums(f[2])?, nums(f[3])?);
    if c.len() != 2 || p.len() != 3 || rm.len() != 2 {
        return None;
    }
    let grasses = f[6].split(',').filter_map(|e| e.split_once('*').map(|(id, w)| (id.to_owned(), w.parse().unwrap_or(1)))).collect();
    Some(([c[0], c[1]], Start {
        template: f[1].to_owned(),
        pos: [p[0], p[1], p[2]],
        rotation: rm[0] as u8,
        mirror: rm[1] as u8,
        tree: f[4].to_owned(),
        vines: f[5].to_owned(),
        grasses,
    }))
}

/// The noted chunks survive a restart: read once per world, written when they change (as AE2's meteorites).
fn persist(host: &dyn ServerHost) {
    let Some(dir) = host.world_dir() else { return };
    let file = dir.join("data").join("tconstruct_island_pending.txt");
    pending(|p| {
        if p.loaded.as_ref() != Some(&dir) {
            p.loaded = Some(dir.clone());
            p.templates.clear();
            p.trees.clear();
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let saved: Vec<([i32; 2], Arc<Start>)> = text.lines().filter_map(parse_line).map(|(c, s)| (c, Arc::new(s))).collect();
            for (c, s) in saved {
                if !p.chunks.iter().any(|(c2, s2)| *c2 == c && **s2 == *s) {
                    p.chunks.push((c, s));
                }
            }
            p.written = text;
            p.written_ids.clear();
        }
        if p.chunks.len() == p.written_ids.len() && p.chunks.iter().zip(&p.written_ids).all(|((c, s), (c2, a))| c == c2 && Arc::as_ptr(s) as usize == *a) {
            return;
        }
        p.written_ids = p.chunks.iter().map(|(c, s)| (*c, Arc::as_ptr(s) as usize)).collect();
        let text: String = p.chunks.iter().map(|(c, s)| line_of(*c, s) + "\n").collect();
        if text != p.written {
            let _ = std::fs::create_dir_all(dir.join("data"));
            if std::fs::write(&file, &text).is_ok() {
                p.written = text;
            }
        }
    });
}

/// Each level's tick: up to two noted chunks of that level get their part of their island.
pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    // Features the markers queued (the clay island's trees) go first, every tick of their level.
    super::features::level_tick(host, level);
    // Nothing noted: the world's saved list is looked for once a second (a world opened since), not every tick.
    static TICKS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = TICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n % 20 != 0 && pending(|p| p.chunks.is_empty() && p.loaded.is_some()) {
        return;
    }
    persist(&*host);
    // Tests only (debug on): `config/tconstruct-island-hold.flag` holds the noted chunks unplaced, so a restart test
    // can quit with chunks pending (tests/island_restart.sh).
    if pending(|p| p.chunks.is_empty()) {
        return;
    }
    if crate::smeltery::util::debug_on() && std::path::Path::new("config/tconstruct-island-hold.flag").exists() {
        return;
    }
    let dim = host.dimension();
    let all: Vec<([i32; 2], Arc<Start>)> = pending(|p| std::mem::take(&mut p.chunks));
    let (work, mut other): (Vec<_>, Vec<_>) = all.into_iter().partition(|(_, s)| dimension_of(s) == dim);
    if work.is_empty() {
        pending(|p| {
            other.extend(std::mem::take(&mut p.chunks));
            p.chunks = other;
        });
        return;
    }
    let mut done = 0;
    let mut wait = Vec::new();
    for (chunk, start) in work {
        if done >= CHUNKS_PER_TICK || level.block_state(&*host, [chunk[0] * 16, start.pos[1], chunk[1] * 16]).is_none() {
            wait.push((chunk, start));
            continue;
        }
        let known = pending(|p| p.templates.get(&start).cloned());
        let template = match known {
            Some(t) => t,
            None => {
                let Some(t) = host.structure_template(&start.template, start.pos, rotation_of(start.rotation), mirror_of(start.mirror)) else {
                    tlog(&format!("island: no template {}", start.template));
                    continue;
                };
                let t = Arc::new(t);
                tlog(&format!("island {} at {:?} (rotation {}, mirror {}), tree {}", start.template, start.pos, start.rotation, start.mirror, start.tree));
                pending(|p| {
                    if p.templates.len() > 256 {
                        p.templates.clear();
                    }
                    p.templates.insert(start.clone(), t.clone());
                });
                t
            }
        };
        place(host, level, chunk, &start, &template);
        done += 1;
    }
    pending(|p| {
        wait.extend(other);
        wait.extend(std::mem::take(&mut p.chunks));
        p.chunks = wait;
    });
    persist(&*host);
}

pub(crate) fn name(host: &dyn Host, state: Option<BlockState>) -> String {
    state.map_or_else(|| "minecraft:void_air".to_owned(), |s| s.block().id(host).unwrap_or_default().to_owned())
}

pub(crate) fn set(host: &mut dyn ServerHost, level: LevelRef, p: [i32; 3], state: &str) -> bool {
    match host.block_state_id(state) {
        Some(s) => level.set_block(host, p, s, BlockUpdateFlags::CLIENTS).unwrap_or(false),
        None => false,
    }
}

/// TemplateStructurePiece.postProcess clipped to the chunk, then the chunk's data markers.
fn place(host: &mut dyn ServerHost, level: LevelRef, chunk: [i32; 2], start: &Arc<Start>, template: &StructureTemplateSnapshot) {
    let inside = |p: [i32; 3]| p[0] >> 4 == chunk[0] && p[2] >> 4 == chunk[1];
    let mut markers = Vec::new();
    let structure_block = host.block_state_id("minecraft:structure_block").map(|s| s.block());
    for b in template.blocks.iter().filter(|b| inside(b.pos)) {
        if Some(b.state.block()) == structure_block {
            if let Some(nbt) = &b.nbt {
                if text(nbt.get(&js("mode"))) == "DATA" {
                    markers.push((b.pos, text(nbt.get(&js("metadata")))));
                }
            }
            continue;
        }
        let _ = level.set_block(host, b.pos, b.state, BlockUpdateFlags::CLIENTS);
    }
    let mut random = Random::new((i64::from(start.pos[0]) << 32 ^ i64::from(start.pos[2])) ^ (i64::from(chunk[0]) * 341_873_128_712 + i64::from(chunk[1]) * 132_897_987_541));
    for (pos, function) in markers {
        match function.as_str() {
            "tconstruct:slime_vine" => {
                if !start.vines.is_empty() && random.next_boolean() {
                    place_vine(host, level, pos, &mut random, &start.vines);
                }
            }
            "tconstruct:slime_tree" => {
                // IslandPiece: `numberOfTreesPlaced < 3 && nextBoolean()`, counted only when `place` succeeds. The
                // game's own features (clay island) are placed a tick later, in marker order: their success is
                // counted then, and the cap checked again then.
                let placed = pending(|p| p.trees.get(start).copied().unwrap_or(0));
                if placed < 3 && random.next_boolean() {
                    let s = start.clone();
                    let then: super::features::Then = Box::new(move |place: &mut dyn FnMut() -> bool| {
                        if pending(|p| p.trees.get(&s).copied().unwrap_or(0)) < 3 && place() {
                            pending(|p| *p.trees.entry(s).or_default() += 1);
                        }
                    });
                    if super::tree::place(&mut super::tree::ServerWorld { host: &mut *host, level }, &start.tree, pos, &mut random, Some(then)) {
                        pending(|p| *p.trees.entry(start.clone()).or_default() += 1);
                    }
                }
            }
            "tconstruct:slime_tall_grass" => {
                if random.next_boolean() {
                    let total: i32 = start.grasses.iter().map(|g| g.1).sum();
                    let mut n = random.next_int(total.max(1));
                    let plant = start.grasses.iter().find(|g| {
                        n -= g.1;
                        n < 0
                    });
                    if let Some((id, _)) = plant {
                        // BushBlock.canSurvive: on dirt or grass (Tinkers' slimy soil included).
                        let below = level.block_state(&*host, [pos[0], pos[1] - 1, pos[2]]);
                        let ok = below.is_some_and(|b| {
                            host.block_in_tag(b.block(), "minecraft:dirt").unwrap_or(false) || host.block_in_tag(b.block(), "tconstruct:slimy_soil").unwrap_or(false)
                        });
                        if ok {
                            let id = if id == "minecraft:grass" { "minecraft:short_grass" } else { id.as_str() };
                            set(host, level, pos, id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// IslandPiece.placeVine: the vine on each side with a full face beside it (not down), then grown down 0-7 times.
fn place_vine(host: &mut dyn ServerHost, level: LevelRef, pos: [i32; 3], random: &mut Random, vine: &str) {
    let sides = [("up", [0, 1, 0]), ("north", [0, 0, -1]), ("south", [0, 0, 1]), ("west", [-1, 0, 0]), ("east", [1, 0, 0])];
    let mut on = Vec::new();
    for (side, d) in sides {
        let n = level.block_state(&*host, [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]]);
        let id = name(&*host, n);
        if n.is_some() && !id.ends_with("air") && !host.block_in_tag(n.unwrap().block(), "minecraft:replaceable").unwrap_or(true) {
            on.push(side);
        }
    }
    if on.is_empty() {
        return;
    }
    let sides_of = |up: bool| {
        let mut v: Vec<String> = on.iter().filter(|s| **s != "up").map(|s| format!("{s}=true")).collect();
        if up && on.contains(&"up") {
            v.push("up=true".to_owned());
        }
        v
    };
    let state = |stage: &str, up: bool| {
        let mut v = sides_of(up);
        v.push(format!("stage={stage}"));
        format!("{vine}[{}]", v.join(","))
    };
    set(host, level, pos, &state("start", true));
    // SlimeVineBlock.grow: down a block at a time while it has a side to hang from.
    if sides_of(false).is_empty() {
        return;
    }
    let size = random.next_int(8);
    let mut p = pos;
    for i in 0..size {
        let below = [p[0], p[1] - 1, p[2]];
        if !name(&*host, level.block_state(&*host, below)).ends_with(":air") {
            break;
        }
        set(host, level, below, &state(if i + 1 == size { "end" } else { "middle" }, false));
        p = below;
    }
}
