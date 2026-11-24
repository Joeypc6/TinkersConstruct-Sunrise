//! Tinkers' Construct 3.12.1.231 (SlimeKnights, MIT) and the parts of Mantle 1.11.128 it uses, for the arismoko/horizon
//! fork's mod API. Hooks and Parts only, no mixins and no `[[inject]]`, so it builds against the fork's prebuilt SDK.
//! The upstream mixin port (`../tconstruct-horizon`) is the reference for the logic; this is it on the fork's APIs.
//!
//! Where things are:
//! - content (blocks, items, recipes, tags, creative tabs, fluids, the W guide) is data: `assets/`, `data/`, written
//!   by `tools/make_fork_content.py` from the jar;
//! - `fluids`: Tinkers' fluids, the table the generator compiles in, the world fluids' models;
//! - `smeltery`: the melter, the heater, the tanks and lanterns, the fluid containers (stage 2A), casting (2B), the
//!   smeltery and foundry multiblocks (2C);
//! - `tools`: materials, parts and the stations (3A), the modular tools (3B).

mod commands;
mod commands_menu;
mod fluids;
pub(crate) mod gadgets;
pub(crate) mod perf;
#[allow(dead_code)]
mod gen_colors;
mod gen_colormaps;
mod gen_fluid_effects;
mod gen_fluids;
#[allow(dead_code)]
mod gen_tools;
pub(crate) mod smeltery;
pub(crate) mod tools;
pub(crate) mod world;

use mod_api::*;

#[derive(Default)]
pub(crate) struct Mod;

/// A slot of a screen at `position` (GUI pixels from the screen's corner), with no empty-slot icon: Sunrise's screens
/// take `ScreenSlot`s since 2d81263.
pub(crate) fn slot_at(position: [f32; 2]) -> ScreenSlot {
    ScreenSlot { position, no_item_icon: None }
}

/// `screen_tooltip` (version 3) takes rich Text/colour pairs: the screens' plain lines (with their `§` codes) are added
/// after whatever the list holds, in the default white, as before.
pub(crate) fn tooltip_lines(lines: &mut Vec<(Text, u32)>, f: impl FnOnce(&mut Vec<String>)) {
    let mut plain = Vec::new();
    f(&mut plain);
    lines.extend(plain.into_iter().map(|l| (Text::literal(l), 0xFFFF_FFFF)));
}

/// A screen's slot places (hidden ones `None`) as its `ScreenSlot`s.
pub(crate) fn screen_slots(places: Vec<Option<[f32; 2]>>) -> Vec<Option<ScreenSlot>> {
    places.into_iter().map(|p| p.map(slot_at)).collect()
}

/// The server's half of a screen's hidden slots. Up to Sunrise 9425651 the server laid the screen out too, and a block
/// menu's slot the layout hid at opening (or one past its list) did not exist there: no click or shift-click reached
/// it. Since main the layout is the client's alone and the server keeps every slot (sunrise debt-58), so the menus'
/// `menu_slot_rules` refuse those slots here, from the same layout the screen opens with (`None`: no screen, nothing
/// shown).
pub(crate) fn refuse_hidden_slot(event: &mut MenuSlotRulesEvent<'_>, layout: Option<&MenuScreenLayout>) {
    if !matches!(event.container, MenuContainer::Main) {
        return;
    }
    let shown = layout.is_some_and(|l| l.slots.get(event.slot).is_some_and(Option::is_some));
    if !shown {
        event.may_place = false;
        event.may_pickup = false;
    }
}

/// `block_entity_tick` (version 4) lends the ticking block entity through `subject.with_view` instead of handing its
/// fields out of the world and taking a reply. The ticks are written against the fields (`f` gets the Host, the
/// position and a copy of them), so this copies them out, runs `f`, and writes back what `f` changed: the saved
/// fields one by one, the item slots that differ, the menu data. As the reply did, a changed save marks the chunk
/// unsaved (the view no longer does it by itself); clients are sent what they show by the ticks' own
/// `block_entity_sync`, as before.
pub(crate) fn tick_fields(event: &mut BlockEntityTickEvent<'_>, f: impl FnOnce(&mut dyn ServerHost, [i32; 3], &mut smeltery::Fields)) {
    scoped_fields(&mut event.subject, true, |host, pos, fields| {
        f(host, pos, fields);
        ((), true)
    });
}

