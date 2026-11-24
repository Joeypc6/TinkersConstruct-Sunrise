//! `/tconstruct`'s subcommands (TConstructCommand and `shared/command/subcommand/*`, 3.12.1.231) on Sunrise's
//! `register_commands` and command executors, with Tinkers' argument types (`shared/command/argument/*`) registered as
//! Sunrise command argument types:
//! - `modifiers`, `materials`, `tool_stats`, `slots`, `durability` (operators of level 2): the held modifiable item of
//!   each target (HeldModifiableItemIterator) changed through the port's tool data (`tools::modifier`, `tools::tool`,
//!   `tools::modrecipe`), checked and rebuilt on the server, or a material's stats and traits;
//! - `report modifier_usage` and `report modifier_priority` (level 1): tables in the game log;
//! - `generate hidden_fluids_tag` (level 2): the `c:hidden_from_recipe_viewers` fluid tag in the world's generated
//!   data pack;
//! - `generate melting_recipes <recipe_type>` (level 2): melting recipes for that type's results, from what their
//!   ingredients melt into, in the same pack (`smeltery::melting_gen`).
//! The bare `/tconstruct` is the commands menu (`commands_menu.rs`, everyone's), pushed beside these by the mod.
//!
//! Left out: `generate part_textures` (a client generator writing sprites into a resource pack: no host API reads
//! every pack's copy of a client resource or lists them outside a reload, and none decodes or encodes a PNG; see
//! `tools/issue-drafts/tinkers-part_textures.md`), and `report modifier_priority <modifier_hook>` and its Hooks column
//! (the port's modifiers have no module hook map).
//!
//! Argument types: Sunrise sends a mod's argument as a Brigadier word (letters, digits, `_ - . +`) or the rest of the
//! line. Tinkers' ids default to the `tconstruct` namespace (IdParser.read("tconstruct", ..)), so a word argument
//! takes the id's path (`haste` for `tconstruct:haste`); the material of `materials set`, the usage's last argument,
//! takes the whole line, so `tconstruct:wood#oak` works there as in Java.

use crate::smeltery::casting_recipes::tag_items;
use crate::smeltery::util::*;
use crate::tools::data::{self, Stats, STAT_TYPES, TIERS};
use crate::tools::modifier::{self as md, Modifiers};
use crate::tools::modrecipe::{self, Err as ModErr, SLOT_TYPES};
use crate::tools::tool::{self, ST_HARVEST_TIER, TOOL_STATS};
use mod_api::serde_json::{self, Value};
use mod_api::text::TextStyle;
use mod_api::*;
use std::collections::HashSet;

const NS: &str = "tconstruct";

// ---- Text (TConstruct.makeTranslation, the components the commands answer with)

fn tr(key: &str, args: Vec<Text>) -> Text {
    Text::translate(key, args)
}

fn cmd(key: &str, args: Vec<Text>) -> Text {
    tr(&format!("command.tconstruct.{key}"), args)
}

fn lit(s: impl Into<String>) -> Text {
    Text::literal(s)
}

fn styled(t: Text, f: impl FnOnce(&mut TextStyle)) -> Text {
    let mut style = t.style.clone();
    f(&mut style);
    t.with_style(style)
}

fn colored(t: Text, rgb: u32) -> Text {
    styled(t, |s| s.color = Some(format!("#{:06X}", rgb & 0xFF_FFFF)))
}

/// GeneratePackHelper.getPathComponent: the path underlined (Java also opens the file on a click, which a mod's text
/// can't carry).
fn path_text(t: Text) -> Text {
    styled(t, |s| s.underlined = Some(true))
}

fn say(host: &mut dyn ServerHost, t: Text) {
    let _ = host.commands().feedback(t);
}

/// A CommandSyntaxException's message: the source's red failure line. Sunrise has no executor failure carrying a
/// message (an executor's `None` answers `command.unknown.command`), so the line goes out as red feedback and the
/// command ends with result 0.
fn fail(host: &mut dyn ServerHost, t: Text) -> Option<i32> {
    let red = styled(t, |s| s.color = Some("red".into()));
    let _ = host.commands().feedback(red);
    Some(0)
}

/// Brigadier's `argument.integer.low` (IntegerArgumentType.integer(min)).
fn integer_low(min: i32, found: i32) -> Text {
    tr("argument.integer.low", vec![lit(min.to_string()), lit(found.to_string())])
}

