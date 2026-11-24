//! What the client draws of the casting blocks: FaucetBlockEntityRenderer (the spout, the drop and the stream into the
//! block below by its `faucet_fluid` height), CastingBlockEntityRenderer (the fluid film or basin fill, fading out over
//! the last quarter of the cooling, the cast and the result at their places and sizes), ChannelBlockEntityRenderer
//! (centre, sides, edges, the way down) and the casting tank (TankInventoryBlockEntityRenderer: the smeltery's tank
//! renderer, and its two items). Mantle's FluidCuboids are drawn as quads: still faces a whole sprite, flowing ones
//! half of it, the frame turned by `facing` as RenderingHelper.applyRotation.
//!
//! The client-only animateTick and clientTick effects (the faucet's red dust while it waits for redstone, the cooling
//! casting's smoke) roll for the blocks drawn lately, as `render::particles` does.

use super::casting::{self, CASTING_TANK, CHANNEL_SIDES, OFF, POURING, dir_index, flow_index, is_basin, is_casting, is_channel, is_faucet, is_table, step};
use super::render;
use super::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub(crate) struct Keys {
    smoke: ParticleKey,
    dust: ParticleKey,
}

static KEYS: OnceLock<Keys> = OnceLock::new();

pub(crate) fn register(r: &mut dyn Registrar) {
    let smoke = r.particle_key("minecraft:smoke");
    let dust = r.particle_key("minecraft:dust");
    let _ = KEYS.set(Keys { smoke, dust });
}

#[derive(Default)]
pub(crate) struct Client {
    /// Faucets waiting for redstone (pouring with no fluid: POWERED) drawn lately, with their facing.
    waiting: HashMap<Pos, (u64, usize)>,
    /// Full castings drawn lately.
    cooling: HashMap<Pos, u64>,
    ticks: u64,
}

static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

fn with_client<T>(f: impl FnOnce(&mut Client) -> T) -> T {
    let mut g = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Client::default))
}

const N: Option<bool> = None;
const S: Option<bool> = Some(false);
const F: Option<bool> = Some(true);

/// A FluidCuboid: the box `from`..`to` (pixels) with the faces given (down, up, north, south, west, east: none, still
/// or flowing), in the fluid's sprites and tint.
pub(crate) fn cuboid(draw: &mut dyn BlockEntityDrawContext, fluid: &str, from: [f32; 3], to: [f32; 3], faces: [Option<bool>; 6]) {
    let (still, flowing, tint) = render::sprites(fluid);
    let [x0, y0, z0] = from.map(|v| v / 16.0);
    let [x1, y1, z1] = to.map(|v| v / 16.0);
    for (i, face) in faces.iter().enumerate() {
        let Some(flows) = face else { continue };
        let (sprite, k) = if *flows { (&flowing, 0.5) } else { (&still, 1.0) };
        let (corners, uv) = match i {
            0 => ([[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]], [from[0], from[2], to[0], to[2]]),
            1 => ([[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]], [from[0], from[2], to[0], to[2]]),
            2 => ([[x1, y1, z0], [x1, y0, z0], [x0, y0, z0], [x0, y1, z0]], [16.0 - to[0], 16.0 - to[1], 16.0 - from[0], 16.0 - from[1]]),
            3 => ([[x0, y1, z1], [x0, y0, z1], [x1, y0, z1], [x1, y1, z1]], [from[0], 16.0 - to[1], to[0], 16.0 - from[1]]),
            4 => ([[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]], [from[2], 16.0 - to[1], to[2], 16.0 - from[1]]),
            _ => ([[x1, y1, z1], [x1, y0, z1], [x1, y0, z0], [x1, y1, z0]], [16.0 - to[2], 16.0 - to[1], 16.0 - from[2], 16.0 - from[1]]),
        };
        // FluidRenderer: coordinates wrapped into the sprite, flowing faces at half scale.
        let uv = uv.map(|v| v.rem_euclid(16.0001) * k);
        draw.quad(sprite, corners, uv, tint);
    }
}

