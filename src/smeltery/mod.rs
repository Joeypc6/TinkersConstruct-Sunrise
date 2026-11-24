//! The smeltery's first blocks (TinkerSmeltery, stage 2A): the seared melter (MelterBlock, MelterBlockEntity with its
//! MeltingModuleInventory and SolidFuelModule), the seared heater (HeaterBlock, HeaterBlockEntity), the seared and
//! scorched fuel and ingot tanks and gauges and the two lanterns (SearedTankBlock, SearedLanternBlock,
//! TankBlockEntity), their menus and screens (MelterContainerMenu, MelterScreen, the heater's SingleItemContainerMenu),
//! the fluid drawn inside them (TankInventoryBlockEntityRenderer, TankBlockEntityRenderer) and Mantle's fluid
//! containers (buckets, the copper can, the tank items; FluidTransferHelper).
//!
//! On the fork:
//! - three block entity types, as Java's: `tconstruct:melter`, `tconstruct:heater`, `tconstruct:tank` (the eight
//!   tanks and gauges and the two lanterns);
//! - the tanks are the game's (`ModFluidTanks`: the `fluids` list of the saved compound), so every mod's pipes and
//!   helpers fill and drain them through `FLUIDS` from any side, as Forge's FLUID_HANDLER;
//! - the melter's inputs and the heater's slot are the block entities' containers, reached through our own `ITEMS`
//!   handlers with Java's rules (MeltingModuleInventory: one item into an empty input; HeaterItemHandler: fuels only);
//! - fluid containers through `ITEM_FLUIDS` (`containers`);
//! - per-state light from the blocks' `state_light` (13 while `active`), facing and `in_structure` from
//!   `state_for_placement` and `update_shape`, comparators from `analog_output`.

pub(crate) mod casting;
pub(crate) mod casting_recipes;
pub(crate) mod casting_render;
pub(crate) mod containers;
pub(crate) mod melter;
pub(crate) mod melting_gen;
pub(crate) mod recipes;
pub(crate) mod render;
pub(crate) mod screen;
pub(crate) mod structure;
pub(crate) mod structure_screen;
pub(crate) mod util;

#[cfg(feature = "jei")]
pub(crate) mod casting_jei;
#[cfg(feature = "jei")]
pub(crate) mod jei;
#[cfg(feature = "jei")]
pub(crate) mod jei_more;

use mod_api::*;
use util::*;

pub(crate) const MELTER: &str = "tconstruct:seared_melter";
pub(crate) const HEATER: &str = "tconstruct:seared_heater";
pub(crate) const MELTER_MENU: &str = "tconstruct:melter";
pub(crate) const HEATER_MENU: &str = "tconstruct:heater";
pub(crate) const MELTER_TYPE: &str = "tconstruct:melter";
pub(crate) const HEATER_TYPE: &str = "tconstruct:heater";
pub(crate) const TANK_TYPE: &str = "tconstruct:tank";
/// MelterBlockEntity: FluidTankAnimated(FluidValues.INGOT * 24).
pub(crate) const MELTER_CAPACITY: i32 = 2160;
/// SearedTankBlock.TankType: the fuel tanks and gauges hold 4 buckets, the ingot ones 48 ingots.
pub(crate) const TANKS: [(&str, i32); 8] = [
    ("tconstruct:seared_fuel_tank", 4000),
    ("tconstruct:seared_fuel_gauge", 4000),
    ("tconstruct:seared_ingot_tank", 4320),
    ("tconstruct:seared_ingot_gauge", 4320),
    ("tconstruct:scorched_fuel_tank", 4000),
    ("tconstruct:scorched_fuel_gauge", 4000),
    ("tconstruct:scorched_ingot_tank", 4320),
    ("tconstruct:scorched_ingot_gauge", 4320),
];
/// SearedLanternBlock(..., 50): a TankBlockEntity of 50.
pub(crate) const LANTERNS: [&str; 2] = ["tconstruct:seared_lantern", "tconstruct:scorched_lantern"];
pub(crate) const LANTERN_CAPACITY: i32 = 50;
/// `#tconstruct:fuel_tanks` and `#tconstruct:heater_controllers` as the jar has them, for where the tags can't be read.
pub(crate) const FUEL_TANKS: [&str; 9] = [
    HEATER,
    "tconstruct:seared_fuel_tank",
    "tconstruct:seared_fuel_gauge",
    "tconstruct:seared_ingot_tank",
    "tconstruct:seared_ingot_gauge",
    "tconstruct:scorched_fuel_tank",
    "tconstruct:scorched_fuel_gauge",
    "tconstruct:scorched_ingot_tank",
    "tconstruct:scorched_ingot_gauge",
];
pub(crate) const HEATER_CONTROLLERS: [&str; 2] = [MELTER, "tconstruct:scorched_alloyer"];

