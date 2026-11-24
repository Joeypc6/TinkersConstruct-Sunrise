//! Stage 3C, the modifier core (spec-modifiers.md 1-3; upstream `tconstruct.rs` 12611-14236): ModifierManager over
//! the jar's modifier files (compiled in: `gen_tools.rs`'s `modifiers` and `modifier_tags`), ComposableModifier and the
//! modules the traits and the common modifiers use, ModifierEntry / IncrementalModifierEntry, the rebuild with
//! modifiers (ModifierTraitHook, the stat, volatile and enchantment modules) and the hooks at play (break speed, tool
//! damage, repair factor, melee hit and block break effects, the inventory tick, the tooltip's names).
//!
//! The tool's modifier data lives in its components (`tools::component_types`):
//! - `tconstruct:upgrades`: what recipes added (ModifierNBT `tic_upgrades`), {name, level[, amount, needed]};
//! - `tconstruct:modifiers`: all of them, upgrades, the traits of the parts and the definition, the traits the `trait`
//!   modules grant, sorted by priority (`tic_modifiers`);
//! - `tconstruct:persistent` (bought or paid slots, the overslime pool, swappable variants) and
//!   `tconstruct:volatile` (granted slots, flags such as `tconstruct:expanded`, the rarity), and
//!   `tconstruct:multipliers` (`tic_multipliers`);
//! - vanilla's `minecraft:enchantments` for `constant_enchantment` / `weapon_looting` (fortune, silk touch, looting),
//!   hidden from the tooltip, which lists the modifiers instead.

use super::data::*;
use super::tool::*;
use crate::smeltery::casting_recipes::tag_items;
use crate::smeltery::recipes::condition_holds;
use crate::smeltery::util::*;
use mod_api::serde_json::{self, Value};
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub(crate) const C_UPGRADES: &str = "tconstruct:upgrades";
pub(crate) const C_PERSISTENT: &str = "tconstruct:persistent";
pub(crate) const C_VOLATILE: &str = "tconstruct:volatile";
pub(crate) const C_MULTIPLIERS: &str = "tconstruct:multipliers";
pub(crate) const ST_OVERSLIME: &str = "tconstruct:overslime";
/// StatOverrideModifier (`/tconstruct tool_stats`): the slotless modifier and its persistent data's keys.
pub(crate) const STAT_OVERRIDE: &str = "tconstruct:stat_override";
pub(crate) const KEY_OVERRIDE_BONUS: &str = "tconstruct:override_bonus";
pub(crate) const KEY_OVERRIDE_MULTIPLY: &str = "tconstruct:override_multiplier";

pub(crate) const TIP_TOOL: u8 = 1;
pub(crate) const TIP_STATION: u8 = 2;
const TIP_ALL: u8 = 31;

/// ModifierTooltip: the contexts a modifier is listed in.
fn parse_tips(v: &Value) -> u8 {
    let one = |s: &str| match s {
        "always" => TIP_ALL,
        "never" => 0,
        "advanced" => TIP_STATION | 8 | 16,
        "bonus_slot" | "parts_only" => TIP_STATION | 4 | 8 | 16,
        "tool" => TIP_TOOL,
        "tinker_station" => TIP_STATION,
        _ => TIP_ALL,
    };
    match v {
        Value::String(s) => one(s),
        Value::Array(a) => a.iter().filter_map(Value::as_str).fold(0, |m, s| m | one(s)),
        _ => TIP_ALL,
    }
}

/// What a code-registered modifier does (TinkerModifiers.MODIFIERS): the traits this stage ports; the rest are
/// registered without behaviour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Code {
    Data,
    Overslime,
    Dwarven,
    Momentum,
    Insatiable,
    Lacerating,
    Overworked,
    Other,
}

/// The 28 static modifiers and their priorities (TinkerModifiers.java 377-436).
const STATIC_MODIFIERS: [(&str, Code, i32); 28] = [
    ("sinistral", Code::Other, 100),
    ("exchanging", Code::Other, 100),
    ("bursting", Code::Other, 100),
    ("wetting", Code::Other, 100),
    ("offhand_attack", Code::Other, 100),
    ("dual_wielding", Code::Other, 100),
    ("ambidextrous", Code::Other, 100),
    ("parrying", Code::Other, 100),
    ("dwarven", Code::Dwarven, 85),
    ("lacerating", Code::Lacerating, 100),
    ("overworked", Code::Overworked, 100),
    ("momentum", Code::Momentum, 75),
    ("insatiable", Code::Insatiable, 100),
    ("conducting", Code::Other, 90),
    ("enderporting", Code::Other, 45),
    ("self_destructive", Code::Other, 100),
    ("strong_bones", Code::Other, 100),
    ("plague", Code::Other, 100),
    ("tank_handler", Code::Other, 300),
    ("energy_handler", Code::Other, 100),
    ("creative_slot", Code::Other, 100),
    ("stat_override", Code::Other, 100),
    ("overslime", Code::Overslime, 150),
    ("piercing", Code::Other, 100),
    ("olympic", Code::Other, 100),
    ("wildfire", Code::Other, 100),
    ("breathtaking", Code::Other, 100),
    ("firebreath", Code::Other, 100),
];

// ---- Levelled values and formulas (LevelingValue, RandomLevelingValue, ModifierFormula, PostFixFormula)

/// LevelingValue / LevelingInt: `flat + each_level · L`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Lv {
    flat: f32,
    each: f32,
}

impl Lv {
    fn parse(v: &Value) -> Lv {
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
        Lv { flat: f("flat"), each: if v.get("each_level").is_some() { f("each_level") } else { f("count") } }
    }

    /// LevelingInt: a plain number is that much a level.
    fn int(v: &Value) -> Lv {
        match v.as_f64() {
            Some(n) => Lv { flat: 0.0, each: n as f32 },
            None if v.is_object() => Lv::parse(v),
            None => Lv { flat: 0.0, each: 1.0 },
        }
    }

    pub(crate) fn at(&self, level: f32) -> f32 {
        self.flat + self.each * level
    }
}

/// RandomLevelingValue: `flat + per_level · L + rand[0,1) · L · random`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RandomLv {
    flat: f32,
    per_level: f32,
    random: f32,
}

impl RandomLv {
    fn parse(v: &Value) -> RandomLv {
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
        RandomLv { flat: f("flat"), per_level: f("per_level"), random: f("random") }
    }

    fn at(&self, level: f32) -> f32 {
        self.flat + self.per_level * level + if self.random > 0.0 { rand_f32() * level * self.random } else { 0.0 }
    }
}

static RNG: Mutex<u64> = Mutex::new(0x2545_F491_4F6C_DD1D);

/// A float in [0, 1) (xorshift64) for the chances and random amounts.
pub(crate) fn rand_f32() -> f32 {
    let mut g = RNG.lock().unwrap_or_else(|e| e.into_inner());
    let mut x = *g;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *g = x;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// ModifierFormula.FallbackFormula for the simple form.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fallback {
    Boost,
    Percent,
    Identity,
    Add,
}

#[derive(Clone, Debug)]
pub(crate) enum Op {
    Num(f32),
    Var(usize),
    Bin(String),
    Unary(String),
}

/// The custom variables a formula names that the port reads; the others give their fallback.
#[derive(Clone, Debug)]
pub(crate) enum Var {
    Lost,
    Durability,
    Stat(String),
    Multiplier(String),
    ModData { key: String, persistent: bool },
    Const(f32),
    /// EntityVariable (`tconstruct:entity`, 3D): read from the attacker, the target or the wearer at the hit, else the
    /// fallback.
    Entity { kind: EntVar, attacker: bool, fallback: f32 },
}

/// The entity variables the port reads (`entity_type`).
#[derive(Clone, Debug)]
pub(crate) enum EntVar {
    /// `tconstruct:conditional` with `tconstruct:full_health`: `if_true` or `if_false`.
    FullHealth { yes: f32, no: f32 },
    EffectLevel(String),
    Health,
    Water,
    ArmorCoverage,
    Temperature,
    /// `tconstruct:equipment_count` over the slots named.
    EquipmentCount(Vec<String>),
    Other,
}

fn parse_var(v: &Value) -> Var {
    let kind = v.as_str().or_else(|| v["type"].as_str()).unwrap_or("");
    let fallback = v["fallback"].as_f64().unwrap_or(0.0) as f32;
    match kind.trim_start_matches("tconstruct:") {
        "tool_lost_durability" => Var::Lost,
        "tool_durability" => Var::Durability,
        "tool_stat" => Var::Stat(pattern_tc(v["stat"].as_str().unwrap_or(""))),
        "stat_multiplier" => Var::Multiplier(pattern_tc(v["stat"].as_str().unwrap_or(""))),
        "mod_data" => Var::ModData { key: pattern_tc(v["key"].as_str().unwrap_or("")), persistent: v["source"].as_str() != Some("volatile") },
        "entity" => Var::Entity {
            kind: match v["entity_type"].as_str().unwrap_or("").trim_start_matches("tconstruct:") {
                "conditional" if v["condition_type"].as_str() == Some("tconstruct:full_health") => EntVar::FullHealth {
                    yes: v["if_true"].as_f64().unwrap_or(1.0) as f32,
                    no: v["if_false"].as_f64().unwrap_or(0.0) as f32,
                },
                "effect_level" => EntVar::EffectLevel(pattern_mc(v["effect"].as_str().unwrap_or(""))),
                "health" => EntVar::Health,
                "water" => EntVar::Water,
                "armor_coverage" => EntVar::ArmorCoverage,
                "biome_temperature" => EntVar::Temperature,
                "equipment_count" => EntVar::EquipmentCount(v["slots"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_owned)).collect()),
                _ => EntVar::Other,
            },
            attacker: v["which"].as_str() == Some("attacker"),
            fallback,
        },
        _ => Var::Const(fallback),
    }
}

