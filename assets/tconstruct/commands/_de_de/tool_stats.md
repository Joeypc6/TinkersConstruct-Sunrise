---
navigation:
  title: "tool_stats"
  parent: index.md
  position: 3
  icon: tconstruct:tinker_station
level: 2
---
# tool_stats

**Verwendung:** `/tconstruct tool_stats <targets> bonus|multiplier|reset ...`

Überschreibt die Werte des gehaltenen Werkzeugs mit dem versteckten Modifikator Stat Override:

- `bonus add <stat> <wert>`: addiert zu einem Wert;
- `bonus set <stat> <wert>`: setzt den Bonus (eine Stufe wie `minecraft:iron` für die Abbaustufe);
- `multiplier multiply|set <stat> <wert>`: multipliziert einen Zahlenwert;
- `reset [stat]`: entfernt Boni und Multiplikatoren, eines Werts oder aller.

<GameScene zoom="5" interactive={true}>
  <Block id="tconstruct:tinker_station" x="0" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>

<Row>
  <ItemImage id="tconstruct:sword" scale="2" />
  <ItemImage id="tconstruct:broad_axe" scale="2" />
</Row>
