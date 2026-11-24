# 浇注口

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_faucets.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

胜任所有厨房和厕所……呃，转移流体的需求。

浇注口是将流体从熔化炉、冶炼炉、储罐和任何流体容器中提取出来的主要方式。只需要将 它放在对应方块上，就能右击让流体流进下面 的容器。

浇注口能从其所连接的方块侧面或底面将流体倒出至其正下方的方块。

浇注口可响应红石信号，因此只要在旁边放置拉杆并激活，就能让它不间断地倒出流体。单 个红石脉冲也能让它持续倒出流体，直到目标 容器装满，或连接的容器倒空。
