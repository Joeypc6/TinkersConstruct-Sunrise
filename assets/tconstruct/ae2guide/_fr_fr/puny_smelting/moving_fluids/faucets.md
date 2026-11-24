# Bidons en Cuivre

<GameScene zoom="3" interactive={true}>
  <ImportStructure src="/scenes/setup_puny_smelting_moving_fluids_faucets.snbt" />
  <IsometricCamera yaw="45" pitch="30" />
</GameScene>

Les bidons en cuivre (et les seaux !) servent au transfert manuel de fluides.

Chaque bidon contient l'équivalent d'un lingot de fluide.

Empilable jusqu'à 16 unités.

Les robinets sont le principal moyen d'extraire des fluides des fourneaux, fonderies, réservoirs ou tout autre conteneur valide. Placez-les simplement sur un bloc et faites un clic droit pour verser dans un récipient situé en dessous.

Les robinets puisent le fluide sur les côtés ou le bas du bloc connecté, et le versent dans le bloc situé directement sous le robinet.

Les robinets réagissent aux signaux redstone : un levier activé à côté versera en continu. Une impulsion redstone ne versera que jusqu'à ce que le récepteur soit plein ou la source vide.