/// Java's Float.toString for the values the messages show.
fn jfloat(f: f32) -> String {
    if f.is_finite() && f == f.trunc() && f.abs() < 1.0e7 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

/// DecimalFormat with at most `places` decimals (HALF_EVEN on the value), `min_int` integer digits and optional
/// grouping: COMMA_FORMAT `#,###,###.##`, BONUS_FORMAT `#.##`.
fn decimal(v: f64, places: usize, min_int: bool, group: bool) -> String {
    let s = format!("{:.*}", places, v.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let frac = frac.trim_end_matches('0');
    let int = if group { commas(int.parse::<i64>().unwrap_or(0)) } else { int.to_owned() };
    let int = if !min_int && int == "0" && !frac.is_empty() { String::new() } else { int };
    let neg = v < 0.0 && (int.chars().any(|c| c != '0' && c != ',') || !frac.is_empty());
    let body = if frac.is_empty() { int } else { format!("{int}.{frac}") };
    let body = if body.is_empty() { "0".to_owned() } else { body };
    if neg { format!("-{body}") } else { body }
}

fn comma(v: f32) -> String {
    decimal(f64::from(v), 2, true, true)
}

/// Mth.hsvToRgb.
fn hsv(h: f32, s: f32, v: f32) -> u32 {
    let i = ((h * 6.0) as i32).rem_euclid(6);
    let f = h * 6.0 - (h * 6.0).floor();
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match i {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let c = |x: f32| ((x * 255.0) as i32).clamp(0, 255) as u32;
    (c(r) << 16) | (c(g) << 8) | c(b)
}

/// IToolStat.formatColored: the value's colour runs with it, red to green.
fn format_colored(key: &str, number: f32, offset: f32, text: String) -> Text {
    let hue = (offset + number).rem_euclid(2.0);
    tr(key, Vec::new()).append(colored(lit(text), hsv(hue / 1.5, 1.0, 0.75)))
}

/// formatColoredPercentBoost (PERCENT_BOOST_FORMAT `#%`).
fn percent_boost(key: &str, number: f32) -> Text {
    format_colored(key, number, 0.5, format!("{}%", decimal(f64::from(number) * 100.0, 0, true, false)))
}

/// formatColoredBonus (BONUS_FORMAT `#.##`).
fn bonus(key: &str, number: f32) -> Text {
    format_colored(key, number, 0.5, decimal(f64::from(number), 2, false, false))
}

/// IToolStat.formatNumber: the stat's name, then the number (COMMA_FORMAT) in its colour.
fn format_number(key: &str, color: u32, number: f32) -> Text {
    tr(key, Vec::new()).append(colored(lit(comma(number)), color))
}

/// The tool stats' colours (ToolStats, OverslimeModule.OVERSLIME_STAT).
fn stat_color(stat: &str) -> u32 {
    match stat.trim_start_matches("tconstruct:") {
        "durability" => 0x47_CC47,
        "attack_damage" | "projectile_damage" => 0xD7_6464,
        "attack_speed" | "armor" | "armor_toughness" | "knockback_resistance" | "draw_speed" | "accuracy" => 0x85_47CC,
        "water_inertia" => 0x5A_82F3,
        "sea_luck" => 0x34_5EC3,
        "lure" => 0xCB_CC18,
        "overslime" => 0x71_DC85,
        _ => 0x78_A0CD,
    }
}

fn stat_key(stat: &str) -> String {
    let (ns, path) = stat.split_once(':').unwrap_or((NS, stat));
    format!("tool_stat.{ns}.{path}")
}

/// IToolStat.getPrefix.
fn stat_prefix(stat: &str) -> Text {
    tr(&stat_key(stat), Vec::new())
}

/// FloatToolStat.formatValue (IntegerToolStat's for durability, lure and overslime).
fn stat_value(stat: &str, v: f32) -> Text {
    let n = if matches!(stat, "tconstruct:durability" | "tconstruct:lure" | "tconstruct:overslime") { (v as i32) as f32 } else { v };
    format_number(&stat_key(stat), stat_color(stat), n)
}

/// HarvestTiers.getName.
fn tier_text(t: usize) -> Text {
    tr(&format!("stat.tconstruct.harvest_tier.{}", TIERS[t.min(TIERS.len() - 1)].replace(':', ".")), Vec::new())
}

/// IMaterialStats.getLocalizedName.
fn material_stat_name(stat: &str) -> Text {
    let (ns, path) = stat.split_once(':').unwrap_or((NS, stat));
    tr(&format!("stat.{ns}.{path}"), Vec::new())
}

/// MaterialTooltipCache.getDisplayName: the material's name in its colour.
fn material_text(id: &str) -> Text {
    let key = data::material_key(id, "");
    let t = tr(&key, Vec::new());
    match data::text_color(&key).filter(|c| c & 0xFF_FFFF != 0xFF_FFFF) {
        Some(c) => colored(t, c),
        None => t,
    }
}

/// Modifier.getDisplayName(level) by its level display (ModifierLevelDisplay), in the modifier's colour.
fn modifier_text(reg: &Modifiers, id: &str, level: i32) -> Text {
    let key = md::modifier_key(Some(reg), id);
    let display = reg.get(id).map(|m| m.display.clone()).unwrap_or(Value::Null);
    let kind = display.as_str().or_else(|| display["type"].as_str()).unwrap_or("tconstruct:default").to_owned();
    let name = || tr(&key, Vec::new());
    let leveled = |level: i32| name().append(lit(format!(" {}", md::roman(level))));
    let t = match kind.as_str() {
        "tconstruct:no_levels" => name(),
        "tconstruct:single_level" if level == 1 => name(),
        "tconstruct:pluses" => name().append(lit("+".repeat((level - 1).max(0) as usize))),
        "tconstruct:unique" => {
            let unique = display["unique_until"].as_i64().unwrap_or(1) as i32;
            if !display["first_unique"].as_bool().unwrap_or(false) && level == 1 {
                name()
            } else if level <= unique {
                tr(&format!("{key}.{level}"), Vec::new())
            } else {
                leveled(level)
            }
        }
        _ => leveled(level),
    };
    match data::text_color(&key) {
        Some(c) => colored(t, c),
        None => t,
    }
}

/// SlotType.getDisplayName.
fn slot_text(slot: &str) -> Text {
    tr(&format!("stat.tconstruct.slot.display.{slot}"), Vec::new())
}

/// ToolStack.tryValidate's component.
fn validate_text(reg: &Modifiers, e: &ModErr) -> Text {
    match e {
        ModErr::ValidateSlots { slot } => tr("recipe.tconstruct.modifier.validate_slots", vec![slot_text(slot)]),
        ModErr::Requirement { id } => {
            let key = reg
                .get(id)
                .and_then(|m| m.modules.iter().find_map(|x| if let md::ModModule::Requirements { key, .. } = x { Some(key.clone()) } else { None }))
                .unwrap_or_default();
            tr(&key, Vec::new())
        }
        _ => lit(format!("{e:?}")),
    }
}

// ---- Arguments (shared/command/argument)

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Modifier,
    Material,
    MaterialVariant,
    MaterialStat,
    ToolStat,
    NumericToolStat,
    SlotType,
    OptionalSlotType,
}

/// The argument types, as TConstructCommand.init registers them (`tconstruct:<name>`).
const KINDS: [(&str, Kind); 8] = [
    ("modifier", Kind::Modifier),
    ("material", Kind::Material),
    ("material_variant", Kind::MaterialVariant),
    ("material_stat", Kind::MaterialStat),
    ("tool_stat", Kind::ToolStat),
    // ToolStatArgument.stat(INumericToolStat.class): Java's one type with a filter is two here.
    ("numeric_tool_stat", Kind::NumericToolStat),
    ("slot_type", Kind::SlotType),
    // SlotTypeArgument.slotType(true): `slotless` too.
    ("optional_slot_type", Kind::OptionalSlotType),
];

struct Arg(Kind);

/// IdParser.read("tconstruct", ..): an id, `tconstruct` when it names no namespace; `argument.id.invalid` else.
fn read_id(input: &str) -> Result<String, Text> {
    let ok = !input.is_empty() && input.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-./:".contains(c));
    let (ns, path) = input.split_once(':').unwrap_or((NS, input));
    if !ok || ns.is_empty() || path.is_empty() || ns.contains('/') || path.contains(':') {
        return Err(tr("argument.id.invalid", Vec::new()));
    }
    Ok(format!("{ns}:{path}"))
}

fn numeric_stat(id: &str) -> bool {
    TOOL_STATS.iter().any(|(s, ..)| *s == id)
}

/// ToolStats.getAllStats.
fn tool_stats() -> impl Iterator<Item = &'static str> {
    TOOL_STATS.iter().map(|(s, ..)| *s).chain([ST_HARVEST_TIER])
}

/// Every modifier id: the registry's, else the compiled-in table's (before any data load).
fn modifier_ids(host: Option<&dyn Host>) -> Vec<String> {
    let reg = match host {
        Some(h) => Some(md::registry(h)),
        None => md::cached().map(|(r, _)| r),
    };
    match reg {
        Some(r) => r.ids.clone(),
        None => data::table().json["modifiers"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default(),
    }
}

fn material_ids(host: Option<&dyn Host>) -> Vec<String> {
    let mats = match host {
        Some(h) => Some(data::materials(h)),
        None => data::cached_materials(),
    };
    let mut ids: Vec<String> = match mats {
        Some(m) => m.map.keys().cloned().collect(),
        None => data::table().json["definition"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default(),
    };
    ids.sort();
    ids
}

/// An argument's text to its canonical value, or the message Java's parser throws. With a host (the executor), the
/// registries of the current data load decide; without one (the dispatcher's parse), the last load's, and an id is
/// let through before any load (the executor checks it again).
fn parse_arg(kind: Kind, input: &str, host: Option<&dyn Host>) -> Result<String, Text> {
    match kind {
        Kind::Modifier => {
            let id = read_id(input)?;
            let known = match host {
                Some(h) => Some(md::registry(h).get(&id).is_some()),
                None => md::cached().map(|(r, _)| r.get(&id).is_some()),
            };
            if known == Some(false) {
                return Err(cmd("modifier.not_found", vec![lit(id)]));
            }
            Ok(id)
        }
        Kind::Material => {
            let id = read_id(input)?;
            let known = match host {
                Some(h) => Some(data::materials(h).get(&id).is_some()),
                None => data::cached_materials().map(|m| m.get(&id).is_some()),
            };
            if known == Some(false) {
                return Err(cmd("material.not_found", vec![lit(id)]));
            }
            Ok(id)
        }
        Kind::MaterialVariant => {
            // MaterialVariantId.read: `[namespace:]path[#variant]`, no registry check.
            let (id, variant) = input.split_once('#').unwrap_or((input, ""));
            let id = if id.contains(':') { id.to_owned() } else { format!("{NS}:{id}") };
            let full = if variant.is_empty() { id } else { format!("{id}#{variant}") };
            data::parse_material(&full).map(|m| data::material_string(&m)).ok_or_else(|| tr("argument.id.invalid", Vec::new()))
        }
        Kind::MaterialStat => {
            let id = read_id(input)?;
            if !STAT_TYPES.iter().any(|(s, ..)| *s == id) {
                return Err(cmd("material_stat.not_found", vec![lit(id)]));
            }
            Ok(id)
        }
        Kind::ToolStat | Kind::NumericToolStat => {
            let id = read_id(input)?;
            if !tool_stats().any(|s| s == id) {
                return Err(cmd("stat_type.not_found", vec![lit(id)]));
            }
            if kind == Kind::NumericToolStat && !numeric_stat(&id) {
                return Err(cmd("stat_type.wrong_type", vec![lit(id), lit("INumericToolStat")]));
            }
            Ok(id)
        }
        Kind::SlotType | Kind::OptionalSlotType => {
            if kind == Kind::OptionalSlotType && input == "slotless" {
                return Ok(input.to_owned());
            }
            if !SLOT_TYPES.contains(&input) {
                return Err(cmd("slot_type.not_found", vec![lit(input)]));
            }
            Ok(input.to_owned())
        }
    }
}

/// SharedSuggestionProvider.matchesSubStr: at the start or after an `_`.
fn matches_sub(input: &str, candidate: &str) -> bool {
    let mut i = 0;
    loop {
        if candidate[i..].starts_with(input) {
            return true;
        }
        match candidate[i..].find('_') {
            Some(n) => i += n + 1,
            None => return false,
        }
    }
}

/// TinkerSuggestionProvider.suggestResource, for a word argument: the `tconstruct` ids by their path (a word can't
/// hold the `:`).
fn suggest_paths(ids: impl IntoIterator<Item = String>, prefix: &str) -> Vec<CommandSuggestion> {
    let prefix = prefix.to_ascii_lowercase();
    ids.into_iter()
        .filter_map(|id| id.strip_prefix("tconstruct:").map(str::to_owned))
        .filter(|path| matches_sub(&prefix, path))
        .map(CommandSuggestion::new)
        .collect()
}

impl CommandArgumentType for Arg {
    fn kind(&self) -> CommandArgumentKind {
        if self.0 == Kind::MaterialVariant { CommandArgumentKind::Greedy } else { CommandArgumentKind::Word }
    }

    fn parse(&self, input: &str) -> Result<String, CommandArgumentError> {
        parse_arg(self.0, input, None).map_err(CommandArgumentError::new)
    }

    fn suggest(&self, _sender: Option<&CommandSender>, prefix: &str) -> Vec<CommandSuggestion> {
        match self.0 {
            Kind::Modifier => suggest_paths(modifier_ids(None), prefix),
            Kind::Material => suggest_paths(material_ids(None), prefix),
            Kind::MaterialVariant => {
                // The whole id fits a greedy argument: filterResources as Java's.
                let input = prefix.to_ascii_lowercase();
                material_ids(None)
                    .into_iter()
                    .filter(|id| {
                        let (ns, path) = id.split_once(':').unwrap_or(("", id.as_str()));
                        if input.contains(':') { matches_sub(&input, id) } else { matches_sub(&input, ns) || (ns == NS && matches_sub(&input, path)) }
                    })
                    .map(CommandSuggestion::new)
                    .collect()
            }
            Kind::MaterialStat => suggest_paths(STAT_TYPES.iter().map(|(s, ..)| (*s).to_owned()), prefix),
            Kind::ToolStat => suggest_paths(tool_stats().map(str::to_owned), prefix),
            Kind::NumericToolStat => suggest_paths(TOOL_STATS.iter().map(|(s, ..)| (*s).to_owned()), prefix),
            Kind::SlotType | Kind::OptionalSlotType => SLOT_TYPES
                .iter()
                .copied()
                .chain((self.0 == Kind::OptionalSlotType).then_some("slotless"))
                .filter(|s| matches_sub(prefix, s))
                .map(CommandSuggestion::new)
                .collect(),
        }
    }
}

/// Registers the argument types (call from `register`).
pub(crate) fn register_arguments(r: &mut dyn Registrar) {
    for (name, kind) in KINDS {
        r.register_command_argument(name, Arg(kind));
    }
}

// ---- Reading the invocation

fn int_arg(c: &CommandInvocation, name: &str) -> Option<i32> {
    match c.argument(name) {
        Some(CommandArgumentValue::Integer(n)) => Some(*n),
        _ => None,
    }
}

fn num_arg(c: &CommandInvocation, name: &str) -> Option<f32> {
    match c.argument(name) {
        Some(CommandArgumentValue::Number(n)) => Some(*n as f32),
        Some(CommandArgumentValue::Integer(n)) => Some(*n as f32),
        _ => None,
    }
}

fn str_arg<'a>(c: &'a CommandInvocation, name: &str) -> Option<&'a str> {
    match c.argument(name) {
        Some(CommandArgumentValue::Custom { value, .. }) | Some(CommandArgumentValue::String(value)) => Some(value.as_str()),
        _ => None,
    }
}

fn targets(c: &CommandInvocation) -> Vec<EntityRef> {
    match c.argument("targets") {
        Some(CommandArgumentValue::Entities(list)) => list.clone(),
        Some(CommandArgumentValue::Entity(e)) => vec![*e],
        Some(CommandArgumentValue::Players(list)) => list.iter().map(|p| p.entity()).collect(),
        _ => Vec::new(),
    }
}

/// A custom argument checked again against the current data load (the server never takes the dispatcher's word for
/// it): its canonical value, or the parser's message.
fn checked(host: &dyn Host, c: &CommandInvocation, name: &str, kind: Kind) -> Result<String, Text> {
    let raw = str_arg(c, name).ok_or_else(|| tr("command.failed", Vec::new()))?;
    parse_arg(kind, raw, Some(host))
}

// ---- HeldModifiableItemIterator

#[derive(Clone, Copy)]
enum Held {
    /// TinkerTags.Items.MODIFIABLE.
    Modifiable,
    /// TinkerTags.Items.DURABILITY.
    Durability,
}

fn entity_name(host: &dyn Host, e: EntityRef) -> String {
    e.name(host).unwrap_or_default()
}

/// The target's display name (LivingEntity.getDisplayName), as the success messages name it.
fn display_name(host: &dyn Host, e: EntityRef) -> Text {
    lit(entity_name(host, e))
}

/// HeldModifiableItemIterator.apply: each target's main hand item, which must be living, holding something, and that
/// something in the tag; `f` answers whether it changed it. The targets `f` changed, or the first refusal (the
/// targets before it keep their change, as Java's).
fn held(
    host: &mut dyn ServerHost,
    targets: &[EntityRef],
    tag: Held,
    mut f: impl FnMut(&mut dyn ServerHost, LivingRef, ItemStack) -> Result<bool, Text>,
) -> Result<Vec<EntityRef>, Text> {
    ensure_ids(&*host);
    let tools = data::tooling(&*host);
    let mut out = Vec::new();
    for &e in targets {
        let Some(living) = host.living(e) else {
            return Err(cmd("held_modifiable.failed.nonliving", vec![lit(entity_name(&*host, e))]));
        };
        let Some(stack) = living.with_hand_item(&*host, Hand::Main, |v| v.project()).flatten() else {
            return Err(cmd("held_modifiable.failed.no_item", vec![lit(entity_name(&*host, e))]));
        };
        let in_tag = match tag {
            Held::Modifiable => tools.modifiable.contains(sid(&stack)),
            Held::Durability => tools.durability.contains(sid(&stack)),
        };
        if !in_tag {
            return Err(cmd("held_modifiable.failed.invalid_item", vec![lit(entity_name(&*host, e))]));
        }
        if f(&mut *host, living, stack)? {
            out.push(e);
        }
    }
    if out.is_empty() {
        return Err(cmd("held_modifiable.failed", Vec::new()));
    }
    Ok(out)
}

/// LivingEntity.setItemInHand(MAIN_HAND, ..).
fn put_back(host: &mut dyn ServerHost, living: LivingRef, stack: ItemStack) {
    let _ = living.set_hand_item(host, Hand::Main, Some(stack));
}

/// The rebuilt tool's validation (ToolStack.tryValidate), as the error the command throws.
fn validated(host: &dyn Host, reg: &Modifiers, stack: &ItemStack) -> Result<(), Text> {
    match modrecipe::try_validate(host, stack) {
        Some(e) => Err(validate_text(reg, &e)),
        None => Ok(()),
    }
}

/// ModifierRemovalHook.onRemoved for the modifiers `before` had and `after` lacks: overslime's pool and the stat
/// override's data go with them.
fn on_removed(before: &ItemStack, after: &mut ItemStack) {
    let gone = |after: &ItemStack, id: &str| md::modifier_level(before, id) > 0 && md::modifier_level(after, id) == 0;
    let (overslime, stat_override) = (gone(after, "tconstruct:overslime"), gone(after, md::STAT_OVERRIDE));
    if overslime {
        md::set_persistent_int(after, md::ST_OVERSLIME, 0);
    }
    if stat_override {
        md::set_persistent(after, md::KEY_OVERRIDE_BONUS, None);
        md::set_persistent(after, md::KEY_OVERRIDE_MULTIPLY, None);
    }
}

/// The success message: `<key>.single` naming the one target, else `<key>.multiple` with the count; `args` go first.
fn success(host: &mut dyn ServerHost, key: &str, args: Vec<Text>, list: &[EntityRef]) -> Option<i32> {
    let mut args = args;
    let line = if list.len() == 1 {
        args.push(display_name(&*host, list[0]));
        cmd(&format!("{key}.single"), args)
    } else {
        args.push(lit(list.len().to_string()));
        cmd(&format!("{key}.multiple"), args)
    };
    say(host, line);
    Some(list.len() as i32)
}

macro_rules! tryf {
    ($host:expr, $e:expr) => {
        match $e {
            Ok(v) => v,
            Err(t) => return fail($host, t),
        }
    };
}

// ---- modifiers (ModifiersCommand)

fn modifiers(host: &mut dyn ServerHost, c: &CommandInvocation, add: bool) -> Option<i32> {
    let reg = md::registry(&*host);
    let id = reg.resolve(&tryf!(host, checked(&*host, c, "modifier", Kind::Modifier)));
    let level = match int_arg(c, "level") {
        Some(n) if n < 1 => return fail(host, integer_low(1, n)),
        Some(n) => n,
        // add: one level; remove: every level the tool has.
        None if add => 1,
        None => -1,
    };
    let mut max_removed = 0;
    let list = tryf!(
        host,
        held(host, &targets(c), Held::Modifiable, |host, living, stack| {
            let mut out = stack.clone();
            let mut ups = md::entries_of(&stack, md::C_UPGRADES);
            if add {
                modrecipe::add_modifier(&mut ups, &id, level);
            } else {
                let current: i32 = ups.iter().filter(|e| e.id == id).map(|e| e.level).sum();
                if current == 0 {
                    return Err(cmd("modifiers.failure.too_few_levels", vec![modifier_text(&reg, &id, level), lit(entity_name(&*host, living.entity()))]));
                }
                let remove = if level == -1 { current } else { level };
                max_removed = max_removed.max(remove);
                modrecipe::remove_modifier(&mut ups, &id, remove);
            }
            md::set_upgrades(&mut out, &ups);
            tool::rebuild(&*host, &mut out);
            validated(&*host, &reg, &out)?;
            if !add {
                on_removed(&stack, &mut out);
            }
            put_back(host, living, out);
            Ok(true)
        })
    );
    let shown = if add { level } else { max_removed };
    let key = if add { "modifiers.success.add" } else { "modifiers.success.remove" };
    success(host, key, vec![modifier_text(&reg, &id, shown)], &list)
}

// ---- slots (SlotsCommand)

fn slots(host: &mut dyn ServerHost, c: &CommandInvocation, set: bool) -> Option<i32> {
    let count = int_arg(c, "count").unwrap_or(1);
    if set && count < 0 {
        return fail(host, integer_low(0, count));
    }
    if count == 0 && !set {
        return fail(host, cmd("slots.failure.invalid_count", Vec::new()));
    }
    let slot = tryf!(host, checked(&*host, c, "slot_type", Kind::SlotType));
    let reg = md::registry(&*host);
    let list = tryf!(
        host,
        held(host, &targets(c), Held::Modifiable, |host, living, stack| {
            let mut out = stack.clone();
            let delta = if set { count - md::free_slots(&stack, &slot) } else { count };
            md::add_persistent_slots(&mut out, &slot, delta);
            tool::rebuild(&*host, &mut out);
            validated(&*host, &reg, &out)?;
            put_back(host, living, out);
            Ok(true)
        })
    );
    if set {
        success(host, "slots.success.set", vec![slot_text(&slot), lit(count.to_string())], &list)
    } else {
        success(host, "slots.success.add", vec![lit(count.to_string()), slot_text(&slot)], &list)
    }
}

// ---- durability (DurabilityCommand)

fn durability(host: &mut dyn ServerHost, c: &CommandInvocation, set: bool) -> Option<i32> {
    let Some(amount) = int_arg(c, "amount") else { return None };
    if set && amount < 0 {
        return fail(host, integer_low(0, amount));
    }
    if amount == 0 && !set {
        return fail(host, cmd("durability.failure.invalid_amount", Vec::new()));
    }
    let list = tryf!(
        host,
        held(host, &targets(c), Held::Durability, |host, living, stack| {
            let mut out = stack.clone();
            let damage = if set { tool::tool_durability(&stack) - amount } else { stack.damage() as i32 - amount };
            tool::set_tool_damage(&mut out, damage.max(0));
            put_back(host, living, out);
            Ok(true)
        })
    );
    let key = if set { "durability.success.set" } else { "durability.success.add" };
    success(host, key, vec![lit(amount.to_string())], &list)
}

// ---- tool_stats (StatsCommand, StatOverrideModifier)

/// A value written by `bonus set`: a numeric stat's float, or the harvest tier.
enum StatValue {
    Num(f32),
    Tier(usize),
}

fn override_tag(stack: &ItemStack, key: &str) -> Option<Compound> {
    match md::persistent_value(stack, key) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

/// StatOverrideModifier.getStat: a float entry, else `default`.
fn get_stat(stack: &ItemStack, key: &str, stat: &str, default: f32) -> f32 {
    match override_tag(stack, key).as_ref().and_then(|c| c.get(&js(stat)).cloned()) {
        Some(Tag::Float(b)) => f32::from_bits(b),
        _ => default,
    }
}

/// StatOverrideModifier.setStat: stores `value`, or removes the entry at `default` (and the compound once empty);
/// whether the compound still has entries.
fn set_stat(stack: &mut ItemStack, key: &str, stat: &str, value: f32, default: f32) -> bool {
    let create = value != default;
    let Some(mut tag) = override_tag(stack, key).or_else(|| create.then(Compound::new)) else { return false };
    if create {
        tag.insert(js(stat), Tag::Float(value.to_bits()));
        md::set_persistent(stack, key, Some(Tag::Compound(tag)));
        return true;
    }
    tag.remove(&js(stat));
    let empty = tag.is_empty();
    md::set_persistent(stack, key, (!empty).then_some(Tag::Compound(tag)));
    !empty
}

/// StatOverrideModifier.set (`bonus set`).
fn set_value(stack: &mut ItemStack, stat: &str, value: &StatValue) -> bool {
    let create = match value {
        StatValue::Num(f) => *f != 0.0,
        // ToolTierStat's default, wood.
        StatValue::Tier(t) => *t != 0,
    };
    let key = md::KEY_OVERRIDE_BONUS;
    let Some(mut tag) = override_tag(stack, key).or_else(|| create.then(Compound::new)) else { return false };
    if create {
        let written = match value {
            StatValue::Num(f) => Tag::Float(f.to_bits()),
            StatValue::Tier(t) => Tag::String(js(TIERS[*t])),
        };
        tag.insert(js(stat), written);
        md::set_persistent(stack, key, Some(Tag::Compound(tag)));
        return true;
    }
    tag.remove(&js(stat));
    let empty = tag.is_empty();
    md::set_persistent(stack, key, (!empty).then_some(Tag::Compound(tag)));
    !empty
}

/// StatOverrideModifier.remove (the bonus only).
fn remove_value(stack: &mut ItemStack, stat: &str) -> bool {
    let key = md::KEY_OVERRIDE_BONUS;
    let Some(mut tag) = override_tag(stack, key) else { return false };
    tag.remove(&js(stat));
    let empty = tag.is_empty();
    md::set_persistent(stack, key, (!empty).then_some(Tag::Compound(tag)));
    !empty
}

/// StatsCommand.updateEntities: `f` changes the copy's override data (answering whether any is left), the stat
/// override modifier is added or removed to match, and the tool is rebuilt and checked.
fn update_entities(host: &mut dyn ServerHost, c: &CommandInvocation, mut f: impl FnMut(&mut ItemStack) -> bool) -> Result<Vec<EntityRef>, Text> {
    let reg = md::registry(&*host);
    held(host, &targets(c), Held::Modifiable, |host, living, stack| {
        let mut out = stack.clone();
        let has_data = f(&mut out);
        let mut ups = md::entries_of(&out, md::C_UPGRADES);
        let has_mod = ups.iter().any(|e| e.id == md::STAT_OVERRIDE && e.level > 0);
        if has_data && !has_mod {
            modrecipe::add_modifier(&mut ups, md::STAT_OVERRIDE, 1);
        } else if has_mod && !has_data {
            modrecipe::remove_modifier(&mut ups, md::STAT_OVERRIDE, 1);
        }
        md::set_upgrades(&mut out, &ups);
        tool::rebuild(&*host, &mut out);
        validated(&*host, &reg, &out)?;
        put_back(host, living, out);
        Ok(true)
    })
}

/// A tag as NbtTagArgument reads it (TagParser.readValue) and its SNBT: numbers with their suffixes, booleans as
/// bytes, quoted or plain strings; a compound or list (no tool stat reads one) is kept as text.
enum Snbt {
    Number(f32, String),
    Str(String),
    Other(String),
}

fn snbt(raw: &str) -> Snbt {
    let s = raw.trim();
    if s.starts_with('{') || s.starts_with('[') {
        return Snbt::Other(s.to_owned());
    }
    let quoted = |q: char| s.len() >= 2 && s.starts_with(q) && s.ends_with(q);
    if quoted('"') || quoted('\'') {
        let inner = &s[1..s.len() - 1];
        let mut out = String::new();
        let mut esc = false;
        for ch in inner.chars() {
            if esc {
                out.push(ch);
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else {
                out.push(ch);
            }
        }
        return Snbt::Str(out);
    }
    match s {
        "true" => return Snbt::Number(1.0, "1b".into()),
        "false" => return Snbt::Number(0.0, "0b".into()),
        _ => {}
    }
    let lower = s.to_ascii_lowercase();
    let (body, suffix) = match lower.chars().last() {
        Some(c @ ('b' | 's' | 'l' | 'f' | 'd')) => (&s[..s.len() - 1], Some(c)),
        _ => (s, None),
    };
    let int_like = !body.is_empty() && body.trim_start_matches(['-', '+']).chars().all(|c| c.is_ascii_digit()) && body.trim_start_matches(['-', '+']).len() > 0;
    let float_like = body.parse::<f64>().is_ok() && body.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c));
    match suffix {
        Some('b') if int_like => body.parse::<i8>().map(|n| Snbt::Number(f32::from(n), format!("{n}b"))).unwrap_or(Snbt::Str(s.to_owned())),
        Some('s') if int_like => body.parse::<i16>().map(|n| Snbt::Number(f32::from(n), format!("{n}s"))).unwrap_or(Snbt::Str(s.to_owned())),
        Some('l') if int_like => body.parse::<i64>().map(|n| Snbt::Number(n as f32, format!("{n}L"))).unwrap_or(Snbt::Str(s.to_owned())),
        Some('f') if float_like => {
            let f = body.parse::<f32>().unwrap_or(0.0);
            Snbt::Number(f, format!("{}f", jfloat(f)))
        }
        Some('d') if float_like => {
            let d = body.parse::<f64>().unwrap_or(0.0);
            let shown = if d == d.trunc() && d.abs() < 1.0e7 { format!("{d:.1}") } else { format!("{d}") };
            Snbt::Number(d as f32, format!("{shown}d"))
        }
        None if int_like => match body.parse::<i32>() {
            Ok(n) => Snbt::Number(n as f32, n.to_string()),
            Err(_) => Snbt::Str(s.to_owned()),
        },
        None if float_like && body.contains(['.', 'e', 'E']) => {
            let d = body.parse::<f64>().unwrap_or(0.0);
            let shown = if d == d.trunc() && d.abs() < 1.0e7 { format!("{d:.1}") } else { format!("{d}") };
            Snbt::Number(d as f32, format!("{shown}d"))
        }
        _ => Snbt::Str(s.to_owned()),
    }
}

/// StringTag.toString: double-quoted, `\` and `"` escaped.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn tool_stats_update(host: &mut dyn ServerHost, c: &CommandInvocation, multiplier: bool, set: bool) -> Option<i32> {
    let (arg, kind) = if multiplier { ("float_stat", Kind::NumericToolStat) } else { ("stat_type", Kind::ToolStat) };
    let stat = tryf!(host, checked(&*host, c, arg, kind));
    let (shown, list) = if set && !multiplier {
        // `bonus set <stat_type> <value:nbt tag>`: IToolStat.read, else `parse_fail`.
        let raw = str_arg(c, "value").unwrap_or("");
        let parsed = snbt(raw);
        let value = match (&parsed, stat == ST_HARVEST_TIER) {
            (Snbt::Number(f, _), false) => Some(StatValue::Num(*f)),
            (Snbt::Str(name), true) => {
                let id = data::pattern_id(name);
                TIERS.iter().position(|t| *t == id).map(StatValue::Tier)
            }
            _ => None,
        };
        let text = match &parsed {
            Snbt::Number(_, t) | Snbt::Other(t) => t.clone(),
            Snbt::Str(s) => quoted(s),
        };
        let Some(value) = value else {
            return fail(host, cmd("stats.success.bonus.set.parse_fail", vec![stat_prefix(&stat), lit(text)]));
        };
        let list = tryf!(host, update_entities(host, c, |tool| set_value(tool, &stat, &value)));
        (text, list)
    } else {
        let Some(value) = num_arg(c, "value") else { return None };
        if !set && value == 0.0 && !multiplier {
            return fail(host, cmd("stats.failure.invalid_add", Vec::new()));
        }
        if !set && value == 1.0 && multiplier {
            return fail(host, cmd("stats.failure.invalid_multiply", Vec::new()));
        }
        let list = tryf!(
            host,
            update_entities(host, c, |tool| {
                if !multiplier {
                    // addBonus
                    value != 0.0 && set_stat(tool, md::KEY_OVERRIDE_BONUS, &stat, get_stat(tool, md::KEY_OVERRIDE_BONUS, &stat, 0.0) + value, 0.0)
                } else if set {
                    // setMultiplier
                    set_stat(tool, md::KEY_OVERRIDE_MULTIPLY, &stat, value, 1.0)
                } else {
                    // multiply
                    value != 1.0 && set_stat(tool, md::KEY_OVERRIDE_MULTIPLY, &stat, get_stat(tool, md::KEY_OVERRIDE_MULTIPLY, &stat, 1.0) * value, 1.0)
                }
            })
        );
        (jfloat(value), list)
    };
    let key = format!("stats.success.{}.{}", if multiplier { "multiply" } else { "bonus" }, if set { "set" } else { "modify" });
    success(host, &key, vec![stat_prefix(&stat), lit(shown)], &list)
}