pub(crate) fn tank_capacity(block: &str) -> Option<i32> {
    if block == MELTER {
        return Some(MELTER_CAPACITY);
    }
    if LANTERNS.contains(&block) {
        return Some(LANTERN_CAPACITY);
    }
    if block == casting::CASTING_TANK {
        return Some(casting::CASTING_TANK_CAPACITY);
    }
    TANKS.iter().find(|(b, _)| *b == block).map(|(_, c)| *c)
}

pub(crate) fn is_tank(block: &str) -> bool {
    TANKS.iter().any(|(b, _)| *b == block)
}

/// The part every one of these blocks, their block entities, items and menus carry.
#[derive(Default)]
pub(crate) struct Smeltery;

pub(crate) fn register(r: &mut dyn Registrar) {
    let tanks: Vec<&str> = TANKS.iter().map(|(b, _)| *b).chain(LANTERNS.iter().copied()).collect();
    for (ty, blocks) in [(MELTER_TYPE, vec![MELTER]), (HEATER_TYPE, vec![HEATER]), (TANK_TYPE, tanks)] {
        let key = r.block_entity_type(ty).blocks(&blocks).build();
        r.block_entity_part(&key, Smeltery).expect("the smeltery's block entity types are native");
        match ty {
            MELTER_TYPE => r.block_entity_capability_before_game(&ITEMS, &key, melter::melter_items),
            HEATER_TYPE => r.block_entity_capability_before_game(&ITEMS, &key, melter::heater_items),
            _ => {}
        }
        // The block entity part is the blocks' part too (`expand_block_entity_parts`).
        let _ = blocks;
    }
    for item in [MELTER, HEATER, containers::COPPER_CAN].iter().chain(TANKS.iter().map(|(b, _)| b)).chain(LANTERNS.iter()) {
        let k = r.item_key(item);
        r.item_part(k, Smeltery);
    }
    for m in [MELTER_MENU, HEATER_MENU] {
        r.menu_part(m, Smeltery);
        if let Err(e) = r.register_menu_screen(m, std::sync::Arc::new(menu_screen)) {
            util::tlog(&format!("{m}: no screen ({e})"));
        }
    }
    containers::register(r);
    render::register(r);
    casting::register(r);
    casting_render::register(r);
    structure::register(r);
}

/// The melter's inputs and the heater's slot (Horizon's container: the menus' slots, dropped when the block goes).
pub(crate) fn block_entity_container(block: &str) -> Option<ModContainer> {
    if block == MELTER {
        // MeltingModuleInventory's three inputs, and slot 3: the heater's item lent to the menu (`melter::lend_tick`).
        // MelterBlockEntity's ITEM_HANDLER (MeltingModuleInventory) on every side: the three inputs reach the faces;
        // the lent slot is the menu's alone (no face), but an input and an output too, as main's
        // `Container.canPlaceItem`/`canTakeItem` read those lists. `can_place_item` keeps the world's inserts to Java's.
        let inputs: Vec<usize> = (0..melter::INPUTS).collect();
        let all: Vec<usize> = (0..=melter::HEATER_SLOT).collect();
        return Some(ModContainer {
            slots: melter::INPUTS + 1,
            faces: std::array::from_fn(|_| inputs.clone()),
            inputs: all.clone(),
            outputs: all,
            insert_rules: Vec::new(),
            menu: Some(MELTER_MENU.to_owned()),
        });
    }
    if block == HEATER {
        return Some(ModContainer { slots: 1, faces: std::array::from_fn(|_| vec![0]), inputs: vec![0], outputs: vec![0], insert_rules: Vec::new(), menu: Some(HEATER_MENU.to_owned()) });
    }
    casting::block_entity_container(block).or_else(|| structure::block_entity_container(block))
}

