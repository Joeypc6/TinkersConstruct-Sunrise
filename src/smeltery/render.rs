//! What the client draws: the fluid inside the melter and the tanks (TankInventoryBlockEntityRenderer,
//! TankBlockEntityRenderer, Mantle's FluidRenderer with FluidTankAnimated's easing; the lanterns' model fluid), and
//! ControllerBlock.animateTick's smoke and flame on an active melter's or heater's front. The fork has no per-block
//! animateTick, so the blocks drawn are remembered (`submit`) and the client's tick rolls for them as often as the
//! level's random display ticks reach a block.

use super::util::*;
use super::{HEATER, LANTERNS, MELTER, tank_capacity};
use crate::fluids;
use mod_api::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub(crate) struct Keys {
    smoke: ParticleKey,
    flame: ParticleKey,
}

static KEYS: OnceLock<Keys> = OnceLock::new();

pub(crate) fn register(r: &mut dyn Registrar) {
    let smoke = r.particle_key("minecraft:smoke");
    let flame = r.particle_key("minecraft:flame");
    let _ = KEYS.set(Keys { smoke, flame });
}

#[derive(Default)]
pub(crate) struct Client {
    /// FluidTankAnimated.renderOffset and the amount last seen, by place.
    offsets: HashMap<Pos, (f32, i32)>,
    /// The melters and heaters drawn lately, with the client tick they were seen.
    seen: HashMap<Pos, u64>,
    ticks: u64,
}

static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

fn with_client<T>(f: impl FnOnce(&mut Client) -> T) -> T {
    let mut g = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Client::default))
}

/// The still and flowing sprites and the tint (0xRRGGBB) of a fluid (IClientFluidTypeExtensions).
pub(crate) fn sprites(fluid: &str) -> (String, String, u32) {
    match fluid {
        "minecraft:water" => ("block/water_still".into(), "block/water_flow".into(), 0x3F_76E4),
        "minecraft:lava" => ("block/lava_still".into(), "block/lava_flow".into(), 0xFF_FFFF),
        _ => match fluids::table().textures.get(fluid) {
            Some((s, f, c)) => (s.clone(), f.clone(), c & 0xFF_FFFF),
            None => ("block/water_still".into(), "block/water_flow".into(), 0xFF_FFFF),
        },
    }
}

/// FluidType.getLightLevel (`fluids::light`: another mod's fluid's from the game).
pub(crate) fn fluid_light(host: &dyn Host, fluid: &str) -> u32 {
    u32::from(fluids::light(host, fluid))
}

/// The model's `fluid` box in pixels: the melter's upper half, a tank's whole inside.
fn fluid_box(block: &str) -> ([f32; 3], [f32; 3]) {
    if block == MELTER {
        ([0.08, 8.08, 0.08], [15.92, 15.92, 15.92])
    } else if block == super::casting::CASTING_TANK {
        // block_fluids/seared_casting_tank: the lower 11 pixels.
        ([0.08, 0.08, 0.08], [15.92, 10.92, 15.92])
    } else {
        ([0.08, 0.08, 0.08], [15.92, 15.92, 15.92])
    }
}

/// Whether `block` has a client tick: the melter and heater, for animateTick's flames and smoke.
pub(crate) fn client_ticks(block: &str) -> bool {
    block == MELTER || block == HEATER
}

/// A melter's or heater's client tick (each loaded copy, drawn or not): kept as seen for `particles`.
pub(crate) fn client_tick(host: &dyn ClientHost, pos: Pos) {
    let Some(level) = here(host) else { return };
    if client_ticks(&block_id(host, level, pos)) {
        with_client(|c| {
            let t = c.ticks;
            c.seen.insert(pos, t);
        });
    }
}