fn tool_stats_reset(host: &mut dyn ServerHost, c: &CommandInvocation) -> Option<i32> {
    if str_arg(c, "stat_type").is_some() {
        let stat = tryf!(host, checked(&*host, c, "stat_type", Kind::ToolStat));
        let list = tryf!(
            host,
            update_entities(host, c, |tool| {
                if numeric_stat(&stat) {
                    set_stat(tool, md::KEY_OVERRIDE_MULTIPLY, &stat, 1.0, 1.0);
                }
                remove_value(tool, &stat)
            })
        );
        return success(host, "stats.success.reset.stat", vec![stat_prefix(&stat)], &list);
    }
    let reg = md::registry(&*host);
    let list = tryf!(
        host,
        held(host, &targets(c), Held::Modifiable, |host, living, stack| {
            if md::modifier_level(&stack, md::STAT_OVERRIDE) <= 0 {
                return Ok(false);
            }
            let mut out = stack.clone();
            let mut ups = md::entries_of(&stack, md::C_UPGRADES);
            modrecipe::remove_modifier(&mut ups, md::STAT_OVERRIDE, 1);
            md::set_upgrades(&mut out, &ups);
            tool::rebuild(&*host, &mut out);
            validated(&*host, &reg, &out)?;
            // StatOverrideModifier.onRemoved.
            md::set_persistent(&mut out, md::KEY_OVERRIDE_BONUS, None);
            md::set_persistent(&mut out, md::KEY_OVERRIDE_MULTIPLY, None);
            put_back(host, living, out);
            Ok(true)
        })
    );
    success(host, "stats.success.reset.all", Vec::new(), &list)
}

