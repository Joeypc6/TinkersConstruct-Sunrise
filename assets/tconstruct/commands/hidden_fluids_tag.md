---
navigation:
  title: "generate hidden_fluids_tag"
  parent: index.md
  position: 8
  icon: tconstruct:seared_fuel_tank
level: 2
---
# generate hidden_fluids_tag

**Usage:** `/tconstruct generate hidden_fluids_tag`

Writes a data pack into the world's `datapacks` folder that hides, in recipe viewers, the molten metals no installed mod uses (tin, lead, silver...). Run `/reload` to turn it on.

<GameScene zoom="4" interactive={true}>
  <Block id="tconstruct:smeltery_controller[in_structure=false,facing=south]" x="0" y="0" z="0" />
  <Block id="tconstruct:seared_bricks" x="1" y="0" z="0" />
  <Block id="tconstruct:seared_bricks" x="-1" y="0" z="0" />
  <Block id="tconstruct:seared_bricks" x="0" y="1" z="0" />
  <Block id="tconstruct:seared_fuel_tank" x="1" y="1" z="0" />
  <Block id="tconstruct:seared_bricks" x="-1" y="1" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
