//! SlimeTreeFeature (`tconstruct:slime_tree`) on the server, for the islands' trees: Sunrise has no feature type a
//! mod can register (no `register_feature_type`, #953), so the configs the islands name are here
//! (`worldgen/feature/{earth,sky}_slime_island_tree.json`). The game's own configured features (the clay island's
//! vanilla trees) are placed by `LevelRef::place_feature` (`super::features`). The ender trees (vanilla's
//! `minecraft:tree` with Tinkers' `leave_vines` decorator and `extra_root_variants` root placer) and the blood fungus
//! (`tconstruct:slime_fungus`) need Tinkers' own feature types, so they wait for #953 (README, gaps).

use super::island::Random;
use mod_api::*;
use std::collections::{HashMap, HashSet};

/// SlimeTreeConfig.
pub(crate) struct Config {
    base_height: i32,
    random_height: i32,
    can_double_height: bool,
    has_vines: bool,
    trunk: &'static str,
    leaves: &'static str,
    vines: &'static str,
}

pub(crate) fn config(id: &str) -> Option<Config> {
    Some(match id {
        "tconstruct:earth_slime_island_tree" | "tconstruct:earth_slime_tree" => Config {
            base_height: 4,
            random_height: 3,
            can_double_height: false,
            has_vines: false,
            trunk: "tconstruct:greenheart_log[axis=y]",
            leaves: "tconstruct:earth_slime_leaves",
            vines: "",
        },
        // The planted sky tree (`sky_slime_tree`): no vines.
        "tconstruct:sky_slime_tree" => Config {
            base_height: 5,
            random_height: 4,
            can_double_height: true,
            has_vines: false,
            trunk: "tconstruct:skyroot_log[axis=y]",
            leaves: "tconstruct:sky_slime_leaves",
            vines: "",
        },
        "tconstruct:sky_slime_island_tree" => Config {
            base_height: 5,
            random_height: 4,
            can_double_height: true,
            has_vines: true,
            trunk: "tconstruct:skyroot_log[axis=y]",
            leaves: "tconstruct:sky_slime_leaves",
            vines: "tconstruct:sky_slime_vine",
        },
        _ => return None,
    })
}

/// The writes a tree needs: the server's level (islands) or a level write host (saplings' ticks and bone meal).
pub(crate) trait World {
    fn state(&mut self, p: [i32; 3]) -> Option<BlockState>;
    fn in_tag(&mut self, state: BlockState, tag: &str) -> bool;
    fn is_air(&mut self, state: BlockState) -> bool;
    fn put(&mut self, p: [i32; 3], state: &str) -> bool;
    /// Places configured feature `id` at `p` as `/place feature` does (`super::features`), at the level's next
    /// tick: `then` gets the placing and its result. False where it can't be queued (`then` then never runs).
    fn place_feature(&mut self, _id: &str, _p: [i32; 3], _then: super::features::Then) -> bool {
        false
    }
}

/// The server's level.
pub(crate) struct ServerWorld<'a> {
    pub(crate) host: &'a mut dyn ServerHost,
    pub(crate) level: LevelRef,
}
impl World for ServerWorld<'_> {
    fn state(&mut self, p: [i32; 3]) -> Option<BlockState> {
        self.level.block_state(&*self.host, p)
    }
    fn in_tag(&mut self, state: BlockState, tag: &str) -> bool {
        self.host.block_in_tag(state.block(), tag).unwrap_or(false)
    }
    fn is_air(&mut self, state: BlockState) -> bool {
        state.block().id(&*self.host).is_some_and(|b| b.ends_with(":air"))
    }
    fn put(&mut self, p: [i32; 3], state: &str) -> bool {
        super::island::set(self.host, self.level, p, state)
    }
    fn place_feature(&mut self, id: &str, p: [i32; 3], then: super::features::Then) -> bool {
        super::features::queue(&self.host.dimension(), id, p, then);
        true
    }
}

/// A level write host (random ticks, bone meal).
pub(crate) struct WriteWorld<'a>(pub(crate) &'a mut dyn LevelWriteHost);
impl World for WriteWorld<'_> {
    fn state(&mut self, p: [i32; 3]) -> Option<BlockState> {
        self.0.block_state(p)
    }
    fn in_tag(&mut self, state: BlockState, tag: &str) -> bool {
        self.0.block_in_tag(state.block(), tag).unwrap_or(false)
    }
    fn is_air(&mut self, state: BlockState) -> bool {
        self.0.state_block(state).is_some_and(|n| n.ends_with(":air"))
    }
    fn put(&mut self, p: [i32; 3], state: &str) -> bool {
        match self.0.block_state_id(state) {
            Some(s) => self.0.set_block(p, s, BlockUpdateFlags::CLIENTS),
            None => false,
        }
    }
}

pub(crate) struct Tree<'a> {
    w: &'a mut dyn World,
    logs: HashSet<[i32; 3]>,
    leaves: HashSet<[i32; 3]>,
}

impl Tree<'_> {
    /// isAirOrLeavesAt.
    fn free(&mut self, p: [i32; 3]) -> bool {
        match self.w.state(p) {
            Some(s) => self.w.is_air(s) || self.w.in_tag(s, "minecraft:leaves"),
            None => false,
        }
    }
    fn log(&mut self, p: [i32; 3], c: &Config) {
        if self.free(p) && self.w.put(p, c.trunk) {
            self.logs.insert(p);
        }
    }
    fn leaf(&mut self, p: [i32; 3], c: &Config) {
        if self.free(p) && self.w.put(p, &format!("{}[distance=7,persistent=false]", c.leaves)) {
            self.leaves.insert(p);
        }
    }
    fn air(&mut self, p: [i32; 3]) {
        if self.free(p) {
            self.w.put(p, "minecraft:air");
            self.leaves.remove(&p);
        }
    }
    fn vine(&mut self, p: [i32; 3], state: &str) {
        if self.free(p) {
            self.w.put(p, state);
            self.leaves.remove(&p);
        }
    }
}