// ---- materials (MaterialsCommand)

fn materials_set(host: &mut dyn ServerHost, c: &CommandInvocation) -> Option<i32> {
    let Some(index) = int_arg(c, "index") else { return None };
    if index < 0 {
        return fail(host, integer_low(0, index));
    }
    let material = tryf!(host, checked(&*host, c, "material", Kind::MaterialVariant));
    let reg = md::registry(&*host);
    let list = tryf!(
        host,
        held(host, &targets(c), Held::Modifiable, |host, living, stack| {
            let tools = data::tooling(&*host);
            let size = tools.defs.get(sid(&stack)).map_or(0, |d| d.stat_types.len());
            let mut mats = tool::tool_materials(&stack);
            // ToolMaterialHook.stats: past the definition's parts; MaterialNBT.replaceMaterial: past the tool's.
            if index as usize >= size || index as usize >= mats.len() {
                return Err(cmd("materials.failure.invalid_index", vec![lit(index.to_string()), display_name(&*host, living.entity())]));
            }
            mats[index as usize] = material.clone();
            let mut out = stack.clone();
            tool::write_tool(&*host, &mut out, &mats);
            validated(&*host, &reg, &out)?;
            on_removed(&stack, &mut out);
            put_back(host, living, out);
            Ok(true)
        })
    );
    let (id, _) = data::parse_material(&material).unwrap_or_default();
    success(host, "materials.success.set", vec![lit(index.to_string()), material_text(&id)], &list)
}

