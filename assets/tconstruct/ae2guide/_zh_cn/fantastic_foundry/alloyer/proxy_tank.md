# 工匠储罐

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_fantastic_foundry_alloyer_proxy_tank.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

<ItemImage id="tconstruct:scorched_proxy_tank" scale="2" />

如果你有接受流体的物品，比如带“储液”的工具，那么工匠储罐可以装填或倒空任意数量的 流体。只需把工具放在储罐当中，用浇注口、 浇注道或另一接受流体的物品向它填入流体。

很遗憾，工匠储罐的设计与桶、铜罐等容量确定的容器无法相互兼容，它们不支持流体通过 浇注口缓慢倒入流体。在这种情形下，你应该 改用浇注储罐。
