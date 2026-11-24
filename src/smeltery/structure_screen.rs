//! The smeltery's and foundry's client side: HeatingStructureScreen with its side inventory (GuiSmelteryTank,
//! GuiFuelModule, GuiMeltingModule, the bucket slot's mode button), the duct's filter screen, and the drawing in the
//! world (HeatingStructureBlockEntityRenderer, SmelteryTankRenderer, the drain's and duct's fluid, the error box).
//!
//! The side inventory hangs on the main window's left, so the mod screen starts at its left edge: the main window is
//! `side_width - 6` to the right (Mantle's overlap), and every place below is the main window's plus that.

use super::casting_render::{cuboid, face_frame, item_at, lit};
use super::screen::{self, data_at, fluid_tank, hovered, sprite};
use super::structure::*;
use super::util::*;
use super::{casting, tank_capacity};
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

const STRUCTURE_TEXTURE: &str = "tconstruct:gui/heating_structure";
const GENERIC_TEXTURE: &str = "tconstruct:gui/generic";
const DUCT_TEXTURE: &str = "tconstruct:gui/duct";
const SMELTERY_TANK: [i32; 4] = [8, 16, 106, 106];
const STRUCTURE_FUEL: [i32; 4] = [152, 32, 16, 90];
const FUEL_HOVER: [i32; 4] = [151, 31, 18, 92];
const BUCKET_BUTTON: [i32; 4] = [124, 69, 18, 18];
const GRAY: u32 = 0xFF40_4040;

#[derive(Default)]
pub(crate) struct Client {
    /// The controller whose menu the last click opens, and its slot count.
    controller: Option<Pos>,
    slots: usize,
    /// The formed structures drawn here: controller to (min, max, foundry).
    shapes: HashMap<Pos, (Pos, Pos, bool)>,
    /// When each controller's error position last changed.
    errors: HashMap<Pos, (Pos, Instant)>,
}

static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

fn with_client<T>(f: impl FnOnce(&mut Client) -> T) -> T {
    let mut g = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Client::default))
}

fn entity_data(host: &dyn Host, pos: Pos) -> Option<Compound> {
    let level = here(host)?;
    host.block_entity(level, pos)?.data(host)
}

/// A click on the client: the controller it opens (the controller itself, a drain's or duct's master, any part of a
/// formed structure drawn here). True when the click is a structure's.
pub(crate) fn clicked(host: &dyn ClientHost, pos: Pos, block: &str) -> bool {
    let controller = if is_controller(block) {
        Some(pos)
    } else if is_io(block) {
        entity_data(host, pos).and_then(|d| named_master(pos, &d))
    } else {
        with_client(|c| c.shapes.iter().find(|(_, (min, max, foundry))| contained(pos, *min, *max, *foundry)).map(|(c, _)| *c))
    };
    let Some(controller) = controller else { return false };
    let slots = entity_data(host, controller).map_or(0, |d| gi(&d, "hz_size", 0).clamp(0, MAX_SLOTS as i32) as usize);
    with_client(|c| {
        c.controller = Some(controller);
        c.slots = slots;
    });
    true
}

/// The side inventory's width, with the slider when its rows don't all show.
fn side_width(slots: usize) -> f32 {
    let (cols, rows, shown) = side_layout(slots);
    (cols * 22 + 14 + if rows > shown { 12 } else { 0 }) as f32
}

/// Where the main window starts in the mod screen.
fn main_x(slots: usize) -> f32 {
    side_width(slots) - 6.0
}

/// A window slot's item position (the side inventory's slot w of the rows shown).
fn side_slot(slots: usize, w: usize) -> Option<[f32; 2]> {
    let (cols, _, shown) = side_layout(slots);
    (w < cols * shown).then(|| [(12 + (w % cols) * 22) as f32, (12 + (w / cols) * 18) as f32])
}

fn open_slots() -> usize {
    with_client(|c| c.slots)
}

pub(crate) fn layout(_host: &dyn Host, block: &str) -> Option<MenuScreenLayout> {
    // The server lays the menu out for the controller it opens; the client for the one it clicked.
    layout_for(block, opening_slots().unwrap_or_else(open_slots))
}

