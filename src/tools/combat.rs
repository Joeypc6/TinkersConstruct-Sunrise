//! Stage 3D: Tinkers' attributes and the hits that read them.
//! - The attributes `tconstruct:player.experience_multiplier` and `tconstruct:generic.knockback_multiplier`
//!   (TinkerAttributes: base 1, 0 to 100), declared as real fork attributes and added to the player's set; the
//!   AttributeModule of the modifiers gives them (and vanilla's attributes) through `item_attribute_modifier`, for the
//!   slots the module names; the armor's stats are its slot's armor, toughness and knockback resistance.
//! - The effects Experienced and Ricochet (TinkerEffects: MULTIPLY_BASE 0.25 and 0.5 a level on those attributes):
//!   a content mob effect carries no attribute modifier at 514ecf9, so the effect's share is added where the attribute
//!   is read (AttributeInstance.calculateValue with that one modifier).
//! - ModifierEvents: onExperienceDrop (`living_experience_drop`), beforeBlockBreak (`break_block` + `block_drops`),
//!   onKnockback (`living_knockback`, the multiplier read where a host can: each player's tick, a player's attack,
//!   a projectile's impact).
//! - ConditionalMeleeDamageModule (smite, bane of sssss, antiaquatic, cooling, killager, scorching, charge and ram
//!   attack): the bonus is worked out in `attack_entity` (the attacker, the held tool and the target are readable
//!   there) and added to the hit in `living_incoming_damage`, scaled by the attack's charge as Java's base damage.

use super::modifier::{self as md, ModModule, ToolMods};
use super::tool::*;
use crate::smeltery::util::*;
use mod_api::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

pub(crate) const EXP_MULT: &str = "tconstruct:player.experience_multiplier";
pub(crate) const KB_MULT: &str = "tconstruct:generic.knockback_multiplier";
/// The gold the wearer counts as wearing (the chrysophilite modifier), read by the loot rules.
pub(crate) const CHRYSOPHILITE: &str = "tconstruct:generic.chrysophilite";
const EXPERIENCED: &str = "tconstruct:experienced";
const RICOCHET: &str = "tconstruct:ricochet";

thread_local! {
    /// The knockback multiplier of each entity a host could read it for (entity id -> (multiplier, kept)); a
    /// player's is kept and refreshed each tick, a mob's is used once.
    static KNOCK: RefCell<HashMap<i32, (f64, bool)>> = RefCell::new(HashMap::new());
    /// The melee bonus of the attack being run: (target, attacker, bonus, the hit's damage without it and a crit).
    static MELEE: Cell<Option<(i32, i32, f32, f32)>> = const { Cell::new(None) };
    /// The damage multiplier of the attack being run when `critical_hit` made it critical.
    static CRIT: Cell<Option<f32>> = const { Cell::new(None) };
    /// beforeBlockBreak's multiplier for the breaking player's block experience.
    static BLOCK_XP: Cell<Option<(u128, f64)>> = const { Cell::new(None) };
}

pub(crate) fn register(r: &mut dyn Registrar) {
    for path in ["player.experience_multiplier", "generic.knockback_multiplier"] {
        if let Err(e) = r.declare_attribute(path, 1.0, 0.0, 100.0, true, Sentiment::Positive, AttributeTooltip::default()) {
            eprintln!("Tinkers' Construct: attribute tconstruct:{path} refused: {e:?}");
        }
    }
    // TinkerAttributes.GOLDEN: 0 by default, up to 4 pieces' worth of levels.
    if let Err(e) = r.declare_attribute("generic.chrysophilite", 0.0, 0.0, 2048.0, true, Sentiment::Positive, AttributeTooltip::default()) {
        eprintln!("Tinkers' Construct: attribute {CHRYSOPHILITE} refused: {e:?}");
    }
    if let Err(e) = r.modify_entity_attributes("minecraft:player", vec![(EXP_MULT.to_owned(), None), (KB_MULT.to_owned(), None), (CHRYSOPHILITE.to_owned(), None)]) {
        eprintln!("Tinkers' Construct: the player's attributes refused: {e:?}");
    }
}

// ---- Attributes

