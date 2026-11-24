# Torneiras

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_faucets.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

Para todas as suas necessidades de movimentação de fluidos... er, na cozinhas e banheiros

As torneiras são a principal maneira de retirar fluido de fundidores, fundições, tanques ou qualquer recipiente de fluido válido. Basta colocá-la no bloco e clicar com o botão direito para despejar o fluido em um recipiente abaixo.

As torneiras despejarão o fluido das laterais ou da parte inferior do bloco conectado ao bloco diretamente abaixo da torneira.

As torneiras respondem a sinais de redstone, então basta colocar uma alavanca ativada ao lado delas para fazer com que o fluido seja despejado constantemente. Um pulso de redstone fará o fluido despejar até que o destino esteja cheio ou a fonte fique vazia.
