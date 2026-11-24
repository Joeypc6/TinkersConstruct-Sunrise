//! Configured features placed from code, for the islands' trees that aren't SlimeTreeFeature (the clay island's
//! vanilla oak, birch, spruce, acacia and jungle trees): `LevelRef::place_feature`, what `/place feature` runs.
//!
//! An island marker queues its feature; the level's next `level_tick_post` (`island::level_tick`, the level in
//! place) places it in queue order. A feature whose 3x3 chunks aren't all loaded yet (`NotLoaded`) waits for a
//! later tick of that level, up to `RETRY_TICKS`, as `/place feature` would refuse it until then.

use mod_api::*;
use std::sync::Mutex;

/// What runs when a queued feature's turn comes: it is handed the placing (true when the feature was placed), and
/// decides whether to place and what a success counts for (the islands' "at most 3 trees", counted on success only).
pub(crate) type Then = Box<dyn FnOnce(&mut dyn FnMut() -> bool) + Send>;

/// Ticks of its level a feature waits for its chunks before it is dropped.
const RETRY_TICKS: u32 = 1200;

struct Queued {
    dimension: String,
    feature: String,
    pos: [i32; 3],
    then: Then,
    waited: u32,
}

static QUEUE: Mutex<Vec<Queued>> = Mutex::new(Vec::new());

/// At that level's next tick, in queue order, hands `then` the placing of configured feature `feature` at `pos` in
/// `dimension`.
pub(crate) fn queue(dimension: &str, feature: &str, pos: [i32; 3], then: Then) {
    if let Ok(mut q) = QUEUE.lock() {
        q.push(Queued { dimension: dimension.to_owned(), feature: feature.to_owned(), pos, then, waited: 0 });
    }
}

/// The ticked level's queued features, placed in queue order.
pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let mine: Vec<Queued> = match QUEUE.lock() {
        Ok(mut q) if q.iter().any(|f| f.dimension == dim) => {
            let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *q).into_iter().partition(|f| f.dimension == dim);
            *q = rest;
            here
        }
        _ => return,
    };
    // The queue's lock is free here, and each `then` runs in queue order (the islands' markers' order). A feature
    // that waits for its chunks keeps every later one of this level waiting too, so the order holds.
    let mut wait: Vec<Queued> = Vec::new();
    for mut f in mine {
        if !wait.is_empty() {
            wait.push(f);
            continue;
        }
        let at = f.pos;
        // `/place feature`'s check (PlaceCommand.checkLoaded), read without placing: a block read in each chunk of
        // the 3x3 around `at` answers only when that chunk is loaded.
        let loaded = (-1..=1).all(|dx| (-1..=1).all(|dz| level.block_state(&*host, [at[0] + dx * 16, at[1], at[2] + dz * 16]).is_some()));
        if !loaded {
            f.waited += 1;
            if f.waited < RETRY_TICKS {
                wait.push(f);
            } else {
                crate::smeltery::util::tlog(&format!("feature {} at {at:?} in {dim}: chunks never loaded, dropped", f.feature));
            }
            continue;
        }
        // Loaded: the placing itself happens inside `then`, which decides (the cap) and counts.
        let feature = f.feature;
        let mut place = || {
            let result = level.place_feature(host, &feature, at);
            crate::smeltery::util::tlog(&format!("feature {feature} at {at:?} in {dim}: {result:?}"));
            result == Ok(true)
        };
        (f.then)(&mut place);
    }
    if !wait.is_empty() {
        if let Ok(mut q) = QUEUE.lock() {
            wait.extend(std::mem::take(&mut *q));
            *q = wait;
        }
    }
}