/// The layout of `block`'s screen, a controller's with `slots` slots.
pub(crate) fn layout_for(block: &str, slots: usize) -> Option<MenuScreenLayout> {
    if is_duct(block) {
        // SingleItemContainerMenu: the slot at 80, 20, the inventory 51 down; BackgroundContainerScreen 176 x 133.
        return Some(MenuScreenLayout { width: 176.0, height: 133.0, slots: vec![Some(crate::slot_at([80.0, 20.0]))], inventory_y: 51.0, inventory_x: 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() });
    }
    if !is_controller(block) {
        return None;
    }
    let x = main_x(slots);
    let mut out = vec![Some([x + 125.0, 46.0]), Some([x + 125.0, 104.0])];
    out.extend((0..WINDOW).map(|w| side_slot(slots, w).filter(|_| w < slots)));
    out.push(None);
    Some(MenuScreenLayout { width: x + 176.0, height: 220.0, slots: crate::screen_slots(out), inventory_y: 138.0, inventory_x: x + 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() })
}

/// GuiSmelteryTank.calcLiquidHeights: each fluid's height, at least `min`, the largest cut first to fit.
fn liquid_heights(amounts: &[i32], capacity: i64, height: i32, min: i32) -> Vec<i32> {
    let capacity = capacity.max(1);
    let mut h: Vec<i32> = amounts.iter().map(|a| ((i64::from(*a) * i64::from(height) + capacity - 1) / capacity).max(i64::from(min)) as i32).collect();
    let total: i64 = amounts.iter().map(|a| i64::from(*a)).sum();
    let mut available = height;
    if total < capacity {
        available -= min;
    }
    loop {
        let sum: i32 = h.iter().sum();
        if sum <= available {
            break;
        }
        let Some(i) = (0..h.len()).max_by(|&a, &b| h[a].cmp(&h[b]).then(b.cmp(&a))) else { break };
        if h[i] <= 0 {
            break;
        }
        h[i] -= 1;
    }
    h
}

/// The client's copy of the open controller.
fn shown(host: &dyn ClientHost) -> Option<(Pos, Compound, String)> {
    let pos = with_client(|c| c.controller)?;
    let level = here(host)?;
    let block = block_id(host, level, pos);
    if !is_controller(&block) {
        return None;
    }
    Some((pos, entity_data(host, pos)?, block))
}

/// The tank's fluid bands from the bottom: (fluid index, top y, height).
fn tank_bands(tank: &STank) -> Vec<(usize, i32, i32)> {
    let [_, y, _, h] = SMELTERY_TANK;
    let amounts: Vec<i32> = tank.fluids.iter().map(|(_, a)| *a).collect();
    let heights = liquid_heights(&amounts, tank.contained().max(i64::from(tank.capacity)), h, 3);
    let mut bottom = y + h;
    heights
        .iter()
        .enumerate()
        .map(|(i, hh)| {
            bottom -= hh;
            (i, bottom, *hh)
        })
        .collect()
}

/// GuiSmelteryTank.getFluidClicked: the fluid under the cursor (main-window coordinates), None over the empty part.
fn tank_fluid_at(tank: &STank, at: [f32; 2]) -> Option<Option<usize>> {
    if !hovered(SMELTERY_TANK, at) {
        return None;
    }
    let cy = at[1].floor() as i32;
    Some(tank_bands(tank).into_iter().find(|(_, top, hh)| cy >= *top && cy < top + hh).map(|(i, _, _)| i))
}

/// MultitankFuelModule.getFuelInfo on the client: the last fuel's tank (else the first with fluid), its fluid summed
/// over the tanks that hold it, with the empty tanks' room.
fn fuel_info(host: &dyn ClientHost, pos: Pos, nbt: &Compound, last: Pos) -> Option<(String, i64, i64)> {
    let level = here(host)?;
    let shape = read_shape(nbt)?;
    let tanks: Vec<(Option<String>, i32, i32)> = shape
        .tanks
        .iter()
        .filter_map(|p| {
            let (f, a) = tank_of(&entity_data(host, *p)?);
            Some((f, a, tank_capacity(&block_id(host, level, *p))?))
        })
        .collect();
    let last = [pos[0] + last[0], pos[1] + last[1], pos[2] + last[2]];
    let lead = shape.tanks.iter().position(|t| *t == last).and_then(|i| tanks.get(i)).and_then(|t| t.0.clone()).or_else(|| tanks.iter().find_map(|t| t.0.clone()))?;
    let (mut amount, mut capacity) = (0i64, 0i64);
    for (f, a, c) in &tanks {
        if f.as_deref() == Some(lead.as_str()) {
            amount += i64::from(*a);
            capacity += i64::from(*c);
        } else if f.is_none() {
            capacity += i64::from(*c);
        }
    }
    Some((lead, amount, capacity))
}

