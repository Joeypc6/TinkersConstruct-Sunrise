---
navigation:
  title: "generate hidden_fluids_tag"
  parent: index.md
  position: 8
  icon: tconstruct:seared_fuel_tank
level: 2
---
# generate hidden_fluids_tag

**Verwendung:** `/tconstruct generate hidden_fluids_tag`

Schreibt ein Datenpaket in den `datapacks`-Ordner der Welt, das in Rezeptanzeigen die geschmolzenen Metalle versteckt, die kein installierter Mod nutzt (Zinn, Blei, Silber...). Mit `/reload` aktivieren.

<GameScene zoom="4" interactive={true}>
  <Block id="tconstruct:smeltery_controller[in_structure=false,facing=south]" x="0" y="0" z="0" />
  <Block id="tconstruct:seared_bricks" x="1" y="0" z="0" />
  <Block id="tconstruct:seared_bricks" x="-1" y="0" z="0" />
  <Block id="tconstruct:seared_bricks" x="0" y="1" z="0" />
  <Block id="tconstruct:seared_fuel_tank" x="1" y="1" z="0" />
  <Block id="tconstruct:seared_bricks" x="-1" y="1" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
