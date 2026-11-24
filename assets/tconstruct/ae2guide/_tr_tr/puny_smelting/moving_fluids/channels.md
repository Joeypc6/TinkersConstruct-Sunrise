# Kanallar

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_channels.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

İçsel benliğini başka yere taşımak için kendini kanaldan geçirmelisin.

Kanallar sıvı ile üstten veya giriş olarak ayarlanmış yanlardan doldurulabilir. Alttan veya çıkış olarak ayarlanmış yanlardan çıktı verir.

Kanalların içerisinde sıvıların akış yönünü gösteren oklar bulunur. Kanalların kenarlarına sağ tıklamak onları giriş, çıkış ve yok arasında geçiş yapar. Altında sıvı tutucu bulunması durumunda kanalın orta kısmına sağ tıklamak çıkışı alta ayarlar. Kanallar yukarı doğru aktaramaz.

Kanallar da redstone'a yanıt verir. Güç vermek aşağı yönlü akmasını etkinleştirir ve etkisizleştirir.
