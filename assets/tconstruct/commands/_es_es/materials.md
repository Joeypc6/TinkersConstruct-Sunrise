---
navigation:
  title: "materials"
  parent: index.md
  position: 5
  icon: tconstruct:part_builder
level: 2
---
# materials

**Uso:** `/tconstruct materials set|stats|traits ...`

Trabaja con materiales de herramientas:

- `set <objetivos> <índice> <material>`: cambia el material de una pieza de la herramienta en la mano (el índice 0 es la primera), como `tconstruct:wood#oak`;
- `stats <tipo> [material] [escala]`: muestra las estadísticas de un material para un tipo de pieza, o las de por defecto;
- `traits <material> [tipo]`: lista los rasgos que da un material.

<GameScene zoom="5" interactive={true}>
  <Block id="tconstruct:part_builder" x="0" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>

<Row>
  <ItemImage id="tconstruct:pick_head" scale="2" />
  <ItemImage id="tconstruct:tool_handle" scale="2" />
  <ItemImage id="tconstruct:tool_binding" scale="2" />
</Row>
