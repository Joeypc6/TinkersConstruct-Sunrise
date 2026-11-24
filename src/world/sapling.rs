//! Slime saplings (SlimeSaplingBlock over SaplingBlock, SlimeTree's grower). Their blocks copy `oak_sapling`, whose
//! own tick and bone meal would grow an oak, so the random tick, the bone meal target, success and growth are taken
//! for them: SaplingBlock.randomTick (light 9+ above, 1/7), isBonemealSuccess (0.45), advanceTree (stage 0 to 1, then
//! the tree). Earth and sky saplings grow SlimeTreeFeature's planted trees (`tree.rs`). The ender, blood and ichor
//! saplings keep their template's (the oak sapling's) random tick and bone meal, and `block_grow_feature` swaps the oak
//! for Tinkers' feature (SlimeTree: the tall ender tree 85 %, else the short one; the fungi as the game's huge fungus on
//! the soil below, see `make_fork_content.py`'s `slime_trees`).

use super::island::Random;
use super::tree::{self, WriteWorld};
use mod_api::*;

fn sapling(name: &str) -> Option<&'static str> {
    Some(match name {
        "tconstruct:earth_slime_sapling" => "tconstruct:earth_slime_tree",
        "tconstruct:sky_slime_sapling" => "tconstruct:sky_slime_tree",
        "tconstruct:ender_slime_sapling" | "tconstruct:blood_slime_sapling" | "tconstruct:ichor_slime_sapling" => "",
        _ => return None,
    })
}

fn kind(level: &dyn LevelHost, state: BlockState) -> Option<&'static str> {
    level.state_block(state).and_then(sapling)
}

fn p(pos: BlockPos) -> [i32; 3] {
    [pos.x, pos.y, pos.z]
}

/// SaplingBlock.advanceTree.
fn advance(level: &mut dyn LevelWriteHost, pos: [i32; 3], state: BlockState, tree_id: &str) {
    let text = level.state_string(state).unwrap_or_default();
    if text.contains("stage=0") {
        if let Some(next) = level.block_state_id(&text.replace("stage=0", "stage=1")) {
            level.set_block(pos, next, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS);
        }
        return;
    }
    if tree_id.is_empty() {
        return;
    }
    let Some(seed) = level.with_random(|r| r.next_long()) else { return };
    let mut random = Random::new(seed);
    // TreeGrower: the sapling goes, the tree is tried, the sapling is put back if it doesn't fit.
    let air = level.block_state_id("minecraft:air");
    if let Some(air) = air {
        level.set_block(pos, air, BlockUpdateFlags::CLIENTS);
    }
    if !tree::place(&mut WriteWorld(&mut *level), tree_id, pos, &mut random, None) {
        level.set_block(pos, state, BlockUpdateFlags::CLIENTS);
    }
}

/// The part every slime sapling gets.
pub(crate) struct SlimeSapling;

pub(crate) fn register(r: &mut dyn Registrar) {
    for t in ["earth", "sky", "ender", "blood", "ichor"] {
        let k = r.block_key(&format!("tconstruct:{t}_slime_sapling"));
        r.block_part(k, SlimeSapling);
    }
}

#[mod_api::sunrise_part]
impl BlockPart for SlimeSapling {
    fn random_tick(&self, level: &mut dyn LevelWriteHost, event: &mut RandomTickEvent) -> Flow {
        let Some(tree_id) = kind(&*level, event.state) else { return Flow::Continue };
        if tree_id.is_empty() {
            return Flow::Continue;
        }
        let pos = p(event.pos);
        let lit = level.raw_brightness([pos[0], pos[1] + 1, pos[2]], 0) >= 9;
        let roll = level.with_random(|r| r.next_int_bound(7));
        if lit && roll == Some(0) {
            advance(level, pos, event.state, tree_id);
        }
        Flow::Cancel
    }

    fn is_valid_bonemeal_target(&self, level: LevelSide, event: &mut IsValidBonemealTargetEvent) -> Option<bool> {
        let lh: &dyn LevelHost = match &level {
            LevelSide::Server(l) => &**l,
            LevelSide::Client(l) => &**l,
        };
        if kind(lh, event.state) == Some("") {
            return None;
        }
        crate::smeltery::util::tlog(&format!("sapling bone meal target at {:?}", p(event.pos)));
        Some(true)
    }

    fn is_bonemeal_success(&self, level: &mut dyn LevelRandomHost, event: &mut IsBonemealSuccessEvent) {
        if let Some(roll) = level.with_random(|r| r.next_float()) {
            event.success = roll < 0.45;
            crate::smeltery::util::tlog(&format!("sapling bone meal success {}", event.success));
        }
    }

    fn perform_bonemeal(&self, level: &mut dyn LevelWriteHost, event: &mut PerformBonemealEvent) {
        let k = kind(&*level, event.state);
        crate::smeltery::util::tlog(&format!("sapling bone meal at {:?}: {:?} ({:?})", p(event.pos), k, level.state_block(event.state)));
        if let Some(tree_id) = k {
            advance(level, p(event.pos), event.state, tree_id);
        }
    }
}