/// A Forge/1.20 attribute id as 26.3 names it; None for the Tinkers' attributes the fork has no use for yet.
fn attribute_id(java: &str) -> Option<String> {
    Some(match java {
        EXP_MULT | KB_MULT | CHRYSOPHILITE => java.to_owned(),
        "forge:entity_gravity" => "minecraft:gravity".to_owned(),
        "forge:step_height_addition" => "minecraft:step_height".to_owned(),
        "forge:block_reach" => "minecraft:block_interaction_range".to_owned(),
        "forge:entity_reach" => "minecraft:entity_interaction_range".to_owned(),
        "tconstruct:generic.safe_fall_distance" => "minecraft:safe_fall_distance".to_owned(),
        "tconstruct:player.mining_speed_multiplier" => "minecraft:block_break_speed".to_owned(),
        j => format!("minecraft:{}", j.strip_prefix("minecraft:generic.").or_else(|| j.strip_prefix("minecraft:player."))?),
    })
}

fn group(slot: &str) -> Option<EquipmentSlotGroup> {
    Some(match slot {
        "mainhand" => EquipmentSlotGroup::MainHand,
        "offhand" => EquipmentSlotGroup::OffHand,
        "head" => EquipmentSlotGroup::Head,
        "chest" => EquipmentSlotGroup::Chest,
        "legs" => EquipmentSlotGroup::Legs,
        "feet" => EquipmentSlotGroup::Feet,
        _ => return None,
    })
}

/// The slot an armor piece is worn in (its content `minecraft:equippable`, by the item's name: the stack holds only
/// its patch), else None (held: both hands).
pub(crate) fn worn_slot(stack: &ItemStack) -> Option<String> {
    let id = sid(stack);
    let slot = if id.ends_with("_helmet") {
        "head"
    } else if id.ends_with("_chestplate") || id.ends_with("_wings") {
        "chest"
    } else if id.ends_with("_leggings") {
        "legs"
    } else if id.ends_with("_boots") {
        "feet"
    } else {
        return None;
    };
    Some(slot.to_owned())
}

fn modifier(attr: &str, id: String, amount: f64, op: AttributeOperation, slot: &str, mods: &mut ItemAttributeModifiers) {
    let (Ok(a), Ok(i)) = (Identifier::parse(attr), Identifier::parse(&id)) else { return };
    if let Some(g) = group(slot) {
        mods.replace(a, AttributeModifier::new(i, amount, op), g);
    }
}

/// ModifiableArmorItem's attributes (armor, toughness, knockback resistance in its slot) and every AttributeModule
/// of the modifiers, in the slots it names that the item is in (an armor piece's own, a held tool's hands).
pub(crate) fn attributes(stack: &ItemStack, mods: &mut ItemAttributeModifiers) {
    if !tool_initialized(stack) || tool_broken(stack) || stack.count() != 1 {
        return;
    }
    let worn = worn_slot(stack);
    if let Some(slot) = worn.as_deref() {
        for (stat, attr) in [("tconstruct:armor", "minecraft:armor"), ("tconstruct:armor_toughness", "minecraft:armor_toughness"), ("tconstruct:knockback_resistance", "minecraft:knockback_resistance")] {
            let v = f64::from(tool_stat(stack, stat));
            if v != 0.0 {
                modifier(attr, format!("tconstruct:{}.{slot}", attr.trim_start_matches("minecraft:")), v, AttributeOperation::AddValue, slot, mods);
            }
        }
    }
    let Some(tm) = ToolMods::cached(stack) else { return };
    let view = tm.view(sid(stack));
    for ((id, level), eff) in tm.mods.iter().zip(&tm.effective) {
        let Some(m) = tm.reg.get(id) else { continue };
        for module in &m.modules {
            let ModModule::Attribute { attribute, op, value, slots, tool, level: range } = module else { continue };
            if !(range.0..=range.1).contains(level) || !tool.matches(&tm.reg, &view) {
                continue;
            }
            let Some(attr) = attribute_id(attribute) else { continue };
            let amount = f64::from(value.at(*eff));
            let op = match op {
                1 => AttributeOperation::AddMultipliedBase,
                2 => AttributeOperation::AddMultipliedTotal,
                _ => AttributeOperation::AddValue,
            };
            let path = attr.split_once(':').map_or(attr.as_str(), |(_, p)| p);
            for slot in slots {
                let here = match worn.as_deref() {
                    Some(w) => slot == w,
                    None => slot == "mainhand" || slot == "offhand",
                };
                if here {
                    modifier(&attr, format!("{id}.{path}.{slot}"), amount, op, slot, mods);
                }
            }
        }
    }
}

