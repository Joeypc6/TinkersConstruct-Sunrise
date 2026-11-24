---
navigation:
  title: "report modifier_usage"
  parent: index.md
  position: 6
  icon: tconstruct:modifier_worktable
level: 1
---
# report modifier_usage

**Verwendung:** `/tconstruct report modifier_usage [all|recipe [slot_type]|material_trait|tool_trait|unused]`

Schreibt eine Modifikatortabelle ins Spiel-Log: ob ein Rezept jeden hinzufügt (und mit welchem Platztyp) und ob ein Material oder ein Werkzeug ihn als Eigenschaft gibt. `unused` listet die Modifikatoren, die nichts nutzt.

<GameScene zoom="5" interactive={true}>
  <Block id="tconstruct:modifier_worktable" x="0" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
