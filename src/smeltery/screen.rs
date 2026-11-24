//! The screens: MelterScreen (GuiTankModule, GuiFuelModule, GuiMeltingModule) and the heater's
//! BackgroundContainerScreen, drawn from the menu's data slots and the client's copy of the melter (its tank, its
//! inputs' ids) and of the block below (a heater, or a fuel tank and its fluid); and Mantle's fluid tooltips
//! (FluidTooltipHandler with the fluids' unit lists).

use super::melter::{HEATER_SLOT, INPUTS};
use super::util::*;
use super::{HEATER, MELTER, MELTER_CAPACITY, tank_capacity};
use crate::fluids;
use mod_api::client::hud_layout::Blend;
use mod_api::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const MELTER_TEXTURE: &str = "tconstruct:gui/melter";
const HEATER_TEXTURE: &str = "tconstruct:gui/heater";
/// MelterContainerMenu: the inputs at 22, 16 + 18i; the heater's slot at 151, 32.
const MELTER_SLOTS: [[f32; 2]; 4] = [[22.0, 16.0], [22.0, 34.0], [22.0, 52.0], [151.0, 32.0]];
/// GuiTankModule(90, 16, 52, 52); GuiFuelModule(153, 32, 12, 36, fire at 152, 15).
const TANK_RECT: [i32; 4] = [90, 16, 52, 52];
const FUEL_RECT: [i32; 4] = [153, 32, 12, 36];
const GRAY: u32 = 0xFF40_4040;

static CLICKED: Mutex<Option<Pos>> = Mutex::new(None);

/// The melter or heater the client last clicked: its screen's block.
pub(crate) fn clicked(pos: Pos) {
    *CLICKED.lock().unwrap_or_else(|e| e.into_inner()) = Some(pos);
}

fn clicked_pos() -> Option<Pos> {
    *CLICKED.lock().unwrap_or_else(|e| e.into_inner())
}

/// The screen's layout. The melter's slot 3 (the heater's) is there only over a heater, as MelterContainerMenu adds
/// it: for the melter the server is opening (`opening`), else the one the menu opened for (`pos`) or, with none, the
/// one the client clicked.
pub(crate) fn layout(host: &dyn Host, block: &str, pos: Option<Pos>) -> Option<MenuScreenLayout> {
    let heater = || match super::melter::with_rt(|r| r.opening.map(|_| r.opening_heater)) {
        Some(h) => h,
        None => pos.or_else(clicked_pos).and_then(|p| here(host).map(|l| block_id(host, l, below(p)) == HEATER)).unwrap_or(true),
    };
    layout_for(block, block == MELTER && heater())
}

/// The layout of `block`'s screen, the melter's with its heater slot when `heater`.
pub(crate) fn layout_for(block: &str, heater: bool) -> Option<MenuScreenLayout> {
    if block == MELTER {
        let slots = (0..=HEATER_SLOT).map(|i| (i < INPUTS || heater).then_some(crate::slot_at(MELTER_SLOTS[i]))).collect();
        return Some(MenuScreenLayout { width: 176.0, height: 166.0, slots, inventory_y: 84.0, inventory_x: 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() });
    }
    if block == HEATER {
        // SingleItemContainerMenu: the slot at 80, 20, the inventory 51 down; BackgroundContainerScreen 176 x 133.
        return Some(MenuScreenLayout { width: 176.0, height: 133.0, slots: vec![Some(crate::slot_at([80.0, 20.0]))], inventory_y: 51.0, inventory_x: 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() });
    }
    None
}

/// A data slot as the client has it: 16 bits on the wire, as vanilla's.
pub(crate) fn data_at(data: &[i32], i: usize) -> i32 {
    data.get(i).map_or(0, |v| i32::from(*v as i16))
}

pub(crate) fn hovered(r: [i32; 4], at: [f32; 2]) -> bool {
    let (x, y) = (at[0].floor() as i32, at[1].floor() as i32);
    x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]
}

/// What the client knows of the melter whose menu is open: its tank, its inputs' ids, and the block below.
pub(crate) struct Shown {
    tank: Tank,
    inputs: Vec<String>,
    heater: bool,
    fuel_tank: Option<(Tank, i32)>,
}