/// The same copy-out, run and write-back for the menu hooks, which (MenuPart v3) lend their block entity through a
/// `BlockEntityScope` instead of handing its fields out of the world: `f` gets the Host, the entity's position and a
/// copy of its fields, and the `bool` it answers says whether its changes are kept (`false` drops them, as an
/// unhandled quick move's entity edits were dropped before). `mark_changed` marks the chunk unsaved after a saved
/// change (a ticker must; a menu hook's view does it by itself). `None` without the entity (a blockless menu).
pub(crate) fn scoped_fields<R>(
    subject: &mut BlockEntityScope<'_>,
    mark_changed: bool,
    f: impl FnOnce(&mut dyn ServerHost, [i32; 3], &mut smeltery::Fields) -> (R, bool),
) -> Option<R> {
    let (pos, handle, before) = subject.with_view(|v| {
        let items = (0..v.item_slots()).map(|i| v.item_slot(i as u32).and_then(|s| s.to_owned())).collect();
        let mut fields = smeltery::Fields::default();
        (fields.nbt, fields.items, fields.data) = (v.nbt().clone(), items, v.data_slots().to_vec());
        (v.position(), v.handle(), fields)
    })?;
    let mut after = before.clone();
    let (answer, keep) = f(subject.host(), pos, &mut after);
    if !keep || after == before {
        return Some(answer);
    }
    let saved = subject.with_view(|mut v| {
        let mut saved = false;
        if after.nbt != before.nbt {
            for (k, t) in after.nbt.iter() {
                if before.nbt.get(k) != Some(t) {
                    saved |= v.set_nbt(k.clone(), Some(t.clone()));
                }
            }
            for k in before.nbt.keys() {
                if !after.nbt.contains_key(k) {
                    saved |= v.set_nbt(k.clone(), None);
                }
            }
        }
        if after.items != before.items {
            for i in 0..after.items.len().max(before.items.len()) {
                let (a, b) = (after.items.get(i).cloned().flatten(), before.items.get(i).cloned().flatten());
                if a != b {
                    // A slot past the declared ones takes nothing (the old reply's extra rows were dropped too).
                    saved |= v.set_item_slot(i as u32, a).unwrap_or(false);
                }
            }
        }
        if after.data != before.data {
            v.set_data_slots(&after.data);
        }
        saved
    });
    if mark_changed && saved == Some(true) {
        let _ = handle.set_changed(subject.host());
    }
    Some(answer)
}

