//! The modular tools (spec-tools.md 4; upstream `tconstruct.rs` 10912-12610): ToolStack's data, rebuildStats,
//! MiningSpeedToolHook, IsEffectiveToolHook, ToolHarvestLogic, ToolDamageUtil, AttributesModifierHook, the durability
//! bar and TooltipUtil, for the tools of the tool definitions.
//!
//! The data lives in the stack's components (registered in `component_types`):
//! - `tconstruct:materials`: the materials in part order (`tic_materials`);
//! - `tconstruct:stats`: each stat off its default, floats, and the harvest tier's id (`tic_stats`);
//! - `tconstruct:modifiers`: the traits, {name, level} (`tic_modifiers`; stage 3C adds the upgrades);
//! - vanilla's `minecraft:damage` and `minecraft:max_damage` (the durability, +1 when broken: ToolDamageUtil's fake
//!   max damage), so the game draws the durability bar; `minecraft:item_name` (ToolNameHook: "Wooden Pickaxe"),
//!   `minecraft:rarity`, and `minecraft:custom_model_data` (the materials as strings, broken as flag 0) for the
//!   generated item models.
//! Mining goes through `player_break_speed` and `player_harvest_check`, wear through `break_block` and `hurt_enemy`,
//! the weapon's damage and speed through `item_attribute_modifier`.

use super::data::*;
use super::modifier::{self as md, ModEntry};
use crate::smeltery::util::*;
use mod_api::*;

pub(crate) const C_MATERIALS: &str = "tconstruct:materials";
pub(crate) const C_STATS: &str = "tconstruct:stats";
pub(crate) const C_MODIFIERS: &str = "tconstruct:modifiers";

/// ToolStats: id, default, min, max.
pub(crate) const TOOL_STATS: [(&str, f32, f32, f32); 19] = [
    ("tconstruct:durability", 1.0, 1.0, 2_147_483_647.0),
    ("tconstruct:use_item_speed", 0.2, 0.0, 1.0),
    ("tconstruct:attack_damage", 0.0, 0.0, 2048.0),
    ("tconstruct:attack_speed", 1.0, 0.0, 1024.0),
    ("tconstruct:mining_speed", 1.0, 0.1, 2048.0),
    ("tconstruct:armor", 0.0, 0.0, 30.0),
    ("tconstruct:armor_toughness", 0.0, 0.0, 20.0),
    ("tconstruct:knockback_resistance", 0.0, 0.0, 1.0),
    ("tconstruct:block_amount", 5.0, 0.0, 2048.0),
    ("tconstruct:block_angle", 120.0, 0.0, 180.0),
    ("tconstruct:draw_speed", 1.0, 0.0, 1024.0),
    ("tconstruct:velocity", 1.0, 0.0, 1024.0),
    ("tconstruct:accuracy", 0.75, 0.1, 1.0),
    ("tconstruct:projectile_damage", 2.0, 0.0, 1024.0),
    ("tconstruct:water_inertia", 0.6, 0.01, 0.99),
    ("tconstruct:sea_luck", 0.0, 0.0, 1024.0),
    ("tconstruct:lure", 0.0, 0.0, 5.0),
    ("tconstruct:overslime", 0.0, 0.0, 2_147_483_647.0),
    // ToolTankHelper.CAPACITY_STAT (CapacityStat: 0 by default; the tank modifiers boost it).
    ("tconstruct:tank_capacity", 0.0, 0.0, 2_147_483_647.0),
];
pub(crate) const ST_DURABILITY: &str = "tconstruct:durability";
pub(crate) const ST_ATTACK_DAMAGE: &str = "tconstruct:attack_damage";
pub(crate) const ST_ATTACK_SPEED: &str = "tconstruct:attack_speed";
pub(crate) const ST_MINING_SPEED: &str = "tconstruct:mining_speed";
pub(crate) const ST_HARVEST_TIER: &str = "tconstruct:harvest_tier";

fn stat_info(id: &str) -> Option<(&'static str, f32, f32, f32)> {
    TOOL_STATS.iter().find(|(s, ..)| *s == id).copied()
}

// ---- Reading the tool's data