fn shown(host: &dyn ClientHost) -> Shown {
    let level = here(host);
    let pos = clicked_pos();
    let data = |p: Pos| level.and_then(|l| host.block_entity(l, p)).and_then(|e| e.data(host));
    let melter = pos.and_then(data);
    let below_pos = pos.map(below);
    let below_block = match (level, below_pos) {
        (Some(l), Some(b)) => block_id(host, l, b),
        _ => String::new(),
    };
    let fuel_tank = below_pos
        .and_then(|b| tank_capacity(&below_block).filter(|_| below_block != MELTER).map(|c| (b, c)))
        .and_then(|(b, c)| data(b).map(|d| (tank_of(&d), c)));
    Shown {
        tank: melter.as_ref().map_or((None, 0), tank_of),
        inputs: melter.as_ref().map_or_else(|| vec![String::new(); INPUTS], |d| get_strings(d, "hz_items", INPUTS)),
        heater: below_block == HEATER,
        fuel_tank,
    }
}

pub(crate) fn sprite(draw: &mut dyn DrawContext, texture: &str, o: [f32; 2], at: [f32; 2], uv: [f32; 4]) {
    if uv[2] > 0.0 && uv[3] > 0.0 {
        draw.sprite(texture, uv, [o[0] + at[0], o[1] + at[1], uv[2], uv[3]], None);
    }
}

/// The still sprite, its texture's height (its frames) and the tint of a fluid.
fn fluid_look(fluid: &str) -> (String, i32, u32) {
    match fluid {
        "minecraft:water" => ("minecraft:block/water_still".into(), 512, 0xFF3F_76E4),
        "minecraft:lava" => ("minecraft:block/lava_still".into(), 320, 0xFFFF_FFFF),
        _ => match fluids::table().textures.get(fluid) {
            Some((still, _, tint)) => {
                let h = heights().get(still).copied().unwrap_or(16);
                (still.clone(), h, 0xFF00_0000 | (tint & 0xFF_FFFF))
            }
            None => ("minecraft:block/water_still".into(), 512, 0xFFFF_FFFF),
        },
    }
}

/// The fluid sprites' texture heights (the generator's `heights`), for drawing their first frame.
fn heights() -> &'static HashMap<String, i32> {
    static H: OnceLock<HashMap<String, i32>> = OnceLock::new();
    H.get_or_init(|| {
        let v: serde_json::Value = serde_json::from_str(crate::gen_fluids::FLUIDS_JSON).unwrap_or_default();
        v["heights"].as_object().map(|o| o.iter().filter_map(|(k, h)| Some((k.clone(), h.as_i64()? as i32))).collect()).unwrap_or_default()
    })
}

/// GuiUtil.renderFluidTank: the fluid's still sprite (its first frame), tinted, in 16-pixel tiles from the bottom of
/// `r` up `amount / capacity` of it, the top row cut to fit.
pub(crate) fn fluid_tank(draw: &mut dyn DrawContext, o: [f32; 2], fluid: &str, amount: i32, capacity: i32, r: [i32; 4]) {
    if capacity <= 0 || amount <= 0 {
        return;
    }
    let h = (i64::from(r[3]) * i64::from(amount) / i64::from(capacity)).min(i64::from(r[3])) as i32;
    if h <= 0 {
        return;
    }
    let (tex, tex_h, color) = fluid_look(fluid);
    let tex = tex.replacen("minecraft:", "", 1);
    let (x0, bottom) = (o[0].round() as i32 + r[0], o[1].round() as i32 + r[1] + r[3]);
    let mut draws = Vec::new();
    let mut y = 0;
    while y < h {
        let piece = (h - y).min(16);
        let mut x = 0;
        while x < r[2] {
            let w = (r[2] - x).min(16);
            draws.push(GuiDraw::Blit {
                texture: tex.clone().into(),
                blend: Blend::Textured,
                x: x0 + x,
                y: bottom - y - piece,
                w,
                h: piece,
                uv: [0.0, (16 - piece) as f32],
                src: [w, piece],
                tex: [16, tex_h],
                color,
            });
            x += 16;
        }
        y += piece;
    }
    if draw.gui_paint(&draws).is_err() {
        draw.fill([x0 as f32, (bottom - h) as f32, r[2] as f32, h as f32], color);
    }
}

