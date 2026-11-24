---
navigation:
  title: "materials"
  parent: index.md
  position: 5
  icon: tconstruct:part_builder
level: 2
---
# materials

**Usage:** `/tconstruct materials set|stats|traits ...`

Works with tool materials:

- `set <targets> <index> <material>`: swaps the material of one part of the held tool (index 0 is the first part), such as `tconstruct:wood#oak`;
- `stats <stat_type> [material] [scale]`: shows a material's stats for a part type, or the defaults;
- `traits <material> [stat_type]`: lists the traits a material gives.

<GameScene zoom="5" interactive={true}>
  <Block id="tconstruct:part_builder" x="0" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>

<Row>
  <ItemImage id="tconstruct:pick_head" scale="2" />
  <ItemImage id="tconstruct:tool_handle" scale="2" />
  <ItemImage id="tconstruct:tool_binding" scale="2" />
</Row>
