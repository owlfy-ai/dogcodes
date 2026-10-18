<!-- Keep feature scope aligned across README.md and docs/i18n/. Screenshots are shared PNG files. -->
<div align="center">

<img src="../../apps/desktop/src-tauri/icons/128x128@2x.png" alt="dog.codes" width="96" height="96" />

# dog.codes

**Vos agents de programmation. Un seul espace de travail.**

<a href="../../README.md">English</a> · <a href="../../docs/i18n/README.zh-CN.md">简体中文</a> · <a href="../../docs/i18n/README.zh-TW.md">繁體中文</a> · <a href="../../docs/i18n/README.ja.md">日本語</a> · <a href="../../docs/i18n/README.ko.md">한국어</a> · <a href="../../docs/i18n/README.es.md">Español</a> · <strong>Français</strong> · <a href="../../docs/i18n/README.de.md">Deutsch</a> · <a href="../../docs/i18n/README.pt-BR.md">Português (Brasil)</a>

</div>

dog.codes réunit vos agents de programmation, conversations, modifications de code et commandes de projet dans une application de bureau. Faites implémenter une fonctionnalité par un agent, demandez une revue à un autre, puis lancez le projet et préparez un commit depuis le même espace.

**Version de développement · macOS en priorité.** Les installateurs et la validation de la publication sont en préparation. Vous pouvez déjà lancer l’application depuis les sources ; la validation sous Windows reste à faire.

