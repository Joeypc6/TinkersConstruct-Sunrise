# 浇注道

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_channels.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

调用内在的感觉，这样就能移动到别的地方。

浇注道可以从顶部或设置为输入端的侧面输入流体，可以向设置为输出端的底面或侧面输出 流体。

浇注道内壁的箭头可以表示流体的传输方向。右击浇注道的侧面能够使它在输入端、输出端 与无连接间轮换。右击浇注道中部会使其直接 朝底面输出，前提是下方有有效的流体容器。 浇注道不能向上传输流体。

浇注道也能响应红石信号。充能与否决定它是否拥有朝底部输出的能力。