/// IMaterialStats.getLocalizedInfo(scale).
fn stats_info(stat: &str, stats: &Stats, scale: f32) -> Vec<Text> {
    let key = |s: &str| format!("tool_stat.tconstruct.{s}");
    let repair = |d: f32| format_number("tool_stat.tconstruct.durability.repair", stat_color("durability"), ((d / 3.0) as i32) as f32);
    match stats {
        Stats::Head { durability, mining_speed, tier, attack } => vec![
            stat_value("tconstruct:durability", *durability as f32 * scale),
            stat_prefix(ST_HARVEST_TIER).append(tier_text(*tier)),
            stat_value("tconstruct:mining_speed", mining_speed * scale),
            stat_value("tconstruct:attack_damage", attack * scale),
        ],
        // HandleMaterialStats: only the durability is scaled, as Java's.
        Stats::Handle { durability, mining_speed, melee_speed, melee_damage } => vec![
            percent_boost(&key("durability"), durability * scale),
            percent_boost(&key("attack_damage"), *melee_damage),
            percent_boost(&key("attack_speed"), *melee_speed),
            percent_boost(&key("mining_speed"), *mining_speed),
        ],
        Stats::Limb { durability, draw_speed, velocity, accuracy } => vec![
            stat_value("tconstruct:durability", *durability as f32 * scale),
            bonus(&key("draw_speed"), draw_speed * scale),
            bonus(&key("velocity"), velocity * scale),
            bonus(&key("accuracy"), accuracy * scale),
        ],
        Stats::Grip { durability, accuracy, melee_damage } => vec![
            percent_boost(&key("durability"), durability * scale),
            bonus(&key("accuracy"), accuracy * scale),
            stat_value("tconstruct:attack_damage", melee_damage * scale),
        ],
        Stats::Plating { durability, armor, toughness, knockback } => {
            let mut v = vec![stat_value("tconstruct:durability", *durability as f32 * scale)];
            // PlatingMaterialStats: the shield's plating shows no armor.
            if stat != "tconstruct:plating_shield" {
                v.push(stat_value("tconstruct:armor", armor * scale));
            }
            v.push(stat_value("tconstruct:armor_toughness", toughness * scale));
            v.push(stat_value("tconstruct:knockback_resistance", knockback * 10.0 * scale));
            v
        }
        Stats::Slime { durability, overslime } => {
            vec![stat_value("tconstruct:durability", *durability as f32 * scale), stat_value("tconstruct:overslime", *overslime as f32 * scale)]
        }
        Stats::Skull { durability, armor } => {
            let mut v = vec![repair(*durability as f32)];
            if *armor > 0 {
                v.push(stat_value("tconstruct:armor", *armor as f32 * scale));
            }
            v
        }
        Stats::Repair { amount } => vec![repair(*amount as f32)],
        Stats::Statless => vec![tr("tool_stat.tconstruct.extra.no_stats", Vec::new())],
    }
}