fn stretch(draw: &mut dyn DrawContext, texture: &str, uv: [f32; 4], at: [f32; 4]) {
    if at[2] > 0.0 && at[3] > 0.0 {
        draw.sprite(texture, uv, at, None);
    }
}

pub(crate) fn draw(host: &dyn ClientHost, block: &str, data: &[i32], draw: &mut dyn DrawContext, origin: [f32; 2]) {
    if is_duct(block) {
        sprite(draw, DUCT_TEXTURE, origin, [0.0, 0.0], [0.0, 0.0, 176.0, 133.0]);
        let name = lang(host, &format!("block.{}", block.replacen(':', ".", 1)), "Duct");
        draw.text(&name, [origin[0] + 8.0, origin[1] + 6.0], GRAY, false);
        draw.text(&lang(host, "container.inventory", "Inventory"), [origin[0] + 8.0, origin[1] + 40.0], GRAY, false);
        return;
    }
    if !is_controller(block) {
        return;
    }
    let Some((pos, nbt, block)) = shown(host) else { return };
    let slots = open_slots();
    let d = |i| data_at(data, i);
    let offset = d(D_OFFSET).max(0) as usize;
    let size = d(D_SIZE).max(0) as usize;
    let x0 = main_x(slots);
    let tank = read_tank(&nbt);
    let m = [origin[0] + x0, origin[1]];
    // The side inventory: Mantle's border (its right edge merged with the main window), the slots, the slider.
    let (cols, rows, shown_rows) = side_layout(slots);
    let sw = side_width(slots);
    let sh = (18 * shown_rows + 14) as f32;
    let top = 4.0;
    let o = origin;
    let pieces: [([f32; 4], [f32; 4]); 9] = [
        ([0.0, 0.0, 7.0, 7.0], [0.0, top, 7.0, 7.0]),
        ([7.0, 0.0, 50.0, 7.0], [7.0, top, sw - 14.0, 7.0]),
        ([14.0, 40.0, 7.0, 7.0], [sw - 7.0, top, 7.0, 7.0]),
        ([0.0, 7.0, 7.0, 50.0], [0.0, top + 7.0, 7.0, sh - 14.0]),
        ([21.0, 45.0, 7.0, 14.0], [sw - 7.0, top + 7.0, 7.0, sh - 14.0]),
        ([7.0, 7.0, 50.0, 50.0], [7.0, top + 7.0, sw - 14.0, sh - 14.0]),
        ([0.0, 57.0, 7.0, 7.0], [0.0, top + sh - 7.0, 7.0, 7.0]),
        ([7.0, 57.0, 50.0, 7.0], [7.0, top + sh - 7.0, sw - 14.0, 7.0]),
        ([14.0, 47.0, 7.0, 7.0], [sw - 7.0, top + sh - 7.0, 7.0, 7.0]),
    ];
    for (uv, at) in pieces {
        stretch(draw, GENERIC_TEXTURE, uv, [o[0] + at[0], o[1] + at[1], at[2], at[3]]);
    }
    for w in 0..cols * shown_rows {
        let Some([sx, sy]) = side_slot(slots, w) else { continue };
        let uv = if offset + w < size { [0.0, 238.0, 22.0, 18.0] } else { [22.0, 238.0, 22.0, 18.0] };
        sprite(draw, STRUCTURE_TEXTURE, o, [sx - 5.0, sy - 1.0], uv);
    }
    if rows > shown_rows {
        let sx = (7 + cols * 22) as f32;
        let (sy, slider_h) = (top + 7.0, sh - 14.0);
        stretch(draw, GENERIC_TEXTURE, [43.0, 7.0, 12.0, 1.0], [o[0] + sx, o[1] + sy, 12.0, 1.0]);
        stretch(draw, GENERIC_TEXTURE, [43.0, 8.0, 12.0, 30.0], [o[0] + sx, o[1] + sy + 1.0, 12.0, slider_h - 2.0]);
        stretch(draw, GENERIC_TEXTURE, [43.0, 38.0, 12.0, 1.0], [o[0] + sx, o[1] + sy + slider_h - 1.0, 12.0, 1.0]);
        let max_row = (rows - shown_rows).max(1) as f32;
        let row = (offset / cols.max(1)) as f32;
        let ky = sy + (slider_h - 15.0) * (row / max_row).min(1.0);
        stretch(draw, GENERIC_TEXTURE, [7.0, 25.0, 10.0, 15.0], [o[0] + sx + 1.0, o[1] + ky, 10.0, 15.0]);
    }
    // The main window, the fuel gauge and the fire.
    sprite(draw, STRUCTURE_TEXTURE, m, [0.0, 0.0], [0.0, 0.0, 176.0, 220.0]);
    let (fuel, quality) = (d(0), d(1));
    if fuel > 0 && quality > 0 {
        let rest = (14 * i64::from(fuel) / i64::from(quality)) as i32 % 14;
        if rest > 0 {
            let off = (14 - rest) as f32;
            sprite(draw, STRUCTURE_TEXTURE, m, [153.0, 15.0 + off], [176.0, 136.0 + off, 14.0, rest as f32]);
        }
    }
    if let Some((f, a, c)) = fuel_info(host, pos, &nbt, [d(4), d(5), d(6)]) {
        let [fx, fy, fw, fh] = STRUCTURE_FUEL;
        let h = if c > 0 { (i64::from(fh) * a / c).min(i64::from(fh)) as i32 } else { 0 };
        if h > 0 {
            fluid_tank(draw, m, &f, h, h, [fx, fy + fh - h, fw, h]);
        }
    }
    // The tank's fluids, bottom up.
    for (i, top_y, h) in tank_bands(&tank) {
        if h > 0 {
            fluid_tank(draw, m, &tank.fluids[i].0, h, h, [8, top_y, 106, h]);
        }
    }
    let title = if block == FOUNDRY { lang(host, "gui.tconstruct.foundry", "Foundry") } else { lang(host, "gui.tconstruct.smeltery", "Smeltery") };
    draw.text(&title, [m[0] + 8.0, m[1] + 6.0], GRAY, false);
    draw.text(&lang(host, "container.inventory", "Inventory"), [m[0] + 8.0, m[1] + 126.0], GRAY, false);
    sprite(draw, STRUCTURE_TEXTURE, m, [8.0, 16.0], [176.0, 0.0, 80.0, 106.0]);
    // The bucket slot's placeholder, the mode button.
    sprite(draw, STRUCTURE_TEXTURE, m, [125.0, 46.0], [224.0, 186.0, 16.0, 16.0]);
    let cursor = screen::mouse();
    let at = [cursor[0] - x0, cursor[1]];
    if hovered(BUCKET_BUTTON, at) {
        sprite(draw, STRUCTURE_TEXTURE, m, [124.0, 69.0], [176.0, 202.0, 18.0, 18.0]);
    }
    let mode_u = [176.0, 192.0, 208.0][d(D_MODE).rem_euclid(3) as usize];
    sprite(draw, STRUCTURE_TEXTURE, m, [125.0, 70.0], [mode_u, 186.0, 16.0, 16.0]);
    // The highlights: the hovered fluid or the empty part, the fuel gauge.
    let highlight = |draw: &mut dyn DrawContext, r: [i32; 4]| {
        if r[2] > 0 && r[3] > 0 {
            draw.fill([m[0] + r[0] as f32, m[1] + r[1] as f32, r[2] as f32, r[3] as f32], 0x80FF_FFFF);
        }
    };
    if let Some(under) = tank_fluid_at(&tank, at) {
        let bands = tank_bands(&tank);
        match under.and_then(|i| bands.iter().find(|b| b.0 == i)) {
            Some((_, top_y, h)) => highlight(draw, [8, *top_y, 106, *h]),
            None => {
                let top_fluid = bands.last().map_or(16 + 106, |b| b.1);
                highlight(draw, [8, 16, 106, top_fluid - 16]);
            }
        }
    }
    if hovered(FUEL_HOVER, at) {
        highlight(draw, STRUCTURE_FUEL);
    }
    // The heat bars of the window's slots that hold an item, against the fuel's temperature.
    let temperature = d(2);
    for w in 0..WINDOW {
        let Some([sx, sy]) = side_slot(slots, w) else { continue };
        let (current, required, temp) = (d(D_WINDOW + w * 3), d(D_WINDOW + w * 3 + 1), d(D_WINDOW + w * 3 + 2));
        if temp < 0 || offset + w >= size {
            continue;
        }
        let (u, progress) = if required == 0 {
            (185.0, 1.0)
        } else if temp > temperature {
            (179.0, 1.0)
        } else if current < 0 {
            (182.0, 1.0)
        } else if current <= required {
            (176.0, current as f32 / required as f32)
        } else {
            (176.0, 1.0)
        };
        let h = if progress > 1.0 { 16 } else if progress < 0.0 { 0 } else { (progress * 16.0 + 0.5) as i32 };
        if h > 0 {
            let dy = (16 - h) as f32;
            sprite(draw, STRUCTURE_TEXTURE, o, [sx - 4.0, sy + dy], [u, 150.0 + dy, 3.0, h as f32]);
        }
    }
}

