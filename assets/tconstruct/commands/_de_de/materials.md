---
navigation:
  title: "materials"
  parent: index.md
  position: 5
  icon: tconstruct:part_builder
level: 2
---
# materials

**Verwendung:** `/tconstruct materials set|stats|traits ...`

Arbeitet mit Werkzeugmaterialien:

- `set <ziele> <index> <material>`: tauscht das Material eines Teils des gehaltenen Werkzeugs (Index 0 ist das erste Teil), etwa `tconstruct:wood#oak`;
- `stats <typ> [material] [skala]`: zeigt die Werte eines Materials für einen Teiltyp oder die Standardwerte;
- `traits <material> [typ]`: listet die Eigenschaften, die ein Material gibt.

<GameScene zoom="5" interactive={true}>
  <Block id="tconstruct:part_builder" x="0" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>

<Row>
  <ItemImage id="tconstruct:pick_head" scale="2" />
  <ItemImage id="tconstruct:tool_handle" scale="2" />
  <ItemImage id="tconstruct:tool_binding" scale="2" />
</Row>
