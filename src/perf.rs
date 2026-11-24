//! Debug-only hook timing (`config/tconstruct-debug.flag`): each hook's time is summed with `Instant` and logged as
//! microseconds per server tick every 200 overworld ticks (`PERF ...` lines in tconstruct-debug.log). Off: one cached
//! bool read per hook, no clock reads.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

pub(crate) const NAMES: [&str; 22] = [
    "level_tick", "island", "smeltery_level", "ranged", "gadgets_level", "sapling", "player_tick", "block_changed",
    "be_melter", "be_casting", "be_structure", "be_station", "inventory_tick", "item_attributes", "living_events",
    "other_hooks", "st_prep", "st_remember", "st_settle", "st_write", "cast_recipes", "capabilities",
];
pub(crate) const LEVEL: usize = 0;
pub(crate) const ISLAND: usize = 1;
pub(crate) const SMELTERY_LEVEL: usize = 2;
pub(crate) const RANGED: usize = 3;
pub(crate) const GADGETS_LEVEL: usize = 4;
pub(crate) const SAPLING: usize = 5;
pub(crate) const PLAYER: usize = 6;
pub(crate) const BLOCK_CHANGED: usize = 7;
pub(crate) const BE_MELTER: usize = 8;
pub(crate) const BE_CASTING: usize = 9;
pub(crate) const BE_STRUCTURE: usize = 10;
pub(crate) const BE_STATION: usize = 11;
pub(crate) const INVENTORY: usize = 12;
pub(crate) const ATTRIBUTES: usize = 13;
pub(crate) const LIVING: usize = 14;
pub(crate) const OTHER: usize = 15;
pub(crate) const ST_PREP: usize = 16;
pub(crate) const ST_REMEMBER: usize = 17;
pub(crate) const ST_SETTLE: usize = 18;
pub(crate) const ST_WRITE: usize = 19;
pub(crate) const CAST_RECIPES: usize = 20;
pub(crate) const CAPS: usize = 21;

static NS: [AtomicU64; 22] = [const { AtomicU64::new(0) }; 22];
static CALLS: [AtomicU64; 22] = [const { AtomicU64::new(0) }; 22];
static TICKS: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Timer(usize, Option<Instant>);

#[inline]
pub(crate) fn t(i: usize) -> Timer {
    Timer(i, crate::smeltery::util::debug_on().then(Instant::now))
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(s) = self.1 {
            NS[self.0].fetch_add(s.elapsed().as_nanos() as u64, Relaxed);
            CALLS[self.0].fetch_add(1, Relaxed);
        }
    }
}

/// Called once per server tick (the overworld's level tick); every 200 ticks logs the sums and starts again.
pub(crate) fn server_tick() {
    if !crate::smeltery::util::debug_on() {
        return;
    }
    let n = TICKS.fetch_add(1, Relaxed) + 1;
    if n < 200 {
        return;
    }
    TICKS.store(0, Relaxed);
    let mut line = String::from("PERF us/tick (calls/tick):");
    let mut total = 0u64;
    for (i, name) in NAMES.iter().enumerate() {
        let ns = NS[i].swap(0, Relaxed);
        let calls = CALLS[i].swap(0, Relaxed);
        if (i < ST_PREP || i == CAPS) && i != ISLAND && i != SMELTERY_LEVEL && i != RANGED && i != GADGETS_LEVEL && i != SAPLING {
            total += ns;
        }
        if calls > 0 {
            line += &format!(" {name}={:.1}({:.1})", ns as f64 / 1000.0 / n as f64, calls as f64 / n as f64);
        }
    }
    line += &format!(" TOTAL={:.1} island_pending={}", total as f64 / 1000.0 / n as f64, crate::world::island::pending_len());
    crate::smeltery::util::tlog(&line);
}