/// Clicks on the screen's own parts, as menu buttons: 0 the bucket mode, 1/2 the fuel tanks, 3 empty into the tank,
/// 4+ a fluid (fill the cursor's item or move it to the bottom), the slider's rows from SCROLL_BUTTON.
pub(crate) fn click(host: &dyn ClientHost, block: &str, _data: &[i32], at: [f32; 2], right: bool) -> Option<i32> {
    if !is_controller(block) {
        return None;
    }
    let (_, nbt, _) = shown(host)?;
    let slots = open_slots();
    let x0 = main_x(slots);
    let local = [at[0] - x0, at[1]];
    if !right && hovered(BUCKET_BUTTON, local) {
        return Some(0);
    }
    if hovered(FUEL_HOVER, local) {
        return Some(if right { 2 } else { 1 });
    }
    let tank = read_tank(&nbt);
    if let Some(under) = tank_fluid_at(&tank, local) {
        return Some(match (right, under) {
            (false, Some(i)) => 4 + i as i32,
            _ => 3,
        });
    }
    // The slider: the row under the cursor.
    let (cols, rows, shown_rows) = side_layout(slots);
    if rows > shown_rows {
        let sx = (7 + cols * 22) as f32;
        let (sy, slider_h) = (11.0, (18 * shown_rows) as f32);
        if at[0] >= sx && at[0] < sx + 12.0 && at[1] >= sy && at[1] < sy + slider_h {
            let row = (((at[1] - sy) / slider_h) * (rows - shown_rows + 1) as f32).floor() as usize;
            return Some(SCROLL_BUTTON + row.min(rows - shown_rows) as i32);
        }
    }
    None
}

