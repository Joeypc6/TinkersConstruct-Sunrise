//! JEI's "+" on a modifier recipe into the tinker station and the tinkers' anvils (Java's TinkerStationTransferInfo
//! over JEI's basic transfer), server side through `MenuPart::menu_transfer`: the recipe viewer sends the card's id
//! (`card_id`), and the server checks and moves everything itself.
//!
//! - The recipe's inputs, in the card's order, go into the station's input slots (TinkerStationContainerMenu's
//!   `getInputSlots`: every slot but the tool slot, so the tool stays where it is). A recipe with more inputs than the
//!   station has slots doesn't fit (`NotIntoMenu`), as `validateRecipeFits` refuses it.
//! - What the input slots hold goes back to the player's inventory first (JEI clears the slots) and counts as the
//!   player's own; an input takes the first of its items the player has enough of. One set, or with Shift as many
//!   as the items and a stack allow.
//! - `Missing` when an input can't be filled, `NoRoom` when what the slots held doesn't fit the inventory; nothing
//!   moves then, nor in a dry run. The tool building cards have no "+" here (this port shows no tool building
//!   category), the part builder has none in Java, and the crafting station's grid is the game's own, which the
//!   game's transfer fills (JEI's CraftingStationTransferInfo).

use super::modrecipe::jei_modifiers;
use super::station::{TINKERS_ANVIL, TINKER_STATION, is_station, tinker_inputs};
use crate::smeltery::util::*;
use mod_api::*;

/// The recipe viewer's category of the modifier recipes.
pub(crate) const MODIFIERS: &str = "tconstruct:modifiers";
/// The player's main inventory: hotbar and the 27 above it.
const INVENTORY: std::ops::Range<i32> = 0..36;

/// The modifier card's id for recipe `id`: what the viewer sends back with "+".
pub(crate) fn card_id(id: &str) -> String {
    format!("{MODIFIERS}/{}", id.replace(':', "/"))
}

/// One input's pick: the input's slot (0-based among the inputs), the item and how many go there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pick {
    pub slot: usize,
    pub item: String,
    pub count: u32,
}

/// JEI's basic transfer plan, worked out without the game: `inputs` are the card's input slots (each its items and
/// the count each needs, empty for a slot the recipe leaves empty), `have` what the player can give (their inventory
/// and what the station's input slots hold), `slots` the station's input slots, `stack` an item's stack size.
/// `max` is Shift: as many sets as the items and a stack allow, else one.
pub(crate) fn plan(inputs: &[Vec<(String, u32)>], have: &[(String, u32)], slots: usize, max: bool, stack: impl Fn(&str) -> u32) -> Result<Vec<Pick>, RecipeTransferResult> {
    if inputs.iter().skip(slots).any(|i| !i.is_empty()) {
        return Err(RecipeTransferResult::NotIntoMenu);
    }
    let mut left: Vec<(String, u32)> = have.to_vec();
    let available = |left: &[(String, u32)], item: &str| left.iter().filter(|(i, _)| i == item).map(|(_, n)| *n).sum::<u32>();
    let take = |left: &mut Vec<(String, u32)>, item: &str, mut n: u32| {
        for (i, c) in left.iter_mut() {
            if i == item && n > 0 {
                let t = (*c).min(n);
                *c -= t;
                n -= t;
            }
        }
    };
    // One set first: each input the first of its items there is enough of, after the inputs before it.
    let mut picks = Vec::new();
    for (slot, items) in inputs.iter().enumerate() {
        if items.is_empty() {
            continue;
        }
        let Some((item, n)) = items.iter().find(|(item, n)| *n > 0 && available(&left, item) >= *n) else {
            return Err(RecipeTransferResult::Missing);
        };
        take(&mut left, item, *n);
        picks.push(Pick { slot, item: item.clone(), count: *n });
    }
    if max && !picks.is_empty() {
        // More sets while every pick still has its count left and fits its stack.
        let mut sets = 1u32;
        loop {
            let fits = picks.iter().all(|p| {
                let per = p.count / sets;
                p.count + per <= stack(&p.item) && available(&left, &p.item) >= per
            });
            if !fits {
                break;
            }
            for p in picks.iter_mut() {
                let per = p.count / sets;
                take(&mut left, &p.item, per);
                p.count += per;
            }
            sets += 1;
        }
    }
    Ok(picks)
}