/// Tinkers' recipe types, every `type` of the data's recipe files outside `minecraft:` (read with `recipes_of_type` or the
/// mod's own loaders): Sunrise refuses a recipe file whose serializer no mod registered, so `register` declares each
/// type and its serializer (the same id). `recipe_type_tests` checks the list against `data/*/recipe`.
pub(crate) const RECIPE_TYPES: &[&str] = &[
    "tconstruct:ageable_severing", "tconstruct:alloy", "tconstruct:armor_dyeing_modifier",
    "tconstruct:armor_trim_modifier", "tconstruct:banner_modifier", "tconstruct:basin_casting_composite",
    "tconstruct:basin_casting_material", "tconstruct:basin_casting_part_swapping", "tconstruct:basin_filling",
    "tconstruct:basin_tool_casting", "tconstruct:casting_basin", "tconstruct:casting_table",
    "tconstruct:casting_table_potion", "tconstruct:casting_table_tipped_clearing", "tconstruct:casting_table_tipping",
    "tconstruct:crafting_modifier_repair", "tconstruct:crafting_overslime_modifier",
    "tconstruct:crafting_shaped_materials", "tconstruct:crafting_shapeless_materials",
    "tconstruct:crafting_table_repair", "tconstruct:damagable_melting", "tconstruct:enchantment_converting",
    "tconstruct:entity_melting", "tconstruct:extract_modifier", "tconstruct:fixed_material_swapping",
    "tconstruct:incremental_modifier", "tconstruct:item_part_builder", "tconstruct:material",
    "tconstruct:material_fluid", "tconstruct:material_melting", "tconstruct:material_value_swapping",
    "tconstruct:melting", "tconstruct:melting_fuel", "tconstruct:modifier", "tconstruct:modifier_repair",
    "tconstruct:modifier_salvage", "tconstruct:modifier_set_worktable", "tconstruct:modifier_sorting",
    "tconstruct:molding_table", "tconstruct:mooshroom_demushrooming", "tconstruct:multilevel_incremental_modifier",
    "tconstruct:multilevel_modifier", "tconstruct:ore_melting", "tconstruct:overslime_modifier",
    "tconstruct:part_builder", "tconstruct:part_builder_recycling", "tconstruct:part_builder_tool_recycling",
    "tconstruct:player_beheading", "tconstruct:remove_modifier", "tconstruct:retextured_casting_basin",
    "tconstruct:severing", "tconstruct:sheep_shearing", "tconstruct:snow_golem_beheading",
    "tconstruct:swappable_modifier", "tconstruct:table_casting_composite", "tconstruct:table_casting_material",
    "tconstruct:table_casting_part_swapping", "tconstruct:table_duplication", "tconstruct:table_filling",
    "tconstruct:table_tool_casting", "tconstruct:tinker_station_damaging", "tconstruct:tinker_station_part_swapping",
    "tconstruct:tinker_station_repair", "tconstruct:tipped_tool_transform", "tconstruct:toggle_interaction",
    "tconstruct:tool_building", "tconstruct:tool_material_swapping",
];