[Site web](https://dog.codes) · [Fonctionnalités](#features) · [Premiers pas](#getting-started)

## Agents pris en charge

Connectez les agents que vous utilisez déjà. Les 12 intégrations ci-dessous sont implémentées ; les modèles, autorisations, données de consommation et fonctions de reprise varient selon l’agent et sa version. Installez chaque agent et connectez-vous avec ses outils officiels.

<p align="center">
  <a href="https://developers.openai.com/codex/"><img src="../../docs/images/readme/agents/codex.png" alt="Codex" height="36" /></a>
  <a href="https://code.claude.com/docs/en/overview"><img src="../../docs/images/readme/agents/claude-code.png" alt="Claude Code" height="36" /></a>
  <a href="https://cursor.com/"><img src="../../docs/images/readme/agents/cursor.png" alt="Cursor" height="36" /></a>
  <a href="https://www.kimi.com/"><img src="../../docs/images/readme/agents/kimi.png" alt="Kimi Code" height="36" /></a>
  <a href="https://grok.com/"><img src="../../docs/images/readme/agents/grok.png" alt="Grok Build" height="36" /></a>
  <a href="https://z.ai/"><img src="../../docs/images/readme/agents/zcode.png" alt="ZCode" height="36" /></a>
  <br />
  <a href="https://antigravity.google/"><img src="../../docs/images/readme/agents/antigravity.png" alt="Antigravity" height="36" /></a>
  <a href="https://www.codebuddy.ai/"><img src="../../docs/images/readme/agents/codebuddy.png" alt="CodeBuddy" height="36" /></a>
  <a href="https://pi.dev/"><img src="../../docs/images/readme/agents/pi.png" alt="Pi" height="36" /></a>
  <a href="https://opencode.ai/"><img src="../../docs/images/readme/agents/opencode.png" alt="OpenCode" height="36" /></a>
  <a href="https://github.com/deepseek-ai/deepseek-harness"><img src="../../docs/images/readme/agents/deepseek-harness.png" alt="DeepSeek Harness" height="36" /></a>
  <a href="https://kiro.dev/"><img src="../../docs/images/readme/agents/kiro.png" alt="Kiro" height="36" /></a>
</p>

## Tout votre travail au même endroit

<!-- Capture à venir: ../../docs/images/readme/overview.png -->

<a id="features"></a>

## Fonctionnalités

### Passez le relais entre agents sans interruption

**Changez à tout moment l'agent qui exécute la même conversation ; le contexte est transmis automatiquement.**

- Confiez chaque tâche à l'agent le plus adapté : l'un conçoit l'architecture, un autre s'occupe des détails.
- Quand un agent a épuisé son quota, un autre reprend le travail sans que vous ayez à répéter les exigences.
- La conversation précédente accompagne le relais, et le nouvel agent peut consulter les échanges antérieurs grâce aux outils d'historique intégrés : il continue vraiment au lieu de repartir de zéro.

<!-- Capture à venir: ../../docs/images/readme/switch-agent.png -->

### Quotas et consommation en un coup d'œil

**Suivez quotas, consommation, heures de réinitialisation et versions des 12 agents de code pris en charge, au même endroit.**

- Voyez le quota restant et l'heure de réinitialisation de chaque agent, planifiez votre travail et faites compter chaque token.
- Gérez les versions des agents au même endroit et mettez-les à jour en un clic.
- Les agents installés et leur état de connexion sont détectés automatiquement.

<!-- Capture à venir: ../../docs/images/readme/usage.png -->

### Utilisez les abonnements que vous avez déjà

**dog.codes pilote les CLI officielles que vous avez installées et auxquelles vous êtes connecté — Claude Code, Codex, Cursor et d'autres — avec vos propres forfaits.**

- Pas de compte dog.codes séparé ni d'achat d'API supplémentaire : les forfaits que vous payez déjà continuent de servir.
- Local d'abord : vos données restent sur votre ordinateur et ne transitent par aucun serveur relais.
- Les agents sont lancés avec les seules variables d'environnement nécessaires, sans exposer vos autres secrets.

<!-- Capture à venir: ../../docs/images/readme/accounts.png -->

### Revue et correction de code entre agents

**Demandez à un autre agent de relire le code qui vient d'être modifié pour repérer bugs et défauts de conception, ou de mener une analyse de sécurité ou de performance.**

- Des agents et des modèles différents se relisent avec un regard neuf, évitant les angles morts d'un agent unique dans une même conversation.
- Les constats sont classés par priorité haute, moyenne et basse ; sélectionnez ceux qui vous intéressent et lancez la correction en un clic.
- La correction revient à l'agent principal avec le rapport de revue et les constats choisis, sans rupture de contexte.

<!-- Capture à venir: ../../docs/images/readme/review-fix.png -->

### Bifurquez d'un agent à l'autre et comparez les approches

**Créez une nouvelle conversation à partir de n'importe quel tour et laissez un autre agent essayer une autre idée, sans toucher à l'originale.**

- Avec les worktrees, les modifications de code de chaque approche restent isolées les unes des autres.
- Faites traiter la même tâche par plusieurs agents, puis gardez la meilleure version.

<!-- Capture à venir: ../../docs/images/readme/fork.png -->

### Une gestion Git qui vous est familière

**Des opérations Git très proches de VS Code, pour gérer le Diff dans la barre latérale comme d'habitude.**

- Indexer, désindexer, annuler, ignorer, Commit et Commit & Push, avec vues arborescente et en liste.
- Faites rédiger le message de commit par n'importe quel agent en un clic, dans la langue de votre choix.

<!-- Capture à venir: ../../docs/images/readme/git-changes.png -->

### Découvrez comment lancer un projet, puis lancez-le en un clic

**dog.codes analyse la façon dont un projet s'exécute et se débogue, et transforme ces commandes et scripts en entrées de lancement cliquables.**

- Plus besoin de retenir la commande de démarrage de chaque projet ni d'ouvrir un terminal juste pour le lancer : un clic suffit.
- La sortie est regroupée au même endroit, consultable à tout moment.

<!-- Capture à venir: ../../docs/images/readme/run-project.png -->

### Projets multi-dossiers

**Frontend, backend et documentation dans des dossiers séparés ? Confiez-les tous à l'agent dans une seule conversation.**

- Même les agents sans prise en charge native du multi-dossier peuvent travailler sur des projets multi-dossiers grâce à dog.codes.

<!-- Capture à venir: ../../docs/images/readme/multi-folder.png -->

### Fermez la fenêtre, le travail continue

**Les exécutions d'agents, les terminaux et les commandes de projet sont pris en charge par un service en arrière-plan : fermer la fenêtre ou un panneau ne les interrompt pas.**

- Rouvrez dog.codes : il se reconnecte aux mêmes terminaux et sorties.
- Après une perte de connexion ou une fermeture inattendue, vous voyez ce qu'est réellement devenue votre dernière opération : rien n'est validé ni poussé deux fois.

<!-- Capture à venir: ../../docs/images/readme/background.png -->

### Consommation par tour, en toute clarté

**Consultez la consommation de tokens, l'occupation du contexte, les succès de cache et l'utilisation du cache de chaque tour.**

- Comprenez précisément ce qu'ont consommé la conversation en cours et chaque tour, et où sont passés vos tokens.

<!-- Capture à venir: ../../docs/images/readme/turn-usage.png -->

**Open source · Gratuit · Multiplateforme** : sous licence Apache-2.0, pour Windows, macOS et Linux.

<a id="getting-started"></a>

## Premiers pas

Pour le moment, lancez l’application depuis les sources. Installez **Rust 1.97.1**, **Node.js 22+**, **pnpm 11** et les [prérequis Tauri de votre plateforme](https://v2.tauri.app/start/prerequisites/). Clonez ou téléchargez ce dépôt, puis exécutez à sa racine :

```bash
pnpm install
pnpm desktop:dev
```

1. Installez au moins un agent compatible et connectez-vous selon sa procédure officielle.
2. Ouvrez dog.codes, connectez un agent disponible et ajoutez vos dossiers de projet.
3. Créez une conversation, sélectionnez l’agent et le modèle, puis décrivez le travail.
4. Ouvrez le panneau d’exécution pour trouver les commandes, ou examinez les changements et lancez une revue.

Les conversations, analyses de projet, revues et générations de messages de commit utilisent le compte de l’agent sélectionné et peuvent consommer son quota.

<details>
<summary>Contribuer au développement</summary>

```bash
cargo test --workspace
pnpm check
```

`pnpm dev` ouvre une démo de l’interface dans le navigateur avec des données simulées. Utilisez `pnpm desktop:dev` pour les agents et commandes réels.

</details>

## En savoir plus

Lisez [CONTRIBUTING.md](../../CONTRIBUTING.md) avant d’ouvrir une pull request ; signalez les failles de sécurité en privé comme indiqué dans [SECURITY.md](../../SECURITY.md). Consultez l’[état du projet](../../docs/CURRENT_WORK.md) pour l’avancement et les [notes sur l’exécution](../../docs/implementation/project-run-capabilities-2026-10-14.md) pour le lancement des commandes en un clic.

Présentation inspirée de [codex-host](https://github.com/BytePioneer-AI/codex-host). Les icônes reprennent les ressources déjà présentes dans le dépôt ; voir leur [provenance et les mentions associées](../../apps/desktop/src/assets/licenses/README.md). Les noms et marques appartiennent à leurs titulaires respectifs.