pub(crate) fn tool_materials(stack: &ItemStack) -> Vec<String> {
    match component(stack, C_MATERIALS) {
        Some(Tag::List(l)) => l.iter().map(|t| if let Tag::String(s) = t { s.to_string_lossy().to_string() } else { String::new() }).collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn tool_initialized(stack: &ItemStack) -> bool {
    component(stack, C_STATS).is_some()
}

/// StatsNBT.get: the stored value, else the stat's default.
pub(crate) fn tool_stat(stack: &ItemStack, id: &str) -> f32 {
    let stored = match component(stack, C_STATS) {
        Some(Tag::Compound(s)) => match s.get(&js(id)) {
            Some(Tag::Float(b)) => Some(f32::from_bits(*b)),
            Some(Tag::Int(i)) => Some(*i as f32),
            _ => None,
        },
        _ => None,
    };
    stored.unwrap_or_else(|| stat_info(id).map_or(0.0, |s| s.1))
}

pub(crate) fn tool_durability(stack: &ItemStack) -> i32 {
    tool_stat(stack, ST_DURABILITY) as i32
}

/// HARVEST_TIER as an index in TIERS (wood when absent).
pub(crate) fn tool_tier(stack: &ItemStack) -> usize {
    match component(stack, C_STATS) {
        Some(Tag::Compound(s)) => gs(&s, ST_HARVEST_TIER).and_then(|t| TIERS.iter().position(|x| *x == t)).unwrap_or(0),
        _ => 0,
    }
}

/// ToolStack.isBroken: the damage reached the durability.
pub(crate) fn tool_broken(stack: &ItemStack) -> bool {
    tool_initialized(stack) && stack.damage() as i32 >= tool_durability(stack)
}

pub(crate) fn tool_modifiers(stack: &ItemStack) -> Vec<Trait> {
    match component(stack, C_MODIFIERS) {
        Some(Tag::List(l)) => l.iter().filter_map(|t| if let Tag::Compound(m) = t { Some((gs(m, "name")?, gi(m, "level", 1))) } else { None }).collect(),
        _ => Vec::new(),
    }
}

// ---- Building the stats (ModifierStatsBuilder, the stat types' apply)

#[derive(Clone, Copy)]
pub(crate) struct Acc {
    base: f32,
    add: f32,
    percent: f32,
    multiply: f32,
}

#[derive(Default)]
pub(crate) struct StatsBuilder {
    stats: Vec<(&'static str, Acc)>,
    tier: Option<usize>,
    /// `tic_multipliers`: multiplyAll's and `multiply_conditional`'s factors.
    multipliers: Vec<(&'static str, f32)>,
}

impl StatsBuilder {
    fn acc(&mut self, id: &str) -> Option<&mut Acc> {
        let (sid, default, ..) = stat_info(id)?;
        let i = match self.stats.iter().position(|(s, _)| *s == sid) {
            Some(i) => i,
            None => {
                self.stats.push((sid, Acc { base: default, add: 0.0, percent: 0.0, multiply: 1.0 }));
                self.stats.len() - 1
            }
        };
        Some(&mut self.stats[i].1)
    }
    pub(crate) fn update(&mut self, id: &str, v: f32) {
        if let Some(a) = self.acc(id) {
            a.add += v;
            a.base = 0.0;
        }
    }
    pub(crate) fn add(&mut self, id: &str, v: f32) {
        if let Some(a) = self.acc(id) {
            a.add = (f64::from(a.add) + f64::from(v)) as f32;
        }
    }
    pub(crate) fn percent(&mut self, id: &str, v: f32) {
        if let Some(a) = self.acc(id) {
            a.percent = (f64::from(a.percent) + f64::from(v)) as f32;
        }
    }
    pub(crate) fn multiply(&mut self, id: &str, f: f64) {
        if let Some(a) = self.acc(id) {
            a.multiply = (f64::from(a.multiply) * f) as f32;
        }
    }
    /// multiplyAll: the stat's multiplier, recorded in `tic_multipliers` too.
    pub(crate) fn multiply_all(&mut self, id: &str, f: f64) {
        self.multiply(id, f);
        self.record_multiplier(id, f as f32);
    }
    /// StatBoostModule's `multiply_conditional`: the recorded multiplier only.
    pub(crate) fn record_multiplier(&mut self, id: &str, f: f32) {
        let Some((sid, ..)) = stat_info(id) else { return };
        match self.multipliers.iter_mut().find(|(s, _)| *s == sid) {
            Some(m) => m.1 *= f,
            None => self.multipliers.push((sid, f)),
        }
    }
    /// The value built so far (StatCopyModule), the default if untouched.
    pub(crate) fn value(&self, id: &str) -> f32 {
        match self.stats.iter().find(|(s, _)| *s == id) {
            Some((_, a)) => (a.base + a.add) * (1.0 + a.percent) * a.multiply,
            None => stat_info(id).map_or(0.0, |s| s.1),
        }
    }
    pub(crate) fn tier(&mut self, t: usize) {
        self.tier = Some(self.tier.map_or(t, |x| x.max(t)));
    }
}

fn apply_stats(b: &mut StatsBuilder, stats: &Stats, s: f32) {
    match stats {
        Stats::Head { durability, mining_speed, tier, attack } => {
            b.update(ST_DURABILITY, *durability as f32 * s);
            b.update(ST_ATTACK_DAMAGE, attack * s);
            b.update(ST_MINING_SPEED, mining_speed * s);
            b.tier(*tier);
        }
        Stats::Handle { durability, mining_speed, melee_speed, melee_damage } => {
            b.percent(ST_DURABILITY, durability * s);
            b.percent(ST_ATTACK_DAMAGE, melee_damage * s);
            b.percent(ST_ATTACK_SPEED, melee_speed * s);
            b.percent(ST_MINING_SPEED, mining_speed * s);
        }
        Stats::Limb { durability, draw_speed, velocity, accuracy } => {
            b.update(ST_DURABILITY, *durability as f32 * s);
            b.add("tconstruct:draw_speed", draw_speed * s);
            b.add("tconstruct:velocity", velocity * s);
            b.add("tconstruct:accuracy", accuracy * s);
        }
        Stats::Grip { durability, accuracy, melee_damage } => {
            b.percent(ST_DURABILITY, durability * s);
            b.add("tconstruct:accuracy", accuracy * s);
            b.update(ST_ATTACK_DAMAGE, melee_damage * s);
        }
        Stats::Plating { durability, armor, toughness, knockback } => {
            b.update(ST_DURABILITY, *durability as f32 * s);
            b.update("tconstruct:armor", armor * s);
            b.update("tconstruct:armor_toughness", toughness * s);
            b.update("tconstruct:knockback_resistance", knockback * s);
        }
        Stats::Slime { durability, .. } => {
            b.update(ST_DURABILITY, *durability as f32 * s);
            b.add("tconstruct:overslime", *durability as f32 * s);
        }
        Stats::Skull { armor, .. } => b.update("tconstruct:armor", *armor as f32 * s),
        Stats::Repair { .. } | Stats::Statless => {}
    }
}

/// What rebuildStats gives a tool.
pub(crate) struct Built {
    pub stats: Vec<(&'static str, f32)>,
    pub tier: Option<usize>,
    pub multipliers: Vec<(&'static str, f32)>,
    pub mods: md::ModBuild,
}

/// ToolStack.rebuildStats from the definition, the materials and the upgrades: the material stats per part (the
/// type's defaults for a material without it), the definition's base stats and multipliers, the traits (the
/// upgrades, the materials' per stat type, then the definition's; one id's levels summed, unknown modifiers dropped),
/// then the modifiers' volatile data, stats and enchantments (`modifier::build_mods`); the overslime capacity times
/// the durability multiplier.
pub(crate) fn build(host: &dyn Host, mats: &Materials, def: &ToolDef, item: &str, materials: &[String], upgrades: &[ModEntry], persistent: &Compound) -> Built {
    let reg = md::registry(host);
    let tags = md::item_tags(host);
    let mut b = StatsBuilder::default();
    let ids: Vec<String> = (0..def.stat_types.len())
        .map(|i| materials.get(i).and_then(|m| parse_material(m)).map_or_else(|| UNKNOWN_MATERIAL.to_owned(), |(id, _)| mats.resolve(&id).to_owned()))
        .collect();
    if !materials.is_empty() {
        for (i, stat) in def.stat_types.iter().enumerate() {
            if let Some(s) = mats.stats(&ids[i], stat).cloned().or_else(|| default_stats(stat)) {
                apply_stats(&mut b, &s, def.scales.get(i).copied().unwrap_or(1.0));
            }
        }
    }
    for (id, v) in &def.base_stats {
        b.update(id, *v);
    }
    if let Some(t) = def.base_tier {
        b.tier(t);
    }
    for (id, f) in &def.multipliers {
        b.multiply_all(id, *f);
    }
    let mut before: Vec<Trait> = Vec::new();
    let mut add = |name: &str, level: i32| {
        if reg.get(name).is_none() || level <= 0 {
            return;
        }
        match before.iter_mut().find(|(n, _)| n == name) {
            Some(e) => e.1 += level,
            None => before.push((name.to_owned(), level)),
        }
    };
    for e in upgrades {
        add(&e.id, e.level);
    }
    for (i, stat) in def.stat_types.iter().enumerate().take(materials.len()) {
        for (name, level) in mats.get(&ids[i]).map(|m| m.traits(stat)).unwrap_or_default() {
            add(name, *level);
        }
    }
    for (name, level) in &def.traits {
        add(name, *level);
    }
    let used = &ids[..ids.len().min(materials.len())];
    let rarity = used.iter().filter_map(|id| mats.get(id)).map(Material::rarity_level).max().unwrap_or(0);
    let mods = md::build_mods(&reg, &tags, item, &def.slots, upgrades, before, &mut b, rarity);
    // StatOverrideModifier.addToolStats (`/tconstruct tool_stats`): the persistent bonuses and multipliers.
    md::apply_stat_overrides(&tags, item, &mods, persistent, &mut b);
    let durability_mult = b.multipliers.iter().find(|(s, _)| *s == ST_DURABILITY).map_or(1.0, |m| m.1);
    let stats = b
        .stats
        .iter()
        .filter_map(|(id, a)| {
            let (_, default, min, max) = stat_info(id)?;
            let mut v = ((a.base + a.add) * (1.0 + a.percent) * a.multiply).clamp(min, max);
            if *id == md::ST_OVERSLIME {
                v *= durability_mult;
            }
            (v != default).then_some((*id, v))
        })
        .collect();
    Built { stats, tier: b.tier.filter(|t| *t != 0), multipliers: b.multipliers, mods }
}

/// ToolStack.createTool / rebuildStats on a stack: the materials and the upgrades it holds give the stats, the
/// modifiers, the volatile data and the enchantments; the damage is kept (at most the durability), the overslime pool
/// at most its capacity, and the derived components follow.
pub(crate) fn write_tool(host: &dyn Host, stack: &mut ItemStack, materials: &[String]) -> bool {
    let tools = tooling(host);
    let Some(def) = tools.defs.get(sid(stack)) else { return false };
    let mats = super::data::materials(host);
    let upgrades = md::entries_of(stack, md::C_UPGRADES);
    let built = build(host, &mats, def, sid(stack), materials, &upgrades, &md::persistent_compound(stack));
    set_component(stack, C_MATERIALS, Some(Tag::List(materials.iter().map(|m| Tag::String(js(m))).collect())));
    let mut stats = Compound::new();
    for (id, v) in &built.stats {
        stats.insert(js(id), Tag::Float(v.to_bits()));
    }
    if let Some(t) = built.tier {
        ss(&mut stats, ST_HARVEST_TIER, TIERS[t]);
    }
    set_component(stack, C_STATS, Some(Tag::Compound(stats)));
    let mut mult = Compound::new();
    for (id, f) in &built.multipliers {
        if *f != 1.0 {
            mult.insert(js(id), Tag::Float(f.to_bits()));
        }
    }
    set_component(stack, md::C_MULTIPLIERS, (!mult.is_empty()).then_some(Tag::Compound(mult)));
    set_component(stack, C_MODIFIERS, Some(Tag::List(built.mods.traits.iter().map(md::entry_tag).collect())));
    let mut volatile = built.mods.volatile.clone();
    if built.mods.rarity > 0 {
        si(&mut volatile, "tconstruct:rarity", built.mods.rarity);
    }
    set_component(stack, md::C_VOLATILE, (!volatile.is_empty()).then_some(Tag::Compound(volatile)));
    // EnchantmentModifierHook: the vanilla enchantments the modifiers grant, hidden (the tooltip lists the modifiers).
    let mut ench = Compound::new();
    for (e, l) in &built.mods.enchantments {
        si(&mut ench, e, *l);
    }
    let has_ench = !ench.is_empty();
    set_component(stack, "minecraft:enchantments", has_ench.then_some(Tag::Compound(ench)));
    let mut display = Compound::new();
    display.insert(js("hidden_components"), Tag::List(vec![Tag::String(js("minecraft:enchantments"))]));
    set_component(stack, "minecraft:tooltip_display", has_ench.then_some(Tag::Compound(display)));
    let rarity = ["common", "uncommon", "rare", "epic"][built.mods.rarity.clamp(0, 3) as usize];
    // Common is the tool items' own default: the game drops a component equal to its default, so the built stack
    // leaves it out to stay the same stack after a round trip (the station's placed result is compared by value).
    set_component(stack, "minecraft:rarity", (rarity != "common").then(|| Tag::String(js(rarity))));
    // ToolNameHook with `fixed_material_name`: "<material> <tool>" from the material at that index.
    let (ns, path) = sid(stack).split_once(':').unwrap_or(("minecraft", ""));
    let base = format!("item.{ns}.{path}");
    let name = def.fixed_name.and_then(|i| materials.get(i)).and_then(|m| parse_material(m)).filter(|m| m.0 != UNKNOWN_MATERIAL).map(|m| named(&base, &m));
    set_component(stack, "minecraft:item_name", name);
    let (pool, cap) = md::overslime(stack);
    if pool > cap {
        md::set_persistent_int(stack, md::ST_OVERSLIME, cap);
    }
    let damage = stack.damage() as i32;
    set_tool_damage(stack, damage);
    true
}

/// ToolStack.rebuildStats on a built tool (after its upgrades or persistent data changed).
pub(crate) fn rebuild(host: &dyn Host, stack: &mut ItemStack) -> bool {
    let list = tool_materials(stack);
    write_tool(host, stack, &list)
}

/// The tools that stack (ModifiableShurikenItem's items: 16 a stack in the content).
pub(crate) fn stackable_tool(stack: &ItemStack) -> bool {
    matches!(sid(stack), "tconstruct:shuriken" | "tconstruct:throwing_axe")
}

/// ToolStack.setDamage: at or past the durability the tool is broken (damage = durability, max damage + 1).
pub(crate) fn set_tool_damage(stack: &mut ItemStack, damage: i32) {
    let durability = tool_durability(stack).max(1);
    let broken = damage >= durability;
    // A stackable tool (shurikens and throwing axes, 16 a stack) can't carry vanilla's damage: 26.3 refuses an item
    // stack that is both damageable and stackable ("Item cannot be both damageable and stackable"), so a built stack
    // was invalid and the inventory tick's insert of it never took. Java keeps their durability in the stats only.
    if stackable_tool(stack) {
        set_component(stack, "minecraft:max_damage", None);
        set_component(stack, "minecraft:damage", None);
    } else {
        stack.set_damage(damage.clamp(0, durability) as u32);
        set_component(stack, "minecraft:max_damage", Some(Tag::Int(durability + i32::from(broken))));
    }
    // The look: the materials as strings (string i = material i) and broken as flag 0.
    let mut look = Compound::new();
    set_strings(&mut look, "strings", &tool_materials(stack));
    look.insert(js("flags"), Tag::List(vec![Tag::Byte(i8::from(broken))]));
    set_component(stack, "minecraft:custom_model_data", Some(Tag::Compound(look)));
}

// ---- Mining and harvest (MiningSpeedToolHook, IsEffectiveToolHook, ToolHarvestLogic)

/// The tag of each tier (TierSortingRegistry's, 26.3's `needs_*_tool` and `incorrect_for_*`).
fn tier_tag(i: usize) -> &'static str {
    ["c:needs_wood_tool", "c:needs_gold_tool", "minecraft:needs_stone_tool", "minecraft:needs_iron_tool", "minecraft:needs_diamond_tool", "c:needs_netherite_tool"][i.min(5)]
}

fn mining_tier(def: &ToolDef, stack: &ItemStack) -> usize {
    let t = tool_tier(stack);
    def.max_tier.map_or(t, |m| t.min(m))
}

/// IsEffectiveToolHook.isEffective: not broken, the predicate, and no higher tier's tag holds the block.
pub(crate) fn tool_effective(host: &dyn Host, def: &ToolDef, stack: &ItemStack, block: BlockKey, name: &str) -> bool {
    if tool_broken(stack) {
        return false;
    }
    let Some((pred, ignore_tier)) = &def.effective else { return false };
    if !pred.matches(host, block, name) {
        return false;
    }
    *ignore_tier || ((mining_tier(def, stack) + 1)..TIERS.len()).all(|t| !host.block_in_tag(block, tier_tag(t)).unwrap_or(false))
}

/// ModifiableItem.getDestroySpeed: 0 for several, 1 without data, 0.3 broken, else the mining speed where effective
/// (1 elsewhere) through the definition's speed modifiers, at least 1.
pub(crate) fn destroy_speed(host: &dyn Host, def: &ToolDef, stack: &ItemStack, block: BlockKey, name: &str) -> f32 {
    if stack.count() != 1 {
        return 0.0;
    }
    if !tool_initialized(stack) {
        return 1.0;
    }
    if tool_broken(stack) {
        return 0.3;
    }
    let mut speed = if tool_effective(host, def, stack, block, name) { tool_stat(stack, ST_MINING_SPEED) } else { 1.0 };
    for (pred, m) in &def.speed_mods {
        if pred.matches(host, block, name) {
            speed *= m;
        }
    }
    speed.max(1.0)
}

/// The main hand of the living entity `id`, when it is a tool with a definition.
fn held(host: &dyn Host, id: u32) -> Option<(LivingRef, ItemStack)> {
    ensure_ids(host);
    let living = LivingRef::new(host.entity(id)?);
    let stack = living.hand_item(host, Hand::Main)?;
    sid(&stack).starts_with("tconstruct:").then_some((living, stack))
}

fn block_at(host: &dyn Host, pos: Pos) -> Option<(BlockKey, String)> {
    let level = here(host)?;
    let key = level.block(host, pos)?;
    Some((key, key.id(host).unwrap_or_default().to_owned()))
}

/// `player_break_speed` (PlayerEvent.BreakSpeed; Player.getDestroySpeed): the tool's speed for the block in place
/// of its item's (1: the content's `minecraft:tool` rule has no speed). Both sides, so the crack keeps up.
pub(crate) fn break_speed(host: &dyn Host, event: &mut PlayerBreakSpeedEvent) {
    let Some((living, stack)) = held(host, event.player) else { return };
    let tools = tooling(host);
    let Some(def) = tools.defs.get(sid(&stack)) else { return };
    let Some((key, name)) = block_at(host, event.pos) else { return };
    let base = event.new_speed;
    event.new_speed *= destroy_speed(host, def, &stack, key, &name);
    // ToolEvents.onBreakSpeed: a built, unbroken tool's modifiers (BreakSpeedModifierHook).
    if stack.count() == 1 && tool_initialized(&stack) && !tool_broken(&stack) {
        let effective = tool_effective(host, def, &stack, key, &name);
        event.new_speed = md::break_speed(host, Some(living), &stack, event.pos, key, &name, effective, event.new_speed, base).max(0.0);
    }
}

/// `player_harvest_check` (Player.hasCorrectToolForDrops): a tool is correct where it is effective; where its item's
/// rule calls it correct but it isn't (a higher tier's block, or broken), not.
pub(crate) fn harvest_check(host: &dyn Host, event: &mut PlayerHarvestCheckEvent) {
    let Some((_, stack)) = held(host, event.player) else { return };
    let tools = tooling(host);
    let Some(def) = tools.defs.get(sid(&stack)) else { return };
    let Some((key, name)) = block_at(host, event.pos) else { return };
    if tool_effective(host, def, &stack, key, &name) {
        event.can_harvest = true;
    } else if def.effective.as_ref().is_some_and(|(p, _)| p.matches(host, key, &name)) && requires_tool(host, event.pos) {
        // Not correct (broken, or too low a tier): Java's isCorrectToolForDrops is false, which keeps the drops only
        // of a block that needs a correct tool. One that doesn't (requiresCorrectToolForDrops false) still drops.
        event.can_harvest = false;
    }
}

/// `BlockState.requiresCorrectToolForDrops()` of the block at `pos`. True where the game has no answer, so a tool that
/// isn't correct keeps refusing, as before the read existed.
fn requires_tool(host: &dyn Host, pos: Pos) -> bool {
    here(host).and_then(|level| level.block_state(host, pos)).and_then(|state| host.block_states().requires_correct_tool(state)).unwrap_or(true)
}

/// `block_drops` (Block.playerDestroy's harvest check, which the game asks of the item's own rule): a tool that isn't
/// effective on a block of its effective tag (too low a tier, broken) drops nothing, as Java's isCorrectToolForDrops.
/// The blocks only shears drop as themselves (their loot tables' `match_tool` asks for `minecraft:shears`).
const SHEARS_ONLY: [&str; 16] = [
    "minecraft:vine", "minecraft:cobweb", "minecraft:glow_lichen", "minecraft:short_grass", "minecraft:tall_grass",
    "minecraft:fern", "minecraft:large_fern", "minecraft:dead_bush", "minecraft:seagrass", "minecraft:hanging_roots",
    "minecraft:twisting_vines", "minecraft:weeping_vines", "minecraft:nether_sprouts", "minecraft:pale_hanging_moss",
    "minecraft:bush", "minecraft:short_dry_grass",
];

/// ShearsAbilityModifier (`tconstruct:shears`): leaves, vines, cobwebs and the plants drop themselves, as shears
/// make them (the loot tables test for vanilla's shears, which a Tinkers' tool isn't).
fn shear_drops(level: &dyn LevelHost, event: &mut BlockDropsEvent<'_>, stack: &ItemStack) {
    if super::modifier::modifier_level(stack, "tconstruct:shears") <= 0 || tool_broken(stack) {
        return;
    }
    let Some(name) = level.state_block(event.state).map(str::to_owned) else { return };
    let leaves = level.block_in_tag(event.state.block(), "minecraft:leaves").unwrap_or(false);
    if !leaves && !SHEARS_ONLY.contains(&name.as_str()) {
        return;
    }
    let item = name.clone();
    if event.drops.iter().any(|d| d.stack().is_some_and(|s| key_id(s.item()) == item)) {
        return;
    }
    let Some(stack) = new_stack(&item, 1) else { return };
    let at = [f64::from(event.pos.x) + 0.5, f64::from(event.pos.y) + 0.5, f64::from(event.pos.z) + 0.5];
    // Shears replace the plant's own drops (seeds, sticks, saplings): the loot tables' alternatives.
    event.drops.clear();
    let _ = event.drops.push(ItemDrop::new(stack, at, [0.0, 0.1, 0.0], 10));
    crate::smeltery::util::tlog(&format!("shears drop {item}"));
}

pub(crate) fn block_drops(level: &dyn LevelHost, event: &mut BlockDropsEvent<'_>) {
    let Some(stack) = event.tool.as_ref().and_then(|t| t.project()) else { return };
    let stack = &stack;
    if event.breaker.is_none() || !tool_initialized(stack) {
        return;
    }
    shear_drops(level, event, stack);
    let Some(tools) = cached_tooling() else { return };
    let Some(def) = tools.defs.get(sid(stack)) else { return };
    let key = event.state.block();
    let Some(name) = level.state_block(event.state).map(str::to_owned) else { return };
    let in_tag = |t: &str| level.block_in_tag(key, t).unwrap_or(false);
    let Some((pred, ignore_tier)) = &def.effective else { return };
    if !pred.test(&in_tag, &name) {
        return;
    }
    let tier_ok = *ignore_tier || ((mining_tier(def, stack) + 1)..TIERS.len()).all(|t| !in_tag(tier_tag(t)));
    if tool_broken(stack) || !tier_ok {
        event.drops.clear();
    }
}

/// ToolDamageUtil.damageAnimated on `living`'s main hand: not in creative, not broken; through the modifiers'
/// `tool_damage` (reinforced, overslime...), at most what is left.
pub(crate) fn wear(host: &mut dyn ServerHost, living: LivingRef, amount: i32) {
    wear_hand(host, living, Hand::Main, amount);
}

/// ToolDamageUtil.damageAnimated on the tool in `hand`.
pub(crate) fn wear_hand(host: &mut dyn ServerHost, living: LivingRef, hand: Hand, amount: i32) {
    if amount <= 0 {
        return;
    }
    let Some(stack) = living.hand_item(&*host, hand) else { return };
    let tools = tooling(&*host);
    if !tools.durability.contains(sid(&stack)) || !tool_initialized(&stack) || tool_broken(&stack) || component(&stack, "minecraft:unbreakable").is_some() {
        return;
    }
    let creative = living.entity().uuid(host).and_then(|u| host.player(u)).and_then(|p| p.is_creative(&*host)).unwrap_or(false);
    if creative {
        return;
    }
    let mut out = stack.clone();
    let amount = md::tool_damage(&*host, &mut out, amount);
    let damage = out.damage() as i32;
    if amount > 0 {
        set_tool_damage(&mut out, damage + amount.min(tool_durability(&stack) - damage));
    }
    if out != stack {
        let _ = living.set_hand_item(host, hand, Some(out));
    }
}

/// `hurt_enemy` (ToolAttackUtil.attackEntity): the hit's modifier effects and area attack, then the weapon wears 1,
/// 2 when melee isn't its main job.
pub(crate) fn hurt_enemy(host: &mut dyn ServerHost, stack: &ItemStack, target: u32) {
    let tools = tooling(&*host);
    let id = sid(stack);
    if !tools.defs.contains_key(id) || tools.unarmed.contains(id) {
        return;
    }
    let Some(player) = host.players().acting_player() else { return };
    let living = LivingRef::new(player.entity());
    if living.hand_item(&*host, Hand::Main).is_none_or(|h| sid(&h) != id) {
        return;
    }
    if tool_initialized(stack) && !tool_broken(stack) {
        md::after_hit(host, living, stack, target);
        // SpillingModule.afterMeleeHit: the tank's fluid on the target.
        if let Some(held) = living.hand_item(&*host, Hand::Main) {
            if let Some(out) = super::tank::spill(host, living, &held, target) {
                let _ = living.set_hand_item(host, Hand::Main, Some(out));
            }
        }
        super::action::area_attack(host, living, stack, target);
    }
    wear(host, living, if tools.melee_primary.contains(id) { 1 } else { 2 });
}

/// `item_attribute_modifier` (AttributesModifierHook): a built, unbroken tool's attack damage and speed as its
/// main hand's base modifiers (Item.BASE_ATTACK_DAMAGE_ID, BASE_ATTACK_SPEED_ID), as Java's tools have them.
pub(crate) fn attributes(stack: &ItemStack, modifiers: &mut ItemAttributeModifiers) {
    if !tool_initialized(stack) || tool_broken(stack) || stack.count() != 1 {
        return;
    }
    let (Ok(dmg), Ok(spd), Ok(dmg_id), Ok(spd_id)) = (
        Identifier::parse("minecraft:attack_damage"),
        Identifier::parse("minecraft:attack_speed"),
        Identifier::parse("minecraft:base_attack_damage"),
        Identifier::parse("minecraft:base_attack_speed"),
    ) else {
        return;
    };
    let damage = f64::from(tool_stat(stack, ST_ATTACK_DAMAGE));
    let speed = f64::from(tool_stat(stack, ST_ATTACK_SPEED)) - 4.0;
    if tooling_cached_melee(stack) {
        modifiers.replace(dmg, AttributeModifier::new(dmg_id, damage, AttributeOperation::AddValue), EquipmentSlotGroup::MainHand);
        modifiers.replace(spd, AttributeModifier::new(spd_id, speed, AttributeOperation::AddValue), EquipmentSlotGroup::MainHand);
    }
    super::combat::attributes(stack, modifiers);
}

/// Armor isn't a weapon: its pieces get no attack damage and speed (ModifiableArmorItem).
fn tooling_cached_melee(stack: &ItemStack) -> bool {
    super::combat::worn_slot(stack).is_none()
}

// ---- Repair (MaterialRepairToolHook, TinkerStationRepairRecipe)

/// MaterialStatsModule.getRepairAmount: the largest durability of the repairable stat types whose part is of
/// `material`.
pub(crate) fn repair_amount(host: &dyn Host, tool: &ItemStack, material: &str) -> f32 {
    let tools = tooling(host);
    let Some(def) = tools.defs.get(sid(tool)) else { return 0.0 };
    let mats = materials(host);
    let list = tool_materials(tool);
    let mut best = 0.0f32;
    for (i, stat) in def.stat_types.iter().enumerate().take(list.len()) {
        if !STAT_TYPES.iter().any(|(id, _, r)| *r && *id == stat.as_str()) {
            continue;
        }
        let Some((id, _)) = parse_material(&list[i]) else { continue };
        if mats.resolve(&id) != material {
            continue;
        }
        let d = match mats.stats(material, stat) {
            Some(Stats::Head { durability, .. } | Stats::Limb { durability, .. } | Stats::Plating { durability, .. } | Stats::Slime { durability, .. } | Stats::Skull { durability, .. }) => *durability as f32,
            Some(Stats::Repair { amount }) => *amount as f32,
            _ => 0.0,
        };
        best = best.max(d);
    }
    best
}

// ---- The tint source (MaterialRenderInfo's vertex colour)

/// The tint source `tconstruct:material`, registered under that id: the render colour of the stack's material (`index`
/// -1: a part's `Material`; i: the tool's material i), else the model's `default`. The tint entry's `index` and
/// `default` are read once when the item definition loads (`config`); the colour per stack.
pub(crate) fn register_tint(event: &mut RegisterItemTintSourcesEvent<'_>) {
    let Ok(id) = Identifier::parse("tconstruct:material") else { return };
    event.register(id, |config: &Tag| {
        let (index, default) = match config {
            Tag::Compound(c) => (gi(c, "index", -1), gi(c, "default", 0xFF_FFFF)),
            _ => (-1, 0xFF_FFFF),
        };
        Ok(move |_host: &dyn ClientHost, input: &ItemTintInput<'_>| tint(input.stack, index, default))
    });
}

fn tint(stack: &ItemStack, index: i32, default: i32) -> u32 {
    let material = if index < 0 {
        gs(&custom_data(stack), "Material")
    } else {
        tool_materials(stack).get(index as usize).cloned()
    };
    let render = &table().render;
    let color = material.and_then(|m| render.get(&m).or_else(|| render.get(m.split('#').next().unwrap_or(""))).copied());
    color.unwrap_or(default as u32 | 0xFF00_0000) | 0xFF00_0000
}

// ---- Tooltips (TooltipUtil.addInformation, ToolPartItem.appendHoverText)

fn fmt(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn stat_line(host: &dyn ClientHost, key: &str, english: &str, value: String, color: u32) -> (Text, u32) {
    (Text::literal(format!("{}§{}{}", lang(host, key, english), color_code(color), value)), 0xFFAA_AAAA)
}

/// The nearest § colour for a stat's colour (the line is one literal).
fn color_code(c: u32) -> char {
    match c & 0xFF_FFFF {
        0x47_CC47 => 'a',
        0x78_A0CD => 'b',
        0xD7_6464 => 'c',
        _ => 'f',
    }
}

fn tier_name(host: &dyn ClientHost, t: usize) -> String {
    let id = TIERS[t.min(5)].replace(':', ".");
    lang(host, &format!("stat.tconstruct.harvest_tier.{id}"), &words(TIERS[t.min(5)].split(':').nth(1).unwrap_or("")))
}

fn modifier_name(host: &dyn ClientHost, id: &str, level: i32) -> String {
    let key = format!("modifier.{}", id.replace(':', "."));
    let name = lang(host, &key, &words(id.split(':').nth(1).unwrap_or(id)));
    let roman = ["", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"];
    if level > 1 { format!("{name} {}", roman.get(level as usize).copied().unwrap_or("")) } else { name }
}

/// A tool's tooltip: the durability (or broken), the tier, speed, damage and attack speed, and the traits.
pub(crate) fn tool_tooltip(host: &dyn ClientHost, stack: &ItemStack, lines: &mut Vec<(Text, u32)>) {
    if !tool_initialized(stack) {
        // TooltipUtil.addInformation for a tool without stats (JEI, the creative tab, the guide): Java's
        // "Place in inventory to initialize", and "Randomized materials" while it has no materials either.
        if tooling(host).defs.contains_key(sid(stack)) {
            lines.push((Text::literal(lang(host, "tooltip.tconstruct.uninitialized", "Place in inventory to initialize")), 0xFFAA_AAAA));
            if tool_materials(stack).is_empty() {
                lines.push((Text::literal(lang(host, "tooltip.tconstruct.random_materials", "Randomized materials")), 0xFFAA_AAAA));
            }
        }
        return;
    }
    let durability = tool_durability(stack);
    if tool_broken(stack) {
        lines.push((Text::literal(lang(host, "tooltip.tconstruct.tool.broken", "Broken")), 0xFFFF_5555));
    }
    let current = (durability - stack.damage() as i32).max(0);
    lines.push(stat_line(host, "tool_stat.tconstruct.durability", "Durability: ", format!("{current} / {durability}"), 0x47_CC47));
    let tools = tooling(host);
    if tools.harvest.contains(sid(stack)) {
        lines.push(stat_line(host, "tool_stat.tconstruct.harvest_tier", "Mining Tier: ", tier_name(host, tool_tier(stack)), 0xFF_FFFF));
        lines.push(stat_line(host, "tool_stat.tconstruct.mining_speed", "Mining Speed: ", fmt(tool_stat(stack, ST_MINING_SPEED)), 0x78_A0CD));
    }
    lines.push(stat_line(host, "tool_stat.tconstruct.attack_damage", "Melee Damage: ", fmt(tool_stat(stack, ST_ATTACK_DAMAGE) + 1.0), 0xD7_6464));
    lines.push(stat_line(host, "tool_stat.tconstruct.attack_speed", "Melee Speed: ", fmt(tool_stat(stack, ST_ATTACK_SPEED)), 0xD7_6464));
    // TooltipUtil.addModifierNames: each modifier by its level display, in its colour.
    for (name, color) in md::modifier_lines(host, stack, md::TIP_TOOL) {
        lines.push((Text::literal(name), color));
    }
}

/// ToolPartItem.appendHoverText: the part's stats for its stat type and the material's traits for it.
pub(crate) fn part_tooltip(host: &dyn ClientHost, stack: &ItemStack, lines: &mut Vec<(Text, u32)>) {
    let Some(PartKind::Tool(stat)) = part_kind(sid(stack)) else { return };
    let mats = materials(host);
    let Some(m) = part_material(&mats, stack) else {
        lines.push((Text::literal(lang(host, "tooltip.tconstruct.part.missing_material", "Unknown material")), 0xFFAA_AAAA));
        return;
    };
    let Some(mat) = mats.get(&m.0) else { return };
    for (id, level) in mat.traits(stat) {
        lines.push((Text::literal(modifier_name(host, id, *level)), 0xFFAA_AAAA));
    }
    let pct = |v: f32| format!("{}{}%", if v >= 0.0 { "+" } else { "" }, fmt(v * 100.0));
    match mat.stats.get(stat) {
        Some(Stats::Head { durability, mining_speed, tier, attack }) => {
            lines.push(stat_line(host, "tool_stat.tconstruct.durability", "Durability: ", durability.to_string(), 0x47_CC47));
            lines.push(stat_line(host, "tool_stat.tconstruct.harvest_tier", "Mining Tier: ", tier_name(host, *tier), 0xFF_FFFF));
            lines.push(stat_line(host, "tool_stat.tconstruct.mining_speed", "Mining Speed: ", fmt(*mining_speed), 0x78_A0CD));
            lines.push(stat_line(host, "tool_stat.tconstruct.attack_damage", "Melee Damage: ", fmt(*attack), 0xD7_6464));
        }
        Some(Stats::Handle { durability, mining_speed, melee_speed, melee_damage }) => {
            lines.push(stat_line(host, "tool_stat.tconstruct.durability", "Durability: ", pct(*durability), 0x47_CC47));
            lines.push(stat_line(host, "tool_stat.tconstruct.mining_speed", "Mining Speed: ", pct(*mining_speed), 0x78_A0CD));
            lines.push(stat_line(host, "tool_stat.tconstruct.attack_speed", "Melee Speed: ", pct(*melee_speed), 0xD7_6464));
            lines.push(stat_line(host, "tool_stat.tconstruct.attack_damage", "Melee Damage: ", pct(*melee_damage), 0xD7_6464));
        }
        _ => {}
    }
}
