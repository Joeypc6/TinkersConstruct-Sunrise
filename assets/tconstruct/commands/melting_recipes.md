---
navigation:
  title: "generate melting_recipes"
  parent: index.md
  position: 9
  icon: tconstruct:seared_melter
level: 2
---
# generate melting_recipes

**Usage:** `/tconstruct generate melting_recipes <recipe_type>`

Writes melting recipes into the world's `datapacks` folder for every result of a recipe type (such as `minecraft:crafting`) that doesn't melt yet. Each recipe gives what its ingredients melt into, split over the result's count; the hottest fluid is the output and the others are byproducts. Tools, books, dyes and items that leave a container are skipped (pack file `tconstruct:command/generate_melting_recipes.json`). Run `/reload` to turn it on.

<Row>
  <ItemImage id="minecraft:iron_door" scale="2" />
  <ItemImage id="tconstruct:molten_iron_bucket" scale="2" />
</Row>