pub(crate) fn tooltip(host: &dyn ClientHost, block: &str, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) {
    *screen::MOUSE.lock().unwrap_or_else(|e| e.into_inner()) = at;
    if !is_controller(block) {
        return;
    }
    let Some((pos, nbt, _)) = shown(host) else { return };
    let slots = open_slots();
    let x0 = main_x(slots);
    let local = [at[0] - x0, at[1]];
    let shift = shift_down(host);
    let d = |i| data_at(data, i);
    if hovered(BUCKET_BUTTON, local) {
        let name = ["auto", "empty_item", "fill_item"][d(D_MODE).rem_euclid(3) as usize];
        let (title, desc) = match name {
            "auto" => ("Mode: Auto", "Slot will attempt to empty then fill the item"),
            "empty_item" => ("Mode: Empty Item", "Slot will attempt to empty the item"),
            _ => ("Mode: Fill Item", "Slot will attempt to fill the item"),
        };
        lines.push(lang(host, &format!("gui.tconstruct.tank.bucket.{name}.title"), title));
        lines.push(format!("§7{}", lang(host, &format!("gui.tconstruct.tank.bucket.{name}.description"), desc)));
        lines.push(format!("§7{}", lang(host, "gui.tconstruct.tank.bucket.clickable", "Click to toggle fluid transfer mode")));
        return;
    }
    let tank = read_tank(&nbt);
    if let Some(under) = tank_fluid_at(&tank, local) {
        match under {
            Some(i) => lines.extend(screen::fluid_lines(host, &tank.fluids[i].0, tank.fluids[i].1, shift, true)),
            None => {
                // The `tconstruct:smeltery` unit list (ingots), Shift for buckets.
                let format = |amount: i32, out: &mut Vec<String>| {
                    if shift {
                        screen::bucket_lines(host, amount, out);
                    } else {
                        let named = screen::units().named.get("tconstruct:smeltery").map_or(&[][..], Vec::as_slice);
                        let rest = screen::unit_lines(host, named, amount, out);
                        screen::millibuckets(host, rest, out);
                    }
                };
                let used = tank.contained().clamp(0, i64::from(i32::MAX)) as i32;
                lines.push(lang(host, "gui.tconstruct.melting.capacity", "Capacity:"));
                format(tank.capacity, &mut *lines);
                if tank.remaining() > 0 {
                    lines.push(lang(host, "gui.tconstruct.melting.available", "Free:"));
                    format(tank.remaining(), &mut *lines);
                }
                if used > 0 {
                    lines.push(lang(host, "gui.tconstruct.melting.used", "Used:"));
                    format(used, &mut *lines);
                }
                if !shift {
                    screen::shift_hint(host, lines);
                }
            }
        }
        return;
    }
    if hovered(FUEL_HOVER, local) {
        match fuel_info(host, pos, &nbt, [d(4), d(5), d(6)]) {
            Some((fluid, amount, _)) => {
                let mut tip = screen::fluid_lines(host, &fluid, amount.clamp(0, i64::from(i32::MAX)) as i32, shift, true);
                let all = super::recipes::recipes(host);
                let line = match super::recipes::fluid_fuel(&all, &fluid) {
                    Some(f) if f.temperature > 0 => format!("§7§o{}", lang(host, "gui.tconstruct.melting.fuel.temperature", "Temperature: %s°C").replacen("%s", &f.temperature.to_string(), 1)),
                    _ => format!("§c{}", lang(host, "gui.tconstruct.melting.fuel.invalid", "Invalid melting fuel!")),
                };
                tip.insert(1.min(tip.len()), line);
                lines.extend(tip);
            }
            None if read_shape(&nbt).is_some_and(|s| !s.tanks.is_empty()) => lines.push(lang(host, "gui.tconstruct.melting.fuel.empty", "No fuel found")),
            None => lines.push(lang(host, "gui.tconstruct.melting.fuel.no_tank", "No tank in structure")),
        }
        return;
    }
    // GuiMeltingModule.drawHeatTooltips over a window slot's bar.
    let offset = d(D_OFFSET).max(0) as usize;
    for w in 0..WINDOW {
        let Some([sx, sy]) = side_slot(slots, w) else { continue };
        let (current, required, temp) = (d(D_WINDOW + w * 3), d(D_WINDOW + w * 3 + 1), d(D_WINDOW + w * 3 + 2));
        if temp < 0 || offset + w >= d(D_SIZE).max(0) as usize || !hovered([sx as i32 - 5, sy as i32 - 1, 4, 18], at) {
            continue;
        }
        if required == 0 {
            lines.push(lang(host, "gui.tconstruct.melting.no_recipe", "Item cannot be molten."));
        } else if temp > d(2) {
            lines.push(lang(host, "gui.tconstruct.melting.no_heat", "Not enough heat to melt this item."));
        } else if current < 0 {
            lines.push(lang(host, "gui.tconstruct.melting.no_space", "Not enough free space for the fluid."));
        }
        return;
    }
}