#[mod_api::sunrise_mod]
impl Hooks for Mod {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        // Tinkers' and Mantle's global loot modifiers (tools::loot).
        tools::loot::register(r);
        for kind in RECIPE_TYPES {
            r.register_recipe_type(kind);
            r.register_raw_recipe_serializer(kind, kind);
        }
        smeltery::register(r);
        tools::register(r);
        world::island::register(r);
        world::sapling::register(r);
        gadgets::register(r);
        declare_storage(r);
        // `/tconstruct`: the subcommands' argument types and the commands menu's channel.
        commands::register_arguments(r);
        commands_menu::register_payload(r);
    }

    /// `/tconstruct` (TConstructCommand): the commands menu (everyone's) and the subcommands (their own levels).
    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        commands::register(event);
        event.commands.push(commands_menu::command());
    }

    fn register_block_tint_sources(&self, _context: &RegistrationContext, event: &mut RegisterBlockTintSourcesEvent<'_>) {
        world::tint::register_block_tints(event);
    }

    fn component_types(&self, _context: &RegistrationContext, event: &mut ComponentTypesEvent<'_>) {
        tools::component_types(event);
    }

    fn register_item_tint_sources(&self, _context: &RegistrationContext, event: &mut RegisterItemTintSourcesEvent<'_>) {
        tools::tool::register_tint(event);
    }

    fn player_break_speed(&self, host: HostSide, event: &mut PlayerBreakSpeedEvent) -> Flow {
        let _t = perf::t(perf::OTHER);
        tools::player_break_speed(host, event);
        Flow::Continue
    }

    fn player_harvest_check(&self, host: HostSide, event: &mut PlayerHarvestCheckEvent) {
        let _t = perf::t(perf::OTHER);
        tools::player_harvest_check(host, event);
    }

    fn break_block(&self, host: &mut dyn ServerHost, event: &mut BreakBlockEvent<'_>) -> Flow {
        let _t = perf::t(perf::OTHER);
        tools::break_block(host, event.pos, event.state);
        Flow::Continue
    }

    fn item_attribute_modifier(&self, event: &mut ItemAttributeModifierEvent<'_>) {
        let _t = perf::t(perf::ATTRIBUTES);
        tools::tool::attributes(event.stack, event.modifiers);
    }

    fn add_server_reload_listeners(&self, _context: &dyn ReloadContext, event: &mut AddServerReloadListenersEvent<'_>) {
        smeltery::add_server_reload_listeners(event);
    }

    fn mod_container_viewers(&self, host: &mut dyn ServerHost, event: &mut ModContainerViewersEvent<'_>) {
        let _t = perf::t(perf::OTHER);
        smeltery::mod_container_viewers(host, event);
        tools::station::viewer_count(event.pos, event.viewers);
    }

    fn player_container_open(&self, _host: &mut dyn ServerHost, event: &mut PlayerContainerOpenEvent<'_>) {
        let _t = perf::t(perf::OTHER);
        smeltery::player_container_open(event);
        tools::station::container_opened(event.dimension, event.player, event.pos, true);
    }

    fn player_container_close(&self, _host: &mut dyn ServerHost, event: &mut PlayerContainerCloseEvent<'_>) {
        let _t = perf::t(perf::OTHER);
        smeltery::player_container_close(event);
        tools::station::container_opened(event.dimension, event.player, event.pos, false);
    }

    fn block_drops(&self, level: &dyn LevelHost, event: &mut BlockDropsEvent<'_>) -> Flow {
        let _t = perf::t(perf::OTHER);
        tools::tool::block_drops(level, event);
        tools::combat::autosmelt(event);
        tools::loot::block_drops(level, event);
        tools::combat::block_experience(event);
        smeltery::block_drops(level, event)
    }

    fn right_click_block(&self, host: HostSide, event: &mut RightClickBlockEvent<'_>) -> Flow {
        let _t = perf::t(perf::OTHER);
        smeltery::right_click_block(host, event);
        Flow::Continue
    }

    /// The recipe caches (materials, tool building, melting, casting, alloys, smelting) are made again from the new data.
    fn tags_updated(&self, _context: &dyn ReloadContext, _event: &mut TagsUpdatedEvent) {
        smeltery::util::recipes_changed();
    }

    fn recipes_received(&self, _host: &mut dyn ClientHost, _event: &mut RecipesReceivedEvent<'_>) {
        smeltery::util::recipes_changed();
    }

    fn datapack_sync(&self, _host: &mut dyn ServerHost, event: &mut DatapackSyncEvent<'_>) {
        smeltery::datapack_sync(event.recipe_types);
    }

    fn client_tick_post(&self, host: &mut dyn ClientHost, _event: &mut ClientTickPostEvent) {
        smeltery::client_tick(host);
        gadgets::client_jump_push(host);
    }

    fn level_tick_post(&self, host: HostSide, event: &mut LevelTickPostEvent) {
        let _t = perf::t(perf::LEVEL);
        if let HostSide::Server(host) = host {
            if host.dimension() == "minecraft:overworld" {
                perf::server_tick();
                tools::loot::server_tick(&*host);
            }
            { let _t = perf::t(perf::ISLAND); world::island::level_tick(host, event.level); }
            { let _t = perf::t(perf::SMELTERY_LEVEL); smeltery::level_tick(host); }
            { let _t = perf::t(perf::RANGED); tools::ranged::tick(host); }
            { let _t = perf::t(perf::GADGETS_LEVEL); gadgets::level_tick(host); }
            { let _t = perf::t(perf::SAPLING); world::sapling::after_growth(host); }
        }
    }

    fn block_changed(&self, level: &dyn LevelHost, event: &mut BlockChangedEvent) {
        let _t = perf::t(perf::BLOCK_CHANGED);
        smeltery::structure::block_changed(level, event);
    }

    fn living_experience_drop(&self, host: &mut dyn ServerHost, event: &mut LivingExperienceDropEvent<'_>) -> Flow {
        let _t = perf::t(perf::LIVING);
        tools::combat::experience_drop(host, event);
        Flow::Continue
    }

    fn living_knockback(&self, _level: &mut dyn LevelRandomHost, event: &mut LivingKnockbackEvent<'_>) -> Flow {
        let _t = perf::t(perf::LIVING);
        tools::combat::knockback(event);
        Flow::Continue
    }

    fn player_tick_post(&self, host: HostSide, _event: &mut PlayerTickPostEvent) {
        let _t = perf::t(perf::PLAYER);
        if let HostSide::Server(host) = host {
            tools::combat::player_tick(&*host);
            tools::armor::player_tick(host);
            gadgets::player_tick(host);
            tools::loot::player_tick(&*host);
        }
    }

    fn hears_projectile_impacts(&self, _host: &dyn Host, event: &mut HearsProjectileImpactsEvent) {
        let _t = perf::t(perf::LIVING);
        // Combat notes each hit's knockback; the launchers' and thrown tools' hits are tracked in `ranged`.
        event.hears = true;
    }

    fn projectile_impact(&self, host: &mut dyn ServerHost, event: &mut ProjectileImpactEvent<'_>) -> Flow {
        let _t = perf::t(perf::LIVING);
        tools::tank::arrow_hit(host, event.projectile, event.entity);
        if gadgets::impact(host, event) {
            return Flow::Cancel;
        }
        if tools::ranged::impact(host, event.projectile, event.entity) {
            return Flow::Cancel;
        }
        tools::combat::projectile_impact(&*host, event);
        Flow::Continue
    }

    fn attack_entity(&self, host: &mut dyn ServerHost, event: &mut AttackEntityEvent<'_>) -> Flow {
        let _t = perf::t(perf::LIVING);
        tools::combat::attack_entity(&*host, event);
        Flow::Continue
    }

    fn living_incoming_damage(&self, _level: &mut dyn HitHost, event: &mut LivingIncomingDamageEvent<'_>) -> Flow {
        let _t = perf::t(perf::LIVING);
        tools::combat::incoming_damage(event);
        tools::armor::incoming_damage(event);
        gadgets::blast_damage(event);
        Flow::Continue
    }

    /// Shearing with a Tinkers tool, on the server (the old claim swung there: SuccessServer).
    fn entity_interact(&self, host: HostSide, event: &mut EntityInteractEvent<'_>) -> Option<InteractionResult> {
        let _t = perf::t(perf::OTHER);
        let HostSide::Server(host) = host else { return None };
        tools::action::shear_entity(host, event).then_some(InteractionResult::SuccessServer)
    }

    fn critical_hit(&self, _host: &mut dyn ServerHost, event: &mut CriticalHitEvent<'_>) {
        let _t = perf::t(perf::LIVING);
        tools::combat::critical_hit(event);
    }

    fn living_shield_block(&self, _level: &mut dyn HitHost, event: &mut LivingShieldBlockEvent<'_>) {
        let _t = perf::t(perf::LIVING);
        tools::armor::shield_block(event);
    }

    fn living_damage_post(&self, _level: &mut dyn HitHost, event: &mut LivingDamagePostEvent<'_>) {
        let _t = perf::t(perf::LIVING);
        tools::armor::damage_post(event);
    }

    fn living_drops(&self, host: &mut dyn ServerHost, event: &mut LivingDropsEvent<'_>) -> Flow {
        let _t = perf::t(perf::LIVING);
        tools::loot::living_drops(host, event);
        tools::armor::drops(host, event);
        Flow::Continue
    }

    fn register_fluid_models(&self, _cx: &RegistrationContext, event: &mut RegisterFluidModelsEvent<'_>) {
        fluids::fluid_models(event);
    }

    fn consume_effect(&self, host: &mut dyn ServerHost, event: &mut ConsumeEffectEvent<'_>) {
        let _t = perf::t(perf::OTHER);
        gadgets::consume_effect(host, event);
    }

    fn explosion_detonate(&self, _host: &mut dyn ServerHost, event: &mut ExplosionDetonateEvent<'_>) {
        let _t = perf::t(perf::OTHER);
        gadgets::detonate(event);
    }

    fn living_fall(&self, _context: &dyn LoopContext, event: &mut LivingFallEvent<'_>) -> Flow {
        let _t = perf::t(perf::LIVING);
        gadgets::living_fall(event);
        Flow::Continue
    }

    fn block_grow_feature(&self, level: &mut dyn LevelRandomHost, event: &mut BlockGrowFeatureEvent<'_>) -> Flow {
        let _t = perf::t(perf::OTHER);
        world::sapling::grow_feature(level, event);
        Flow::Continue
    }

    fn key_mapping_pre(&self, _host: &mut dyn ClientHost, event: &mut KeyMappingPreEvent<'_>) -> Flow {
        gadgets::key_mapping(event);
        Flow::Continue
    }
}

