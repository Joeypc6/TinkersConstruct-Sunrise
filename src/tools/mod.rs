//! Stage 3: tool building. 3A: the materials and parts (`data`), the stations (`station`); 3B: the tools (`tool`);
//! 3C: the modifiers (`modifier`), their recipes and the modifier worktable (`modrecipe`), AOE, tool actions and area
//! attacks (`action`). The ranged tools and armor (3D) build on these.

pub(crate) mod action;
pub(crate) mod armor;
pub(crate) mod combat;
pub(crate) mod data;
pub(crate) mod loot;
pub(crate) mod modifier;
pub(crate) mod modrecipe;
pub(crate) mod ranged;
pub(crate) mod station;
pub(crate) mod tank;
pub(crate) mod tool;
pub(crate) mod transfer;

use crate::smeltery::util::*;
use mod_api::*;
use station::*;

/// The recipe kinds the clients need (the part builder's screen, the tooltips, the recipe viewer).
pub(crate) const SYNCED: [&str; 22] = [
    "tconstruct:material",
    "tconstruct:part_builder",
    "tconstruct:item_part_builder",
    "tconstruct:tool_building",
    "tconstruct:material_fluid",
    "tconstruct:table_casting_material",
    // The modifier recipes: the worktable's screen and the station's error text are computed on the client too.
    "tconstruct:modifier",
    "tconstruct:incremental_modifier",
    "tconstruct:multilevel_modifier",
    "tconstruct:multilevel_incremental_modifier",
    "tconstruct:swappable_modifier",
    "tconstruct:overslime_modifier",
    "tconstruct:modifier_repair",
    "tconstruct:remove_modifier",
    "tconstruct:extract_modifier",
    "tconstruct:modifier_sorting",
    "tconstruct:modifier_set_worktable",
    "tconstruct:toggle_interaction",
    "tconstruct:enchantment_converting",
    "tconstruct:modifier_salvage",
    // The severing recipes: the recipe viewer's severing category.
    "tconstruct:severing",
    "tconstruct:ageable_severing",
];

/// Java's block entity types: the crafting station's, the part builder's and the tinker station's (the anvils too).
const TYPES: [(&str, &[&str]); 4] = [
    ("tconstruct:crafting_station", &[CRAFTING_STATION]),
    ("tconstruct:part_builder", &[PART_BUILDER]),
    ("tconstruct:tinker_station", &[TINKER_STATION, TINKERS_ANVIL, SCORCHED_ANVIL]),
    ("tconstruct:modifier_worktable", &[MODIFIER_WORKTABLE]),
];

#[derive(Default)]
pub(crate) struct Tools;

pub(crate) fn register(r: &mut dyn Registrar) {
    combat::register(r);
    tank::register(r);
    for (ty, blocks) in TYPES {
        let key = r.block_entity_type(ty).blocks(blocks).build();
        r.block_entity_part(&key, Tools).expect("the stations' block entity types are native");
    }
    for m in [CRAFTING_STATION, PART_BUILDER, TINKER_STATION, TINKERS_ANVIL, MODIFIER_WORKTABLE] {
        r.menu_part(m, Tools);
        if let Err(e) = r.register_menu_screen(m, std::sync::Arc::new(menu_screen)) {
            crate::smeltery::util::tlog(&format!("{m}: no screen ({e})"));
        }
    }
    let _ = modifier::EFFECT_KEYS.set(modifier::EFFECTS.iter().map(|e| (*e, r.effect_key(e))).collect());
    let t: &mod_api::serde_json::Value = &data::table().json;
    // TinkerTools: the definition `slime_chestplate` is the item `slimy_chestplate`'s.
    let tools: Vec<String> = t["tool_definitions"].as_object().map(|o| o.keys().map(|k| if k == "tconstruct:slime_chestplate" { "tconstruct:slimy_chestplate".to_owned() } else { k.clone() }).collect()).unwrap_or_default();
    for item in tools.iter().map(String::as_str).chain(data::PARTS.iter().map(|(p, _)| *p)).chain([modrecipe::MODIFIER_CRYSTAL]) {
        let k = r.item_key(item);
        r.item_part(k, Tools);
    }
}