impl Var {
    fn value(&self, stack: &ItemStack) -> f32 {
        match self {
            Var::Lost => stack.damage() as f32,
            Var::Durability => (tool_durability(stack) - stack.damage() as i32).max(0) as f32,
            Var::Stat(s) => tool_stat(stack, s),
            Var::Multiplier(s) => tool_multiplier(stack, s),
            Var::ModData { key, persistent: true } => persistent_int(stack, key) as f32,
            Var::ModData { key, persistent: false } => volatile_int(stack, key) as f32,
            Var::Const(f) => *f,
            Var::Entity { fallback, .. } => *fallback,
        }
    }
}

/// A module's formula: the simple `{flat, each_level}` with its fallback, or a postfix formula over the built-in
/// variables then the module's own.
#[derive(Clone, Debug)]
pub(crate) struct Formula {
    simple: Option<(Lv, Fallback)>,
    ops: Vec<Op>,
    vars: Vec<Var>,
    builtins: usize,
}

impl Formula {
    pub(crate) fn parse(v: &Value, builtins: &[&str], fallback: Fallback) -> Formula {
        let Some(list) = v["formula"].as_array() else {
            let fb = if fallback == Fallback::Boost && v["percent"].as_bool() == Some(true) { Fallback::Percent } else { fallback };
            return Formula { simple: Some((Lv::parse(v), fb)), ops: Vec::new(), vars: Vec::new(), builtins: builtins.len() };
        };
        let mut names: Vec<String> = builtins.iter().map(|s| (*s).to_owned()).collect();
        let mut vars = Vec::new();
        for (k, var) in v["variables"].as_object().into_iter().flatten() {
            names.push(k.clone());
            vars.push(parse_var(var));
        }
        let ops = list
            .iter()
            .map(|t| match t {
                Value::Number(n) => Op::Num(n.as_f64().unwrap_or(0.0) as f32),
                Value::String(s) => match s.strip_prefix('$') {
                    Some(name) => Op::Var(names.iter().position(|n| n == name).unwrap_or(usize::MAX)),
                    None if matches!(s.as_str(), "negate" | "floor" | "ceil" | "sqrt" | "abs" | "non-negative" | "percent_clamp" | "swap" | "duplicate") => Op::Unary(s.clone()),
                    None => Op::Bin(s.clone()),
                },
                _ => Op::Num(0.0),
            })
            .collect();
        Formula { simple: None, ops, vars, builtins: builtins.len() }
    }

    /// processLevel: the simple form's value at the level, else the level.
    pub(crate) fn level(&self, level: f32) -> f32 {
        self.simple.as_ref().map_or(level, |(lv, _)| lv.at(level))
    }

    pub(crate) fn apply(&self, args: &[f32], stack: &ItemStack) -> f32 {
        self.apply_ctx(args, stack, &|_| None)
    }

    /// `apply` with the entity variables read by `ctx` (None: the variable's fallback).
    pub(crate) fn apply_ctx(&self, args: &[f32], stack: &ItemStack, ctx: &dyn Fn(&Var) -> Option<f32>) -> f32 {
        if let Some((_, fb)) = &self.simple {
            let a = |i: usize| args.get(i).copied().unwrap_or(0.0);
            return match fb {
                Fallback::Identity => a(0),
                Fallback::Add => a(1) + a(0),
                Fallback::Percent => a(1) * (1.0 + a(0)),
                Fallback::Boost => a(1) + a(0) * a(2),
            };
        }
        let mut all: Vec<f32> = args.iter().copied().take(self.builtins).collect();
        all.resize(self.builtins, 0.0);
        all.extend(self.vars.iter().map(|v| ctx(v).unwrap_or_else(|| v.value(stack))));
        let mut st: Vec<f32> = Vec::with_capacity(5);
        for op in &self.ops {
            match op {
                Op::Num(n) => st.push(*n),
                Op::Var(i) => st.push(all.get(*i).copied().unwrap_or(0.0)),
                Op::Unary(u) => {
                    let top = st.pop().unwrap_or(0.0);
                    match u.as_str() {
                        "negate" => st.push(-top),
                        "floor" => st.push(top.floor()),
                        "ceil" => st.push(top.ceil()),
                        "sqrt" => st.push(top.sqrt()),
                        "abs" => st.push(top.abs()),
                        "non-negative" => st.push(top.max(0.0)),
                        "percent_clamp" => st.push(top.clamp(0.0, 1.0)),
                        "duplicate" => {
                            st.push(top);
                            st.push(top);
                        }
                        _ => {
                            let second = st.pop().unwrap_or(0.0);
                            st.push(top);
                            st.push(second);
                        }
                    }
                }
                Op::Bin(b) => {
                    let right = st.pop().unwrap_or(0.0);
                    let left = st.pop().unwrap_or(0.0);
                    let t = |c: bool| if c { 1.0 } else { 0.0 };
                    st.push(match b.as_str() {
                        "+" => left + right,
                        "-" => left - right,
                        "!-" => right - left,
                        "*" => left * right,
                        "/" => if right == 0.0 { 0.0 } else { left / right },
                        "!/" => if left == 0.0 { 0.0 } else { right / left },
                        "^" => f64::from(left).powf(f64::from(right)) as f32,
                        "!^" => f64::from(right).powf(f64::from(left)) as f32,
                        "==" => t(left == right),
                        "!=" => t(left != right),
                        "<" => t(left < right),
                        "<=" => t(left <= right),
                        ">" => t(left > right),
                        ">=" => t(left >= right),
                        "min" => left.min(right),
                        "max" => left.max(right),
                        _ => 0.0,
                    });
                }
            }
        }
        if st.len() == 1 { st[0] } else { 0.0 }
    }
}

// ---- Conditions (ModifierCondition, Mantle's item predicates)

#[derive(Clone, Debug)]
pub(crate) enum ToolPred {
    Any,
    Tag(String),
    Set(Vec<String>),
    HasModifier { single: Option<String>, tag: Option<String>, upgrades: bool, min: i32, max: i32 },
    And(Vec<ToolPred>),
    Or(Vec<ToolPred>),
    Not(Box<ToolPred>),
    Never,
}

/// An IntRange field: an int is exactly it, `{min, max}` the range.
pub(crate) fn int_range(v: &Value, lo: i32, hi: i32) -> (i32, i32) {
    match v {
        Value::Number(n) => {
            let i = n.as_i64().unwrap_or(i64::from(lo)) as i32;
            (i, i)
        }
        Value::Object(_) => (v["min"].as_i64().map_or(lo, |n| n as i32), v["max"].as_i64().map_or(hi, |n| n as i32)),
        _ => (lo, hi),
    }
}

pub(crate) fn parse_tool_pred(v: &Value, key: &str) -> ToolPred {
    if v.is_null() {
        return ToolPred::Any;
    }
    let list = |k: &str| -> Vec<ToolPred> { v[k].as_array().into_iter().flatten().map(|p| parse_tool_pred(p, "type")).collect() };
    match v[key].as_str().or_else(|| v.as_str()).unwrap_or("") {
        "mantle:any" | "mantle:always" => ToolPred::Any,
        "mantle:tag" => ToolPred::Tag(pattern_id(v["tag"].as_str().unwrap_or(""))),
        "mantle:set" => ToolPred::Set(v["items"].as_array().into_iter().flatten().filter_map(|b| b.as_str().map(pattern_id)).collect()),
        "mantle:and" => ToolPred::And(list("predicates")),
        "mantle:or" => ToolPred::Or(list("predicates")),
        "mantle:inverted" => ToolPred::Not(Box::new(parse_tool_pred(v, "inverted_type"))),
        "tconstruct:has_modifier" => {
            let m = &v["modifier"];
            let (min, max) = int_range(&v["level"], 1, i32::MAX);
            ToolPred::HasModifier {
                single: (m["type"].as_str() == Some("tconstruct:single")).then(|| m["modifier"].as_str().map(pattern_tc)).flatten(),
                tag: (m["type"].as_str() == Some("tconstruct:tag")).then(|| m["tag"].as_str().map(pattern_tc)).flatten(),
                upgrades: v["check"].as_str() == Some("upgrades"),
                min,
                max,
            }
        }
        _ => ToolPred::Never,
    }
}

/// A modifier id or tag, `tconstruct:` when it has no namespace.
pub(crate) fn pattern_tc(s: &str) -> String {
    if s.contains(':') { s.to_owned() } else { format!("tconstruct:{s}") }
}

/// A namespaced id, `minecraft:` when it has none.
pub(crate) fn pattern_mc(s: &str) -> String {
    if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") }
}

/// What a tool predicate reads: the item, its modifiers and upgrades, and the item tags.
pub(crate) struct ToolView<'a> {
    pub item: &'a str,
    pub mods: &'a [Trait],
    pub upgrades: &'a [Trait],
    pub tags: &'a ItemTags,
}

