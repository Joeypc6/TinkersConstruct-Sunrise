//! Stage 3D: armor and shields at the hit, and soulbound.
//! - Shields (BlockingModifier, ToolEvents' shield block): a raised Tinkers' blocking tool stops `min(damage, block
//!   amount)` (the content's `minecraft:blocks_attacks` makes the game raise it and block; `living_shield_block` then
//!   caps what it stops) and wears `1 + floor(blocked)` from 3 blocked (ModifiableItem.damageItem's shield wear).
//!   The hook's host reads no entity, so each player's raised tool is noted in their tick.
//! - Protection (ProtectionModule, ArmorUtil.getDamageForEvent): the worn pieces' protection modules for the damage
//!   type, summed in each player's tick (the wearer predicates and entity variables read there), applied to the hit
//!   in `living_incoming_damage` as Java's protection on the damage after armor, capped at 20 (80 %).
//! - Armor wear (ModifiableArmorItem.damageItem through Player.hurtArmor): each worn Tinkers' piece loses
//!   `max(1, damage / 4)` of a hit armor applies to, through the modifiers' tool damage; done in the next tick.
//! - Soulbound (`tconstruct:soulbound`, ModifierEvents.onPlayerDropItems / PlayerEvent.Clone): the stacks leave the
//!   drops and come back a second after the player is alive again (an insert into a just-respawned player's
//!   inventory doesn't take: Ender IO's finding).

use super::combat::EntPred;
use super::modifier::{self as md, ModModule, ToolMods};
use super::tool::*;
use crate::smeltery::util::*;
use mod_api::serde_json::Value;
use mod_api::*;
use std::cell::RefCell;
use std::collections::HashMap;

// ---- Damage sources (Mantle's DamageSourcePredicate)

#[derive(Clone, Debug)]
pub(crate) enum DamagePred {
    Any,
    /// CAN_PROTECT: not `#minecraft:bypasses_enchantments` nor `#minecraft:bypasses_invulnerability`.
    CanProtect,
    Tag(String),
    Indirect,
    And(Vec<DamagePred>),
    Or(Vec<DamagePred>),
    Not(Box<DamagePred>),
    Never,
}

impl DamagePred {
    pub(crate) fn parse(v: &Value) -> DamagePred {
        DamagePred::parse_key(v, "type")
    }

    fn parse_key(v: &Value, key: &str) -> DamagePred {
        if v.is_null() {
            return DamagePred::Any;
        }
        let kind = v.as_str().or_else(|| v[key].as_str()).unwrap_or("");
        let list = |v: &Value| v["predicates"].as_array().into_iter().flatten().map(DamagePred::parse).collect();
        match kind.trim_start_matches("mantle:") {
            "any" | "always" => DamagePred::Any,
            "can_protect" => DamagePred::CanProtect,
            "tag" => DamagePred::Tag(v["tag"].as_str().unwrap_or("").to_owned()),
            "is_indirect" => DamagePred::Indirect,
            "and" => DamagePred::And(list(v)),
            "or" => DamagePred::Or(list(v)),
            "inverted" => DamagePred::Not(Box::new(DamagePred::parse_key(v, "inverted_type"))),
            _ => DamagePred::Never,
        }
    }

    fn matches(&self, damage_type: &str, indirect: bool) -> bool {
        match self {
            DamagePred::Any => true,
            DamagePred::CanProtect => !in_tag("minecraft:bypasses_enchantments", damage_type) && !in_tag("minecraft:bypasses_invulnerability", damage_type),
            DamagePred::Tag(t) => in_tag(t, damage_type),
            DamagePred::Indirect => indirect,
            DamagePred::And(l) => l.iter().all(|p| p.matches(damage_type, indirect)),
            DamagePred::Or(l) => l.iter().any(|p| p.matches(damage_type, indirect)),
            DamagePred::Not(p) => !p.matches(damage_type, indirect),
            DamagePred::Never => false,
        }
    }
}