pub(crate) fn draw(host: &dyn ClientHost, block: &str, data: &[i32], draw: &mut dyn DrawContext, origin: [f32; 2]) {
    if block == HEATER {
        sprite(draw, HEATER_TEXTURE, origin, [0.0, 0.0], [0.0, 0.0, 176.0, 133.0]);
        // BackgroundContainerScreen: the title centred, the inventory's label at height - 94.
        let title = lang(host, "gui.tconstruct.heater", "Heater");
        let w = draw.text_width(&title);
        draw.text(&title, [origin[0] + ((176.0 - w) / 2.0).floor(), origin[1] + 6.0], GRAY, false);
        draw.text(&lang(host, "container.inventory", "Inventory"), [origin[0] + 8.0, origin[1] + 39.0], GRAY, false);
        return;
    }
    if block != MELTER {
        return;
    }
    let s = shown(host);
    let d = |i| data_at(data, i);
    let (fuel, quality, temperature) = (d(0), d(1), d(2));
    sprite(draw, MELTER_TEXTURE, origin, [0.0, 0.0], [0.0, 0.0, 176.0, 166.0]);
    // The fuel's frame: the heater's slot, or the fuel tank's gauge.
    if s.heater {
        sprite(draw, MELTER_TEXTURE, origin, [150.0, 31.0], [176.0, 52.0, 18.0, 36.0]);
    } else {
        sprite(draw, MELTER_TEXTURE, origin, [152.0, 31.0], [194.0, 52.0, 14.0, 38.0]);
    }
    // The fire (drawScaledYUp: only what is past whole fires shows, so a full one shows none).
    if fuel > 0 && quality > 0 {
        let rest = (14 * i64::from(fuel) / i64::from(quality)) as i32 % 14;
        if rest > 0 {
            let off = (14 - rest) as f32;
            sprite(draw, MELTER_TEXTURE, origin, [152.0, 15.0 + off], [176.0, 136.0 + off, 14.0, rest as f32]);
        }
    }
    if !s.heater
        && let Some(((Some(fluid), amount), capacity)) = &s.fuel_tank
    {
        fluid_tank(draw, origin, fluid, *amount, (*capacity).max(*amount), FUEL_RECT);
    }
    if let Some(f) = &s.tank.0 {
        fluid_tank(draw, origin, f, s.tank.1, MELTER_CAPACITY, TANK_RECT);
    }
    draw.text(&lang(host, "gui.tconstruct.melter", "Melter"), [origin[0] + 8.0, origin[1] + 6.0], GRAY, false);
    draw.text(&lang(host, "container.inventory", "Inventory"), [origin[0] + 8.0, origin[1] + 72.0], GRAY, false);
    // The hovered fluid's highlight (white, half see-through): the tank's filled or empty part, the fuel's area.
    let at = mouse();
    let highlight = |draw: &mut dyn DrawContext, r: [i32; 4]| {
        if r[2] > 0 && r[3] > 0 {
            draw.fill([origin[0] + r[0] as f32, origin[1] + r[1] as f32, r[2] as f32, r[3] as f32], 0x80FF_FFFF);
        }
    };
    let [tx, ty, tw, th] = TANK_RECT;
    if hovered([tx - 1, ty - 1, tw + 2, th + 2], at) {
        let filled = (i64::from(th) * i64::from(s.tank.1) / i64::from(MELTER_CAPACITY)) as i32;
        let middle = ty + th - filled;
        if at[1].floor() as i32 > middle {
            highlight(draw, [tx, middle, tw, filled]);
        } else {
            highlight(draw, [tx, ty, tw, th - filled]);
        }
    }
    let [fx, fy, fw, fh] = FUEL_RECT;
    if hovered([fx - 1, fy - 1, fw + 2, fh + 2], at) {
        if s.heater {
            if at[1].floor() as i32 > fy + 18 {
                highlight(draw, [fx, fy + 18, fw, fh - 18]);
            }
        } else {
            highlight(draw, FUEL_RECT);
        }
    }
    // The tank's scale.
    sprite(draw, MELTER_TEXTURE, origin, [90.0, 16.0], [176.0, 0.0, 52.0, 52.0]);
    // The heat bars beside each input that holds an item (GuiMeltingModule.drawHeatBars, drawProgressUp).
    for i in 0..INPUTS {
        if s.inputs.get(i).is_none_or(|id| id.is_empty()) {
            continue;
        }
        let (current, required, temp) = (d(4 + i * 3), d(5 + i * 3), d(6 + i * 3));
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
            let [x, y] = MELTER_SLOTS[i];
            sprite(draw, MELTER_TEXTURE, origin, [x - 4.0, y + dy], [u, 150.0 + dy, 3.0, h as f32]);
        }
    }
}

/// The pointer, from the last tooltip ask (the drawing hook isn't told it).
pub(crate) static MOUSE: Mutex<[f32; 2]> = Mutex::new([-1000.0, -1000.0]);

pub(crate) fn mouse() -> [f32; 2] {
    *MOUSE.lock().unwrap_or_else(|e| e.into_inner())
}