/// getRandomizedVine: no sides, then 1-3 random sides (of north, east, south, west) set.
fn vine_state(random: &mut Random, c: &Config, up: bool) -> String {
    let names = ["north", "east", "south", "west"];
    let mut on = [false; 4];
    for _ in 0..random.next_int(3) + 1 {
        on[random.next_int(4) as usize] = true;
    }
    let mut props: Vec<String> = names.iter().zip(on).map(|(n, b)| format!("{n}={b}")).collect();
    props.push(format!("up={up}"));
    props.push("stage=middle".to_owned());
    format!("{}[{}]", c.vines, props.join(","))
}

/// SlimeTreeFeature.place: true when the tree was placed. A configured feature the game knows (the clay island's
/// vanilla trees) is placed by the game a tick later: `then` (if given) gets that placing, decides and counts (Java
/// counts a tree only when `place` succeeds), and this returns false.
pub(crate) fn place(w: &mut dyn World, id: &str, pos: [i32; 3], random: &mut Random, then: Option<super::features::Then>) -> bool {
    let Some(c) = config(id) else {
        crate::smeltery::util::tlog(&format!("island tree {id}: the game's configured feature"));
        let then = then.unwrap_or_else(|| {
            Box::new(|place: &mut dyn FnMut() -> bool| {
                place();
            })
        });
        w.place_feature(id, pos, then);
        return false;
    };
    let mut height = random.next_int(c.random_height) + c.base_height;
    if c.can_double_height && random.next_int(10) == 0 {
        height *= 2;
    }
    let below = [pos[0], pos[1] - 1, pos[2]];
    let below_state = w.state(below);
    let soil = below_state.is_some_and(|s| w.in_tag(s, "tconstruct:slimy_soil"));
    if pos[1] + height + 1 > 320 || !soil {
        return false;
    }
    // setDirtAt: dirt under the trunk becomes plain dirt.
    if below_state.is_some_and(|s| w.in_tag(s, "minecraft:dirt")) {
        w.put(below, "minecraft:dirt");
    }
    let mut t = Tree { w, logs: HashSet::new(), leaves: HashSet::new() };
    let at = |p: [i32; 3], x: i32, y: i32, z: i32| [p[0] + x, p[1] + y, p[2] + z];
    // placeTrunk.
    for y in 0..height {
        t.log(at(pos, 0, y, 0), &c);
    }
    // placeCanopy.
    let mut p = at(pos, 0, height, 0);
    for i in 0..4 {
        let range = i + 1;
        let layer = at(p, 0, -i, 0);
        for x in -range..=range {
            for z in -range..=range {
                if x.abs() + z.abs() <= range {
                    t.leaf(at(layer, x, 0, z), &c);
                }
            }
        }
    }
    p = at(p, 0, -3, 0);
    for (x, z) in [(4, 0), (-4, 0), (0, 4), (0, -4)] {
        t.air(at(p, x, 0, z));
    }
    if c.has_vines {
        for (x, z) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
            t.air(at(p, x, 0, z));
        }
    }
    p = at(p, 0, -1, 0);
    for (x, z) in [(3, 0), (-3, 0), (0, -3), (0, 3)] {
        t.leaf(at(p, x, 0, z), &c);
    }
    if !c.has_vines {
        for (x, z) in [(1, 1), (-3, 0), (-1, 1), (-1, -1)] {
            t.leaf(at(p, x, 0, z), &c);
        }
    } else {
        p = at(p, 0, -1, 0);
        for (x, z) in [(3, 0), (-3, 0), (0, -3), (0, 3)] {
            let v = vine_state(random, &c, true);
            t.vine(at(p, x, 0, z), &v);
        }
        for (x, z) in [(2, 2), (2, -2), (-2, 2), (-2, -2)] {
            let v = vine_state(random, &c, false);
            t.vine(at(p, x, 1, z), &v.replace("up=false", "up=true"));
            t.vine(at(p, x, 0, z), &v);
        }
    }
    // updateLeaves: each leaf's distance from the logs (1-6; 7 stays and decays, as Java's).
    let mut dist: HashMap<[i32; 3], i32> = HashMap::new();
    let dirs = [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]];
    let mut frontier: Vec<[i32; 3]> = Vec::new();
    for l in &t.logs {
        for d in dirs {
            let n = at(*l, d[0], d[1], d[2]);
            if t.leaves.contains(&n) && !dist.contains_key(&n) {
                dist.insert(n, 1);
                frontier.push(n);
            }
        }
    }
    for distance in 1..6 {
        let mut next = Vec::new();
        for q in &frontier {
            for d in dirs {
                let n = at(*q, d[0], d[1], d[2]);
                if t.leaves.contains(&n) && !dist.contains_key(&n) {
                    dist.insert(n, distance + 1);
                    next.push(n);
                }
            }
        }
        frontier = next;
    }
    for (p, d) in dist {
        t.w.put(p, &format!("{}[distance={d},persistent=false]", c.leaves));
    }
    !t.logs.is_empty()
}