// ---- Drawing in the world

/// The structure's fluids as SmelteryTankRenderer stacks them (by the tank's capacity), from the inside's lowest
/// corner, one block column at a time.
fn draw_structure_fluids(host: &dyn Host, draw: &mut dyn BlockEntityDrawContext, tank: &STank, size: [i32; 3], o: [f32; 3]) {
    let [xd, yd, zd] = size;
    let amounts: Vec<i32> = tank.fluids.iter().map(|(_, a)| *a).collect();
    let heights = liquid_heights(&amounts, i64::from(tank.capacity), yd * 1000 - 9, 100);
    let mut y = 5;
    for (i, h) in heights.iter().enumerate() {
        let (y0, y1) = (y, y + h);
        y = y1;
        if *h <= 0 {
            continue;
        }
        let fluid = &tank.fluids[i].0;
        let light = lit(host, draw, fluid);
        let (still, _, tint) = super::render::sprites(fluid);
        for by in (y0 / 1000)..=((y1 - 1) / 1000).min(yd - 1) {
            // In pixels within this block row.

            let hi = (y1 - by * 1000).min(1000) as f32 * 0.016;
            let top = y1 <= (by + 1) * 1000;
            for bx in 0..xd {
                for bz in 0..zd {
                    let px0 = if bx == 0 { 0.08 } else { 0.0 };
                    let px1 = if bx == xd - 1 { 15.92 } else { 16.0 };
                    let pz0 = if bz == 0 { 0.08 } else { 0.0 };
                    let pz1 = if bz == zd - 1 { 15.92 } else { 16.0 };
                    let b = |px: f32, py: f32, pz: f32| [o[0] + bx as f32 + px / 16.0, o[1] + by as f32 + py / 16.0, o[2] + bz as f32 + pz / 16.0];
                    // The surface (SmelteryTankRenderer: one flat top at the fluid's height), seen from above.
                    if top {
                        draw.quad(&still, [b(px0, hi, pz0), b(px0, hi, pz1), b(px1, hi, pz1), b(px1, hi, pz0)], [px0, pz0, px1, pz1], tint);
                    }
                    // No sides: they stand against the walls (SmelteryTankRenderer's outer faces are hidden by them).
                }
            }
        }
        draw.set_light(light);
    }
}

