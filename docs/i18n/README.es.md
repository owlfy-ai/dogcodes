<!-- Keep feature scope aligned across README.md and docs/i18n/. Screenshots are shared PNG files. -->
<div align="center">

<img src="../../apps/desktop/src-tauri/icons/128x128@2x.png" alt="dog.codes" width="96" height="96" />

# dog.codes

**Todos los agentes de código. Un solo espacio. Dirígelos desde donde sea.**

<a href="../../README.md">English</a> · <a href="../../docs/i18n/README.zh-CN.md">简体中文</a> · <a href="../../docs/i18n/README.zh-TW.md">繁體中文</a> · <a href="../../docs/i18n/README.ja.md">日本語</a> · <a href="../../docs/i18n/README.ko.md">한국어</a> · <strong>Español</strong> · <a href="../../docs/i18n/README.fr.md">Français</a> · <a href="../../docs/i18n/README.de.md">Deutsch</a> · <a href="../../docs/i18n/README.pt-BR.md">Português (Brasil)</a>

</div>

dog.codes reúne agentes de programación, conversaciones, cambios de código y comandos del proyecto en una aplicación de escritorio. Implementa una función con un agente, pide una revisión a otro y después ejecuta el proyecto y prepara el commit, sin salir del mismo espacio de trabajo. Cuando te alejas del escritorio, puedes dirigirlo todo desde el móvil.

