# Canais

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_channels.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

Você deve canalizar o seu eu interior para se mover para outro lugar.

Os canais podem ser preenchidos com fluidos pela parte superior ou pelas laterais que foram definidos como entrada. Eles saem pela parte inferior ou pelas laterais definidas para saída.

Os canais têm setas dentro deles que indicam a direção para a qual o fluido irá. Clicar com o botão direito nas laterais dos canais alternará entre entrada, saída e nenhum. Clicar com o botão direito no meio do canal configurará a saída para diretamente abaixo, caso haja um recipiente de fluido válido embaixo. Os canais não podem transferir fluido para cima.

Canais também respondem a redstone. Acione-o e ele ativará e desativará o fluxo de fluido para baixo.