/// The damage type tags the protection modules and the armor wear name (vanilla 26.3's and Tinkers' own files): no
/// API reads a damage type tag at 514ecf9.
fn in_tag(tag: &str, t: &str) -> bool {
    let t = t.strip_prefix("minecraft:").unwrap_or(t);
    let any = |l: &[&str]| l.contains(&t);
    match tag.trim_start_matches('#') {
        "minecraft:is_fire" => any(&["in_fire", "campfire", "on_fire", "lava", "hot_floor", "unattributed_fireball", "fireball"]),
        "minecraft:is_lightning" => any(&["lightning_bolt"]),
        "minecraft:is_explosion" => any(&["fireworks", "explosion", "player_explosion", "bad_respawn_point"]),
        "minecraft:is_fall" => any(&["fall", "ender_pearl", "stalagmite"]),
        "minecraft:is_projectile" => any(&["arrow", "trident", "mob_projectile", "unattributed_fireball", "fireball", "wither_skull", "thrown", "wind_charge", "spit"]),
        "minecraft:witch_resistant_to" => any(&["magic", "indirect_magic", "sonic_boom", "thorns"]),
        "minecraft:bypasses_enchantments" => any(&["sonic_boom", "tconstruct:bleeding"]),
        "minecraft:bypasses_invulnerability" => any(&["out_of_world", "generic_kill"]),
        "minecraft:bypasses_armor" => any(&[
            "on_fire", "in_wall", "cramming", "drown", "fly_into_wall", "generic", "wither", "dragon_breath", "starve", "fall", "ender_pearl", "freeze",
            "stalagmite", "magic", "indirect_magic", "out_of_world", "generic_kill", "sonic_boom", "outside_border",
        ]),
        "tconstruct:is_melee" => any(&["player_attack", "mob_attack", "mob_attack_no_aggro", "sting", "tconstruct:fluid_impact_melee", "tconstruct:fluid_spike_melee"]),
        "tconstruct:protection/blast" => in_tag("minecraft:is_explosion", t),
        "tconstruct:protection/fall" => in_tag("minecraft:is_fall", t) || t == "fly_into_wall",
        "tconstruct:protection/fire" => in_tag("minecraft:is_fire", t) || in_tag("minecraft:is_lightning", t) || t == "tconstruct:shock",
        "tconstruct:protection/magic" => in_tag("minecraft:witch_resistant_to", t) || any(&["wither", "wither_skull", "dragon_breath"]),
        "tconstruct:protection/melee" => in_tag("tconstruct:is_melee", t) || t == "cramming",
        "tconstruct:protection/projectile" => in_tag("minecraft:is_projectile", t) || any(&["falling_anvil", "falling_block", "falling_stalactite"]),
        _ => false,
    }
}

// ---- Each player's worn armor and raised shield, noted in their tick