impl ToolPred {
    pub(crate) fn matches(&self, reg: &Modifiers, t: &ToolView<'_>) -> bool {
        match self {
            ToolPred::Any => true,
            ToolPred::Tag(tag) => t.tags.has(tag, t.item),
            ToolPred::Set(s) => s.iter().any(|i| i == t.item),
            ToolPred::HasModifier { single, tag, upgrades, min, max } => {
                let list = if *upgrades { t.upgrades } else { t.mods };
                let level = |id: &str| list.iter().filter(|(n, _)| n == id).map(|(_, l)| *l).sum::<i32>();
                if let Some(id) = single {
                    (*min..=*max).contains(&level(id))
                } else if let Some(tag) = tag {
                    list.iter().any(|(n, l)| reg.in_tag(n, tag) && (*min..=*max).contains(l))
                } else {
                    false
                }
            }
            ToolPred::And(l) => l.iter().all(|p| p.matches(reg, t)),
            ToolPred::Or(l) => l.iter().any(|p| p.matches(reg, t)),
            ToolPred::Not(p) => !p.matches(reg, t),
            ToolPred::Never => false,
        }
    }
}

/// ModifierCondition: the tool predicate and the modifier's level range.
#[derive(Clone, Debug)]
pub(crate) struct Cond {
    tool: ToolPred,
    min: i32,
    max: i32,
}

impl Cond {
    fn parse(v: &Value) -> Cond {
        let (min, max) = int_range(&v["modifier_level"], 1, 32767);
        Cond { tool: parse_tool_pred(&v["tool"], "type"), min, max }
    }

    pub(crate) fn matches(&self, reg: &Modifiers, t: &ToolView<'_>, level: i32) -> bool {
        (self.min..=self.max).contains(&level) && self.tool.matches(reg, t)
    }
}

// ---- Modules (LM/modules/**): the ones the traits and common modifiers of tools use

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum StatOp {
    Add,
    Percent,
    MultiplyBase,
    MultiplyConditional,
    MultiplyAll,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EffectUse {
    Weapon,
    Usage,
}

#[derive(Clone, Debug)]
pub(crate) enum ModModule {
    /// SlingModule (SlingLeapModule 0, SlingTeleportModule 1, SlingKnockbackModule 2): the staffs' charged abilities.
    /// `target` 0 any, 1 on the ground looking at a block (flinging), 2 not gliding (springing).
    Sling { kind: u8, drawtime: f32, force: Lv, height: f32, bias_up: bool, leave_ground: bool, target: u8, damage: f32 },
    StatBoost { stat: String, op: StatOp, value: Lv, cond: Cond },
    SetStat { stat: String, value: f32, tier: Option<usize>, cond: Cond },
    StatCopy { target: String, source: String, value: Lv, cond: Cond },
    Slot { name: String, value: Lv, cond: Cond },
    Trait { name: String, level: i32, fixed: bool, cond: Cond },
    VolatileFlag { flag: String, cond: Cond },
    VolatileNumber { flag: String, value: Lv, float: bool, cond: Cond },
    Rarity(i32),
    Repair { formula: Formula, cond: Cond },
    ReduceDamage { formula: Formula, cond: Cond },
    DamageRange { min: i32, max: i32 },
    MiningSpeed { formula: Formula, blocks: BlockPred, require_effective: bool, cond: Cond },
    MobEffect { kind: EffectUse, effect: String, level: RandomLv, time: RandomLv, chance: f32, cond: Cond },
    Overgrowth { chance: Lv },
    Enchant { name: String, level: Lv, cond: Cond },
    Requirements { requirement: ToolPred, key: String },
    Transform { action: String, sound: String, require_ground: bool, event: i32, cond: Cond },
    FieryAttack { seconds: Lv },
    Lifesteal { percent: Lv, level: (i32, i32) },
    /// AttributeModule (3D): `attribute` (Java's id), `op` (0 add, 1 multiply base, 2 multiply total), the slots it
    /// applies in, the tool and modifier level conditions.
    Attribute { attribute: String, op: u8, value: Lv, slots: Vec<String>, tool: ToolPred, level: (i32, i32) },
    /// ConditionalMeleeDamageModule without a formula (3D): `value` a level times the tool's damage multiplier when
    /// the target and attacker predicates hold.
    MeleeDamage { value: Lv, target: super::combat::EntPred, attacker: super::combat::EntPred },
    /// ConditionalMeleeDamageModule with a formula or as a percent (3D): builtins level, damage, multiplier,
    /// base_damage.
    MeleeFormula { formula: Formula, target: super::combat::EntPred, attacker: super::combat::EntPred },
    /// ProtectionModule (3D): builtins level, protection; the damage source, wearer and attacker predicates and the
    /// tool condition.
    Protection { formula: Formula, source: super::armor::DamagePred, wearer: super::combat::EntPred, attacker: super::combat::EntPred, tool: ToolPred },
    /// `tconstruct:harvest` (HarvestModule, the kama's right click; 3D).
    Harvest,
    /// AutosmeltModule (3D): `extra_drop_chance`.
    Autosmelt { chance: f32 },
    Other,
}

fn parse_module(v: &Value) -> ModModule {
    let cond = || Cond::parse(v);
    let hooks: Option<Vec<String>> = v["hooks"].as_array().map(|a| a.iter().filter_map(|h| h.as_str().map(pattern_tc)).collect());
    let on = |hook: &str| hooks.as_ref().is_none_or(|h| h.iter().any(|x| x == hook));
    let kind = v["type"].as_str().unwrap_or("");
    let stat = |k: &str| pattern_tc(v[k].as_str().unwrap_or(""));
    match kind.trim_start_matches("tconstruct:") {
        "stat_boost" => ModModule::StatBoost {
            stat: stat("stat"),
            op: match v["operation"].as_str().unwrap_or("add") {
                "percent" => StatOp::Percent,
                "multiply_base" => StatOp::MultiplyBase,
                "multiply_conditional" => StatOp::MultiplyConditional,
                "multiply_all" => StatOp::MultiplyAll,
                _ => StatOp::Add,
            },
            value: Lv::parse(v),
            cond: cond(),
        },
        "set_stat" => ModModule::SetStat {
            stat: stat("stat"),
            value: v["value"].as_f64().unwrap_or(0.0) as f32,
            tier: v["value"].as_str().and_then(|t| TIERS.iter().position(|x| *x == pattern_mc(t))),
            cond: cond(),
        },
        "stat_copy" => ModModule::StatCopy { target: stat("target"), source: stat("source"), value: Lv::parse(v), cond: cond() },
        "modifier_slot" => ModModule::Slot { name: v["name"].as_str().unwrap_or("upgrades").to_owned(), value: Lv::parse(v), cond: cond() },
        "trait" => ModModule::Trait {
            name: pattern_tc(v["name"].as_str().unwrap_or("")),
            level: v["level"].as_i64().unwrap_or(1) as i32,
            fixed: v["fixed_level"].as_bool().unwrap_or(false),
            cond: cond(),
        },
        "volatile_flag" if on("tconstruct:volatile_data") => ModModule::VolatileFlag { flag: pattern_tc(v["flag"].as_str().unwrap_or("")), cond: cond() },
        "volatile_int" | "volatile_float" if on("tconstruct:volatile_data") => {
            ModModule::VolatileNumber { flag: pattern_tc(v["flag"].as_str().unwrap_or("")), value: Lv::parse(v), float: kind.ends_with("float"), cond: cond() }
        }
        "sweeping_edge" if on("tconstruct:volatile_data") => ModModule::VolatileNumber { flag: "tconstruct:sweep_percent".to_owned(), value: Lv::parse(v), float: true, cond: cond() },
        "rarity" => ModModule::Rarity(match v["rarity"].as_str().unwrap_or("common") {
            "uncommon" => 1,
            "rare" => 2,
            "epic" => 3,
            _ => 0,
        }),
        "repair" => ModModule::Repair { formula: Formula::parse(v, &["level", "factor"], Fallback::Percent), cond: cond() },
        "reduce_tool_damage" => ModModule::ReduceDamage { formula: Formula::parse(v, &["level"], Fallback::Identity), cond: cond() },
        "tool_damage_range" => ModModule::DamageRange { min: v["min"].as_i64().unwrap_or(0) as i32, max: v["max"].as_i64().unwrap_or(i64::from(i32::MAX)) as i32 },
        "conditional_mining_speed" => ModModule::MiningSpeed {
            formula: Formula::parse(v, &["level", "speed", "multiplier", "original_speed"], Fallback::Boost),
            blocks: match &v["blocks"] {
                Value::Null => BlockPred::Any,
                Value::String(s) if s == "tconstruct:can_melt" => BlockPred::Any,
                Value::String(_) => parse_block_pred(&serde_json::json!({ "type": v["blocks"] }), "type"),
                other => parse_block_pred(other, "type"),
            },
            require_effective: v["require_effective"].as_bool().unwrap_or(true),
            cond: cond(),
        },
        "weapon_mob_effect" | "tool_usage_mob_effect" => ModModule::MobEffect {
            kind: if kind.ends_with("weapon_mob_effect") { EffectUse::Weapon } else { EffectUse::Usage },
            effect: pattern_mc(v["effect"].as_str().unwrap_or("")),
            level: RandomLv::parse(&v["level"]),
            time: RandomLv::parse(&v["time"]),
            chance: v["chance"].as_f64().map_or(1.0, |c| c as f32),
            cond: cond(),
        },
        "overgrowth" => ModModule::Overgrowth { chance: Lv::parse(&v["chance"]) },
        "constant_enchantment" | "enchantment_ignoring_protection" if on("tconstruct:enchantments") => {
            ModModule::Enchant { name: pattern_mc(v["name"].as_str().unwrap_or("")), level: Lv::int(&v["level"]), cond: cond() }
        }
        "weapon_looting" => ModModule::Enchant { name: "minecraft:looting".to_owned(), level: Lv::int(&v["level"]), cond: cond() },
        "requirements" => ModModule::Requirements { requirement: parse_tool_pred(&v["requirement"], "type"), key: v["translation_key"].as_str().unwrap_or("").to_owned() },
        "tool_action_transform" => ModModule::Transform {
            action: v["tool_action"].as_str().unwrap_or("").to_owned(),
            sound: pattern_mc(v["sound"].as_str().unwrap_or("")),
            require_ground: v["require_ground"].as_bool().unwrap_or(false),
            event: v["event_id"].as_i64().map_or(-1, |e| e as i32),
            cond: cond(),
        },
        "fiery_attack" => ModModule::FieryAttack { seconds: Lv::parse(&v["seconds"]) },
        // GoldenAttributeModule: an attribute of the armor piece in the slot it is worn in (chrysophilite).
        "golden_attribute" => ModModule::Attribute {
            attribute: v["attribute"].as_str().unwrap_or("").to_owned(),
            op: match v["operation"].as_str().unwrap_or("addition") {
                "multiply_base" => 1,
                "multiply_total" => 2,
                _ => 0,
            },
            value: Lv::parse(v),
            slots: ["head", "chest", "legs", "feet"].map(str::to_owned).to_vec(),
            tool: parse_tool_pred(&Value::Null, "type"),
            level: int_range(&v["modifier_level"], 1, 32767),
        },
        "attribute" if v["formula"].is_null() => ModModule::Attribute {
            attribute: v["attribute"].as_str().unwrap_or("").to_owned(),
            op: match v["operation"].as_str().unwrap_or("addition") {
                "multiply_base" => 1,
                "multiply_total" => 2,
                _ => 0,
            },
            value: Lv::parse(v),
            slots: v["slots"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_owned)).collect(),
            tool: parse_tool_pred(&v["tool"], "type"),
            level: int_range(&v["modifier_level"], 1, 32767),
        },
        "conditional_melee_damage" if v["formula"].is_null() && v["percent"].as_bool() != Some(true) => ModModule::MeleeDamage {
            value: Lv::parse(v),
            target: super::combat::EntPred::parse(&v["target"]),
            attacker: super::combat::EntPred::parse(&v["attacker"]),
        },
        "conditional_melee_damage" => ModModule::MeleeFormula {
            formula: Formula::parse(v, &["level", "damage", "multiplier", "base_damage"], Fallback::Boost),
            target: super::combat::EntPred::parse(&v["target"]),
            attacker: super::combat::EntPred::parse(&v["attacker"]),
        },
        "protection" => ModModule::Protection {
            formula: Formula::parse(v, &["level", "protection"], Fallback::Add),
            source: super::armor::DamagePred::parse(&v["damage_source"]),
            wearer: super::combat::EntPred::parse(&v["wearing_entity"]),
            attacker: super::combat::EntPred::parse(&v["attacker"]),
            tool: parse_tool_pred(&v["tool"], "type"),
        },
        "harvest" => ModModule::Harvest,
        k @ ("sling_leap" | "sling_teleport" | "sling_knockback") => {
            let target = match v["target"]["type"].as_str().unwrap_or("") {
                "mantle:and" => 1,
                "mantle:inverted" => 2,
                _ => 0,
            };
            ModModule::Sling {
                kind: match k { "sling_leap" => 0, "sling_teleport" => 1, _ => 2 },
                drawtime: v["drawtime_multiplier"].as_f64().unwrap_or(1.0) as f32,
                force: Lv::parse(&v["force_multiplier"]),
                height: v["height_reduction"].as_f64().unwrap_or(1.0).max(0.01) as f32,
                bias_up: v["bias_upwards"].as_bool().unwrap_or(false),
                leave_ground: v["leave_ground"].as_bool().unwrap_or(false),
                target,
                damage: v["damage_multiplier"].as_f64().unwrap_or(0.0) as f32,
            }
        }
        "autosmelt" => ModModule::Autosmelt { chance: v["extra_drop_chance"].as_f64().unwrap_or(0.0) as f32 },
        "lifesteal" => ModModule::Lifesteal { percent: Lv::parse(&v["percentage"]), level: int_range(&v["modifier_level"], 1, 32767) },
        _ => ModModule::Other,
    }
}

