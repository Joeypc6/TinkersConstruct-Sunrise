//! Stage 4, the world: the slime islands (`island`), their trees (`tree`) and the foliage and glass tints (`tint`).
//! Cobalt ore and the four geodes are data (`worldgen/feature`, `placed_feature`, `forge/biome_modifier`), placed by
//! the game's own features through the fork's biome modifiers.

pub(crate) mod features;
pub(crate) mod island;
pub(crate) mod sapling;
pub(crate) mod tint;
pub(crate) mod tree;
