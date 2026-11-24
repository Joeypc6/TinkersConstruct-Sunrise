---
navigation:
  title: "report modifier_usage"
  parent: index.md
  position: 6
  icon: tconstruct:modifier_worktable
level: 1
---
# report modifier_usage

**Usage:** `/tconstruct report modifier_usage [all|recipe [slot_type]|material_trait|tool_trait|unused]`

Writes a table of modifiers to the game log: whether a recipe adds each one (and with which slot type), and whether a material or a tool gives it as a trait. `unused` lists the modifiers nothing uses.

<GameScene zoom="5" interactive={true}>
  <Block id="tconstruct:modifier_worktable" x="0" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