/// A Modifier (ComposableModifier, or a code one).
pub(crate) struct Modifier {
    pub priority: i32,
    pub display: Value,
    pub tips: u8,
    pub key: String,
    pub modules: Vec<ModModule>,
    pub code: Code,
}

/// ComposableModifier.computePriority.
fn module_priority(v: &Value) -> Option<i32> {
    match v["type"].as_str().unwrap_or("").trim_start_matches("tconstruct:") {
        "modifier_slot" | "stat_copy" | "swappable_slot" => Some(50),
        "projectile_explosion" => Some(25),
        "stat_tooltip" => Some(1000),
        "conditional_melee_damage" | "conditional_mining_speed" | "conditional_stat" | "conditional_power" if v["percent"].as_bool() == Some(true) => Some(75),
        _ => None,
    }
}

fn parse_modifier(id: &str, doc: &Value) -> Modifier {
    let modules: Vec<&Value> = doc["modules"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let priority = doc["priority"].as_i64().map(|p| p as i32).or_else(|| modules.iter().find_map(|m| module_priority(m))).unwrap_or(100);
    let (ns, path) = id.split_once(':').unwrap_or(("tconstruct", id));
    Modifier {
        priority,
        display: doc.get("level_display").cloned().unwrap_or(Value::Null),
        tips: parse_tips(doc.get("show_in_tooltips").or_else(|| doc.get("tooltip_display")).unwrap_or(&Value::Null)),
        key: doc["translation_key"].as_str().map_or_else(|| format!("modifier.{ns}.{path}"), str::to_owned),
        modules: modules.into_iter().map(parse_module).collect(),
        code: Code::Data,
    }
}

/// The modifier registry of one data load, and the sorted ids (a modifier's index reaches a client's screen in a data
/// slot).
#[derive(Default)]
pub(crate) struct Modifiers {
    map: HashMap<String, Arc<Modifier>>,
    redirects: HashMap<String, String>,
    tags: HashMap<String, HashSet<String>>,
    pub ids: Vec<String>,
}

impl Modifiers {
    pub(crate) fn get(&self, id: &str) -> Option<&Arc<Modifier>> {
        self.map.get(self.redirects.get(id).map_or(id, String::as_str))
    }

    pub(crate) fn in_tag(&self, id: &str, tag: &str) -> bool {
        self.tags.get(tag).is_some_and(|t| t.contains(self.redirects.get(id).map_or(id, String::as_str)))
    }

    pub(crate) fn priority(&self, id: &str) -> i32 {
        self.get(id).map_or(100, |m| m.priority)
    }

    /// The id a redirect leads to (ModifierManager.getValue's modifier id), else `id`.
    pub(crate) fn resolve(&self, id: &str) -> String {
        self.redirects.get(id).cloned().unwrap_or_else(|| id.to_owned())
    }

    pub(crate) fn index(&self, id: &str) -> i32 {
        self.ids.binary_search_by(|x| x.as_str().cmp(id)).map_or(-1, |i| i as i32)
    }
}

/// ModifierManager over the jar's files: the static modifiers, then the data ones (replacing a static id), the
/// redirects whose condition holds, the modifier tags.
fn build_modifiers(host: &dyn Host) -> Modifiers {
    let mut out = Modifiers::default();
    for (path, code, priority) in STATIC_MODIFIERS {
        out.map.insert(format!("tconstruct:{path}"), Arc::new(Modifier { priority, display: Value::Null, tips: TIP_ALL, key: format!("modifier.tconstruct.{path}"), modules: Vec::new(), code }));
    }
    let t = &table().json;
    let holds = |c: &Value| c.is_null() || condition_holds(host, c);
    let mut redirects = Vec::new();
    for (id, doc) in t["modifiers"].as_object().into_iter().flatten() {
        if let Some(list) = doc["redirects"].as_array() {
            if let Some(to) = list.iter().find(|e| holds(&e["condition"])).and_then(|e| e["id"].as_str()) {
                redirects.push((id.clone(), pattern_tc(to)));
            }
            continue;
        }
        if !holds(&doc["condition"]) {
            continue;
        }
        let code = out.map.get(id).map_or(Code::Data, |m| m.code);
        let mut m = parse_modifier(id, doc);
        if code != Code::Data && m.modules.is_empty() {
            m.code = code;
        }
        out.map.insert(id.clone(), Arc::new(m));
    }
    for (from, to) in redirects {
        if out.map.contains_key(&to) {
            out.redirects.insert(from, to);
        }
    }
    let raw: HashMap<&str, &Value> = t["modifier_tags"].as_object().map(|o| o.iter().map(|(k, v)| (k.as_str(), v)).collect()).unwrap_or_default();
    fn members(raw: &HashMap<&str, &Value>, tag: &str, seen: &mut HashSet<String>, out: &mut HashSet<String>) {
        if !seen.insert(tag.to_owned()) {
            return;
        }
        for v in raw.get(tag).and_then(|t| t["values"].as_array()).into_iter().flatten() {
            let Some(s) = v.as_str().or_else(|| v["id"].as_str()) else { continue };
            match s.strip_prefix('#') {
                Some(t) => members(raw, t, seen, out),
                None => {
                    out.insert(s.to_owned());
                }
            }
        }
    }
    for tag in raw.keys() {
        let mut set = HashSet::new();
        members(&raw, tag, &mut HashSet::new(), &mut set);
        out.tags.insert((*tag).to_owned(), set);
    }
    let mut ids: Vec<String> = out.map.keys().cloned().collect();
    ids.sort();
    out.ids = ids;
    out
}

static REGISTRY: Mutex<Option<(Key, Arc<Modifiers>)>> = Mutex::new(None);

/// The registry and tags of the last data load, for hooks with no host (`item_attribute_modifier`).
pub(crate) fn cached() -> Option<(Arc<Modifiers>, Arc<ItemTags>)> {
    let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(_, m)| m.clone())?;
    let t = TAGS.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    Some((r, t))
}