/// An effect's amplifier on `living`, if it has it.
fn amplifier(host: &dyn Host, living: LivingRef, effect: &str) -> Option<i32> {
    living.effect(host, md::effect_key(host, effect)?).map(|e| e.amplifier)
}

/// A Tinkers' multiplier attribute's value on `living` with its effect's MULTIPLY_BASE share (`per` a level); 1.0
/// for an entity without the attribute and the effect.
fn multiplier(host: &dyn Host, living: LivingRef, attr: &str, effect: &str, per: f64) -> f64 {
    let base = living.attribute_value(host, attr).unwrap_or(1.0);
    let share = amplifier(host, living, effect).map_or(0.0, |a| per * f64::from(a + 1));
    (base + share).clamp(0.0, 100.0)
}

// ---- Experience (ModifierEvents)

/// onExperienceDrop: `1 + ` the dying entity's Experienced amplifier, with an attacking player `+` their experience
/// multiplier and the held tool's volatile `experience` (Java's sum: a player's kill always adds the base 1).
pub(crate) fn experience_drop(host: &mut dyn ServerHost, event: &mut LivingExperienceDropEvent<'_>) {
    let Some(e) = host.entity(event.entity) else { return };
    let mut k = 1.0 + f64::from(amplifier(&*host, LivingRef::new(e), EXPERIENCED).unwrap_or(0));
    if let Some(p) = event.attacking_player.and_then(|u| host.player(u)) {
        let living = LivingRef::new(p.entity());
        k += multiplier(&*host, living, EXP_MULT, EXPERIENCED, 0.25);
        if let Some(held) = living.hand_item(&*host, Hand::Main).filter(tool_initialized) {
            k += f64::from(md::volatile_float(&held, "tconstruct:experience"));
        }
    }
    event.amount = (f64::from(event.amount) * k) as i32;
}

/// beforeBlockBreak: the experience multiplier, the held tool's experienced level × 0.5; applied to the block's
/// experience in `block_drops`.
pub(crate) fn before_break(host: &dyn ServerHost) {
    smelting_table(host);
    let Some(p) = host.players().acting_player() else { return };
    let living = LivingRef::new(p.entity());
    let mut k = multiplier(host, living, EXP_MULT, EXPERIENCED, 0.25);
    if let Some(held) = living.hand_item(host, Hand::Main).filter(tool_initialized) {
        k += f64::from(md::modifier_level(&held, EXPERIENCED)) * 0.5;
    }
    BLOCK_XP.with(|c| c.set(Some((p.uuid(), k))));
}

/// `critical_hit` (Player.attack's CriticalHitEvent): the multiplier of a hit that ends critical.
pub(crate) fn critical_hit(event: &CriticalHitEvent<'_>) {
    CRIT.with(|c| c.set(event.critical.then_some(event.damage_multiplier)));
}

// ---- Autosmelt (AutosmeltModule.processLoot)

thread_local! {
    /// The smelting recipes by input item (output, count), for the recipe table they were read from.
    static SMELT: RefCell<(usize, std::rc::Rc<HashMap<String, (String, u32)>>)> = RefCell::new((0, std::rc::Rc::new(HashMap::new())));
}