fn with_lines(mut head: Text, lines: Vec<Text>) -> Text {
    for l in lines {
        head = head.append(lit("\n* ")).append(l);
    }
    head
}

fn materials_stats(host: &mut dyn ServerHost, c: &CommandInvocation) -> Option<i32> {
    let stat = tryf!(host, checked(&*host, c, "stat_type", Kind::MaterialStat));
    let scale = num_arg(c, "scale").unwrap_or(1.0);
    if str_arg(c, "material").is_none() {
        let Some(stats) = data::default_stats(&stat) else { return None };
        let head = cmd("materials.success.stats.default", vec![material_stat_name(&stat), lit(jfloat(scale))]);
        say(host, with_lines(head, stats_info(&stat, &stats, scale)));
        return Some(1);
    }
    let material = tryf!(host, checked(&*host, c, "material", Kind::Material));
    let mats = data::materials(&*host);
    let id = mats.resolve(&material).to_owned();
    let Some(stats) = mats.stats(&id, &stat).cloned() else {
        return fail(host, cmd("materials.failure.invalid_stats", vec![lit(stat), material_text(&id)]));
    };
    let head = cmd("materials.success.stats.material", vec![material_stat_name(&stat), material_text(&id), lit(jfloat(scale))]);
    say(host, with_lines(head, stats_info(&stat, &stats, scale)));
    Some(1)
}

fn materials_traits(host: &mut dyn ServerHost, c: &CommandInvocation) -> Option<i32> {
    let material = tryf!(host, checked(&*host, c, "material", Kind::Material));
    let stat = match str_arg(c, "stat_type") {
        Some(_) => Some(tryf!(host, checked(&*host, c, "stat_type", Kind::MaterialStat))),
        None => None,
    };
    let mats = data::materials(&*host);
    let reg = md::registry(&*host);
    let id = mats.resolve(&material).to_owned();
    let Some(m) = mats.get(&id) else { return fail(host, cmd("material.not_found", vec![lit(material)])) };
    let (traits, head) = match &stat {
        None => (m.traits_default.clone(), cmd("materials.success.traits.default", vec![material_text(&id)])),
        Some(s) => (m.traits(s).to_vec(), cmd("materials.success.traits.stat", vec![material_stat_name(s), material_text(&id)])),
    };
    let mut out = head;
    if traits.is_empty() {
        out = out.append(lit("\n* ")).append(styled(cmd("materials.failure.no_traits", Vec::new()), |s| s.italic = Some(true)));
    } else {
        for (t, level) in &traits {
            let gray = styled(lit(format!(" ({t})")), |s| s.color = Some("gray".into()));
            out = out.append(lit("\n* ")).append(modifier_text(&reg, t, *level)).append(gray);
        }
    }
    say(host, out);
    Some(traits.len() as i32)
}

// ---- report (ModifierUsageCommand, ModifierPriorityCommand)

/// Forge's TablePrinter (markdown-compliant): `| a | b |` rows padded to the columns' widths, the header's
/// separator of dashes.
fn table(out: &mut String, headers: &[&str], rows: &[Vec<String>]) {
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| rows.iter().map(|r| r[i].chars().count()).chain([h.chars().count()]).max().unwrap_or(0))
        .collect();
    let line = |out: &mut String, cells: &[String]| {
        out.push('|');
        for (c, w) in cells.iter().zip(&widths) {
            out.push(' ');
            out.push_str(c);
            out.push_str(&" ".repeat(w - c.chars().count()));
            out.push_str(" |");
        }
        out.push('\n');
    };
    let names: Vec<String> = headers.iter().map(|h| (*h).to_owned()).collect();
    line(&mut *out, names.as_slice());
    out.push('|');
    for w in &widths {
        out.push_str(&"-".repeat(w + 2));
        out.push('|');
    }
    out.push('\n');
    for r in rows {
        line(&mut *out, r.as_slice());
    }
}

