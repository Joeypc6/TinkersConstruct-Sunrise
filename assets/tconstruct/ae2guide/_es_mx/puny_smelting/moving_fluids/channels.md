# Canales

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_channels.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

Debes canalizar tu yo interior para moverte hacia otro lugar.

Los canales se llenan desde arriba o por los lados configurados como entrada. El fluido sale por la base o por los lados configurados como salida.

Las flechas internas muestran la dirección del flujo. Haz clic derecho en los lados para alternar entre entrada, salida o nada. Clic derecho en el centro para verter hacia abajo si hay un contenedor válido. No pueden transportar fluidos hacia arriba.

También responden a redstone. Al recibir energía, se activa o desactiva el flujo hacia abajo del canal.
