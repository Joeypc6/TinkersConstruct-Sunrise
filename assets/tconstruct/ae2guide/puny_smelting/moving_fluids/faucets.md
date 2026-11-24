# Faucets

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_faucets.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

For all your kitchen and bathroom... er fluid moving needs.

Faucets are your main way to get fluids out from melters, smelteries, tanks, or any valid fluid container. Simply place it on the block, and right click to pour into a fluid container below.

Faucets will pour out from the sides or bottom of the connected block to the block directly below the faucet itself.

Faucets respond to redstone signals, so simply placing an activated lever next to it will make it constantly pour out its contents. A redstone pulse will pour out its contents until the destination is full or the source is empty.