pub(crate) fn component_types(event: &mut ComponentTypesEvent<'_>) {
    for c in [tool::C_MATERIALS, tool::C_STATS, tool::C_MODIFIERS, modifier::C_UPGRADES, modifier::C_PERSISTENT, modifier::C_VOLATILE, modifier::C_MULTIPLIERS] {
        event.register(c.to_owned());
    }
}

/// Whether `stack` is a part of the material `want` (MaterialVariantId.matchesVariant), for material melting.
pub(crate) fn part_is(host: &dyn Host, stack: &ItemStack, want: &str) -> bool {
    let mats = data::materials(host);
    match (data::parse_material(want), data::part_material(&mats, stack)) {
        (Some(w), Some(have)) => data::matches_variant(&w, &have),
        _ => false,
    }
}

pub(crate) fn player_break_speed(host: HostSide, event: &mut PlayerBreakSpeedEvent) {
    match host {
        HostSide::Server(h) => tool::break_speed(&*h, event),
        HostSide::Client(h) => tool::break_speed(&*h, event),
    }
}

pub(crate) fn player_harvest_check(host: HostSide, event: &mut PlayerHarvestCheckEvent) {
    match host {
        HostSide::Server(h) => tool::harvest_check(&*h, event),
        HostSide::Client(h) => tool::harvest_check(&*h, event),
    }
}

/// A player breaks a block: a station's virtual result goes first; the tool wears, its modifiers act and its AOE
/// breaks the other blocks.
pub(crate) fn break_block(host: &mut dyn ServerHost, pos: Pos, state: BlockState) {
    ensure_ids(&*host);
    combat::before_break(&*host);
    if let Some(level) = here(&*host) {
        clear_result(host, level, pos);
    }
    action::block_broken(host, pos, state);
}

#[mod_api::sunrise_part]
impl BlockPart for Tools {
    fn use_without_item(&self, host: &mut dyn ServerHost, event: &mut UseWithoutItemEvent<'_>) -> Option<InteractionResult> {
        let player = host.player(event.click.player)?;
        let level = player.level(&*host)?;
        let entity = host.block_entity(level, event.click.pos)?;
        // Every slot held before the menu opens (sunrise-org/sunrise#4463).
        let block = block_id(&*host, level, event.click.pos);
        pad_slots(host, entity, &block);
        match player.open_menu(host, entity) {
            Ok(true) => Some(InteractionResult::Success),
            _ => None,
        }
    }

    /// Mantle's InventoryBlock.use opens the station whatever the hand holds (a held item is not placed or used on it;
    /// sneaking with an item never reaches here).
    fn use_item_on(&self, _host: &mut dyn ServerHost, _event: &mut UseItemOnEvent<'_>) -> Option<InteractionResult> {
        Some(InteractionResult::TryWithEmptyHand)
    }

    fn on_block_exploded(&self, host: &mut dyn ServerHost, event: &mut OnBlockExplodedEvent<'_>) {
        if let Some(level) = here(&*host) {
            clear_result(host, level, [event.pos.x, event.pos.y, event.pos.z]);
        }
    }
}

#[mod_api::sunrise_part]
impl BlockEntityPart for Tools {
    fn block_entity_on_load(&self, event: &mut BlockEntityOnLoadEvent<'_>) {
        crate::smeltery::util::pad_on_load(&mut event.subject);
    }

    fn block_entity(&self, _level: LevelSide, _event: &mut BlockEntityEvent<'_>) -> Option<Compound> {
        Some(Compound::new())
    }

    fn block_entity_ticks(&self, _host: &dyn Host, event: &mut BlockEntityTicksEvent<'_>) {
        event.ticks = true;
    }

    fn block_entity_tick(&self, event: &mut BlockEntityTickEvent<'_>) {
        let _t = crate::perf::t(crate::perf::BE_STATION);
        crate::tick_fields(event, settle_fields);
    }

