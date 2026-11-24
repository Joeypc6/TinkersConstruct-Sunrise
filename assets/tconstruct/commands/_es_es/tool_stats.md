---
navigation:
  title: "tool_stats"
  parent: index.md
  position: 3
  icon: tconstruct:tinker_station
level: 2
---
# tool_stats

**Uso:** `/tconstruct tool_stats <targets> bonus|multiplier|reset ...`

Modifica las estadísticas de la herramienta en la mano con el modificador oculto Stat Override:

- `bonus add <stat> <valor>`: suma a una estadística;
- `bonus set <stat> <valor>`: fija la bonificación (un nivel como `minecraft:iron` para el nivel de minería);
- `multiplier multiply|set <stat> <valor>`: multiplica una estadística numérica;
- `reset [stat]`: quita bonificaciones y multiplicadores, de una estadística o de todas.

<GameScene zoom="5" interactive={true}>
  <Block id="tconstruct:tinker_station" x="0" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>

<Row>
  <ItemImage id="tconstruct:sword" scale="2" />
  <ItemImage id="tconstruct:broad_axe" scale="2" />
</Row>