/// Before a block breaks (a host that reads recipes): the smelting table, read again when the recipes reload.
fn smelting_table(host: &dyn ServerHost) {
    let key = crate::smeltery::util::tables_key(host, &["minecraft:smelting"])[0];
    let key = key.0 ^ key.1.rotate_left(32) ^ 0x5bd1_e995;
    if SMELT.with_borrow(|s| s.0 == key) {
        return;
    }
    let table = host.recipes_of_type("minecraft:smelting");
    let mut map = HashMap::new();
    for r in table.iter() {
        let Some(j) = crate::smeltery::recipes::chosen(host, &r.json) else { continue };
        let Some(out) = j["result"]["id"].as_str().or_else(|| j["result"].as_str()) else { continue };
        let count = j["result"]["count"].as_u64().unwrap_or(1) as u32;
        for item in crate::smeltery::recipes::ingredient_items(host, &j["ingredient"], &mut Vec::new()) {
            map.entry(item).or_insert_with(|| (out.to_owned(), count));
        }
    }
    SMELT.with_borrow_mut(|s| *s = (key, std::rc::Rc::new(map)));
}

/// `block_drops` with an autosmelt tool: each drop with a smelting recipe (not `#tconstruct:autosmelt_blacklist`)
/// becomes its result x the count; above level 1 each of `count x (L - 1)` rolls at the extra drop chance adds one
/// (not `#tconstruct:autosmelt_plus_blacklist`).
pub(crate) fn autosmelt(event: &mut BlockDropsEvent<'_>) {
    let Some(tool) = event.tool.as_ref().and_then(|t| t.project()) else { return };
    let tool = &tool;
    if event.breaker.is_none() || !tool_initialized(tool) || tool_broken(tool) {
        return;
    }
    let Some(tm) = ToolMods::cached(tool) else { return };
    let found = tm.entries().into_iter().find_map(|(m, _, eff)| {
        m.modules.iter().find_map(|x| match x {
            ModModule::Autosmelt { chance } => Some((*chance, eff)),
            _ => None,
        })
    });
    let Some((chance, eff)) = found else { return };
    let level = eff.floor() as u32;
    let map = SMELT.with_borrow(|s| s.1.clone());
    for i in 0..event.drops.len() {
        let Some(mut d) = event.drops.get_mut(i) else { continue };
        let Some((id, have)) = d.stack().map(|s| (key_id(s.item()), s.count())) else { continue };
        if tm.tags.has("tconstruct:autosmelt_blacklist", id) {
            continue;
        }
        let Some((out, n)) = map.get(id) else { continue };
        let mut count = have * n;
        if level > 1 && chance > 0.0 && !tm.tags.has("tconstruct:autosmelt_plus_blacklist", out) {
            let extra = have * (level - 1);
            count += if chance >= 1.0 { extra } else { (0..extra).filter(|_| md::rand_f32() < chance).count() as u32 };
        }
        if let Some(s) = new_stack(out, count) {
            let _ = d.set_stack(s);
        }
    }
}

pub(crate) fn block_experience(event: &mut BlockDropsEvent<'_>) {
    let Some((who, k)) = BLOCK_XP.with(Cell::get) else { return };
    if event.breaker == Some(who) && k != 1.0 {
        event.experience = (f64::from(event.experience) * k).max(0.0) as u32;
    }
}

// ---- Knockback (ModifierEvents.onKnockback)

fn note_knockback(host: &dyn Host, entity: EntityRef, kept: bool) {
    let k = multiplier(host, LivingRef::new(entity), KB_MULT, RICOCHET, 0.5);
    let id = entity.id() as i32;
    KNOCK.with(|m| {
        let mut m = m.borrow_mut();
        if k == 1.0 { m.remove(&id) } else { m.insert(id, (k, kept)) };
    });
}

/// Each player's tick: their knockback multiplier, kept for the hits they take.
pub(crate) fn player_tick(host: &dyn ServerHost) {
    if let Some(p) = host.players().acting_player() {
        note_knockback(host, p.entity(), true);
    }
}

/// A projectile hits an entity: that entity's multiplier, for the knockback that follows.
pub(crate) fn projectile_impact(host: &dyn Host, event: &ProjectileImpactEvent<'_>) {
    if let Some(e) = event.entity.and_then(|e| host.entity(e)) {
        note_knockback(host, e, false);
    }
}

/// `setStrength(strength * knockback)` when the multiplier isn't 1.
pub(crate) fn knockback(event: &mut LivingKnockbackEvent<'_>) {
    let k = KNOCK.with(|m| {
        let mut m = m.borrow_mut();
        let k = m.get(&event.entity).copied();
        if k.is_some_and(|(_, kept)| !kept) {
            m.remove(&event.entity);
        }
        k
    });
    if let Some((k, _)) = k.filter(|(k, _)| *k != 1.0) {
        event.strength = f64::from((f64::from(event.strength as f32) * k) as f32);
    }
}

