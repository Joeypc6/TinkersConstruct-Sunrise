# Tinker Tank

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_fantastic_foundry_alloyer_proxy_tank.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

<ItemImage id="tconstruct:scorched_proxy_tank" scale="2" />

The tinker tank is great for filling or emptying variable size fluid items, such as tools with tanks. Simply place the tool in the tank, then fill it using fluid items, faucets, or channels.

This design unfortunately does not work well with containers like buckets or copper cans, as their rigid size prevents faucets from slowly filling them. You are better off using a casting tank for them.
