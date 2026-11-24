---
navigation:
  title: "report modifier_usage"
  parent: index.md
  position: 6
  icon: tconstruct:modifier_worktable
level: 1
---
# report modifier_usage

**Uso:** `/tconstruct report modifier_usage [all|recipe [slot_type]|material_trait|tool_trait|unused]`

Escribe en el registro del juego una tabla de modificadores: si una receta añade cada uno (y con qué tipo de ranura) y si un material o una herramienta lo da como rasgo. `unused` lista los que nada usa.

<GameScene zoom="5" interactive={true}>
  <Block id="tconstruct:modifier_worktable" x="0" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
