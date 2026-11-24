---
navigation:
  title: "tool_stats"
  parent: index.md
  position: 3
  icon: tconstruct:tinker_station
level: 2
---
# tool_stats

**Usage:** `/tconstruct tool_stats <targets> bonus|multiplier|reset ...`

Overrides the held tool's stats through the hidden Stat Override modifier:

- `bonus add <stat> <value>`: adds to a stat;
- `bonus set <stat> <value>`: sets the bonus (a tier name such as `minecraft:iron` for the mining tier);
- `multiplier multiply|set <stat> <value>`: multiplies a number stat;
- `reset [stat]`: removes the bonuses and multipliers, of one stat or all.

<GameScene zoom="5" interactive={true}>
  <Block id="tconstruct:tinker_station" x="0" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>

<Row>
  <ItemImage id="tconstruct:sword" scale="2" />
  <ItemImage id="tconstruct:broad_axe" scale="2" />
</Row>
