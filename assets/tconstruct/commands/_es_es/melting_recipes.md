---
navigation:
  title: "generate melting_recipes"
  parent: index.md
  position: 9
  icon: tconstruct:seared_melter
level: 2
---
# generate melting_recipes

**Uso:** `/tconstruct generate melting_recipes <recipe_type>`

Escribe en la carpeta `datapacks` del mundo recetas de fundición para cada resultado de un tipo de receta (como `minecraft:crafting`) que aún no se funde. Cada receta da lo que funden sus ingredientes, repartido entre la cantidad del resultado; el fluido más caliente es el resultado y los demás son subproductos. Se omiten herramientas, libros, tintes y objetos que dejan un recipiente (archivo `tconstruct:command/generate_melting_recipes.json`). Usa `/reload` para activarlo.

<Row>
  <ItemImage id="minecraft:iron_door" scale="2" />
  <ItemImage id="tconstruct:molten_iron_bucket" scale="2" />
</Row>