/// A thin outline of the block at `at` (in the drawing's frame): the error box.
fn draw_box(draw: &mut dyn BlockEntityDrawContext, at: Pos, tint: u32) {
    let sprite = "block/white_concrete";
    let (e, t) = (-0.005f32, 0.04f32);
    let (a, b) = (e, 1.0 - e);
    let [x, y, z] = at.map(|v| v as f32);
    let p = |v: [f32; 3]| [x + v[0], y + v[1], z + v[2]];
    for axis in 0..3 {
        for (u, v) in [(a, a), (a, b), (b, a), (b, b)] {
            let corner = |s: f32, du: f32, dv: f32| match axis {
                0 => [s, u + du, v + dv],
                1 => [u + du, s, v + dv],
                _ => [u + du, v + dv, s],
            };
            let (su, sv) = (if u < 0.5 { t } else { -t }, if v < 0.5 { t } else { -t });
            draw.quad(sprite, [p(corner(a, 0.0, 0.0)), p(corner(b, 0.0, 0.0)), p(corner(b, su, 0.0)), p(corner(a, su, 0.0))], [0.0, 0.0, 16.0, 16.0], tint);
            draw.quad(sprite, [p(corner(a, 0.0, 0.0)), p(corner(a, 0.0, sv)), p(corner(b, 0.0, sv)), p(corner(b, 0.0, 0.0))], [0.0, 0.0, 16.0, 16.0], tint);
        }
    }
}

fn facing_of(host: &dyn ClientHost, state: BlockState) -> usize {
    host.block_state_string(state)
        .and_then(|s| s.split_once('[').map(|(_, p)| p.trim_end_matches(']').to_owned()))
        .and_then(|p| p.split(',').find_map(|kv| kv.strip_prefix("facing=").map(str::to_owned)))
        .and_then(|f| casting::dir_index(&f))
        .unwrap_or(2)
}

fn formed_state(host: &dyn ClientHost, state: BlockState) -> bool {
    host.block_state_string(state).is_some_and(|s| s.contains("in_structure=true"))
}