// ---- Conditional melee damage

/// The Mantle living-entity predicates the melee modules use.
#[derive(Clone, Debug)]
pub(crate) enum EntPred {
    Any,
    /// `mantle:mob_type` (an entity type tag in 26.3: undead, arthropod, illager, aquatic) or `mantle:tag`.
    Tag(String),
    FireImmune,
    OnFire,
    Sprinting,
    /// `tconstruct:airborne`: not on the ground.
    Airborne,
    Crouching,
    HasEffect(String),
    Or(Vec<EntPred>),
    Never,
}

impl EntPred {
    pub(crate) fn parse(v: &mod_api::serde_json::Value) -> EntPred {
        if v.is_null() {
            return EntPred::Any;
        }
        match v["type"].as_str().or_else(|| v.as_str()).unwrap_or("") {
            "mantle:any" | "mantle:always" => EntPred::Any,
            "mantle:mob_type" => EntPred::Tag(match v["mobs"].as_str().unwrap_or("") {
                "minecraft:water" => "minecraft:aquatic".to_owned(),
                m => m.to_owned(),
            }),
            "mantle:tag" => EntPred::Tag(v["tag"].as_str().unwrap_or("").replacen("forge:", "c:", 1)),
            "mantle:fire_immune" => EntPred::FireImmune,
            "mantle:on_fire" => EntPred::OnFire,
            "mantle:sprinting" => EntPred::Sprinting,
            "tconstruct:airborne" => EntPred::Airborne,
            "mantle:crouching" => EntPred::Crouching,
            "mantle:has_effect" => EntPred::HasEffect(v["effect"].as_str().unwrap_or("").to_owned()),
            "mantle:or" => EntPred::Or(v["predicates"].as_array().into_iter().flatten().map(EntPred::parse).collect()),
            _ => EntPred::Never,
        }
    }

    pub(crate) fn matches(&self, host: &dyn Host, e: EntityRef, kind: &str) -> bool {
        match self {
            EntPred::Any => true,
            EntPred::Tag(t) => host.entity_type_in_tag(kind, t),
            EntPred::FireImmune => e.fire_immune(host).unwrap_or(false),
            EntPred::OnFire => e.fire_ticks(host).unwrap_or(0) > 0,
            EntPred::Sprinting => e.is_sprinting(host).unwrap_or(false),
            EntPred::Airborne => !e.on_ground(host).unwrap_or(true),
            EntPred::Crouching => e.is_crouching(host).unwrap_or(false),
            EntPred::HasEffect(id) => md::effect_key(host, id).is_some_and(|k| LivingRef::new(e).effect(host, k).is_some()),
            EntPred::Or(l) => l.iter().any(|p| p.matches(host, e, kind)),
            EntPred::Never => false,
        }
    }
}