/// The block light raised to the fluid's own (FluidRenderer); the old light back.
pub(crate) fn lit(host: &dyn Host, draw: &mut dyn BlockEntityDrawContext, fluid: &str) -> u32 {
    let light = draw.light();
    let block_light = ((light >> 8) & 0xFF).max(render::fluid_light(host, fluid).min(15) * 16);
    draw.set_light((light & 0xFF) | (block_light << 8) | (light & 0xFFFF_0000));
    light
}

/// RenderingHelper.applyRotation: a horizontal facing other than south turns the frame about the block's centre.
pub(crate) fn face_frame(draw: &mut dyn BlockEntityDrawContext, facing: usize) {
    let steps = match facing {
        4 => 1.0,
        2 => 2.0,
        5 => 3.0,
        _ => 0.0,
    };
    if steps != 0.0 {
        draw.rotate_about([0.0, 1.0, 0.0], -90.0 * steps, [0.5, 0.5, 0.5]);
    }
}

fn facing_of(host: &dyn ClientHost, state: BlockState) -> usize {
    host.block_state_string(state)
        .and_then(|s| s.split_once('[').map(|(_, p)| p.trim_end_matches(']').to_owned()))
        .and_then(|p| p.split(',').find_map(|kv| kv.strip_prefix("facing=").map(str::to_owned)))
        .and_then(|f| dir_index(&f))
        .unwrap_or(2)
}

/// RenderingHelper.renderFaucetFluids: the stream into the block below (`mantle/model/faucet_fluid`: tables 15,
/// basins 4, channels 8, else 1), its side cuboid under a horizontal faucet, its centre one under a downward one.
fn stream(host: &dyn ClientHost, draw: &mut dyn BlockEntityDrawContext, pos: Pos, fluid: &str, vertical: bool) {
    let block = here(host).map(|l| block_id(host, l, below(pos))).unwrap_or_default();
    let (bottom, faces) = if is_table(&block) {
        (15.0, [N, N, F, F, F, F])
    } else if is_basin(&block) {
        (4.0, [N, N, F, F, F, F])
    } else if is_channel(&block) {
        (8.0, [S, N, F, F, F, F])
    } else {
        (1.0, [S, N, F, F, F, F])
    };
    let to = if vertical { [10.0, 16.0, 10.0] } else { [10.0, 16.0, 8.0] };
    cuboid(draw, fluid, [6.0, bottom - 16.0, 6.0], [to[0], to[1] - 16.0, to[2]], faces);
}

fn faucet(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let f = casting::read_faucet(&event.entity.nbt);
    let facing = facing_of(host, event.state);
    let pos = event.pos;
    let Some(fluid) = f.render.filter(|_| f.state == POURING) else { return };
    let draw = &mut *event.draw;
    let light = lit(host, draw, &fluid);
    draw.push();
    face_frame(draw, facing);
    if facing == 0 {
        // templates/faucet_up.
        cuboid(draw, &fluid, [6.0, 0.0, 6.0], [10.0, 16.0, 10.0], [N, S, F, F, F, F]);
    } else {
        // templates/faucet: the spout, then the drop.
        cuboid(draw, &fluid, [6.0, 6.0, 0.0], [10.0, 9.0, 6.0], [N, F, S, N, N, N]);
        cuboid(draw, &fluid, [6.0, 0.0, 6.0], [10.0, 9.0, 8.0], [N, F, F, F, F, F]);
    }
    // A guide scene's faucet: no world block below it, so the scene names how far the stream falls (`hz_scene_bottom`).
    match gi(&event.entity.nbt, "hz_scene_bottom", 0) {
        b if b > 0 => {
            let to = if facing == 0 { [10.0, 16.0, 10.0] } else { [10.0, 16.0, 8.0] };
            cuboid(draw, &fluid, [6.0, b as f32 - 16.0, 6.0], [to[0], to[1] - 16.0, to[2]], [N, N, F, F, F, F]);
        }
        _ => stream(host, draw, pos, &fluid, facing == 0),
    }
    draw.pop();
    draw.set_light(light);
}