/// TConstruct.LOG.info: the game's log (the launcher's `<instance>.log`).
fn log_info(text: &str) {
    for l in text.lines() {
        eprintln!("[TConstruct] {l}");
    }
}

/// ResourceLocation.compareTo: the path first, then the namespace.
fn id_order(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |s: &str| {
        let (ns, path) = s.split_once(':').unwrap_or(("minecraft", s));
        (path.to_owned(), ns.to_owned())
    };
    split(a).cmp(&split(b))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Usage {
    All,
    Recipe,
    MaterialTrait,
    ToolTrait,
    Unused,
}

fn modifier_usage(host: &mut dyn ServerHost, c: &CommandInvocation, usage: Usage) -> Option<i32> {
    // `recipe <slot_type>`: Some(None) for `slotless`.
    let filter: Option<Option<String>> = match (usage, str_arg(c, "slot_type")) {
        (Usage::Recipe, Some(_)) => {
            let s = tryf!(host, checked(&*host, c, "slot_type", Kind::OptionalSlotType));
            Some((s != "slotless").then_some(s))
        }
        _ => None,
    };
    let reg = md::registry(&*host);
    let mats = data::materials(&*host);
    let tools = data::tooling(&*host);
    let recipes = modrecipe::mod_recipes(&*host);
    let material_traits: HashSet<String> = mats
        .map
        .values()
        .flat_map(|m| m.traits_default.iter().chain(m.traits_per_stat.values().flatten()).map(|(id, _)| id.clone()))
        .collect();
    let tool_traits: HashSet<String> = tools.defs.iter().filter(|(item, _)| tools.modifiable.contains(*item)).flat_map(|(_, d)| d.traits.iter().map(|(id, _)| id.clone())).collect();
    let is_recipe = |id: &str, slot: Option<&str>| recipes.slot_types.get(id).is_some_and(|set| set.contains(&slot.map(str::to_owned)));
    let any_recipe = |id: &str| recipes.slot_types.get(id).is_some_and(|set| !set.is_empty());
    let mut ids: Vec<String> = match usage {
        Usage::Recipe => match &filter {
            Some(slot) => recipes.slot_types.iter().filter(|(_, set)| set.contains(slot)).map(|(id, _)| id.clone()).collect(),
            None => recipes.slot_types.iter().filter(|(_, set)| !set.is_empty()).map(|(id, _)| id.clone()).collect(),
        },
        Usage::MaterialTrait => material_traits.iter().cloned().collect(),
        Usage::ToolTrait => tool_traits.iter().cloned().collect(),
        Usage::All | Usage::Unused => reg.ids.clone(),
    };
    ids.retain(|id| reg.get(id).is_some());
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(reg.resolve(id)));
    if usage == Usage::Unused {
        ids.retain(|id| !any_recipe(id.as_str()) && !material_traits.contains(id) && !tool_traits.contains(id));
    }
    ids.sort_by(|a, b| id_order(a, b));
    let mut headers = vec!["ID"];
    if usage != Usage::Unused {
        if usage != Usage::Recipe || filter.is_none() {
            headers.push("Recipe");
        }
        if usage != Usage::MaterialTrait {
            headers.push("material Trait");
        }
        if usage != Usage::ToolTrait {
            headers.push("tool Trait");
        }
    }
    let rows: Vec<Vec<String>> = ids
        .iter()
        .map(|id| {
            let slots: Vec<&str> = SLOT_TYPES.iter().copied().filter(|s| is_recipe(id.as_str(), Some(*s))).collect();
            let recipe = if slots.is_empty() { if is_recipe(id.as_str(), None) { "slotless".to_owned() } else { String::new() } } else { slots.join(",") };
            headers
                .iter()
                .map(|h| match *h {
                    "ID" => id.clone(),
                    "Recipe" => recipe.clone(),
                    "material Trait" => if material_traits.contains(id) { "Material trait".into() } else { String::new() },
                    _ => if tool_traits.contains(id) { "Tool trait".into() } else { String::new() },
                })
                .collect()
        })
        .collect();
    let mut out = String::from(match usage {
        Usage::Unused => "Unused modifiers:",
        Usage::Recipe => "Recipe modifiers:",
        Usage::MaterialTrait => "Material trait modifiers:",
        Usage::ToolTrait => "Tool trait modifiers:",
        Usage::All => "All modifiers:",
    });
    if let Some(slot) = &filter {
        match slot {
            None => out.push_str(" (slotless)"),
            Some(s) => out.push_str(&format!(" ({s})")),
        }
    }
    out.push('\n');
    table(&mut out, &headers, &rows);
    log_info(&out);
    say(host, cmd("modifier_usage", Vec::new()));
    Some(ids.len() as i32)
}

fn modifier_priority(host: &mut dyn ServerHost, _c: &CommandInvocation) -> Option<i32> {
    let reg = md::registry(&*host);
    let mut ids = reg.ids.clone();
    // Comparator.comparingInt(Modifier::getPriority).reversed().thenComparing(Modifier::getId).
    ids.sort_by(|a, b| reg.priority(b).cmp(&reg.priority(a)).then_with(|| id_order(a, b)));
    let rows: Vec<Vec<String>> = ids.iter().map(|id| vec![id.clone(), reg.priority(id).to_string()]).collect();
    let mut out = String::from("Modifier Priorities:\n");
    table(&mut out, &["ID", "Priority"], &rows);
    log_info(&out);
    say(host, cmd("modifier_priority", Vec::new()));
    Some(ids.len() as i32)
}

// ---- generate hidden_fluids_tag (GenerateHiddenFluidsCommand, GeneratePackHelper)

/// SmelteryCompat: each compat fluid and the material that makes it present (else its `c:ingots/<name>` tag).
const SMELTERY_COMPAT: [(&str, &str, Option<&str>); 28] = [
    ("tin", "molten_tin", None),
    ("aluminum", "molten_aluminum", Some("aluminum")),
    ("lead", "molten_lead", Some("lead")),
    ("silver", "molten_silver", Some("silver")),
    ("nickel", "molten_nickel", None),
    ("zinc", "molten_zinc", None),
    ("platinum", "molten_platinum", None),
    ("tungsten", "molten_tungsten", None),
    ("osmium", "molten_osmium", Some("osmium")),
    ("uranium", "molten_uranium", Some("necronium")),
    ("chromium", "molten_chromium", None),
    ("cadmium", "molten_cadmium", None),
    ("bronze", "molten_bronze", Some("bronze")),
    ("brass", "molten_brass", Some("plated_slimewood")),
    ("electrum", "molten_electrum", Some("electrum")),
    ("invar", "molten_invar", Some("invar")),
    ("constantan", "molten_constantan", Some("constantan")),
    ("pewter", "molten_pewter", Some("pewter")),
    ("enderium", "molten_enderium", None),
    ("lumium", "molten_lumium", None),
    ("signalum", "molten_signalum", None),
    ("refined_glowstone", "molten_refined_glowstone", None),
    ("refined_obsidian", "molten_refined_obsidian", None),
    ("nicrosil", "molten_nicrosil", Some("nicrosil")),
    ("duralumin", "molten_duralumin", None),
    ("bendalloy", "molten_bendalloy", None),
    ("steeleaf", "molten_steeleaf", Some("steeleaf")),
    ("fiery", "fiery_liquid", Some("fiery")),
];

/// GeneratePackHelper's pack: `SlimeKnightsGenerated` in the world's `datapacks/`.
const PACK_NAME: &str = "SlimeKnightsGenerated";
const PACK_DESCRIPTION: &str = "Data pack generated by commands from SlimeKnights mods.";
/// The game's data pack format (Sunrise's `DATA_PACK_FORMAT` for 26.3), which saveMcmeta writes.
const PACK_FORMAT: i64 = 121;

