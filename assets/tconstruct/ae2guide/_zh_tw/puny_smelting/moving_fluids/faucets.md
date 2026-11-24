# 澆注口

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_faucets.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

勝任所有廚房和廁所……呃，轉移流體的需求。

澆注口是將流體從熔化爐、冶煉爐、儲罐或任何流體容器中提取出來的主要方式。只需要將其放在對應方塊上，右鍵點擊就能將其中流體倒入下方容器。

澆注口能從其所連接的方塊側面或底面將流體倒出至其正下方的方塊。

澆注口受紅石訊號影響，在其側放置控制桿並啟用就能使其不間斷地倒出流體。一次紅石脈衝能使其倒出流體至目標容器，直至目標容器被充滿或是其所連接的容器被倒光。