fn stacks(host: &dyn ClientHost, nbt: &Compound, key: &str) -> Vec<Option<ItemStack>> {
    match nbt.get(&js(key)) {
        Some(Tag::List(l)) => l
            .iter()
            .map(|t| match t {
                Tag::Compound(c) if !c.is_empty() => ItemStack::from_nbt(host, c),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// RenderingHelper.renderItem: at `center` (pixels), `size` pixels big, turned about x then y, in `display`.
pub(crate) fn item_at(draw: &mut dyn BlockEntityDrawContext, stack: &ItemStack, center: [f32; 3], size: f32, rot: [f32; 2], display: ItemDisplayContext, seed: i32) {
    draw.push();
    draw.translate(center.map(|v| v / 16.0));
    draw.scale([size / 16.0; 3]);
    if rot[0] != 0.0 {
        draw.rotate_about([1.0, 0.0, 0.0], rot[0], [0.0, 0.0, 0.0]);
    }
    if rot[1] != 0.0 {
        draw.rotate_about([0.0, 1.0, 0.0], rot[1], [0.0, 0.0, 0.0]);
    }
    draw.item(stack, display, None, seed);
    draw.pop();
}

fn table(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let block = event.block;
    let nbt = &event.entity.nbt;
    let facing = facing_of(host, event.state);
    let t = casting::cast_tank(nbt);
    let cooling = gi(nbt, "hz_cooling", -1);
    let draw = &mut *event.draw;
    // The client's cooling timer (CastingBlockEntity.clientTick counts it up while full).
    let timer = if cooling >= 0 { (draw.game_time() - i64::from(gi(nbt, "hz_start", 0))).clamp(0, i64::from(cooling.max(0))) as i32 } else { 0 };
    // CastingBlockEntityRenderer: the result fades in as it cools; the fluid fades out over the last quarter.
    let (mut item_opacity, mut fluid_opacity) = (0, 255);
    if timer > 0 && cooling > 0 {
        let o = 1020 * timer / cooling;
        item_opacity = o / 4;
        if o > 765 {
            fluid_opacity = 1020 - o;
        }
    }
    let basin = is_basin(block);
    draw.push();
    face_frame(draw, facing);
    if let Some(fluid) = t.fluid.as_deref().filter(|_| t.capacity > 0) {
        let (from, to, faces) = if basin { ([2.1, 4.0, 2.1], [13.9, 15.9, 13.9], [N, S, S, S, S, S]) } else { ([1.0, 15.0, 1.0], [15.0, 15.9, 15.0], [N, S, N, N, N, N]) };
        let top = if t.amount >= t.capacity { to[1] } else { from[1] + (to[1] - from[1]) * t.amount as f32 / t.capacity as f32 };
        if fluid_opacity >= 255 {
            let light = lit(host, draw, fluid);
            cuboid(draw, fluid, from, [to[0], top, to[2]], faces);
            draw.set_light(light);
        } else if fluid_opacity > 0 {
            // RenderUtils.renderTransparentCuboid: the fluid's tint with its alpha scaled (the quads carry no alpha, so
            // the fork's fluid box draws it).
            let (_, _, tint) = render::sprites(fluid);
            let alpha = (fluid_opacity.clamp(0, 255) as u32) << 24;
            if let Some(k) = host.fluid_key(fluid) {
                let stack = FluidStack::new(k, t.amount.max(1) as u32);
                let b = [from[0] / 16.0, from[1] / 16.0, from[2] / 16.0, to[0] / 16.0, top / 16.0, to[2] / 16.0];
                let _ = draw.fluid_box(&stack, b, FluidBoxOptions::new(false, 0.0).with_argb(alpha | (tint & 0xFF_FFFF)));
            }
        }
    }
    let items = stacks(host, nbt, "hz_items");
    let (center, sizes, rot, display) =
        if basin { ([8.0, 10.0, 8.0], [11.95, 12.0], [0.0, 0.0], ItemDisplayContext::None) } else { ([8.0, 15.5, 8.0], [14.0, 14.1], [270.0, 180.0], ItemDisplayContext::Fixed) };
    if let Some(Some(s)) = items.first() {
        item_at(draw, s, center, sizes[0], rot, display, 0);
    }
    match items.get(1) {
        Some(Some(s)) => item_at(draw, s, center, sizes[1], rot, display, 1),
        // The cooling result (getRecipeOutput): items draw opaque here, so it shows from half way (README, gaps).
        _ if item_opacity > 127 => {
            if let Some(Tag::Compound(c)) = nbt.get(&js("hz_preview"))
                && let Some(p) = ItemStack::from_nbt(host, c)
            {
                item_at(draw, &p, center, sizes[1], rot, display, 1);
            }
        }
        _ => {}
    }
    draw.pop();
}

fn channel(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let c = casting::read_channel(&event.entity.nbt);
    let Some(fluid) = c.fluid.clone() else { return };
    let Some(level) = here(host) else { return };
    let pos = event.pos;
    let block = event.block;
    let flowing = c.flow.map(|v| v > 0);
    let draw = &mut *event.draw;
    let light = lit(host, draw, &fluid);
    let mut center_flow: Option<Option<usize>> = None;
    for side in CHANNEL_SIDES {
        let value = level.block_property(host, pos, casting::DIR_NAMES[side]).unwrap_or_default();
        if value != "in" && value != "out" {
            continue;
        }
        draw.push();
        // The side cuboids are in the south side's frame.
        face_frame(draw, side);
        if flowing[flow_index(side)] {
            if value == "out" {
                cuboid(draw, &fluid, [6.0, 8.0, 10.0], [10.0, 8.0, 16.0], [N, F, N, N, N, N]);
                center_flow = Some(match center_flow {
                    None => Some(side),
                    Some(Some(s)) if s == side => Some(s),
                    _ => None,
                });
            } else {
                // side.in: the up face's flowing sprite turned 180 (drawn the same way: a quad's UVs are not turned).
                cuboid(draw, &fluid, [6.0, 8.0, 10.0], [10.0, 8.0, 16.0], [N, F, N, N, N, N]);
            }
            if block_id(host, level, step(pos, side)) != block {
                cuboid(draw, &fluid, [6.0, 6.0, 16.0], [10.0, 8.0, 16.0], [N, N, N, F, N, N]);
            }
        } else {
            cuboid(draw, &fluid, [6.0, 6.0, 10.0], [10.0, 8.0, 10.0], [N, N, N, S, N, N]);
        }
        draw.pop();
    }
    match center_flow {
        Some(Some(side)) => {
            draw.push();
            face_frame(draw, side);
            cuboid(draw, &fluid, [6.0, 6.0, 6.0], [10.0, 8.0, 10.0], [F, F, N, N, N, N]);
            draw.pop();
        }
        _ => cuboid(draw, &fluid, [6.0, 6.0, 6.0], [10.0, 8.0, 10.0], [S, S, N, N, N, N]),
    }
    if flowing[0] && level.block_property(host, pos, "down").as_deref() == Some("true") {
        cuboid(draw, &fluid, [6.0, 0.0, 6.0], [10.0, 6.0, 10.0], [N, N, F, F, F, F]);
        stream(host, draw, pos, &fluid, true);
    }
    draw.set_light(light);
}

fn casting_tank(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    render::submit(host, event);
    let facing = facing_of(host, event.state);
    let items = stacks(host, &event.entity.nbt, "hz_items");
    let draw = &mut *event.draw;
    draw.push();
    face_frame(draw, facing);
    for (i, s) in items.iter().enumerate() {
        if let Some(s) = s {
            item_at(draw, s, [8.0, 15.5, 8.0], if i == 0 { 14.0 } else { 14.1 }, [270.0, 180.0], ItemDisplayContext::Fixed, i as i32);
        }
    }
    draw.pop();
}

pub(crate) fn submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    ensure_ids(host);
    let block = event.block;
    if is_faucet(block) {
        faucet(host, event);
    } else if is_casting(block) {
        table(host, event);
    } else if is_channel(block) {
        channel(host, event);
    } else if block == CASTING_TANK {
        casting_tank(host, event);
    }
}

/// Whether `block` has a client tick: the faucets (their red dust while waiting) and the casting tables and basins
/// (their smoke while cooling).
pub(crate) fn client_ticks(block: &str) -> bool {
    is_faucet(block) || is_casting(block)
}

/// A faucet's or casting block's client tick (each loaded copy, drawn or not): what `particles` draws from.
pub(crate) fn client_tick(event: &mut BlockEntityClientTickEvent<'_>) {
    // The copy's position, state and merged saved fields, lent by the scope; then the Host.
    let Some((pos, state, nbt)) = event.subject.with_view(|v| (v.position(), v.state(), v.nbt().clone())) else { return };
    let nbt = &nbt;
    let host: &dyn ClientHost = event.subject.host();
    ensure_ids(host);
    let Some(level) = here(host) else { return };
    let block = block_id(host, level, pos);
    if is_faucet(&block) {
        // A faucet "pouring" with no fluid is one waiting for redstone (its animateTick's red dust).
        let f = casting::read_faucet(nbt);
        let facing = facing_of(host, state);
        with_client(|c| {
            if f.state != OFF && f.render.is_none() {
                let t = c.ticks;
                c.waiting.insert(pos, (t, facing));
            } else {
                c.waiting.remove(&pos);
            }
        });
    } else if is_casting(&block) {
        // CastingBlockEntity.clientTick: a full casting that is cooling smokes.
        let cooling = gi(nbt, "hz_cooling", -1);
        let full = casting::cast_tank(nbt).fluid.is_some();
        with_client(|c| {
            if cooling >= 0 && full {
                let k = c.ticks;
                c.cooling.insert(pos, k);
            } else {
                c.cooling.remove(&pos);
            }
        });
    }
}

fn next(seed: &mut u64) -> f64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 11) as f64 / (1u64 << 53) as f64
}

/// FaucetBlock.animateTick (a quarter of its random display ticks, while waiting: red dust 0.3 toward the input) and
/// CastingBlockEntity.clientTick (a full casting: smoke at (x + rand, y + 1.1, z + rand) one tick in ten).
pub(crate) fn particles(host: &mut dyn ClientHost) {
    let now = with_client(|c| {
        c.ticks += 1;
        let now = c.ticks;
        c.waiting.retain(|_, (seen, _)| now.saturating_sub(*seen) < 40);
        c.cooling.retain(|_, seen| now.saturating_sub(*seen) < 40);
        now
    });
    let Some(keys) = KEYS.get() else { return };
    let Some(player) = host.local_player().and_then(|p| p.entity().position(&*host)) else { return };
    let (waiting, cooling): (Vec<(Pos, usize)>, Vec<Pos>) = with_client(|c| (c.waiting.iter().map(|(p, (_, f))| (*p, *f)).collect(), c.cooling.keys().copied().collect()));
    let mut seed = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) ^ now.rotate_left(17)) | 1;
    for (p, facing) in waiting {
        let d = (0..3).map(|i| (f64::from(p[i]) + 0.5 - player[i]).abs()).fold(0.0, f64::max);
        let chance = if d <= 16.0 { 667.0 / 35937.0 + 667.0 / 274625.0 } else if d <= 32.0 { 667.0 / 274625.0 } else { 0.0 };
        if next(&mut seed) >= chance || next(&mut seed) >= 0.25 {
            continue;
        }
        let v = step([0, 0, 0], facing);
        let at = [0, 1, 2].map(|i| f64::from(p[i]) + 0.5 - 0.3 * f64::from(v[i]));
        host.add_particle(&ParticleOptions::dust(keys.dust, [1.0, 0.0, 0.0], 0.5), at, [0.0; 3]);
    }
    for p in cooling {
        if next(&mut seed) > 0.9 {
            let at = [f64::from(p[0]) + next(&mut seed), f64::from(p[1]) + 1.1, f64::from(p[2]) + next(&mut seed)];
            host.add_particle(&ParticleOptions::simple(keys.smoke), at, [0.0; 3]);
        }
    }
}