pub(crate) fn submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let block = event.block;
    let pos = event.pos;
    let Some(capacity) = tank_capacity(block) else { return };
    let (fluid, amount) = tank_of(&event.entity.nbt);
    let draw = &mut *event.draw;
    // FluidTankAnimated: a change eases in (RenderUtils.getFluidAmount); the offset is the client's own.
    let partial = draw.partial();
    let offset = with_client(|c| {
        let e = c.offsets.entry(pos).or_insert((0.0, amount));
        if e.1 != amount {
            e.0 += (amount - e.1) as f32;
            e.1 = amount;
        }
        if e.0.abs() > 1.2 {
            e.0 -= (e.0 / 12.0 + 0.1) * partial;
        } else {
            e.0 = 0.0;
        }
        e.0
    });
    let Some(fluid) = fluid else { return };
    let shown = (amount as f32 - offset).clamp(0.0, capacity as f32);
    if shown <= 0.0 {
        return;
    }
    let lantern = LANTERNS.contains(&block);
    let (mut from, mut to) = fluid_box(block);
    let mut top = from[1] + (to[1] - from[1]) * shown / capacity as f32;
    if lantern {
        // The lantern's model fluid (template/lantern.json, increments 10; hanging one pixel up), in whole tenths
        // (TankModel: clamp(amount * 10 / capacity, 1, 10)), the still sprite on every face.
        let hanging = host.block_state_string(event.state).is_some_and(|s| s.contains("hanging=true"));
        let lift = if hanging { 1.0 } else { 0.0 };
        from = [5.05, 1.0 + lift, 5.05];
        to = [10.95, 6.0 + lift, 10.95];
        let k = (amount * 10 / capacity).clamp(1, 10) as f32;
        top = from[1] + k * (to[1] - from[1]) / 10.0;
    }
    let (still, flowing, tint) = sprites(&fluid);
    let flowing = if lantern { still.clone() } else { flowing };
    let light = draw.light();
    // FluidRenderer: the block light raised to the fluid's own.
    let block_light = ((light >> 8) & 0xFF).max(fluid_light(host, &fluid).min(15) * 16);
    draw.set_light((light & 0xFF) | (block_light << 8) | (light & 0xFFFF_0000));
    let [x0, y0, z0] = from.map(|v| v / 16.0);
    let (x1, y1, z1) = (to[0] / 16.0, top / 16.0, to[2] / 16.0);
    draw.quad(&still, [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]], [from[0], from[2], to[0], to[2]], tint);
    // The sides in the flowing sprite, half of it as Mantle's FluidCuboid takes it.
    let (v0, v1) = ((16.0 - top) * 0.5, (16.0 - from[1]) * 0.5);
    let (ux0, ux1, uz0, uz1) = (from[0] * 0.5, to[0] * 0.5, from[2] * 0.5, to[2] * 0.5);
    draw.quad(&flowing, [[x1, y1, z0], [x1, y0, z0], [x0, y0, z0], [x0, y1, z0]], [ux0, v0, ux1, v1], tint);
    draw.quad(&flowing, [[x0, y1, z1], [x0, y0, z1], [x1, y0, z1], [x1, y1, z1]], [ux0, v0, ux1, v1], tint);
    draw.quad(&flowing, [[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]], [uz0, v0, uz1, v1], tint);
    draw.quad(&flowing, [[x1, y1, z1], [x1, y0, z1], [x1, y0, z0], [x1, y1, z0]], [uz0, v0, uz1, v1], tint);
    if lantern || block != MELTER {
        // A tank's bottom shows through its glass.
        draw.quad(&still, [[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]], [from[0], from[2], to[0], to[2]], tint);
    }
    draw.set_light(light);
}

fn next(seed: &mut u64) -> f64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 11) as f64 / (1u64 << 53) as f64
}

/// ControllerBlock.animateTick for the melters and heaters near the player: while `active`, smoke and a flame on the
/// `facing` face, 0.52 out, up to 0.3 to either side, `rand * 6/16` up a melter and `rand * 14/16` up a heater. As
/// often as ClientLevel.animateTick's 667 tries in 16 blocks and 667 in 32 reach a block each tick.
pub(crate) fn particles(host: &mut dyn ClientHost) {
    let now = with_client(|c| {
        c.ticks += 1;
        let now = c.ticks;
        c.seen.retain(|_, seen| now.saturating_sub(*seen) < 40);
        if now % 200 == 0 {
            let seen = &c.seen;
            c.offsets.retain(|p, _| seen.contains_key(p));
        }
        now
    });
    let Some(keys) = KEYS.get() else { return };
    let Some(player) = host.local_player().and_then(|p| p.entity().position(&*host)) else { return };
    let Some(level) = here(&*host) else { return };
    let seen: Vec<Pos> = with_client(|c| c.seen.keys().copied().collect());
    let mut seed = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) ^ now) | 1;
    for p in seen {
        let d = (0..3).map(|i| (f64::from(p[i]) + 0.5 - player[i]).abs()).fold(0.0, f64::max);
        let chance = if d <= 16.0 { 667.0 / 35937.0 + 667.0 / 274625.0 } else if d <= 32.0 { 667.0 / 274625.0 } else { 0.0 };
        if next(&mut seed) >= chance {
            continue;
        }
        if level.block_property(&*host, p, "active").as_deref() != Some("true") {
            continue;
        }
        let block = block_id(&*host, level, p);
        let height = if block == MELTER { 6.0 / 16.0 } else if block == HEATER { 14.0 / 16.0 } else { continue };
        let facing = level.block_property(&*host, p, "facing").unwrap_or_default();
        let (x, z) = (f64::from(p[0]) + 0.5, f64::from(p[2]) + 0.5);
        let y = f64::from(p[1]) + next(&mut seed) * height;
        let side = next(&mut seed) * 0.6 - 0.3;
        let (px, pz) = match facing.as_str() {
            "west" => (x - 0.52, z + side),
            "east" => (x + 0.52, z + side),
            "south" => (x + side, z + 0.52),
            _ => (x + side, z - 0.52),
        };
        host.add_particle(&ParticleOptions::simple(keys.smoke), [px, y, pz], [0.0; 3]);
        host.add_particle(&ParticleOptions::simple(keys.flame), [px, y, pz], [0.0; 3]);
    }
}
