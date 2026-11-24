# Canaux

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_channels.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

Vous devez canaliser votre force intérieure pour vous déplacer vers un autre endroit.

Les canaux peuvent être remplis de fluide par le haut ou par les côtés configurés en entrée. Ils se vident par le bas ou les côtés configurés en sortie.

Des flèches à l'intérieur des canaux indiquent la direction d'écoulement. Un clic droit sur les côtés permet de basculer entre entrée, sortie et neutre. Un clic droit au centre configure une sortie vers le bas si un réceptacle valide se trouve en dessous. Les canaux ne peuvent pas transférer vers le haut.

Les canaux réagissent aussi à la redstone. L'alimenter active/désactive l'écoulement vers le bas.