/// The melter's and the tanks' tank: one, filled and drained from every side, any fluid.
pub(crate) fn block_entity_fluids(block: &str) -> Option<ModFluidTanks> {
    let capacity = tank_capacity(block)?;
    let mut t = ModFluidTanks::default();
    t.tanks = vec![capacity];
    t.faces = std::array::from_fn(|_| vec![0]);
    t.inputs = vec![0];
    t.outputs = vec![0];
    Some(t)
}

pub(crate) fn add_server_reload_listeners(event: &mut AddServerReloadListenersEvent<'_>) {
    containers::add_reload_listener(event);
}

pub(crate) fn mod_container_viewers(host: &mut dyn ServerHost, event: &mut ModContainerViewersEvent<'_>) {
    melter::container_viewers(host, event.dimension, event.pos, event.viewers);
}

pub(crate) fn player_container_open(event: &mut PlayerContainerOpenEvent<'_>) {
    melter::container_opened(event.dimension, event.player, event.pos, true);
}

pub(crate) fn player_container_close(event: &mut PlayerContainerCloseEvent<'_>) {
    melter::container_opened(event.dimension, event.player, event.pos, false);
}

/// A tank's fluid kept in its item (TankBlockEntity's drop: "Retains liquid when broken").
pub(crate) fn block_drops(level: &dyn LevelHost, event: &mut BlockDropsEvent<'_>) -> Flow {
    let Some(block) = level.state_block(event.state).map(str::to_owned) else { return Flow::Continue };
    if !(is_tank(&block) || LANTERNS.contains(&block.as_str()) || block == casting::CASTING_TANK) {
        return Flow::Continue;
    }
    let key = (event.dimension.key().id().to_owned(), [event.pos.x, event.pos.y, event.pos.z]);
    let Some((Some(fluid), amount)) = melter::removed_tank(&key) else { return Flow::Continue };
    for i in 0..event.drops.len() {
        let Some(mut d) = event.drops.get_mut(i) else { continue };
        if d.stack().is_some_and(|s| key_id(s.item()) == block && s.count() == 1)
            && let Some(stack) = d.stack().and_then(|s| s.project())
            && let Some(s) = containers::with_item_tank(&stack, Some(&fluid), amount)
        {
            let _ = d.set_stack(s);
        }
    }
    Flow::Continue
}

/// The client remembers which melter it clicked: the screen hook is told only the block.
pub(crate) fn right_click_block(host: HostSide, event: &mut RightClickBlockEvent<'_>) {
    if let HostSide::Client(host) = host
        && let Some(level) = here(&*host)
    {
        let block = block_id(&*host, level, event.click.pos);
        if block == MELTER || block == HEATER {
            screen::clicked(event.click.pos);
        }
        casting::client_click(&*host, &block, event);
        structure::client_click(&*host, &block, event);
        // MelterBlock.use / SearedTankBlock.use answer SUCCESS for a fluid container, so the item's own use (a bucket
        // placing its fluid) never runs: the client ends its click as used (the server's click still goes to the block,
        // whose `use_item_on` moves the fluid).
        let sneaking = host.local_player().and_then(|p| p.entity().is_shift_key_down(&*host)).unwrap_or(false);
        if (block == MELTER || is_tank(&block))
            && !sneaking
            && event.click.stack.is_some_and(|s| containers::maybe_container(&*host, s))
        {
            event.cancelled = true;
            *event.cancellation_result = InteractionResult::Success;
        }
    }
}

/// The melting and fuel recipes go to the clients (the screen's fuel tooltip, the recipe viewer).
pub(crate) fn datapack_sync(types: &mut Vec<String>) {
    for t in recipes::MELTING_KINDS.iter().chain([recipes::FUEL_KIND, recipes::MATERIAL_MELTING].iter()).chain(crate::tools::SYNCED.iter()).chain(casting_recipes::all_kinds().iter()).chain([structure::ALLOY_KIND, structure::ENTITY_MELTING_KIND].iter()) {
        if !types.iter().any(|x| x == t) {
            types.push((*t).to_owned());
        }
    }
}

pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    ensure_ids(&*host);
    melter::ensure_fuels(&*host);
    render::particles(host);
    casting_render::particles(host);
    #[cfg(feature = "jei")]
    mod_api::if_installed!(host, jei, {
        jei::register(host);
        casting_jei::register(host);
        jei_more::register(host);
        jei_more::debug_show();
    });
}

pub(crate) fn level_tick(host: &mut dyn ServerHost) {
    melter::level_tick(host);
    casting::level_tick(host);
    structure::level_tick(host);
}

// ---- Parts

/// ControllerBlock.getStateForPlacement: facing the placer; `in_structure` from the block below (melter) or above
/// (heater).
fn placement(level: &dyn LevelHost, event: &StateForPlacementEvent<'_>) -> Option<Option<BlockState>> {
    let block = level.state_block(event.state)?.to_owned();
    if block != MELTER && block != HEATER {
        return None;
    }
    let c = event.context;
    let y = placement_yaw(&c).rem_euclid(360.0);
    let facing = if !(45.0..315.0).contains(&y) {
        "north"
    } else if y < 135.0 {
        "east"
    } else if y < 225.0 {
        "south"
    } else {
        "west"
    };
    let p = c.pos;
    let other = if block == MELTER { [p.x, p.y - 1, p.z] } else { [p.x, p.y + 1, p.z] };
    let formed = level.block_state(other).is_some_and(|s| melter::structure_neighbour(level, &block, s));
    let s = level.state_with_property(event.state, "facing", facing)?;
    Some(level.state_with_property(s, "in_structure", if formed { "true" } else { "false" }))
}

#[mod_api::sunrise_part]
impl BlockPart for Smeltery {
    fn state_for_placement(&self, level: LevelSide<'_>, event: &mut StateForPlacementEvent<'_>) -> Option<Option<BlockState>> {
        let (LevelSide::Server(level) | LevelSide::Client(level)) = level;
        placement(level, event)
    }

    fn update_shape(&self, level: &mut dyn LevelTicksHost, event: &mut UpdateShapeEvent) -> Option<BlockState> {
        melter::update_shape(level, event)
    }

    fn has_analog_output(&self, level: &dyn LevelHost, event: &mut HasAnalogOutputEvent) {
        if level.state_block(event.state).is_some_and(|b| b != HEATER) {
            event.output = true;
        }
    }

    fn analog_output(&self, _level: &mut dyn LevelRandomHost, event: &mut AnalogOutputEvent) {
        let key = (event.dimension.key().id().to_owned(), [event.pos.x, event.pos.y, event.pos.z]);
        event.output = melter::signal_at(&key);
    }

    fn use_item_on(&self, host: &mut dyn ServerHost, event: &mut UseItemOnEvent<'_>) -> Option<InteractionResult> {
        melter::use_block(host, event.click, true)
    }

    fn use_without_item(&self, host: &mut dyn ServerHost, event: &mut UseWithoutItemEvent<'_>) -> Option<InteractionResult> {
        melter::use_block(host, event.click, false)
    }
}

#[mod_api::sunrise_part]
impl BlockEntityPart for Smeltery {
    fn block_entity_on_load(&self, event: &mut BlockEntityOnLoadEvent<'_>) {
        crate::smeltery::util::pad_on_load(&mut event.subject);
    }

    fn block_entity(&self, _level: LevelSide, _event: &mut BlockEntityEvent<'_>) -> Option<Compound> {
        Some(Compound::new())
    }

    fn block_entity_ticks(&self, _host: &dyn Host, event: &mut BlockEntityTicksEvent<'_>) {
        // The melter ticks (MelterBlock's server ticker); the tanks too, for what Java does on each fluid change (their
        // clients and comparators); the heater only so its container gets its slot (the game sizes a mod block
        // entity's item list nowhere else).
        event.ticks = true;
    }

    fn block_entity_tick(&self, event: &mut BlockEntityTickEvent<'_>) {
        let _t = crate::perf::t(crate::perf::BE_MELTER);
        crate::tick_fields(event, melter::tick);
    }

    fn block_entity_update_tag(&self, event: &mut BlockEntityUpdateTagEvent<'_>) -> Option<Compound> {
        Some(melter::update_tag(&event.entity.nbt))
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        render::submit(host, event);
    }