/// The modifier registry, read again on a new data load (its load conditions read the game's tags).
pub(crate) fn registry(host: &dyn Host) -> Arc<Modifiers> {
    let key = recipes_key(host, &["tconstruct:material"]);
    if let Some(m) = REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, m)| m.clone()) {
        return m;
    }
    let out = Arc::new(build_modifiers(host));
    *REGISTRY.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, out.clone()));
    out
}

/// The item tags the conditions read, filled as asked, for one data load.
#[derive(Default)]
pub(crate) struct ItemTags {
    sets: Mutex<HashMap<String, HashSet<String>>>,
    host_key: Key,
}

impl ItemTags {
    pub(crate) fn has(&self, tag: &str, item: &str) -> bool {
        self.sets.lock().unwrap_or_else(|e| e.into_inner()).get(tag).is_some_and(|s| s.contains(item))
    }

    /// Fills the tags `pred`-like lists name (called with a host before a `has`).
    fn fill(&self, host: &dyn Host, tags: &[&str]) {
        let mut g = self.sets.lock().unwrap_or_else(|e| e.into_inner());
        for t in tags {
            if !g.contains_key(*t) {
                g.insert((*t).to_owned(), tag_items(host, t));
            }
        }
    }
}

static TAGS: Mutex<Option<Arc<ItemTags>>> = Mutex::new(None);