fn save_json(path: &std::path::Path, v: &Value) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(v).unwrap_or_default())
}

/// GeneratePackHelper.saveMcmeta: a failure is only logged, as Java's.
fn save_mcmeta(pack: &std::path::Path) {
    let meta = serde_json::json!({"pack": {"description": PACK_DESCRIPTION, "pack_format": PACK_FORMAT, "min_format": [PACK_FORMAT, 0], "max_format": [PACK_FORMAT, 0]}});
    if let Err(e) = save_json(&pack.join("pack.mcmeta"), &meta) {
        eprintln!("[Mantle] Couldn't create JSON at {}: {e}", pack.join("pack.mcmeta").display());
    }
}

fn generate_hidden_fluids(host: &mut dyn ServerHost, _c: &CommandInvocation) -> Option<i32> {
    let tag = "c:hidden_from_recipe_viewers";
    let write_error = || tr("command.mantle.modify_tag.write_error", vec![lit("fluid"), lit(tag)]);
    let Some(world) = host.world_dir() else { return fail(host, write_error()) };
    let pack = world.join("datapacks").join(PACK_NAME);
    save_mcmeta(&pack);
    // `data/c/tags/fluid/` (26.3's tag folder; 1.20.1's `tags/fluids`).
    let tag_path = pack.join("data").join("c").join("tags").join("fluid").join("hidden_from_recipe_viewers.json");
    let mats = data::materials(&*host);
    let mut add: Vec<String> = Vec::new();
    for (name, fluid, material) in SMELTERY_COMPAT {
        let present = material.is_some_and(|m| mats.get(&format!("tconstruct:{m}")).is_some()) || !tag_items(&*host, &format!("c:ingots/{name}")).is_empty();
        if !present {
            // A flowing fluid's local tag (FlowingFluidObject.getLocalTag).
            add.push(format!("#tconstruct:{fluid}"));
        }
    }
    // TagFile(add, replace false, remove none): only `values` is written.
    if let Err(e) = save_json(&tag_path, &serde_json::json!({ "values": add })) {
        eprintln!("[Mantle] Couldn't save fluid tag {tag} to {}: {e}", tag_path.display());
        return fail(host, write_error());
    }
    let line = cmd(
        "generate_hidden_fluids",
        vec![lit(add.len().to_string()), path_text(lit(tag)), path_text(lit(pack.display().to_string()))],
    );
    say(host, line);
    Some(add.len() as i32)
}

// ---- generate melting_recipes (GenerateMeltingRecipesCommand)

fn generate_melting_recipes(host: &mut dyn ServerHost, c: &CommandInvocation) -> Option<i32> {
    let start = std::time::Instant::now();
    let kind = match c.argument("recipe_type") {
        Some(CommandArgumentValue::Id(id)) => id.to_string(),
        _ => return None,
    };
    let invalid = || cmd("generate.melting_recipes.invalid_config", Vec::new());
    let Some(world) = host.world_dir() else { return fail(host, invalid()) };
    let pack = world.join("datapacks").join(PACK_NAME);
    save_mcmeta(&pack);
    match crate::smeltery::melting_gen::generate(host, &kind, &pack.join("data")) {
        Ok(n) => {
            // `(nanoTime - start) / 1000000.0F`: milliseconds, though the line says seconds.
            let time = start.elapsed().as_nanos() as f32 / 1_000_000.0;
            let line = cmd("generate.melting_recipes", vec![lit(n.to_string()), lit(jfloat(time)), path_text(lit(pack.display().to_string()))]);
            say(host, line);
            Some(n)
        }
        Err(crate::smeltery::melting_gen::GenError::InvalidConfig) => fail(host, invalid()),
        // An exception out of the command: Brigadier's `command.failed`.
        Err(crate::smeltery::melting_gen::GenError::Failed) => fail(host, tr("command.failed", Vec::new())),
    }
}

// ---- The tree (TConstructCommand.registerCommand)

type Exec = fn(&mut dyn ServerHost, &CommandInvocation) -> Option<i32>;

/// The usages, Java's `requires` levels and their executors.
const USAGES: &[(&str, u8, Exec)] = &[
    ("tconstruct modifiers <targets:entities> add <modifier:tconstruct:modifier> [level:integer]", 2, |h, c| modifiers(h, c, true)),
    ("tconstruct modifiers <targets:entities> remove <modifier:tconstruct:modifier> [level:integer]", 2, |h, c| modifiers(h, c, false)),
    ("tconstruct materials set <targets:entities> <index:integer> <material:tconstruct:material_variant>", 2, materials_set),
    ("tconstruct materials stats <stat_type:tconstruct:material_stat> [scale:number]", 2, materials_stats),
    ("tconstruct materials stats <stat_type:tconstruct:material_stat> <material:tconstruct:material> [scale:number]", 2, materials_stats),
    ("tconstruct materials traits <material:tconstruct:material> [stat_type:tconstruct:material_stat]", 2, materials_traits),
    ("tconstruct tool_stats <targets:entities> bonus add <stat_type:tconstruct:tool_stat> <value:number>", 2, |h, c| tool_stats_update(h, c, false, false)),
    // NbtTagArgument.nbtTag(): any SNBT tag, the rest of the line here.
    ("tconstruct tool_stats <targets:entities> bonus set <stat_type:tconstruct:tool_stat> <value:text>", 2, |h, c| tool_stats_update(h, c, false, true)),
    ("tconstruct tool_stats <targets:entities> multiplier multiply <float_stat:tconstruct:numeric_tool_stat> <value:number>", 2, |h, c| tool_stats_update(h, c, true, false)),
    ("tconstruct tool_stats <targets:entities> multiplier set <float_stat:tconstruct:numeric_tool_stat> <value:number>", 2, |h, c| tool_stats_update(h, c, true, true)),
    ("tconstruct tool_stats <targets:entities> reset [stat_type:tconstruct:tool_stat]", 2, tool_stats_reset),
    ("tconstruct slots <targets:entities> add <slot_type:tconstruct:slot_type> [count:integer]", 2, |h, c| slots(h, c, false)),
    ("tconstruct slots <targets:entities> set <slot_type:tconstruct:slot_type> <count:integer>", 2, |h, c| slots(h, c, true)),
    ("tconstruct durability <targets:entities> add <amount:integer>", 2, |h, c| durability(h, c, false)),
    ("tconstruct durability <targets:entities> set <amount:integer>", 2, |h, c| durability(h, c, true)),
    ("tconstruct report modifier_usage", 1, |h, c| modifier_usage(h, c, Usage::All)),
    ("tconstruct report modifier_usage all", 1, |h, c| modifier_usage(h, c, Usage::All)),
    ("tconstruct report modifier_usage recipe [slot_type:tconstruct:optional_slot_type]", 1, |h, c| modifier_usage(h, c, Usage::Recipe)),
    ("tconstruct report modifier_usage material_trait", 1, |h, c| modifier_usage(h, c, Usage::MaterialTrait)),
    ("tconstruct report modifier_usage tool_trait", 1, |h, c| modifier_usage(h, c, Usage::ToolTrait)),
    ("tconstruct report modifier_usage unused", 1, |h, c| modifier_usage(h, c, Usage::Unused)),
    ("tconstruct report modifier_priority", 1, modifier_priority),
    ("tconstruct generate hidden_fluids_tag", 2, generate_hidden_fluids),
    ("tconstruct generate melting_recipes <recipe_type:resource:minecraft:recipe_type>", 2, generate_melting_recipes),
];

/// Pushes the subcommands (the bare `/tconstruct`, the menu, is pushed beside them).
pub(crate) fn register(event: &mut RegisterCommandsEvent<'_>) {
    for &(usage, level, exec) in USAGES {
        // An executor's `None` answered `command.unknown.command` before executors could fail with a message: still so.
        event.commands.push(Command::op(usage, level).executes(move |host, _source, c| {
            exec(host, c).ok_or_else(|| Box::new(Text::translate("command.unknown.command", Vec::new())))
        }));
    }
}