    /// The melter's and heater's flames and smoke run on the client, for every loaded copy (`render::particles`).
    fn block_entity_client_ticks(&self, _host: &dyn ClientHost, event: &mut BlockEntityClientTicksEvent<'_>) {
        if render::client_ticks(event.block) {
            event.ticks = true;
        }
    }

    fn block_entity_client_tick(&self, event: &mut BlockEntityClientTickEvent<'_>) {
        let Some(pos) = event.subject.with_view(|v| v.position()) else { return };
        render::client_tick(&*event.subject.host(), pos);
    }

    /// Hoppers and droppers through the container's faces, by the rules of Java's ITEM_HANDLER on these blocks (the
    /// `ITEMS` providers keep the same ones): MelterBlockEntity's MeltingModuleInventory takes one item into an empty
    /// input only (insertItem, getSlotLimit 1), never the heater's lent slot; HeaterBlockEntity's HeaterItemHandler
    /// fuels only (isItemValid: burn time over 3).
    fn can_place_item(&self, event: &mut CanPlaceItemEvent<'_>) -> Option<bool> {
        let items = &event.entity.items;
        match event.entity.id() {
            MELTER_TYPE => Some(event.slot < melter::INPUTS && items.get(event.slot).is_none_or(Option::is_none)),
            HEATER_TYPE => Some(event.slot == 0 && melter::is_known_fuel(event.stack)),
            _ => None,
        }
    }
}

#[mod_api::sunrise_part]
impl ItemPart for Smeltery {
    fn tooltip(&self, host: &dyn ClientHost, event: &mut TooltipEvent<'_>) {
        containers::tooltip(host, event.stack, event.lines);
    }
}

#[mod_api::sunrise_part]
impl MenuPart for Smeltery {
    /// The melter's and heater's buttons (the fuel slot's carried-stack clicks), on the server; none without the block.
    fn menu_button(&self, event: &mut MenuButtonEvent<'_>) {
        let (player, Some(pos), id) = (event.context.player, event.context.pos, event.context.id) else { return };
        let carried = &mut *event.carried;
        crate::scoped_fields(&mut event.subject, false, |host, _, e| (melter::menu_button(host, player, pos, id, e, carried), true));
    }

    /// Every id reaches `menu_button`, as every `clickMenuButton` reached the Java menus: it checks its own.
    fn menu_button_valid(&self, _host: &dyn Host, event: &mut MenuButtonValidEvent<'_>) -> Flow {
        if event.block.is_some() { Flow::Cancel } else { Flow::Continue }
    }

    fn menu_slot_rules(&self, host: &dyn Host, event: &mut MenuSlotRulesEvent<'_>) {
        // The fuel list the heater's slot reads, from the first click on (before any melter or heater ticked, an
        // unbuilt list took anything).
        ensure_ids(host);
        melter::ensure_fuels(host);
        melter::slot_rules(event);
        // The slots the screen hides (the melter's heater slot with no heater below): none on the server's menu.
        if matches!(event.container, MenuContainer::Main) {
            let at = event.context.pos.and_then(|p| here(host).map(|l| (block_id(host, l, p), p)));
            let layout = at.and_then(|(block, p)| screen::layout_for(&block, block_id(host, here(host)?, below(p)) == HEATER));
            crate::refuse_hidden_slot(event, layout.as_ref());
        }
    }
}

/// The melter's and heater's screens, made by the client for each opening of the block's menu (none without it).
fn menu_screen(host: &dyn ClientHost, open: MenuScreenOpen<'_>) -> Option<MenuScreen> {
    let block = open.block?.to_owned();
    let layout = screen::layout(host, &block, open.pos)?;
    let (draw_block, tip_block) = (block.clone(), block.clone());
    Some(
        MenuScreen::new(layout)
            .with_draw(move |host, frame, draw| screen::draw(host, &draw_block, frame.data, draw, frame.origin))
            .with_click(move |host, frame, right| screen::click(host, &block, frame.at, right))
            .with_tooltip(move |host, frame, _slot, lines| {
                crate::tooltip_lines(lines, |out| screen::tooltip(host, &tip_block, frame.data, frame.at, out))
            }),
    )
}

/// The block entity's fields (`ModBlockEntityFields` isn't exported by name).
pub(crate) type Fields = <ModBlockEntity as std::ops::Deref>::Target;