/// `menu_transfer` for the tinker station and the anvils; None for other menus.
pub(crate) fn menu_transfer(host: &mut dyn ServerHost, e: &MenuTransferEvent<'_>) -> Option<RecipeTransferResult> {
    use RecipeTransferResult::*;
    if !matches!(e.menu, TINKER_STATION | TINKERS_ANVIL) {
        return None;
    }
    let Some(recipe) = jei_modifiers(&*host).into_iter().find(|m| card_id(&m.id) == e.recipe) else { return Some(NotIntoMenu) };
    let Some(p) = host.player(e.player) else { return Some(NotIntoMenu) };
    let (Some(pos), Some(level), Some(menu)) = (e.pos, p.level(&*host), p.container_menu(&*host)) else { return Some(NotIntoMenu) };
    let block = block_id(&*host, level, pos);
    if !is_station(&block) {
        return Some(NotIntoMenu);
    }
    let slots = tinker_inputs(&block);
    // What the player can give: their inventory's plain stacks and the input slots' (station slots 1..=slots).
    let plain = |s: &ItemStack| new_stack(sid(s), 1).is_some_and(|one| s.same_components(&one));
    let mut have: Vec<(String, u32)> = Vec::new();
    for slot in INVENTORY {
        if let Some(s) = (ItemPlace::PlayerSlot { player: e.player, slot }).stack(host).filter(plain) {
            have.push((sid(&s).to_owned(), s.count()));
        }
    }
    let held: Vec<(usize, ItemStack)> = (1..=slots).filter_map(|i| (ItemPlace::MenuSlot { menu, slot: i }).stack(host).map(|s| (i, s))).collect();
    have.extend(held.iter().filter(|(_, s)| plain(s)).map(|(_, s)| (sid(s).to_owned(), s.count())));
    let picks = match plan(&recipe.inputs, &have, slots, e.max, |item| item_key(item).and_then(|k| host.max_stack_size(k)).unwrap_or(64).clamp(1, 99)) {
        Ok(p) => p,
        Err(r) => return Some(r),
    };
    if e.dry_run {
        return Some(Moves);
    }
    let player = e.player;
    let moved = host.transaction(|host, tx| {
        // The input slots back to the inventory (a slot's own stack, whole).
        for (i, s) in &held {
            let res = ItemResource::of(s);
            let out = (ItemPlace::MenuSlot { menu, slot: *i }).extract(host, tx, &res, s.count()).unwrap_or(0);
            if out != s.count() || into_inventory(host, tx, player, &res, out) != out {
                return Err(NoRoom);
            }
        }
        // Each pick from the inventory into its slot.
        for pick in &picks {
            let Some(one) = new_stack(&pick.item, 1) else { return Err(Missing) };
            let res = ItemResource::of(&one);
            let mut need = pick.count;
            for slot in INVENTORY {
                if need == 0 {
                    break;
                }
                need -= (ItemPlace::PlayerSlot { player, slot }).extract(host, tx, &res, need).unwrap_or(0);
            }
            if need > 0 {
                return Err(Missing);
            }
            if (ItemPlace::MenuSlot { menu, slot: pick.slot + 1 }).insert(host, tx, &res, pick.count) != Ok(pick.count) {
                return Err(NoRoom);
            }
        }
        Ok(())
    });
    tlog(&format!("modifier transfer {} at {pos:?} (max {}): {moved:?}", e.recipe, e.max));
    Some(match moved {
        Ok(()) => Moves,
        Err(TransactionError::Aborted(r)) => r,
        // The transaction couldn't run or commit: nothing moved.
        Err(_) => NotIntoMenu,
    })
}

/// `amount` of `res` into the player's inventory, stacking first as vanilla's `Inventory.add` does: how many went in.
fn into_inventory(host: &mut dyn ServerHost, tx: &TransactionContext, player: u128, res: &ItemResource, amount: u32) -> u32 {
    let mut left = amount;
    // Onto the stacks of it first, then the empty slots.
    for pass in 0..2 {
        for slot in INVENTORY {
            if left == 0 {
                return amount;
            }
            let place = ItemPlace::PlayerSlot { player, slot };
            let empty = place.stack(host).is_none();
            if (pass == 0) == empty {
                continue;
            }
            left -= place.insert(host, tx, res, left).unwrap_or(0);
        }
    }
    amount - left
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(list: &[(&str, u32)]) -> Vec<(String, u32)> {
        list.iter().map(|(i, n)| ((*i).to_owned(), *n)).collect()
    }

    #[test]
    fn modifier_transfer_plan() {
        // Haste: 1 redstone block... as one of its inputs, a second input of 2 redstone or 2 glowstone.
        let inputs = vec![items(&[("minecraft:redstone_block", 1)]), items(&[("minecraft:redstone", 2), ("minecraft:glowstone_dust", 2)]), vec![]];
        let have = items(&[("minecraft:redstone_block", 5), ("minecraft:glowstone_dust", 64), ("minecraft:redstone", 1)]);
        // Redstone falls short, so glowstone.
        let one = plan(&inputs, &have, 3, false, |_| 64).unwrap();
        assert_eq!(one, vec![Pick { slot: 0, item: "minecraft:redstone_block".into(), count: 1 }, Pick { slot: 1, item: "minecraft:glowstone_dust".into(), count: 2 }]);
        // Shift: five blocks make five sets.
        let max = plan(&inputs, &have, 3, true, |_| 64).unwrap();
        assert_eq!(max.iter().map(|p| p.count).collect::<Vec<_>>(), vec![5, 10]);
        // A stack of 4 caps it.
        let capped = plan(&inputs, &have, 3, true, |_| 4).unwrap();
        assert_eq!(capped.iter().map(|p| p.count).collect::<Vec<_>>(), vec![2, 4]);
        // Nothing for an input: Missing.
        assert_eq!(plan(&inputs, &items(&[("minecraft:redstone_block", 1)]), 3, false, |_| 64), Err(RecipeTransferResult::Missing));
        // More inputs than the station has slots: it doesn't fit.
        let five = vec![vec![], vec![], vec![], items(&[("minecraft:diamond", 1)])];
        assert_eq!(plan(&five, &items(&[("minecraft:diamond", 1)]), 3, false, |_| 64), Err(RecipeTransferResult::NotIntoMenu));
        assert!(plan(&five, &items(&[("minecraft:diamond", 1)]), 5, false, |_| 64).is_ok());
        assert_eq!(card_id("tconstruct:tools/modifiers/upgrade/haste"), "tconstruct:modifiers/tconstruct/tools/modifiers/upgrade/haste");
    }
}