/// ClickableTankModule.tryClick: the mouse button, plus 2 for the fuel's tank.
pub(crate) fn click(_host: &dyn ClientHost, block: &str, at: [f32; 2], right: bool) -> Option<i32> {
    if block != MELTER {
        return None;
    }
    let b = i32::from(right);
    let [tx, ty, tw, th] = TANK_RECT;
    let [fx, fy, fw, fh] = FUEL_RECT;
    if hovered([tx - 1, ty - 1, tw + 2, th + 2], at) {
        Some(b)
    } else if hovered([fx - 1, fy - 1, fw + 2, fh + 2], at) {
        Some(b + 2)
    } else {
        None
    }
}

// ---- Fluid tooltips (FluidTooltipHandler)

pub(crate) struct Units {
    lists: Vec<(Vec<String>, Vec<(String, i32)>)>,
    pub named: HashMap<String, Vec<(String, i32)>>,
    fallback: Vec<(String, i32)>,
}

pub(crate) fn units() -> &'static Units {
    static U: OnceLock<Units> = OnceLock::new();
    U.get_or_init(|| {
        let v: serde_json::Value = serde_json::from_str(crate::gen_fluids::FLUIDS_JSON).unwrap_or_default();
        let list = |v: &serde_json::Value| -> Vec<(String, i32)> {
            v.as_array().map(|a| a.iter().filter_map(|u| Some((u[0].as_str()?.to_owned(), u[1].as_i64()?.max(1) as i32))).collect()).unwrap_or_default()
        };
        let strs = |v: &serde_json::Value| -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() };
        Units {
            lists: v["unit_lists"].as_array().map(|a| a.iter().map(|l| (strs(&l["fluids"]), list(&l["units"]))).collect()).unwrap_or_default(),
            named: v["named"].as_object().map(|o| o.iter().map(|(k, u)| (k.clone(), list(u))).collect()).unwrap_or_default(),
            fallback: list(&v["fallback"]),
        }
    })
}

/// FluidUnit.getText for each unit of a list: the whole units as gray lines; what is left.
pub(crate) fn unit_lines(host: &dyn ClientHost, list: &[(String, i32)], mut amount: i32, lines: &mut Vec<String>) -> i32 {
    for (key, needed) in list {
        let full = amount / needed;
        if full > 0 {
            // Mantle's own lang isn't in the content: its English.
            let english = match key.rsplit('.').next() {
                Some("bucket") => "%s buckets".to_owned(),
                Some("kilobucket") => "%s kb".to_owned(),
                Some(unit) => format!("%s {unit}s"),
                None => "%s".to_owned(),
            };
            lines.push(format!("§7{}", lang(host, key, &english).replacen("%s", &commas(i64::from(full)), 1)));
        }
        amount %= needed;
    }
    amount
}

pub(crate) fn millibuckets(host: &dyn ClientHost, amount: i32, lines: &mut Vec<String>) {
    if amount > 0 {
        lines.push(format!("§7{}", lang(host, "gui.mantle.fluid.millibucket", "%s mb").replacen("%s", &commas(i64::from(amount)), 1)));
    }
}

/// FluidTooltipHandler.appendBuckets: the fallback list (buckets), then millibuckets.
pub(crate) fn bucket_lines(host: &dyn ClientHost, amount: i32, lines: &mut Vec<String>) {
    let rest = unit_lines(host, &units().fallback, amount, lines);
    millibuckets(host, rest, lines);
}

pub(crate) fn shift_hint(host: &dyn ClientHost, lines: &mut Vec<String>) {
    lines.push(String::new());
    lines.push(format!("§7{}", lang(host, "gui.mantle.fluid.hold_shift", "Hold §e§oShift§r§7 for buckets")));
}

/// FluidTooltipHandler.getFluidTooltip (`with_mod`: the mod's name last, as a screen's tooltip has it): the name, then
/// its units (the fluid's own list, or buckets with Shift).
pub(crate) fn fluid_lines(host: &dyn ClientHost, fluid: &str, amount: i32, shift: bool, with_mod: bool) -> Vec<String> {
    let mut lines = vec![fluid_name(host, fluid)];
    if shift {
        bucket_lines(host, amount, &mut lines);
    } else {
        let list = units().lists.iter().find(|(f, _)| f.iter().any(|x| x == fluid)).map(|(_, u)| u.as_slice());
        let rest = unit_lines(host, list.unwrap_or(units().fallback.as_slice()), amount, &mut lines);
        millibuckets(host, rest, &mut lines);
        if list.is_some() {
            shift_hint(host, &mut lines);
        }
    }
    if with_mod {
        let ns = fluid.split_once(':').map_or("minecraft", |(n, _)| n);
        let module = match ns {
            "minecraft" => "Minecraft".to_owned(),
            "tconstruct" => "Tinkers' Construct".to_owned(),
            other => words(other),
        };
        lines.push(format!("§9§o{module}"));
    }
    lines
}

