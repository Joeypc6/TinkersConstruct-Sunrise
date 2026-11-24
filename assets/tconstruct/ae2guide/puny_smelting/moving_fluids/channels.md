# Channels

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_channels.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

You must channel your inner self to move to another place.

Channels can be filled with fluid from the top or from sides set to input. They output into the bottom, or sides that are set to output.

Channels have arrows inside them that show the direction that fluid will travel. Right clicking the sides of channels will switch them between input, output, and none. Right clicking the middle of the channel will set it to output to directly below if there is a valid fluid container underneath. Channels cannot transfer upwards.

Channels respond to redstone as well. Powering it will enable and disable downwards flowing.