    fn menu_quick_move(&self, event: &mut MenuQuickMoveEvent<'_>) -> Flow {
        // The entity's edits are kept only when the move is handled (an unhandled one's were dropped before too).
        let (container_slots, slot, stack) = (event.container_slots, event.slot, &mut *event.stack);
        crate::scoped_fields(&mut event.subject, false, |host, pos, e| {
            let flow = station::quick_move(host, pos, container_slots, slot, e, stack);
            (flow, matches!(flow, Flow::Cancel))
        })
        .unwrap_or(Flow::Continue)
    }
}

/// The stations' screens, made by the client for each opening of the block's menu (none without the block).
fn menu_screen(host: &dyn ClientHost, open: MenuScreenOpen<'_>) -> Option<MenuScreen> {
    let block = open.block?.to_owned();
    ensure_ids(host);
    let layout = station::screen(host, &block)?;
    let (draw_block, tip_block) = (block.clone(), block.clone());
    Some(
        MenuScreen::new(layout)
            .with_draw(move |host, frame, draw| station::draw(host, &draw_block, frame.data, draw, frame.origin))
            .with_click(move |_host, frame, _right| station::click(&block, frame.data, frame.at))
            .with_tooltip(move |host, frame, _slot, lines| {
                crate::tooltip_lines(lines, |out| station::tooltip(host, &tip_block, frame.data, frame.at, out))
            }),
    )
}

#[mod_api::sunrise_part]
impl MenuPart for Tools {
    /// The station's button `id` (the part builder's patterns, the worktable's choices), on the server; a menu opened
    /// without the station's block has no buttons.
    fn menu_button(&self, event: &mut MenuButtonEvent<'_>) {
        let (Some(pos), id) = (event.context.pos, event.context.id) else { return };
        crate::scoped_fields(&mut event.subject, false, |host, _, e| (station::menu_button(host, pos, id, e), true));
    }

    /// Every id reaches `menu_button`, as every `clickMenuButton` reached the Java menus: each checks its own range.
    fn menu_button_valid(&self, _host: &dyn Host, event: &mut MenuButtonValidEvent<'_>) -> Flow {
        if event.block.is_some() { Flow::Cancel } else { Flow::Continue }
    }

    fn menu_slot_rules(&self, host: &dyn Host, event: &mut MenuSlotRulesEvent<'_>) {
        station::slot_rules(host, event);
    }

    /// JEI's "+" on a modifier recipe into the tinker station or an anvil (`transfer`).
    fn menu_transfer(&self, host: &mut dyn ServerHost, event: &mut MenuTransferEvent<'_>) -> Option<RecipeTransferResult> {
        ensure_ids(&*host);
        transfer::menu_transfer(host, event)
    }
}

#[mod_api::sunrise_part]
impl ItemPart for Tools {
    fn tooltip(&self, host: &dyn ClientHost, event: &mut TooltipEvent<'_>) {
        ensure_ids(host);
        tool::tool_tooltip(host, event.stack, event.lines);
        tool::part_tooltip(host, event.stack, event.lines);
        modrecipe::crystal_tooltip(host, event.stack, event.lines);
    }

    fn use_duration(&self, _host: &dyn Host, event: &mut UseDurationEvent<'_>) {
        ranged::use_duration(event);
    }