#[derive(Default)]
pub(crate) struct Worn {
    /// Each protection module that applies to this wearer now: its damage source and what it adds.
    protection: Vec<(DamagePred, f32)>,
    armor: f32,
    toughness: f32,
    /// A Tinkers' piece is worn (it wears on hits).
    tinkers: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum Wear {
    Hand(Hand, i32),
    Armor(i32),
}

thread_local! {
    static WORN: RefCell<HashMap<i32, Worn>> = RefCell::new(HashMap::new());
    /// The raised Tinkers' blocking tool of each player: (hand, block amount).
    static SHIELD: RefCell<HashMap<i32, (Hand, f32)>> = RefCell::new(HashMap::new());
    /// The wear the damage hooks found, done in that player's next tick.
    static WEAR: RefCell<Vec<(i32, Wear)>> = const { RefCell::new(Vec::new()) };
    /// The hits the armor applies to: entity, the damage before armor.
    static HITS: RefCell<Vec<(i32, f32)>> = const { RefCell::new(Vec::new()) };
    /// Soulbound stacks of each dead player, and the ticks they have been alive since.
    static KEPT: RefCell<HashMap<u128, (Vec<ItemStack>, i32)>> = RefCell::new(HashMap::new());
}

const ARMOR_SLOTS: [EquipmentSlot; 4] = [EquipmentSlot::Head, EquipmentSlot::Chest, EquipmentSlot::Legs, EquipmentSlot::Feet];

fn blocking_tool(stack: &ItemStack) -> bool {
    tool_initialized(stack) && !tool_broken(stack) && (md::modifier_level(stack, "tconstruct:blocking") > 0 || md::modifier_level(stack, "tconstruct:parrying") > 0)
}

/// The acting player's tick (server): the pending wear, the raised shield, the worn protection, soulbound returns.
pub(crate) fn player_tick(host: &mut dyn ServerHost) {
    let Some(p) = host.players().acting_player() else { return };
    let (uuid, entity) = (p.uuid(), p.entity());
    let living = LivingRef::new(entity);
    let id = entity.id() as i32;
    let wear: Vec<Wear> = WEAR.with_borrow_mut(|w| {
        let mine = w.iter().filter(|(e, _)| *e == id).map(|(_, x)| *x).collect();
        w.retain(|(e, _)| *e != id);
        mine
    });
    for w in wear {
        match w {
            Wear::Hand(hand, n) => wear_hand(host, living, hand, n),
            Wear::Armor(n) => {
                for slot in ARMOR_SLOTS {
                    wear_slot(host, living, slot, n);
                }
            }
        }
    }
    let shield = living
        .used_item_hand(&*host)
        .and_then(|h| Some((h, living.hand_item(&*host, h)?)))
        .filter(|(_, s)| blocking_tool(s))
        .map(|(h, s)| (h, tool_stat(&s, "tconstruct:block_amount")));
    SHIELD.with_borrow_mut(|m| match shield {
        Some(s) => {
            m.insert(id, s);
        }
        None => {
            m.remove(&id);
        }
    });
    let worn = worn(&*host, living);
    WORN.with_borrow_mut(|m| {
        m.insert(id, worn);
    });
    soulbound_tick(host, uuid);
}

/// The worn pieces' protection for this wearer now (the wearer predicates and the formulas' entity variables).
fn worn(host: &dyn ServerHost, living: LivingRef) -> Worn {
    let entity = living.entity();
    let kind = entity.kind(host).and_then(|k| k.id(host)).map(|k| k.as_str().to_owned()).unwrap_or_default();
    let mut w = Worn {
        armor: living.attribute_value(host, "minecraft:armor").unwrap_or(0.0) as f32,
        toughness: living.attribute_value(host, "minecraft:armor_toughness").unwrap_or(0.0) as f32,
        ..Worn::default()
    };
    for slot in ARMOR_SLOTS {
        let Some(stack) = living.equipment(host, slot) else { continue };
        if !tool_initialized(&stack) {
            continue;
        }
        w.tinkers = true;
        if tool_broken(&stack) {
            continue;
        }
        let tm = ToolMods::of(host, &stack);
        let view = tm.view(sid(&stack));
        for (m, _, l) in tm.entries() {
            for module in &m.modules {
                let ModModule::Protection { formula, source, wearer, attacker, tool } = module else { continue };
                // The attacker can't be read at the hit (no entity reads in the damage hook): only modules for any
                // attacker apply.
                if !matches!(attacker, EntPred::Any) || !tool.matches(&tm.reg, &view) || !wearer.matches(host, entity, &kind) {
                    continue;
                }
                let ctx = |v: &md::Var| super::combat::entity_var(host, v, entity, entity);
                let value = formula.apply_ctx(&[formula.level(l), 0.0], &stack, &ctx);
                if value != 0.0 {
                    w.protection.push((source.clone(), value));
                }
            }
        }
    }
    w
}

/// ToolDamageUtil.damageAnimated on a worn piece.
fn wear_slot(host: &mut dyn ServerHost, living: LivingRef, slot: EquipmentSlot, amount: i32) {
    let Some(stack) = living.equipment(&*host, slot) else { return };
    if !tool_initialized(&stack) || tool_broken(&stack) || component(&stack, "minecraft:unbreakable").is_some() {
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
        let _ = living.set_equipment(host, slot, Some(out));
    }
}

// ---- The hit

/// CombatRules.getDamageAfterAbsorb.
fn after_armor(damage: f32, armor: f32, toughness: f32) -> f32 {
    let divisor = 2.0 + toughness / 4.0;
    let protection = (armor - damage / divisor).clamp(armor * 0.2, 20.0);
    damage * (1.0 - protection / 25.0)
}

/// The damage the armor turns into `after` (increasing in the damage: a bisection).
fn before_armor(after: f32, armor: f32, toughness: f32) -> f32 {
    if after <= 0.0 || armor <= 0.0 {
        return after;
    }
    let (mut lo, mut hi) = (after, after * 5.0 + 1.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if after_armor(mid, armor, toughness) < after { lo = mid } else { hi = mid }
    }
    (lo + hi) / 2.0
}

/// `living_incoming_damage` for a player: the worn protection (Java's on top of the armor, as the fork applies the
/// armor after this hook, the change is made on the damage before armor), and the hit noted for the armor's wear.
pub(crate) fn incoming_damage(event: &mut LivingIncomingDamageEvent<'_>) {
    if event.kind != "minecraft:player" || event.cancelled {
        return;
    }
    let ty = event.source.damage_type;
    let indirect = event.source.causing != event.source.direct;
    let bypasses_armor = in_tag("minecraft:bypasses_armor", ty);
    let found = WORN.with_borrow(|w| {
        let e = w.get(&event.entity)?;
        let value: f32 = e.protection.iter().filter(|(p, _)| p.matches(ty, indirect)).map(|(_, v)| v).sum();
        Some((value, e.armor, e.toughness, e.tinkers))
    });
    let Some((value, armor, toughness, tinkers)) = found else { return };
    let before = event.container.new_damage();
    if value != 0.0 && before > 0.0 {
        let mut damage = if bypasses_armor { before } else { after_armor(before, armor, toughness) };
        damage *= 1.0 - value.clamp(-20.0, 20.0) / 25.0;
        let damage = if bypasses_armor { damage } else { before_armor(damage, armor, toughness) };
        if damage.is_finite() {
            let _ = event.container.set_new_damage(damage.max(0.0));
        }
    }
    if tinkers && !bypasses_armor {
        HITS.with_borrow_mut(|h| {
            if h.len() > 64 {
                h.clear();
            }
            h.push((event.entity, before));
        });
    }
}

/// `living_shield_block`: a raised Tinkers' shield stops at most its block amount, and wears.
pub(crate) fn shield_block(event: &mut LivingShieldBlockEvent<'_>) {
    if !event.original_blocked || !event.block.blocked {
        return;
    }
    let Some((hand, amount)) = SHIELD.with_borrow(|m| m.get(&event.entity).copied()) else { return };
    let blocked = event.damage.min(amount.max(0.0));
    event.block.blocked_damage = blocked;
    // The game's own item wear is refused for built tools: Tinkers' wear is done in the tick.
    event.block.shield_damage = 0;
    if blocked >= 3.0 {
        WEAR.with_borrow_mut(|w| w.push((event.entity, Wear::Hand(hand, 1 + blocked.floor() as i32))));
    }
}

/// `living_damage_post` for a player: a hit the armor applied to wears each worn Tinkers' piece `max(1, damage / 4)`
/// (the damage before armor, what the shield let through).
pub(crate) fn damage_post(event: &LivingDamagePostEvent<'_>) {
    if event.kind != "minecraft:player" {
        return;
    }
    let Some(before) = HITS.with_borrow_mut(|h| h.iter().rposition(|(e, _)| *e == event.entity).map(|i| h.remove(i).1)) else { return };
    let damage = before - event.container.blocked();
    if damage <= 0.0 {
        return;
    }
    let amount = ((damage / 4.0) as i32).max(1);
    WEAR.with_borrow_mut(|w| w.push((event.entity, Wear::Armor(amount))));
}

// ---- Soulbound

fn soulbound(stack: &ItemStack) -> bool {
    tool_initialized(stack) && md::modifier_level(stack, "tconstruct:soulbound") > 0
}

/// `living_drops` of a player: soulbound stacks leave the drops.
pub(crate) fn drops(host: &mut dyn ServerHost, event: &mut LivingDropsEvent<'_>) {
    let player = event.player.or_else(|| (event.kind == "minecraft:player").then(|| { let e = host.entity(event.entity); e.and_then(|e| e.uuid(host)) }).flatten());
    let Some(player) = player else { return };
    let mut kept = Vec::new();
    event.drops.retain(|d| {
        if soulbound(&d.stack) {
            kept.push(d.stack.clone());
            false
        } else {
            true
        }
    });
    if !kept.is_empty() {
        KEPT.with_borrow_mut(|k| k.entry(player).or_insert_with(|| (Vec::new(), 0)).0.extend(kept));
    }
}

/// The player's kept stacks come back after 20 ticks alive: hotbar slot k gets stack k as the held stack of that slot
/// (the write that takes on a just-respawned player), the rest through the inventory, else dropped.
fn soulbound_tick(host: &mut dyn ServerHost, uuid: u128) {
    let Some(p) = host.player(uuid) else { return };
    let living = LivingRef::new(p.entity());
    let alive = living.alive(&*host).unwrap_or(false) && living.health(&*host).unwrap_or(0.0) > 0.0;
    let ready = KEPT.with_borrow_mut(|k| {
        let e = k.get_mut(&uuid)?;
        e.1 = if alive { e.1 + 1 } else { 0 };
        if e.1 < 20 {
            return None;
        }
        k.remove(&uuid).map(|(s, _)| s)
    });
    let Some(stacks) = ready else { return };
    let before = p.selected_slot(&*host);
    for (k, stack) in stacks.into_iter().enumerate() {
        if k < 9 && p.set_selected_slot(host, k) && living.hand_item(&*host, Hand::Main).is_none() {
            let _ = living.set_hand_item(host, Hand::Main, Some(stack));
            continue;
        }
        let r = ItemResource::of(&stack);
        let n = stack.count();
        let mut went = 0;
        for slot in 0..36 {
            if went >= n {
                break;
            }
            let place = ItemPlace::PlayerSlot { player: uuid, slot };
            went += host.transaction(|host, tx| Ok::<u32, ()>(place.insert(host, tx, &r, n - went).unwrap_or(0))).unwrap_or(0);
        }
        if let Some(rest) = with_count(&stack, n - went) {
            let _ = p.drop_stack(host, rest, false);
        }
    }
    if let Some(b) = before {
        let _ = p.set_selected_slot(host, b);
    }
}
