---
navigation:
  title: "generate melting_recipes"
  parent: index.md
  position: 9
  icon: tconstruct:seared_melter
level: 2
---
# generate melting_recipes

**Verwendung:** `/tconstruct generate melting_recipes <recipe_type>`

Schreibt in den `datapacks`-Ordner der Welt Schmelzrezepte für jedes Ergebnis eines Rezepttyps (etwa `minecraft:crafting`), das noch nicht schmilzt. Jedes Rezept ergibt, was seine Zutaten schmelzen, aufgeteilt auf die Menge des Ergebnisses; die heißeste Flüssigkeit ist das Ergebnis, die anderen sind Nebenprodukte. Werkzeuge, Bücher, Farbstoffe und Gegenstände, die einen Behälter zurücklassen, werden übersprungen (Datei `tconstruct:command/generate_melting_recipes.json`). Mit `/reload` aktivieren.

<Row>
  <ItemImage id="minecraft:iron_door" scale="2" />
  <ItemImage id="tconstruct:molten_iron_bucket" scale="2" />
</Row>