[Sitio web](https://dog.codes) · [Funciones](#features) · [Control desde el móvil](#phone) · [Primeros pasos](#getting-started)

## Agentes compatibles

Conecta los agentes que ya utilizas. Las 12 integraciones siguientes están implementadas; los modelos, las aprobaciones, las métricas y la recuperación dependen del agente y de su versión. Instala cada agente e inicia sesión con sus herramientas oficiales.

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

## Todo tu trabajo en un solo lugar

<!-- Captura pendiente: ../../docs/images/readme/overview.png -->

<a id="features"></a>

## Funciones

### Pasa el trabajo entre agentes sin interrupciones

**Cambia en cualquier momento el agente que ejecuta la misma conversación; el contexto se traspasa automáticamente.**

- Asigna cada tarea al agente más adecuado: uno diseña la arquitectura y otro se ocupa de los detalles.
- Si un agente agota su cuota, otro continúa el trabajo sin que tengas que repetir los requisitos.
- La conversación anterior viaja con el traspaso y el nuevo agente puede consultar turnos previos con las herramientas de historial integradas: de verdad continúa, no empieza de cero.

<!-- Captura pendiente: ../../docs/images/readme/switch-agent.png -->

### Cuotas y uso de un vistazo

**Controla cuotas, uso, horas de reinicio y versiones de los 12 agentes de programación compatibles en un solo lugar.**

- Ve la cuota restante y la hora de reinicio de cada agente, planifica tu trabajo y aprovecha cada token.
- Gestiona las versiones de los agentes en un solo lugar y actualízalos con un clic.
- Los agentes instalados y su estado de inicio de sesión se detectan automáticamente.

<!-- Captura pendiente: ../../docs/images/readme/usage.png -->

### Usa las suscripciones que ya tienes

**dog.codes ejecuta las CLI oficiales que ya instalaste y en las que iniciaste sesión —Claude Code, Codex, Cursor y más— con tus propios planes.**

- Sin cuenta de dog.codes aparte ni compras de API adicionales: los planes que ya pagas siguen funcionando.
- Local primero: tu código, tus conversaciones y tus credenciales se quedan en tu ordenador. La conexión opcional con el móvil va cifrada de extremo a extremo, así que el servicio de retransmisión nunca ve tus datos.
- Los agentes se inician solo con las variables de entorno que necesitan, así tus otros secretos no quedan expuestos.

<!-- Captura pendiente: ../../docs/images/readme/accounts.png -->

### Revisión y corrección de código entre agentes

**Pide a otro agente que revise el código recién modificado para detectar errores y fallos de diseño, o que haga un análisis de seguridad o rendimiento.**

- Distintos agentes y modelos revisan el trabajo con una mirada nueva, evitando los puntos ciegos de un solo agente dentro de una misma conversación.
- Los hallazgos se clasifican en prioridad alta, media y baja; elige los que quieras y empieza la corrección con un clic.
- La corrección vuelve al agente principal junto con el informe de revisión y los hallazgos elegidos, sin perder el contexto.

<!-- Captura pendiente: ../../docs/images/readme/review-fix.png -->

### Bifurca entre agentes y compara enfoques

**Bifurca una nueva conversación desde cualquier turno y deja que otro agente pruebe otra idea, sin tocar la original.**

- Con worktrees, los cambios de código de cada enfoque quedan aislados entre sí.
- Haz que varios agentes resuelvan la misma tarea y quédate con la mejor versión.

<!-- Captura pendiente: ../../docs/images/readme/fork.png -->

### Gestión de Git que resulta familiar

**Operaciones de Git muy parecidas a VS Code, para gestionar el Diff en la barra lateral como ya lo haces.**

- Preparar, quitar de preparación, descartar, ignorar, Commit y Commit & Push, con vista de árbol y de lista.
- Haz que cualquier agente redacte el mensaje de commit con un clic, en el idioma que elijas.

<!-- Captura pendiente: ../../docs/images/readme/git-changes.png -->

### Descubre cómo ejecutar un proyecto y hazlo con un clic

**dog.codes analiza cómo se ejecuta y depura un proyecto y convierte esos comandos y scripts en accesos de inicio con un clic.**

- Olvídate de memorizar el comando de inicio de cada proyecto o de abrir una terminal solo para lanzarlo: basta un clic.
- La salida se reúne en un solo lugar para consultarla cuando quieras.

<!-- Captura pendiente: ../../docs/images/readme/run-project.png -->

### Proyectos con varias carpetas

**¿Frontend, backend y documentación en carpetas distintas? Entrégaselas todas al agente en una sola conversación.**

- Incluso los agentes sin soporte nativo para varias carpetas pueden trabajar en proyectos multicarpeta a través de dog.codes.

<!-- Captura pendiente: ../../docs/images/readme/multi-folder.png -->

### Uso por turno, con total claridad

**Consulta el consumo de tokens, la ocupación de contexto, los aciertos de caché y el uso de caché de cada turno.**

- Entiende con detalle lo que consumieron la conversación actual y cada turno, y en qué se fueron tus tokens.

<!-- Captura pendiente: ../../docs/images/readme/turn-usage.png -->

**Código abierto · Gratis · Multiplataforma**: con licencia Apache-2.0, creado para macOS, Windows y Linux.

<a id="phone"></a>

## Dirige tus agentes desde el móvil

**Deja el portátil en casa. Tus agentes siguen trabajando en tu ordenador mientras tu móvil sigue al mando.**

Empareja un móvil una sola vez y haz todo lo que sigue desde cualquier lugar. Tu código y tus agentes se quedan en tu ordenador.

<!-- Captura pendiente: ../../docs/images/readme/phone-remote.png -->

- **Continúa o inicia conversaciones.** Retoma cualquier conversación de tu ordenador o empieza una nueva: elige agente, modelo y nivel de razonamiento, mira la salida en directo y busca, fija, archiva o bifurca sesiones.
- **Aprueba y responde.** Las solicitudes de permiso y las preguntas de los agentes llegan al móvil con notificaciones push; un toque y el agente continúa. Cada aprobación se comprueba con la ejecución original en tu ordenador.
- **Revisa y confirma.** Consulta los cambios de cada carpeta del proyecto, prepara archivos, deja que un agente escriba el mensaje del commit y luego confirma, sube, descarga o sincroniza. Explora el historial de commits y compara ramas en modo de solo lectura.
- **Añade proyectos desde tu PC.** Explora las carpetas de tu ordenador y añade una como proyecto sin volver a sentarte en el escritorio.
- **Ejecuta comandos en la terminal de tu PC.** Abre terminales en cualquier carpeta del proyecto y trabaja en el shell real de tu ordenador.
- **Ejecuta el proyecto y míralo en el móvil.** Inicia las entradas de lanzamiento en tu ordenador y abre la app web en ejecución directamente en el móvil.
- **Explora archivos y envía adjuntos.** Lee los archivos del proyecto y adjunta archivos e imágenes a tus mensajes.
- **Escritorio remoto cuando lo necesites.** Mira la pantalla de tu ordenador y toca, arrastra, desplázate, pellizca, escribe o pega texto e imágenes, con una calidad que se adapta a la red.

<!-- Captura pendiente: ../../docs/images/readme/remote-desktop.png -->

### Cómo se conecta

- Escaneas un código QR una vez y no hace falta cuenta. La conexión va cifrada de extremo a extremo. Empareja varios ordenadores, comprueba cuáles están en línea y revoca un móvil cuando quieras desde la app de escritorio.
- El móvil se conecta directamente cuando puede y, si no, recurre al servicio de retransmisión de dog.codes, que solo reenvía tráfico cifrado.
- Tu ordenador debe seguir encendido con dog.codes abierto o minimizado. Mientras haya trabajo o una sesión remota activa, dog.codes puede evitar que el ordenador entre en suspensión automática; puedes desactivarlo.
- El escritorio remoto funciona con ordenadores macOS, Windows y Linux (X11 y Wayland); tu sistema operativo te pedirá los permisos de pantalla y entrada que necesita.

<a id="getting-started"></a>

## Primeros pasos

Por ahora, ejecuta la aplicación desde el código fuente. Instala **Rust 1.97.1**, **Node.js 22+**, **pnpm 11** y los [requisitos de Tauri para tu plataforma](https://v2.tauri.app/start/prerequisites/). Clona o descarga este repositorio y ejecuta estos comandos en su raíz:

```bash
pnpm install
pnpm desktop:dev
```

1. Instala y autentica al menos un agente compatible siguiendo sus instrucciones oficiales.
2. Abre dog.codes, conecta un agente disponible y añade las carpetas del proyecto.
3. Inicia una conversación, elige un agente y un modelo, y describe el trabajo.
4. Abre el panel de ejecución para descubrir comandos o revisa los cambios y solicita una revisión.
5. Para dirigir tus agentes desde el móvil, abre **Conectar móvil** en la app de escritorio y escanea el código de emparejamiento con la app móvil.

Las conversaciones, los análisis del proyecto, las revisiones y la generación de mensajes de commit utilizan la cuenta del agente seleccionado y pueden consumir su cuota.

<details>
<summary>Para contribuir al desarrollo</summary>

```bash
cargo test --workspace
pnpm check
```

`pnpm dev` abre una demostración de la interfaz en el navegador con datos simulados. Usa `pnpm desktop:dev` para agentes y comandos reales.

</details>

## Más información

Lee [CONTRIBUTING.md](../../CONTRIBUTING.md) antes de abrir un pull request e informa de los problemas de seguridad en privado según [SECURITY.md](../../SECURITY.md).

### Integración de agentes

dog.codes se conecta a cada agente mediante su interfaz de proceso local. ACP significa Agent Client Protocol; los agentes con otra interfaz nativa utilizan un controlador específico.

| Agente | Integración | Instalación compatible |
| --- | --- | --- |
| Codex | `app-server` nativo (JSON-RPC) | Cliente de escritorio + CLI |
| Claude Code | `stream-json` nativo | Cliente de escritorio + CLI |
| Cursor | ACP (`cursor-agent acp`) | CLI |
| Kimi Code | ACP (`kimi acp`) | CLI |
| Grok Build | ACP (`grok agent stdio`) | CLI |
| ZCode | `app-server` nativo (`zcode.cjs` incluido) | Cliente de escritorio (CLI incluida) |
| Antigravity | `stream-json` nativo sin interfaz gráfica (`agy`) | CLI |
| CodeBuddy | ACP (`codebuddy --acp`) | CLI |
| Pi | RPC nativo (`pi --mode rpc`) | CLI |
| OpenCode | ACP (`opencode acp`) | Cliente de escritorio + CLI |
| DeepSeek Harness | ACP (`dsh --profile acp`) | Cliente de escritorio + CLI |
| Kiro | ACP (`kiro-cli acp`) | CLI |

«Cliente de escritorio» significa reutilizar la CLI o el entorno sin interfaz gráfica que ese cliente incluye o instala, no automatizar su GUI. Debe estar instalado el entorno correspondiente; las rutas de detección y la disponibilidad varían según el sistema operativo y la versión. La detección automática del entorno incluido en OpenCode Desktop está implementada actualmente en Windows.

### Agradecimientos

Computer use utiliza [Cua Driver](https://github.com/trycua/cua), con licencia MIT, para consultar ventanas, tomar capturas e interactuar con el teclado y el ratón. Gracias al equipo de Cua y a sus colaboradores por su trabajo de automatización de escritorio de código abierto. La integración del controlador incluido admite actualmente Windows x64.

Los iconos reutilizan los recursos incluidos en este repositorio; consulta su [procedencia y avisos](../../apps/desktop/src/assets/licenses/README.md). Los nombres y las marcas pertenecen a sus respectivos titulares.