/// The tool tags of the conditions, filled for the registry's predicates on a new data load.
pub(crate) fn item_tags(host: &dyn Host) -> Arc<ItemTags> {
    let key = recipes_key(host, &["tconstruct:material", "tconstruct:tool_building"]);
    let mut g = TAGS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = g.as_ref().filter(|t| t.host_key == key) {
        return t.clone();
    }
    let t = Arc::new(ItemTags { sets: Mutex::new(HashMap::new()), host_key: key });
    // Every tag a module's tool predicate names, and the stats' tags (IToolStat.supports).
    let mut names: HashSet<String> = ["durability", "held", "melee", "melee/weapon", "harvest", "armor", "ranged", "ranged/launcher", "ammo", "fishing_rods"].iter().map(|s| format!("tconstruct:modifiable/{s}")).collect();
    fn walk(v: &Value, out: &mut HashSet<String>) {
        match v {
            Value::Object(o) => {
                if o.get("type").and_then(Value::as_str) == Some("mantle:tag") || o.get("inverted_type").and_then(Value::as_str) == Some("mantle:tag") {
                    if let Some(t) = o.get("tag").and_then(Value::as_str) {
                        out.insert(pattern_id(t));
                    }
                }
                o.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    walk(&table().json["modifiers"], &mut names);
    let list: Vec<&str> = names.iter().map(String::as_str).collect();
    t.fill(host, &list);
    *g = Some(t.clone());
    t
}

// ---- Entries (ModifierEntry, IncrementalModifierEntry, ModifierNBT)

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ModEntry {
    pub id: String,
    pub level: i32,
    /// An unfinished incremental level: `amount` of `needed` (needed 0: a plain entry).
    pub amount: i32,
    pub needed: i32,
}

impl ModEntry {
    /// IncrementalModifierEntry.of.
    pub(crate) fn of(id: &str, level: i32, amount: i32, needed: i32) -> ModEntry {
        if needed <= 0 || amount >= needed {
            ModEntry { id: id.to_owned(), level, amount: 0, needed: 0 }
        } else if amount <= 0 {
            ModEntry { id: id.to_owned(), level: level - 1, amount: 0, needed: 0 }
        } else {
            ModEntry { id: id.to_owned(), level, amount, needed }
        }
    }

    pub(crate) fn effective(&self) -> f32 {
        if self.needed > 0 { self.level as f32 + (self.amount - self.needed) as f32 / self.needed as f32 } else { self.level as f32 }
    }

    pub(crate) fn int_effective(&self) -> i32 {
        if self.needed > 0 { self.level - 1 } else { self.level }
    }

    pub(crate) fn amount_or(&self, fallback: i32) -> i32 {
        if self.needed > 0 { self.amount } else { fallback }
    }
}

/// A modifier list component (ModifierEntry.readFromNBT: a name and a level above 0, then the increment).
pub(crate) fn entries_of(stack: &ItemStack, comp: &str) -> Vec<ModEntry> {
    let Some(Tag::List(list)) = component(stack, comp) else { return Vec::new() };
    list.iter()
        .filter_map(|t| match t {
            Tag::Compound(m) => {
                let name = gs(m, "name")?;
                let level = gi(m, "level", 0);
                (level > 0).then(|| ModEntry::of(&name, level, gi(m, "amount", 0), gi(m, "needed", 0)))
            }
            _ => None,
        })
        .filter(|e| e.level > 0)
        .collect()
}

pub(crate) fn entry_tag(e: &ModEntry) -> Tag {
    let mut c = Compound::new();
    ss(&mut c, "name", &e.id);
    si(&mut c, "level", e.level);
    if e.needed > 0 {
        si(&mut c, "amount", e.amount);
        si(&mut c, "needed", e.needed);
    }
    Tag::Compound(c)
}

pub(crate) fn set_upgrades(stack: &mut ItemStack, list: &[ModEntry]) {
    set_component(stack, C_UPGRADES, (!list.is_empty()).then(|| Tag::List(list.iter().map(entry_tag).collect())));
}

/// IToolStackView.getModifierLevel over all the modifiers.
pub(crate) fn modifier_level(stack: &ItemStack, id: &str) -> i32 {
    tool_modifiers(stack).into_iter().filter(|(n, _)| n == id).map(|(_, l)| l).sum()
}

// ---- Persistent and volatile data (ModDataNBT)

fn data_of(stack: &ItemStack, comp: &str) -> Compound {
    match component(stack, comp) {
        Some(Tag::Compound(c)) => c,
        _ => Compound::new(),
    }
}

fn num(c: &Compound, key: &str) -> Option<f32> {
    match c.get(&js(key)) {
        Some(Tag::Int(i)) => Some(*i as f32),
        Some(Tag::Float(b)) => Some(f32::from_bits(*b)),
        Some(Tag::Byte(b)) => Some(f32::from(*b)),
        _ => None,
    }
}

/// The tool's persistent data (ModDataNBT), empty when it has none.
pub(crate) fn persistent_compound(stack: &ItemStack) -> Compound {
    data_of(stack, C_PERSISTENT)
}

/// A persistent value as stored.
pub(crate) fn persistent_value(stack: &ItemStack, key: &str) -> Option<Tag> {
    data_of(stack, C_PERSISTENT).get(&js(key)).cloned()
}

/// TagUtil.isNumeric then NumericTag.getAsFloat.
pub(crate) fn tag_number(t: &Tag) -> Option<f32> {
    Some(match t {
        Tag::Byte(b) => f32::from(*b),
        Tag::Short(s) => f32::from(*s),
        Tag::Int(i) => *i as f32,
        Tag::Long(l) => *l as f32,
        Tag::Float(b) => f32::from_bits(*b),
        Tag::Double(b) => f64::from_bits(*b) as f32,
        _ => return None,
    })
}

pub(crate) fn persistent_int(stack: &ItemStack, key: &str) -> i32 {
    gi(&data_of(stack, C_PERSISTENT), key, 0)
}

pub(crate) fn volatile_int(stack: &ItemStack, key: &str) -> i32 {
    num(&data_of(stack, C_VOLATILE), key).map_or(0, |f| f as i32)
}

pub(crate) fn volatile_float(stack: &ItemStack, key: &str) -> f32 {
    num(&data_of(stack, C_VOLATILE), key).unwrap_or(0.0)
}

/// A persistent value (None removes it; an empty compound goes).
pub(crate) fn set_persistent(stack: &mut ItemStack, key: &str, value: Option<Tag>) {
    let mut p = data_of(stack, C_PERSISTENT);
    match value {
        Some(v) => {
            p.insert(js(key), v);
        }
        None => {
            p.remove(&js(key));
        }
    }
    set_component(stack, C_PERSISTENT, (!p.is_empty()).then_some(Tag::Compound(p)));
}

pub(crate) fn set_persistent_int(stack: &mut ItemStack, key: &str, value: i32) {
    set_persistent(stack, key, (value != 0).then_some(Tag::Int(value)));
}

pub(crate) fn persistent_string(stack: &ItemStack, key: &str) -> Option<String> {
    gs(&data_of(stack, C_PERSISTENT), key)
}

/// IToolStackView.getMultiplier: 1 when absent.
pub(crate) fn tool_multiplier(stack: &ItemStack, id: &str) -> f32 {
    num(&data_of(stack, C_MULTIPLIERS), id).unwrap_or(1.0)
}

/// getFreeSlots: the persistent (bought or paid) plus the volatile (granted).
pub(crate) fn free_slots(stack: &ItemStack, slot: &str) -> i32 {
    persistent_int(stack, slot) + volatile_int(stack, slot)
}

pub(crate) fn add_persistent_slots(stack: &mut ItemStack, slot: &str, delta: i32) {
    let v = persistent_int(stack, slot) + delta;
    set_persistent_int(stack, slot, v);
}

/// The overslime pool and capacity.
pub(crate) fn overslime(stack: &ItemStack) -> (i32, i32) {
    (persistent_int(stack, ST_OVERSLIME), tool_stat(stack, ST_OVERSLIME) as i32)
}

/// OverslimeModule.addAmount: × (1 + overworked), capped at the capacity.
pub(crate) fn add_overslime(stack: &mut ItemStack, amount: i32) {
    let (pool, cap) = overslime(stack);
    let add = amount.saturating_mul(1 + modifier_level(stack, "tconstruct:overworked"));
    set_persistent_int(stack, ST_OVERSLIME, pool.saturating_add(add).min(cap).max(0));
}

// ---- The rebuild with modifiers (ModifierTraitHook, the stat, volatile and enchantment modules)

/// StatOverrideModifier.addToolStats, when the tool has the modifier: each `tconstruct:override_bonus` entry through
/// its stat's `update` (a numeric stat from any numeric tag, the harvest tier from its name), each
/// `tconstruct:override_multiplier` entry through `multiply`; entries naming no tool stat are skipped.
pub(crate) fn apply_stat_overrides(tags: &ItemTags, item: &str, mods: &ModBuild, persistent: &Compound, b: &mut StatsBuilder) {
    if !mods.traits.iter().any(|e| e.id == STAT_OVERRIDE) {
        return;
    }
    let numeric = |stat: &str| TOOL_STATS.iter().any(|(s, ..)| *s == stat) && stat_supports(tags, stat, item);
    if let Some(Tag::Compound(bonus)) = persistent.get(&js(KEY_OVERRIDE_BONUS)) {
        for (k, v) in bonus.iter() {
            let stat = k.to_string_lossy().to_string();
            if stat == ST_HARVEST_TIER {
                if let Tag::String(s) = v {
                    let id = pattern_id(&s.to_string_lossy());
                    if let Some(t) = TIERS.iter().position(|x| *x == id) {
                        b.tier(t);
                    }
                }
            } else if numeric(&stat) {
                if let Some(f) = tag_number(v) {
                    b.update(&stat, f);
                }
            }
        }
    }
    if let Some(Tag::Compound(mult)) = persistent.get(&js(KEY_OVERRIDE_MULTIPLY)) {
        for (k, v) in mult.iter() {
            let stat = k.to_string_lossy().to_string();
            if numeric(&stat) {
                if let Some(f) = tag_number(v) {
                    b.multiply(&stat, f64::from(f));
                }
            }
        }
    }
}

/// ToolStats' item tags: a stat module acts only on the items its stat supports (IToolStat.supports).
fn stat_supports(tags: &ItemTags, stat: &str, item: &str) -> bool {
    let tag = |t: &str| tags.has(&format!("tconstruct:modifiable/{t}"), item);
    match stat {
        "tconstruct:durability" | "tconstruct:overslime" => tag("durability"),
        "tconstruct:use_item_speed" | "tconstruct:block_amount" | "tconstruct:block_angle" => tag("held"),
        "tconstruct:attack_damage" => tag("melee"),
        "tconstruct:attack_speed" => tag("melee/weapon"),
        "tconstruct:mining_speed" => tag("harvest"),
        "tconstruct:armor" | "tconstruct:armor_toughness" | "tconstruct:knockback_resistance" => tag("armor"),
        "tconstruct:draw_speed" => tag("ranged"),
        "tconstruct:velocity" | "tconstruct:accuracy" => tag("ranged") || tag("ammo"),
        "tconstruct:projectile_damage" => tag("ranged/launcher") || tag("ammo"),
        "tconstruct:sea_luck" | "tconstruct:lure" => tag("fishing_rods"),
        _ => true,
    }
}

/// ModifierTraitHook.TraitBuilder: each entry in priority order, then the traits its `trait` modules grant
/// (recursively; a fixed level only on the granting modifier's first encounter), merged by id; sorted by priority.
pub(crate) fn expand_traits(reg: &Modifiers, tags: &ItemTags, item: &str, upgrades: &[Trait], mut before: Vec<Trait>) -> Vec<Trait> {
    fn merge(out: &mut Vec<Trait>, name: &str, level: i32) {
        match out.iter_mut().find(|(n, _)| n == name) {
            Some(e) => e.1 += level,
            None => out.push((name.to_owned(), level)),
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn grant(reg: &Modifiers, tags: &ItemTags, item: &str, upgrades: &[Trait], out: &mut Vec<Trait>, seen: &mut HashSet<String>, id: &str, level: i32, chain: &mut Vec<String>) {
        let Some(m) = reg.get(id).cloned() else { return };
        let first = seen.insert(id.to_owned());
        for module in &m.modules {
            let ModModule::Trait { name, level: l, fixed, cond } = module else { continue };
            let snapshot = out.clone();
            if !cond.matches(reg, &ToolView { item, mods: &snapshot, upgrades, tags }, level) {
                continue;
            }
            let lv = if *fixed {
                if !first {
                    continue;
                }
                *l
            } else {
                l.saturating_mul(level)
            };
            if lv <= 0 || chain.iter().any(|c| c == name) || reg.get(name).is_none() {
                continue;
            }
            merge(out, name, lv);
            chain.push(name.clone());
            grant(reg, tags, item, upgrades, out, seen, name, lv, chain);
            chain.pop();
        }
    }
    before.sort_by_key(|(id, _)| std::cmp::Reverse(reg.priority(id)));
    let mut out: Vec<Trait> = Vec::new();
    let mut seen = HashSet::new();
    for (id, level) in before {
        merge(&mut out, &id, level);
        let mut chain = vec![id.clone()];
        grant(reg, tags, item, upgrades, &mut out, &mut seen, &id, level, &mut chain);
    }
    out.sort_by_key(|(id, _)| std::cmp::Reverse(reg.priority(id)));
    out
}

/// ModDataNBT's add: ints and floats summed, a flag set.
fn volatile_add(out: &mut Compound, key: &str, v: Tag) {
    let merged = match (out.get(&js(key)), &v) {
        (Some(Tag::Int(a)), Tag::Int(b)) => Tag::Int(a.saturating_add(*b)),
        (Some(Tag::Float(a)), Tag::Float(b)) => Tag::Float((f32::from_bits(*a) + f32::from_bits(*b)).to_bits()),
        _ => v,
    };
    out.insert(js(key), merged);
}

/// What the modifiers add to a rebuild.
pub(crate) struct ModBuild {
    pub traits: Vec<ModEntry>,
    pub volatile: Compound,
    pub rarity: i32,
    pub enchantments: Vec<(String, i32)>,
}

/// The modifier half of ToolStack.rebuildStats: the traits expanded from `before` (the upgrades, the materials' and
/// the definition's traits), then the volatile data (the definition's slots, each modifier's `volatile_data`), the
/// stat modules on `b`, and the enchantments. `partial` keeps unfinished incremental upgrades' amounts.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_mods(reg: &Modifiers, tags: &ItemTags, item: &str, def_slots: &[(String, i32)], upgrades: &[ModEntry], before: Vec<Trait>, b: &mut StatsBuilder, rarity: i32) -> ModBuild {
    let ups: Vec<Trait> = upgrades.iter().map(|e| (e.id.clone(), e.level)).collect();
    let traits = expand_traits(reg, tags, item, &ups, before);
    let view = ToolView { item, mods: &traits, upgrades: &ups, tags };
    let partial = |id: &str| upgrades.iter().find(|p| p.id == id && p.needed > 0);
    let effective = |id: &str, level: i32| partial(id).map_or(level as f32, |p| ModEntry { level, ..p.clone() }.effective());
    let mut volatile = Compound::new();
    for (slot, n) in def_slots {
        volatile_add(&mut volatile, slot, Tag::Int(*n));
    }
    let mut rarity = rarity;
    for (id, level) in &traits {
        let Some(m) = reg.get(id) else { continue };
        let eff = effective(id, *level);
        for module in &m.modules {
            match module {
                ModModule::Slot { name, value, cond } if cond.matches(reg, &view, *level) => {
                    let n = value.at(eff) as i32;
                    if n != 0 {
                        volatile_add(&mut volatile, name, Tag::Int(n));
                    }
                }
                ModModule::VolatileFlag { flag, cond } if cond.matches(reg, &view, *level) => volatile_add(&mut volatile, flag, Tag::Byte(1)),
                ModModule::VolatileNumber { flag, value, float, cond } if cond.matches(reg, &view, *level) => {
                    let v = value.at(eff);
                    volatile_add(&mut volatile, flag, if *float { Tag::Float(v.to_bits()) } else { Tag::Int(v as i32) });
                }
                ModModule::Rarity(r) => rarity = rarity.max(*r),
                _ => {}
            }
        }
    }
    for (id, level) in &traits {
        let Some(m) = reg.get(id) else { continue };
        let l = effective(id, *level);
        for module in &m.modules {
            match module {
                ModModule::StatBoost { stat, op, value, cond } if cond.matches(reg, &view, *level) && stat_supports(tags, stat, item) => {
                    let v = value.at(l);
                    match op {
                        StatOp::Add => b.add(stat, v),
                        StatOp::Percent => b.percent(stat, v),
                        StatOp::MultiplyBase => b.multiply(stat, f64::from(1.0 + v)),
                        StatOp::MultiplyConditional => b.record_multiplier(stat, 1.0 + v),
                        StatOp::MultiplyAll => b.multiply_all(stat, f64::from(1.0 + v)),
                    }
                }
                ModModule::SetStat { stat, value, tier, cond } if cond.matches(reg, &view, *level) => {
                    if stat == ST_HARVEST_TIER {
                        if let Some(t) = tier {
                            b.tier(*t);
                        }
                    } else if stat_supports(tags, stat, item) {
                        b.update(stat, *value);
                    }
                }
                ModModule::StatCopy { target, source, value, cond } if cond.matches(reg, &view, *level) && stat_supports(tags, target, item) => {
                    let v = b.value(source) * value.at(l);
                    b.add(target, v);
                }
                _ => {}
            }
        }
        // OverslimeModifier.addToolStats: +50 capacity; without an overslime friend, the tool's main stat × 0.9.
        if m.code == Code::Overslime {
            let tag = |t: &str| tags.has(&format!("tconstruct:modifiable/{t}"), item);
            if tag("durability") {
                b.add(ST_OVERSLIME, 50.0);
            }
            if !traits.iter().any(|(n, _)| reg.in_tag(n, "tconstruct:overslime_friend")) {
                if tag("melee") {
                    b.multiply(ST_ATTACK_DAMAGE, 0.9);
                }
                if tag("harvest") {
                    b.multiply(ST_MINING_SPEED, 0.9);
                }
                if tag("armor") {
                    b.add("tconstruct:armor", -0.5);
                }
            }
        }
    }
    let mut enchantments: Vec<(String, i32)> = Vec::new();
    for (id, level) in &traits {
        let Some(m) = reg.get(id) else { continue };
        for module in &m.modules {
            let ModModule::Enchant { name, level: lv, cond } = module else { continue };
            if !cond.matches(reg, &view, *level) {
                continue;
            }
            let n = lv.at(effective(id, *level)) as i32;
            if n <= 0 {
                continue;
            }
            match enchantments.iter_mut().find(|(e, _)| e == name) {
                Some(e) => e.1 = (e.1 + n).min(255),
                None => enchantments.push((name.clone(), n.min(255))),
            }
        }
    }
    let traits = traits
        .into_iter()
        .map(|(id, level)| match partial(&id) {
            Some(p) => ModEntry { id, level, amount: p.amount, needed: p.needed },
            None => ModEntry { id, level, amount: 0, needed: 0 },
        })
        .collect();
    ModBuild { traits, volatile, rarity, enchantments }
}

// ---- The hooks at play

/// The tool's modifiers with their registry entries, in order, and the lists a condition reads.
pub(crate) struct ToolMods {
    pub reg: Arc<Modifiers>,
    pub tags: Arc<ItemTags>,
    pub mods: Vec<Trait>,
    pub upgrades: Vec<Trait>,
    pub effective: Vec<f32>,
}

impl ToolMods {
    pub(crate) fn of(host: &dyn Host, stack: &ItemStack) -> ToolMods {
        ToolMods::with(registry(host), item_tags(host), stack)
    }

    /// The last data load's registry, for a hook with no host; None before any load.
    pub(crate) fn cached(stack: &ItemStack) -> Option<ToolMods> {
        let (reg, tags) = cached()?;
        Some(ToolMods::with(reg, tags, stack))
    }

    fn with(reg: Arc<Modifiers>, tags: Arc<ItemTags>, stack: &ItemStack) -> ToolMods {
        let entries = entries_of(stack, C_MODIFIERS);
        ToolMods {
            reg,
            tags,
            mods: entries.iter().map(|e| (e.id.clone(), e.level)).collect(),
            upgrades: entries_of(stack, C_UPGRADES).into_iter().map(|e| (e.id, e.level)).collect(),
            effective: entries.iter().map(ModEntry::effective).collect(),
        }
    }

    /// Each (modifier, level, effective level) in order.
    pub(crate) fn entries(&self) -> Vec<(Arc<Modifier>, i32, f32)> {
        self.mods.iter().zip(&self.effective).filter_map(|((id, l), e)| Some((self.reg.get(id)?.clone(), *l, *e))).collect()
    }

    pub(crate) fn view<'a>(&'a self, item: &'a str) -> ToolView<'a> {
        ToolView { item, mods: &self.mods, upgrades: &self.upgrades, tags: &self.tags }
    }
}

/// The effects the code modifiers use, requested at registration. Any other effect a data modifier names is looked up
/// by name at run time (`Host::effect_key`, which allocates nothing).
pub(crate) const EFFECTS: [&str; 14] = [
    "minecraft:poison",
    "tconstruct:experienced",
    "tconstruct:ricochet",
    "minecraft:glowing",
    "minecraft:levitation",
    "minecraft:slowness",
    "minecraft:strength",
    "minecraft:wither",
    "tconstruct:enderference",
    "tconstruct:magnetic",
    "tconstruct:pierce",
    "tconstruct:bleeding",
    "tconstruct:momentum_harvest",
    "tconstruct:insatiable_melee",
];
pub(crate) static EFFECT_KEYS: std::sync::OnceLock<HashMap<&'static str, EffectKey>> = std::sync::OnceLock::new();

pub(crate) fn effect_key(host: &dyn Host, id: &str) -> Option<EffectKey> {
    EFFECT_KEYS.get().and_then(|k| k.get(id).copied()).or_else(|| host.effect_key(id))
}

/// The level of one of `living`'s effects (TinkerEffect.getLevel: amplifier + 1, 0 without it).
fn effect_level(host: &dyn Host, living: LivingRef, id: &str) -> i32 {
    let Some(key) = effect_key(host, id) else { return 0 };
    living.effect(host, key).map_or(0, |e| e.amplifier + 1)
}

/// MobEffectInstance on a living entity; Tinkers' own effects hide their particles (TinkerEffect.apply).
pub(crate) fn give_effect(host: &mut dyn ServerHost, living: LivingRef, id: &str, duration: i32, amplifier: i32) {
    if duration <= 0 {
        return;
    }
    let Some(key) = effect_key(&*host, id) else { return };
    let visible = !id.starts_with("tconstruct:");
    let _ = living.add_effect(host, &ActiveEffect::new(key, duration, amplifier.clamp(0, 255), false, visible, true));
}

/// BreakSpeedModifierHook in order (a result below 0 stops): conditional mining speed, dwarven, momentum.
#[allow(clippy::too_many_arguments)]
pub(crate) fn break_speed(host: &dyn Host, player: Option<LivingRef>, stack: &ItemStack, pos: Pos, block: BlockKey, name: &str, effective: bool, mut speed: f32, original: f32) -> f32 {
    let tm = ToolMods::of(host, stack);
    let view = tm.view(sid(stack));
    let mining_mult = tool_multiplier(stack, ST_MINING_SPEED);
    for (m, level, l) in tm.entries() {
        for module in &m.modules {
            if let ModModule::MiningSpeed { formula, blocks, require_effective, cond } = module {
                if (effective || !require_effective) && cond.matches(&tm.reg, &view, level) && blocks.matches(host, block, name) {
                    speed = formula.apply(&[formula.level(l), speed, mining_mult, original], stack);
                }
            }
        }
        match m.code {
            // DwarvenModifier.getBoost: below y 64 + L·clamp((64 − y)/64, 0, 2)·6·mult; high up slower.
            Code::Dwarven if effective => {
                let y = pos[1] as f32;
                let bonus = mining_mult * 6.0;
                if y < 64.0 {
                    speed += l * ((64.0 - y) / 64.0).clamp(0.0, 2.0) * bonus;
                } else if y > 128.0 {
                    speed *= if y >= 256.0 { 0.25 } else { 1.0 - (y - 128.0) / 128.0 * 0.75 };
                }
            }
            Code::Momentum if effective => {
                let lv = player.map_or(0, |p| effect_level(host, p, "tconstruct:momentum_harvest"));
                speed *= 1.0 + l * lv as f32 / 40.0;
            }
            _ => {}
        }
        if speed < 0.0 {
            break;
        }
    }
    speed
}

/// ToolDamageModifierHook in order on `stack` (the overslime pool it takes from changes there); at 0 or less nothing
/// is worn.
pub(crate) fn tool_damage(host: &dyn Host, stack: &mut ItemStack, mut amount: i32) -> i32 {
    let tm = ToolMods::of(host, stack);
    for (m, level, l) in tm.entries() {
        for module in &m.modules {
            match module {
                // ReduceToolDamageModule: p ≥ 1 none, else each point kept with chance 1 − p.
                ModModule::ReduceDamage { formula, cond } if cond.matches(&tm.reg, &tm.view(sid(stack)), level) => {
                    let p = formula.apply(&[formula.level(l)], stack);
                    if p >= 1.0 {
                        amount = 0;
                    } else if p > 0.0 {
                        amount = (0..amount).filter(|_| rand_f32() >= p).count() as i32;
                    }
                }
                ModModule::DamageRange { min, max } => amount = amount.clamp(*min, (*max).max(*min)),
                _ => {}
            }
            if amount <= 0 {
                return 0;
            }
        }
        // DurabilityShieldModule.onDamageTool: the pool takes what it can.
        if m.code == Code::Overslime {
            let (pool, _) = overslime(stack);
            if pool > 0 {
                if pool >= amount {
                    set_persistent_int(stack, ST_OVERSLIME, pool - amount);
                    return 0;
                }
                amount -= pool;
                set_persistent_int(stack, ST_OVERSLIME, 0);
            }
        }
    }
    amount
}

/// RepairFactorModifierHook: 1, through each `repair` module.
pub(crate) fn repair_factor(host: &dyn Host, stack: &ItemStack) -> f32 {
    let tm = ToolMods::of(host, stack);
    let mut factor = 1.0f32;
    for (m, level, l) in tm.entries() {
        for module in &m.modules {
            if let ModModule::Repair { formula, cond } = module {
                if cond.matches(&tm.reg, &tm.view(sid(stack)), level) {
                    factor = formula.apply(&[formula.level(l), factor], stack);
                }
            }
        }
    }
    factor
}

/// After a melee hit landed: `weapon_mob_effect`s on the target, `tool_usage_mob_effect`s on the attacker, fiery's
/// fire, lacerating's bleeding, insatiable's and necrotic's (lifesteal) halves.
pub(crate) fn after_hit(host: &mut dyn ServerHost, attacker: LivingRef, stack: &ItemStack, target: u32) {
    let tm = ToolMods::of(&*host, stack);
    let Some(target_entity) = host.entity(target) else { return };
    let target_living = LivingRef::new(target_entity);
    let living = target_living.health(&*host).is_some();
    for (m, level, l) in tm.entries() {
        for module in &m.modules {
            match module {
                ModModule::MobEffect { kind, effect, level: lv, time, chance, cond } => {
                    if !cond.matches(&tm.reg, &tm.view(sid(stack)), level) || (*chance < 1.0 && rand_f32() >= *chance) {
                        continue;
                    }
                    let (amp, duration) = (lv.at(l) as i32 - 1, time.at(l) as i32);
                    match kind {
                        EffectUse::Usage => give_effect(host, attacker, effect, duration, amp),
                        EffectUse::Weapon if living => give_effect(host, target_living, effect, duration, amp),
                        EffectUse::Weapon => {}
                    }
                }
                ModModule::FieryAttack { seconds } => {
                    let ticks = seconds.at(l).round() as i32 * 20;
                    let _ = target_entity.set_remaining_fire_ticks(host, ticks);
                }
                ModModule::Lifesteal { percent, level: range } if living && (range.0..=range.1).contains(&level) => {
                    let heal = percent.at(l) * tool_stat(stack, ST_ATTACK_DAMAGE);
                    if heal > 0.0 {
                        let _ = attacker.heal(host, heal);
                    }
                }
                _ => {}
            }
        }
        match m.code {
            Code::Insatiable => {
                let amp = effect_level(&*host, attacker, "tconstruct:insatiable_melee").min(4);
                give_effect(host, attacker, "tconstruct:insatiable_melee", 100, amp);
            }
            Code::Lacerating if living => give_effect(host, target_living, "tconstruct:bleeding", level * 40, 0),
            _ => {}
        }
    }
}

/// After a block broke with the tool (BlockBreakModifierHook): `tool_usage_mob_effect`s on the user; momentum's effect
/// after an effective, non-AOE harvest.
pub(crate) fn after_break(host: &mut dyn ServerHost, player: LivingRef, stack: &ItemStack, hardness: f32, aoe: bool) {
    let tm = ToolMods::of(&*host, stack);
    let harvest = tm.tags.has("tconstruct:modifiable/harvest", sid(stack));
    for (m, level, l) in tm.entries() {
        for module in &m.modules {
            let ModModule::MobEffect { kind: EffectUse::Usage, effect, level: lv, time, chance, cond } = module else { continue };
            if cond.matches(&tm.reg, &tm.view(sid(stack)), level) && (*chance >= 1.0 || rand_f32() < *chance) {
                give_effect(host, player, effect, time.at(l) as i32, lv.at(l) as i32 - 1);
            }
        }
        if m.code == Code::Momentum && !aoe && harvest {
            let duration = 100.max((50.0 * hardness / tool_stat(stack, ST_MINING_SPEED).max(0.1)) as i32);
            let amp = effect_level(&*host, player, "tconstruct:momentum_harvest").min(9);
            give_effect(host, player, "tconstruct:momentum_harvest", duration, amp);
        }
    }
}

/// InventoryTickModifierHook every 20 ticks (OvergrowthModule): chance 0.05·L to add 1 overslime below the capacity.
pub(crate) fn inventory_tick(host: &dyn Host, stack: &ItemStack) -> Option<ItemStack> {
    let tm = ToolMods::of(host, stack);
    let mut out = stack.clone();
    let mut changed = false;
    for (m, _, l) in tm.entries() {
        for module in &m.modules {
            if let ModModule::Overgrowth { chance } = module {
                let (pool, cap) = overslime(&out);
                if pool < cap && rand_f32() < chance.at(l) {
                    add_overslime(&mut out, 1);
                    changed = true;
                }
            }
        }
    }
    changed.then_some(out)
}

// ---- Names (Modifier.getDisplayName, ModifierLevelDisplay)

pub(crate) fn roman(n: i32) -> String {
    let mut left = n.max(0);
    let mut s = String::new();
    for (v, r) in [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")] {
        while left >= v {
            s.push_str(r);
            left -= v;
        }
    }
    s
}

/// A modifier's translation key and the English fallback of its name.
pub(crate) fn modifier_key(reg: Option<&Modifiers>, id: &str) -> String {
    reg.and_then(|r| r.get(id)).map_or_else(|| format!("modifier.{}", id.replacen(':', ".", 1)), |m| m.key.clone())
}

/// Modifier.getDisplayName(level) by its level display, as one string; its colour.
pub(crate) fn level_name(host: &dyn ClientHost, reg: &Modifiers, id: &str, level: i32) -> (String, u32) {
    let key = modifier_key(Some(reg), id);
    let name = lang(host, &key, &words(id.split(':').nth(1).unwrap_or(id)));
    let color = text_color(&key).unwrap_or(0xFF_FFFF) | 0xFF00_0000;
    let display = reg.get(id).map(|m| m.display.clone()).unwrap_or(Value::Null);
    let kind = display.as_str().or_else(|| display["type"].as_str()).unwrap_or("tconstruct:default").to_owned();
    let leveled = |level: i32| format!("{name} {}", roman(level));
    let s = match kind.as_str() {
        "tconstruct:no_levels" => name.clone(),
        "tconstruct:single_level" if level == 1 => name.clone(),
        "tconstruct:pluses" => format!("{name}{}", "+".repeat((level - 1).max(0) as usize)),
        "tconstruct:unique" => {
            let unique = display["unique_until"].as_i64().unwrap_or(1) as i32;
            if !display["first_unique"].as_bool().unwrap_or(false) && level == 1 {
                name.clone()
            } else if level <= unique {
                lang(host, &format!("{key}.{level}"), &leveled(level))
            } else {
                leveled(level)
            }
        }
        _ => leveled(level),
    };
    (s, color)
}

/// TooltipUtil.addModifierNames: the modifiers listed in `context`, each by its display name (overslime's
/// "Overslime: amount / capacity"; an unfinished level "Name: amount / needed").
pub(crate) fn modifier_lines(host: &dyn ClientHost, stack: &ItemStack, context: u8) -> Vec<(String, u32)> {
    let reg = registry(host);
    let mut out = Vec::new();
    for e in entries_of(stack, C_MODIFIERS) {
        let Some(m) = reg.get(&e.id) else { continue };
        if m.tips & context == 0 {
            continue;
        }
        let (name, color) = level_name(host, &reg, &e.id, e.level);
        if e.needed > 0 {
            out.push((format!("{name}: {} / {}", e.amount, e.needed), color));
        } else if m.code == Code::Overslime {
            let (pool, cap) = overslime(stack);
            let plain = lang(host, &m.key, "Overslime");
            out.push((format!("{plain}: {pool} / {cap}"), color));
        } else {
            out.push((name, color));
        }
    }
    out
}