/// The slots and tanks of every Tinkers block with a block entity, declared once (Sunrise's `block_container` and
/// `block_fluids`; the fork asked per block at run time): the melter and heater, the tanks and lanterns, the casting
/// tables, basins and tank, the smeltery and foundry controllers and ducts, and the stations.
fn declare_storage(r: &mut dyn Registrar) {
    use smeltery::{casting, structure};
    let blocks = [smeltery::MELTER, smeltery::HEATER, casting::CASTING_TANK, structure::SMELTERY, structure::FOUNDRY]
        .into_iter()
        .chain(smeltery::TANKS.iter().map(|(b, _)| *b))
        .chain(smeltery::LANTERNS)
        .chain(casting::TABLES)
        .chain(casting::BASINS)
        .chain(structure::DUCTS)
        .chain(tools::station::STATIONS);
    for block in blocks {
        let container = smeltery::block_entity_container(block).or_else(|| tools::station::container(block));
        let fluids = smeltery::block_entity_fluids(block);
        if container.is_none() && fluids.is_none() {
            continue;
        }
        let key = r.block_key(block);
        if let Some(c) = container {
            r.block_container(&key, c);
        }
        if let Some(t) = fluids {
            r.block_fluids(&key, t);
        }
    }
}

#[cfg(test)]
mod recipe_type_tests {
    /// The top-level `"type"` of a recipe file's JSON object (a small scanner: the mod has no JSON parser of its own).
    fn top_type(text: &str) -> Option<String> {
        let b = text.as_bytes();
        let (mut i, mut depth, mut key, mut expect_value) = (0usize, 0i32, None::<String>, false);
        while i < b.len() {
            match b[i] {
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                b':' if depth == 1 => expect_value = true,
                b',' if depth == 1 => (expect_value, key) = (false, None),
                b'"' => {
                    let start = i + 1;
                    i += 1;
                    while b[i] != b'"' {
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                    let s = &text[start..i];
                    if depth == 1 {
                        if !expect_value {
                            key = Some(s.to_owned());
                        } else if key.as_deref() == Some("type") {
                            return Some(s.to_owned());
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "json") {
                out.push(p);
            }
        }
    }

    /// Every recipe file's type outside `minecraft:` is registered (else Sunrise refuses the whole recipe load).
    #[test]
    fn every_data_recipe_type_is_registered() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        let mut files = Vec::new();
        for ns in std::fs::read_dir(&data).into_iter().flatten().flatten() {
            walk(&ns.path().join("recipe"), &mut files);
        }
        assert!(!files.is_empty(), "no recipe files under {}", data.display());
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let t = top_type(text.trim_start_matches('\u{feff}')).unwrap_or_default();
            assert!(
                t.starts_with("minecraft:") || super::RECIPE_TYPES.contains(&t.as_str()),
                "{}: {t} is not registered",
                f.display()
            );
        }
    }
}