    fn use_animation(&self, _host: &dyn Host, event: &mut UseAnimationEvent<'_>) {
        ranged::use_animation(event);
    }

    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(h) => {
                ensure_ids(&*h);
                // BucketModule.onToolUse: a fluid source on the look into the tool's tank.
                if let Some(player) = h.players().acting_player() {
                    let living = LivingRef::new(player.entity());
                    if let Some(out) = tank::bucket_pickup(h, living, event.hand, event.stack) {
                        let _ = living.set_hand_item(h, event.hand, Some(out));
                        return Some(InteractionResult::Success);
                    }
                }
                ranged::use_item(HostSide::Server(h), event)
            }
            client => ranged::use_item(client, event),
        }
    }

    fn release_using(&self, host: HostSide, event: &mut ReleaseUsingEvent<'_>) {
        match host {
            HostSide::Server(host) => {
                ensure_ids(&*host);
                ranged::release(host, event);
                ranged::sling_release(host, event);
            }
            HostSide::Client(host) => ranged::sling_client(host, event.stack, event.time_left),
        }
    }

    fn hurt_enemy(&self, host: &mut dyn ServerHost, event: &mut HurtEnemyEvent<'_>) {
        ensure_ids(&*host);
        tool::hurt_enemy(host, event.stack, event.entity);
    }

    /// ModifiableItem.useOn: the right-click tool actions (strip, till, flatten...) and their AOE.
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        // The server's, as the fork's was: a claimed use swung there.
        let HostSide::Server(host) = host else { return None };
        ensure_ids(&*host);
        // BucketModule.afterBlockUse: sneaking, the tank's fluid placed against the face.
        if let Some(player) = host.player(event.player) {
            let living = LivingRef::new(player.entity());
            if let Some(out) = tank::bucket_place(host, living, event.stack, event.pos, event.face) {
                let _ = living.set_hand_item(host, event.hand, Some(out));
                return Some(InteractionResult::SuccessServer);
            }
        }
        action::use_on(host, event).then_some(InteractionResult::SuccessServer)
    }

    /// InventoryTickModifierHook every 20 ticks (overgrowth's overslime).
    fn inventory_tick(&self, host: &mut dyn ServerHost, event: &mut InventoryTickEvent<'_>) {
        let _t = crate::perf::t(crate::perf::INVENTORY);
        ensure_ids(&*host);
        let Some(player) = host.players().acting_player() else { return };
        // IModifiable.verifyTagAfterLoad: a tool given with its materials but no data is built from them.
        let mut fresh = None;
        // A tool without parts (the staffs) is built from no materials.
        let partless = || data::tooling(&*host).defs.get(sid(event.stack)).is_some_and(|d| d.parts.is_empty());
        if !tool::tool_initialized(event.stack) && (!tool::tool_materials(event.stack).is_empty() || partless()) {
            let mut out = event.stack.clone();
            if tool::rebuild(&*host, &mut out) {
                fresh = Some(out);
            } else {
                static SEEN: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);
                let key = format!("{} {:?}", sid(event.stack), tool::tool_materials(event.stack));
                if SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().insert(key.clone()) {
                    crate::smeltery::util::tlog(&format!("{key} could not be built (slot {})", event.slot));
                }
            }
        }
        let out = match fresh {
            Some(out) => Some(out),
            None if event.slot < 0 || player.entity().tick_count(&*host).unwrap_or(1) % 20 != 0 || !tool::tool_initialized(event.stack) => return,
            None => modifier::inventory_tick(&*host, event.stack),
        };
        if let Some(out) = out {
            let place = ItemPlace::PlayerSlot { player: player.uuid(), slot: event.slot };
            // The whole stack (a stack of shurikens is rebuilt at once: one alone can't go back in beside the rest).
            let (res, n) = (ItemResource::of(event.stack), event.stack.count());
            let ok = host.transaction(|host, tx| {
                if place.extract(host, tx, &res, n) == Ok(n) && place.insert(host, tx, &ItemResource::of(&out), n) == Ok(n) {
                    return Ok(());
                }
                Err(())
            });
            if !tool::tool_initialized(event.stack) {
                crate::smeltery::util::tlog(&format!("built {} x{n} in slot {}: {}", sid(event.stack), event.slot, if ok.is_ok() { "ok" } else { "refused" }));
            }
        }
    }

    /// ModifiableItem.damageItem: a tool wears only through ToolDamageUtil (its own hooks): the game's own wear,
    /// which would destroy it at its max damage, is refused.
    fn damage_item(&self, _host: &mut dyn ServerHost, event: &mut DamageItemEvent<'_>) -> Flow {
        if tool::tool_initialized(event.stack) { Flow::Cancel } else { Flow::Continue }
    }
}