pub(crate) fn submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    ensure_ids(host);
    let (pos, block) = (event.pos, event.block);
    let formed = formed_state(host, event.state);
    let facing = facing_of(host, event.state);
    let nbt = &event.entity.nbt;
    let draw = &mut *event.draw;
    if is_io(block) {
        if !formed {
            return;
        }
        let fluid = if is_duct(block) { gs(nbt, "hz_filter") } else { gs(nbt, "hz_bottom") };
        let Some(fluid) = fluid.filter(|f| !f.is_empty()) else { return };
        if is_chute(block) {
            return;
        }
        // The drain's spouts (io_fluid's `fluid` faces) show the master's bottom fluid; a duct's its filter's.
        let light = lit(host, draw, &fluid);
        draw.push();
        face_frame(draw, facing);
        let n = None;
        let s = Some(false);
        cuboid(draw, &fluid, [-0.92, 4.0, 4.0], [-0.91, 12.0, 12.0], [n, n, n, n, s, n]);
        cuboid(draw, &fluid, [16.91, 4.0, 4.0], [16.92, 12.0, 12.0], [n, n, n, n, n, s]);
        cuboid(draw, &fluid, [4.0, 4.0, 16.91], [12.0, 12.0, 16.92], [n, n, n, s, n, n]);
        cuboid(draw, &fluid, [4.0, 16.91, 4.0], [12.0, 16.92, 12.0], [n, s, n, n, n, n]);
        cuboid(draw, &fluid, [4.0, -0.92, 4.0], [12.0, -0.91, 12.0], [s, n, n, n, n, n]);
        draw.pop();
        draw.set_light(light);
        return;
    }
    if !is_controller(block) {
        return;
    }
    let shape = read_shape(nbt).filter(|_| formed);
    with_client(|c| match &shape {
        Some(s) => {
            c.shapes.insert(pos, (s.min, s.max, block == FOUNDRY));
        }
        None => {
            c.shapes.remove(&pos);
        }
    });
    let tank = read_tank(nbt);
    if let (Some(sh), true) = (&shape, formed) {
        // The window in the front shows the bottom fluid (controller_fluid's `fluid` part).
        if let Some((bottom, _)) = tank.fluids.first() {
            let light = lit(host, draw, bottom);
            draw.push();
            face_frame(draw, facing);
            // controller_fluid's `fluid` element (3, 10, -1 to 13, 15, 0 on the north front), just behind the window frame's face.
            cuboid(draw, bottom, [3.0, 10.0, 16.0], [13.0, 15.0, 16.97], [None, None, None, Some(false), None, None]);
            draw.pop();
            draw.set_light(light);
        }
        let lo = sh.lo();
        let size = sh.size();
        draw_structure_fluids(host, draw, &tank, size, [(lo[0] - pos[0]) as f32, (lo[1] - pos[1]) as f32, (lo[2] - pos[2]) as f32]);
        draw.push();
        draw.translate([(lo[0] - pos[0]) as f32, (lo[1] - pos[1]) as f32, (lo[2] - pos[2]) as f32]);
        // The items inside, one a block from the lowest corner, row by row (facing turns each).
        let layer = (size[0] * size[2]).max(1) as usize;
        let turn = match facing {
            3 => 0.0,
            4 => -90.0,
            2 => -180.0,
            _ => -270.0,
        };
        for (i, id) in get_strings(nbt, "hz_items", RENDER_ITEMS.min(gi(nbt, "hz_size", 0).max(0) as usize)).iter().enumerate() {
            if id.is_empty() {
                continue;
            }
            let (h, r) = (i / layer, i % layer);
            let (x, z) = (r % size[0].max(1) as usize, r / size[0].max(1) as usize);
            if h as i32 >= size[1] {
                break;
            }
            let Some(stack) = new_stack(id, 1) else { continue };
            item_at(draw, &stack, [(x as f32 + 0.5) * 16.0, (h as f32 + 0.5) * 16.0, (z as f32 + 0.5) * 16.0], 15.0, [0.0, turn], ItemDisplayContext::None, i as i32);
        }
        draw.pop();
    }
    // The error box: shown 10 s after it changes on an unformed structure (StructureErrorPositionPacket).
    if let Some(err) = get_pos(nbt, "hz_error") {
        let since = with_client(|c| {
            let e = c.errors.entry(pos).or_insert((err, Instant::now()));
            if e.0 != err {
                *e = (err, Instant::now());
            }
            e.1
        });
        if !formed && since.elapsed().as_secs() < 10 {
            draw_box(draw, [err[0] - pos[0], err[1] - pos[1], err[2] - pos[2]], 0xFF_0000);
        }
    }
}

/// `block_entity_render_bounding_box`: a formed controller draws over its whole inside.
pub(crate) fn render_bounds(pos: Pos, entity: &ModBlockEntity, bounds: &mut [f32; 6]) {
    let mut lo = [-1.0f32, -1.0, -1.0];
    let mut hi = [2.0f32, 2.0, 2.0];
    let mut widen = |p: Pos| {
        for i in 0..3 {
            lo[i] = lo[i].min((p[i] - pos[i]) as f32 - 1.0);
            hi[i] = hi[i].max((p[i] - pos[i]) as f32 + 2.0);
        }
    };
    if let Some(s) = read_shape(&entity.nbt) {
        widen(s.min);
        widen(s.max);
    }
    if let Some(e) = get_pos(&entity.nbt, "hz_error") {
        widen(e);
    }
    *bounds = [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]];
}