/// `block_grow_feature`: an ender, blood or ichor slime sapling growing through its template's growth grows Tinkers'
/// feature instead of the oak.
pub(crate) fn grow_feature(level: &mut dyn LevelRandomHost, event: &mut BlockGrowFeatureEvent<'_>) {
    let pos = p(event.pos);
    let Some(name) = level.block_state(pos).and_then(|s| level.state_block(s)).map(str::to_owned) else { return };
    let id = match name.as_str() {
        "tconstruct:ender_slime_sapling" => {
            let f = level.with_random(|r| r.next_float());
            snapshot_roots(&*level, event.dimension.key().id(), pos);
            if f.unwrap_or(0.0) < 0.85 { "tconstruct:ender_slime_tree_tall".to_owned() } else { "tconstruct:ender_slime_tree".to_owned() }
        }
        "tconstruct:blood_slime_sapling" | "tconstruct:ichor_slime_sapling" => {
            let fungus = if name.contains("blood") { "blood_slime_fungus" } else { "ichor_slime_fungus" };
            let soil = level.block_state([pos[0], pos[1] - 1, pos[2]]).and_then(|s| level.state_block(s)).unwrap_or("").to_owned();
            format!("tconstruct:{fungus}_on_{}", soil.split(':').nth(1).unwrap_or(""))
        }
        _ => return,
    };
    crate::smeltery::util::tlog(&format!("sapling {name} at {pos:?} grows {id} (was {:?})", event.feature));
    *event.feature = Some(id);
}

// ---- The ender tree's own parts the game's tree types lack: ExtraRootVariantPlacer's root variants and
// LeaveVineDecorator's ender slime vines, done on the level tick after the game placed the tree.

/// (dimension, sapling position, the root-variant blocks under it before the growth: position, variant root).
static GROWN: std::sync::Mutex<Vec<(String, [i32; 3], Vec<([i32; 3], &'static str)>)>> = std::sync::Mutex::new(Vec::new());

/// ExtraRootVariantPlacer's variants: the blocks a root grows into, and the root it becomes there.
fn root_variant(block: &str) -> Option<&'static str> {
    Some(match block {
        "tconstruct:sky_enderbark_roots" | "tconstruct:sky_congealed_slime" => "tconstruct:sky_enderbark_roots",
        "tconstruct:ichor_enderbark_roots" | "tconstruct:ichor_congealed_slime" => "tconstruct:ichor_enderbark_roots",
        "tconstruct:ender_enderbark_roots" | "tconstruct:ender_congealed_slime" => "tconstruct:ender_enderbark_roots",
        _ => return None,
    })
}

fn snapshot_roots(level: &dyn LevelHost, dim: &str, pos: [i32; 3]) {
    let mut found = Vec::new();
    for dy in -16..=3 {
        for dx in -8..=8 {
            for dz in -8..=8 {
                let q = [pos[0] + dx, pos[1] + dy, pos[2] + dz];
                if let Some(v) = level.block_state(q).and_then(|s| level.state_block(s)).and_then(root_variant) {
                    found.push((q, v));
                }
            }
        }
    }
    let mut g = GROWN.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() < 64 {
        g.push((dim.to_owned(), pos, found));
    }
}

/// The level tick after an ender tree grew: the roots that grew into a variant's block become that variant, and the
/// leaves get hanging ender slime vines (1/8 a side into air, then down up to 4 more).
pub(crate) fn after_growth(host: &mut dyn ServerHost) {
    let list = std::mem::take(&mut *GROWN.lock().unwrap_or_else(|e| e.into_inner()));
    for (dim, pos, roots) in list {
        let Some(level) = host.level(&dim) else { continue };
        let name = |host: &dyn ServerHost, q: [i32; 3]| level.block_state(host, q).and_then(|s| host.block_state_string(s)).unwrap_or_default();
        // The trunk starts over the roots (the mangrove root placer's trunk offset, more on the tall tree): a growth
        // that placed nothing (the sapling stays, and bone meal tries again) has neither here.
        if !(0..=8).any(|dy| {
            let n = name(&*host, [pos[0], pos[1] + dy, pos[2]]);
            n.starts_with("tconstruct:enderbark_log") || n.contains("enderbark_roots")
        }) {
            crate::smeltery::util::tlog(&format!("ender tree at {pos:?}: no trunk (the growth placed nothing)"));
            continue;
        }
        let mut variants = 0;
        for (q, v) in roots {
            let n = name(&*host, q);
            if n.starts_with("tconstruct:enderbark_roots") || n.starts_with("tconstruct:earth_enderbark_roots") {
                if let Some(s) = host.block_state_id(v) {
                    let _ = level.set_block(host, q, s, BlockUpdateFlags::CLIENTS);
                    variants += 1;
                }
            }
        }
        let mut random = Random::new(i64::from(pos[0]) * 3_129_871 ^ i64::from(pos[2]) * 116_129_781 ^ i64::from(pos[1]));
        let mut vines = 0;
        let sides: [([i32; 3], &str); 4] = [([0, 0, -1], "south"), ([1, 0, 0], "west"), ([0, 0, 1], "north"), ([-1, 0, 0], "east")];
        for dy in 0..=16 {
            for dx in -6..=6 {
                for dz in -6..=6 {
                    let q = [pos[0] + dx, pos[1] + dy, pos[2] + dz];
                    if !name(&*host, q).starts_with("tconstruct:ender_slime_leaves") {
                        continue;
                    }
                    for (d, face) in sides {
                        if random.next_int(8) != 0 {
                            continue;
                        }
                        let mut t = [q[0] + d[0], q[1], q[2] + d[2]];
                        let Some(vine) = host.block_state_id(&format!("tconstruct:ender_slime_vine[{face}=true]")) else { continue };
                        for i in 0..5 {
                            if !name(&*host, t).ends_with(":air") {
                                break;
                            }
                            let _ = level.set_block(host, t, vine, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS);
                            vines += 1;
                            t[1] -= 1;
                            let _ = i;
                        }
                    }
                }
            }
        }
        crate::smeltery::util::tlog(&format!("ender tree at {pos:?}: {variants} root variants, {vines} vines"));
    }
}
