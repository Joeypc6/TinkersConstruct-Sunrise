//! The slime foliage's colours (SlimeColorizer, WorldClientEvents.registerBlockColorHandlers) and clear stained
//! glass (Mantle's `colored_block` model, ClearStainedGlassBlock.GlassColor), as the fork's block tint sources.
//!
//! Java colours slime grass, ferns, tall grass and potted ferns by position from each foliage type's 256 x 256
//! colormap (`textures/colormap/<type>_grass_color.png`, compiled in as `gen_colormaps.bin` by the generator); the
//! leaves and vines read it 128 blocks over (`SlimeColorizer.LOOP_OFFSET`). It is no biome colour: no resolver blend.

use mod_api::*;
use std::sync::Arc;

/// FoliageType: earth, sky, ichor, ender, blood, with their default colours (`getColor`, drawn away from a level).
const TYPES: [&str; 5] = ["earth", "sky", "ichor", "ender", "blood"];
const COLORS: [u32; 5] = [0x8C_D782, 0x00_F4DA, 0xD0_9800, 0xA9_2DFF, 0xB8_0000];
/// The dirts a slime grass grows on (`<foliage>_<dirt>_slime_grass`).
const DIRTS: [&str; 5] = ["vanilla", "earth", "sky", "ender", "ichor"];
/// GlassColor, in its order.
const GLASS: [(&str, u32); 16] = [
    ("white", 16777215), ("orange", 14188339), ("magenta", 11685080), ("light_blue", 6724056), ("yellow", 15066419),
    ("lime", 8375321), ("pink", 15892389), ("gray", 5000268), ("light_gray", 10066329), ("cyan", 5013401),
    ("purple", 8339378), ("blue", 3361970), ("brown", 6704179), ("green", 6717235), ("red", 10040115), ("black", 1644825),
];

/// The colormaps, decoded once from the generated base64 (`gen_colormaps.rs`).
fn maps() -> &'static [u8] {
    static MAPS: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    MAPS.get_or_init(|| {
        let val = |c: u8| match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };
        let src = crate::gen_colormaps::COLORMAPS_B64.as_bytes();
        let mut out = Vec::with_capacity(src.len() / 4 * 3);
        for q in src.chunks(4) {
            let n = q.iter().filter(|c| **c != b'=').count();
            let v = q.iter().take(n).fold(0u32, |acc, c| acc << 6 | u32::from(val(*c))) << (6 * (4 - n));
            out.extend_from_slice(&v.to_be_bytes()[1..n]);
        }
        out
    })
}

/// SlimeColorizer.getColor(buffer, x, z).
pub(crate) fn colormap(kind: usize, x: i32, z: i32) -> u32 {
    let fold = |p: i32| ((256.0f32 - (p.unsigned_abs() as f32) % 512.0) / 256.0).abs();
    let (mut a, mut b) = (fold(x), fold(z));
    if a < b {
        std::mem::swap(&mut a, &mut b);
    }
    let i = kind * 65536 + (((a * 255.0) as usize) << 8 | (b * 255.0) as usize);
    match maps().get(i * 3..i * 3 + 3) {
        Some(p) => u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]),
        None => COLORS[kind],
    }
}

/// getSlimeColorByPos(pos, type, add): the type's colour without a world, else the colormap's at the position
/// (moved by LOOP_OFFSET for leaves and vines).
pub(crate) struct Slime {
    kind: usize,
    offset: bool,
}

impl BlockTintSource for Slime {
    fn color(&self, _state: &BlockState, _metadata: &dyn BlockMetadata) -> u32 {
        0xFF00_0000 | COLORS[self.kind]
    }
    fn color_in_world(&self, _state: &BlockState, _metadata: &dyn BlockMetadata, _level: &dyn BlockAndTintGetter, pos: [i32; 3]) -> u32 {
        let d = if self.offset { 128 } else { 0 };
        0xFF00_0000 | colormap(self.kind, pos[0].wrapping_add(d), pos[2].wrapping_add(d))
    }
}

pub(crate) fn register_block_tints(event: &mut RegisterBlockTintSourcesEvent<'_>) {
    let id = |s: String| format!("tconstruct:{s}");
    for (kind, t) in TYPES.iter().enumerate() {
        let plain: TintLayer = Arc::new(Slime { kind, offset: false }).into();
        let shifted: TintLayer = Arc::new(Slime { kind, offset: true }).into();
        let mut blocks: Vec<String> = DIRTS.iter().map(|d| id(format!("{t}_{d}_slime_grass"))).collect();
        blocks.extend([id(format!("{t}_slime_fern")), id(format!("{t}_slime_tall_grass")), id(format!("potted_{t}_slime_fern"))]);
        event.sources.register(vec![plain], blocks);
        event.sources.register(vec![shifted.clone()], vec![id(format!("{t}_slime_leaves"))]);
        if matches!(*t, "sky" | "ender") {
            event.sources.register(vec![shifted], vec![id(format!("{t}_slime_vine"))]);
        }
    }
    for (color, rgb) in GLASS {
        let layer = TintLayer::Vanilla(VanillaTint::Constant(0xFF00_0000 | rgb));
        event.sources.register(vec![layer], vec![id(format!("{color}_clear_stained_glass")), id(format!("{color}_clear_stained_glass_pane"))]);
    }
}