/// A player attacks: the target's knockback multiplier, and the held tool's melee bonus against it
/// (MeleeDamageModifierHook: `value · level · the tool's attack damage multiplier` for each module whose predicates
/// hold), scaled by the attack's charge (Player.attack: `0.2 + charge² · 0.8`).
pub(crate) fn attack_entity(host: &dyn ServerHost, event: &AttackEntityEvent<'_>) {
    MELEE.with(|c| c.set(None));
    CRIT.with(|c| c.set(None));
    let Some(target) = (event.entity >= 0).then(|| host.entity(event.entity as u32)).flatten() else { return };
    note_knockback(host, target, false);
    let Some(p) = host.players().acting_player() else { return };
    let attacker = p.entity();
    let Some(held) = LivingRef::new(attacker).hand_item(host, Hand::Main) else { return };
    if !tool_initialized(&held) || tool_broken(&held) {
        return;
    }
    let tm = ToolMods::of(host, &held);
    let attacker_kind = attacker.kind(host).and_then(|k| k.id(host)).map(|k| k.as_str().to_owned()).unwrap_or_default();
    let mult = md::tool_multiplier(&held, ST_ATTACK_DAMAGE);
    // MeleeDamageModifierHook in the modifiers' order, from the attack damage attribute (the tool's damage).
    let base = LivingRef::new(attacker).attribute_value(host, "minecraft:attack_damage").unwrap_or(1.0) as f32;
    let ctx = |v: &md::Var| entity_var(host, v, attacker, target);
    let mut damage = base;
    for (m, _, l) in tm.entries() {
        for module in &m.modules {
            match module {
                ModModule::MeleeDamage { value, target: t, attacker: a } if t.matches(host, target, event.kind) && a.matches(host, attacker, &attacker_kind) => {
                    damage += value.at(l) * mult;
                }
                ModModule::MeleeFormula { formula, target: t, attacker: a } if t.matches(host, target, event.kind) && a.matches(host, attacker, &attacker_kind) => {
                    damage = formula.apply_ctx(&[formula.level(l), damage, mult, base], &held, &ctx);
                }
                _ => {}
            }
        }
    }
    let bonus = damage - base;
    if bonus != 0.0 {
        let charge = p.attack_strength_scale(host, 0.5).unwrap_or(1.0);
        let scale = 0.2 + charge * charge * 0.8;
        MELEE.with(|c| c.set(Some((event.entity, attacker.id() as i32, bonus * scale, base * scale))));
    }
}

/// An EntityVariable of the attacker or the target (the wearer for armor: both the same entity).
pub(crate) fn entity_var(host: &dyn Host, v: &md::Var, attacker: EntityRef, target: EntityRef) -> Option<f32> {
    let md::Var::Entity { kind, attacker: a, .. } = v else { return None };
    let e = if *a { attacker } else { target };
    let l = LivingRef::new(e);
    let filled = |slots: &mut dyn Iterator<Item = EquipmentSlot>| slots.filter(|s| l.equipment(host, *s).is_some()).count() as f32;
    Some(match kind {
        md::EntVar::FullHealth { yes, no } => {
            if l.health(host)? >= l.max_health(host)? { *yes } else { *no }
        }
        md::EntVar::EffectLevel(id) => md::effect_key(host, id).and_then(|k| l.effect(host, k)).map_or(0.0, |x| (x.amplifier + 1) as f32),
        md::EntVar::Health => l.health(host)?,
        md::EntVar::Water => {
            if e.is_in_water(host)? { 1.0 } else { 0.0 }
        }
        md::EntVar::ArmorCoverage => {
            filled(&mut [EquipmentSlot::Head, EquipmentSlot::Chest, EquipmentSlot::Legs, EquipmentSlot::Feet].into_iter()) / 4.0
        }
        md::EntVar::Temperature => {
            let level = here(host)?;
            let pos = e.position(host)?;
            let biome = level.biome_at(host, [pos[0].floor() as i32, pos[1].floor() as i32, pos[2].floor() as i32])?;
            level.biome_properties(host, &biome)?.temperature
        }
        md::EntVar::EquipmentCount(slots) => filled(&mut slots.iter().filter_map(|s| {
            Some(match s.as_str() {
                "mainhand" => EquipmentSlot::MainHand,
                "offhand" => EquipmentSlot::OffHand,
                "head" => EquipmentSlot::Head,
                "chest" => EquipmentSlot::Chest,
                "legs" => EquipmentSlot::Legs,
                "feet" => EquipmentSlot::Feet,
                _ => return None,
            })
        })),
        md::EntVar::Other => return None,
    })
}

/// The hit of that attack: its damage plus the bonus.
pub(crate) fn incoming_damage(event: &mut LivingIncomingDamageEvent<'_>) {
    let Some((target, attacker, bonus, plain)) = MELEE.with(Cell::get) else { return };
    if event.entity == target && event.source.causing == attacker && event.source.damage_type == "minecraft:player_attack" {
        MELEE.with(|c| c.set(None));
        let d = event.container.new_damage();
        // A critical hit (`critical_hit`'s multiplier) multiplies the bonus too, as ToolAttackUtil does.
        let _ = plain;
        let k = CRIT.with(|c| c.take()).unwrap_or(1.0);
        let _ = event.container.set_new_damage((d + bonus * k).max(0.0));
    }
}