pub(crate) fn tooltip(host: &dyn ClientHost, block: &str, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) {
    *MOUSE.lock().unwrap_or_else(|e| e.into_inner()) = at;
    if block != MELTER {
        return;
    }
    let s = shown(host);
    let shift = shift_down(host);
    let d = |i| data_at(data, i);
    // GuiTankModule.renderTooltip: over the fluid, the fluid; above it, the capacity and the room in ingots.
    let [tx, ty, tw, th] = TANK_RECT;
    if hovered([tx - 1, ty - 1, tw + 2, th + 2], at) {
        let filled = (i64::from(th) * i64::from(s.tank.1) / i64::from(MELTER_CAPACITY)) as i32;
        if let (Some(fluid), true) = (&s.tank.0, at[1].floor() as i32 > ty + th - filled) {
            lines.extend(fluid_lines(host, fluid, s.tank.1, shift, true));
        } else {
            // MelterContainerMenu.TOOLTIP_FORMAT: the `tconstruct:melter` list (ingots), then buckets; with Shift,
            // buckets.
            let formatter = |amount: i32, out: &mut Vec<String>| {
                if shift {
                    bucket_lines(host, amount, out);
                } else {
                    let named = units().named.get("tconstruct:melter").map_or(&[][..], Vec::as_slice);
                    let rest = unit_lines(host, named, amount, out);
                    bucket_lines(host, rest, out);
                }
            };
            lines.push(lang(host, "gui.tconstruct.melting.capacity", "Capacity:"));
            formatter(MELTER_CAPACITY, &mut *lines);
            if s.tank.1 != MELTER_CAPACITY {
                lines.push(lang(host, "gui.tconstruct.melting.available", "Free:"));
                formatter(MELTER_CAPACITY - s.tank.1, &mut *lines);
            }
            if !shift {
                shift_hint(host, lines);
            }
        }
        return;
    }
    // GuiMeltingModule.drawHeatTooltips: why an input doesn't melt.
    for i in 0..INPUTS {
        if s.inputs.get(i).is_none_or(|id| id.is_empty()) {
            continue;
        }
        let [x, y] = MELTER_SLOTS[i];
        if !hovered([x as i32 - 5, y as i32 - 1, 4, 18], at) {
            continue;
        }
        let (current, required, temp) = (d(4 + i * 3), d(5 + i * 3), d(6 + i * 3));
        if required == 0 {
            lines.push(lang(host, "gui.tconstruct.melting.no_recipe", "Item cannot be molten."));
        } else if temp > d(2) {
            lines.push(lang(host, "gui.tconstruct.melting.no_heat", "Not enough heat to melt this item."));
        } else if current < 0 {
            lines.push(lang(host, "gui.tconstruct.melting.no_space", "Not enough free space for the fluid."));
        }
        return;
    }
    // GuiFuelModule.addTooltip.
    let [fx, fy, fw, fh] = FUEL_RECT;
    if !hovered([fx - 1, fy - 1, fw + 2, fh + 2], at) {
        return;
    }
    let temperature_line = |t: i32| format!("§7§o{}", lang(host, "gui.tconstruct.melting.fuel.temperature", "Temperature: %s°C").replacen("%s", &t.to_string(), 1));
    if !s.heater {
        match &s.fuel_tank {
            Some(((Some(fluid), amount), _)) => {
                let mut tip = fluid_lines(host, fluid, *amount, shift, true);
                let all = super::recipes::recipes(host);
                let line = match super::recipes::fluid_fuel(&all, fluid) {
                    Some(f) if f.temperature > 0 => temperature_line(f.temperature),
                    _ => format!("§c{}", lang(host, "gui.tconstruct.melting.fuel.invalid", "Invalid melting fuel!")),
                };
                tip.insert(1, line);
                lines.extend(tip);
            }
            _ => lines.push(lang(host, "gui.tconstruct.melting.fuel.empty", "No fuel found")),
        }
    } else if at[1].floor() as i32 > fy + 18 {
        let t = d(2);
        if t > 0 {
            lines.push(lang(host, "gui.tconstruct.melting.fuel.solid", "Solid fuel"));
            lines.push(temperature_line(t));
        } else {
            lines.push(lang(host, "gui.tconstruct.melting.fuel.empty", "No fuel found"));
        }
    }
}
