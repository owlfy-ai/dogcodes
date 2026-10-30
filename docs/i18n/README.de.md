<!-- Keep feature scope aligned across README.md and docs/i18n/. Screenshots are shared PNG files. -->
<div align="center">

<img src="../../apps/desktop/src-tauri/icons/128x128@2x.png" alt="dog.codes" width="96" height="96" />

# dog.codes

**Alle Coding-Agents. Ein Arbeitsbereich. Von überall steuerbar.**

<a href="../../README.md">English</a> · <a href="../../docs/i18n/README.zh-CN.md">简体中文</a> · <a href="../../docs/i18n/README.zh-TW.md">繁體中文</a> · <a href="../../docs/i18n/README.ja.md">日本語</a> · <a href="../../docs/i18n/README.ko.md">한국어</a> · <a href="../../docs/i18n/README.es.md">Español</a> · <a href="../../docs/i18n/README.fr.md">Français</a> · <strong>Deutsch</strong> · <a href="../../docs/i18n/README.pt-BR.md">Português (Brasil)</a>

</div>

dog.codes vereint Coding-Agenten, Gespräche, Codeänderungen und Projektbefehle in einer Desktop-App. Lass einen Agenten eine Funktion umsetzen und einen anderen den Code prüfen. Anschließend kannst du das Projekt starten und einen Commit vorbereiten – alles im selben Arbeitsbereich. Wenn du den Schreibtisch verlässt, steuerst du alles vom Smartphone aus.

[Website](https://dog.codes) · [Funktionen](#features) · [Smartphone-Fernsteuerung](#phone) · [Erste Schritte](#getting-started)

## Unterstützte Agenten

Verbinde die Agenten, die du bereits nutzt. Die folgenden 12 Integrationen sind implementiert. Modelle, Freigaben, Verbrauchsdaten und Wiederaufnahme hängen vom Agenten und seiner Version ab. Installiere den jeweiligen Agenten zuerst über seine offiziellen Werkzeuge und melde dich dort an.

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

## Alles an einem Ort

<!-- Screenshot folgt: ../../docs/images/readme/overview.png -->

<a id="features"></a>

## Funktionen

### Arbeit nahtlos zwischen Agenten übergeben

**Wechsle jederzeit den Agenten, der dieselbe Unterhaltung ausführt; der Kontext wird automatisch übergeben.**

- Gib jede Aufgabe dem passenden Agenten: Einer entwirft die Architektur, ein anderer kümmert sich um die Details.
- Ist das Kontingent eines Agenten aufgebraucht, übernimmt ein anderer – ohne dass du die Anforderungen wiederholen musst.
- Die bisherige Unterhaltung wird mitgegeben, und der neue Agent kann frühere Runden über die integrierten Verlaufswerkzeuge nachschlagen – er macht wirklich weiter, statt neu anzufangen.

<!-- Screenshot folgt: ../../docs/images/readme/switch-agent.png -->

### Kontingente und Verbrauch auf einen Blick

**Kontingente, Verbrauch, Rücksetzzeiten und Versionen aller 12 unterstützten Coding-Agenten an einem Ort.**

- Sieh das Restkontingent und die Rücksetzzeit jedes Agenten, plane deine Arbeit danach und nutze jedes Token.
- Verwalte die Versionen der Agenten zentral und aktualisiere sie mit einem Klick.
- Installierte Agenten und ihr Anmeldestatus werden automatisch erkannt.

<!-- Screenshot folgt: ../../docs/images/readme/usage.png -->

### Nutze die Abonnements, die du schon hast

**dog.codes steuert die offiziellen CLIs, die du installiert und bei denen du angemeldet bist – Claude Code, Codex, Cursor und weitere – mit deinen eigenen Plänen.**

- Kein eigenes dog.codes-Konto und kein zusätzlicher API-Kauf: Die Pläne, für die du schon zahlst, funktionieren weiter.
- Lokal zuerst: Dein Code, deine Gespräche und deine Zugangsdaten bleiben auf deinem Computer. Die optionale Smartphone-Verbindung ist Ende-zu-Ende-verschlüsselt, das Relay sieht deine Daten also nie.
- Agenten erhalten nur die Umgebungsvariablen, die sie brauchen – deine übrigen Geheimnisse bleiben verborgen.

<!-- Screenshot folgt: ../../docs/images/readme/accounts.png -->

### Code-Review und Korrektur über Agenten hinweg

**Lass einen anderen Agenten den gerade geänderten Code prüfen, um Fehler und Designmängel zu finden, oder eine Sicherheits- bzw. Performance-Analyse durchführen.**

- Verschiedene Agenten und Modelle prüfen einander mit frischem Blick und vermeiden so die blinden Flecken eines einzelnen Agenten in einer Unterhaltung.
- Befunde werden nach hoher, mittlerer und niedriger Priorität eingestuft; wähle die gewünschten aus und starte die Korrektur mit einem Klick.
- Die Korrektur geht samt Review-Bericht und ausgewählten Befunden an den Haupt-Agenten zurück – der Kontext bleibt erhalten.

<!-- Screenshot folgt: ../../docs/images/readme/review-fix.png -->

### Über Agenten hinweg verzweigen und Ansätze vergleichen

**Verzweige aus jeder beliebigen Runde eine neue Unterhaltung und lass einen anderen Agenten eine andere Idee ausprobieren – das Original bleibt unberührt.**

- Mit Worktrees bleiben die Codeänderungen der einzelnen Ansätze voneinander getrennt.
- Lass mehrere Agenten dieselbe Aufgabe lösen und übernimm die beste Version.

<!-- Screenshot folgt: ../../docs/images/readme/fork.png -->

### Git-Verwaltung, die sich vertraut anfühlt

**Git-Bedienung eng an VS Code angelehnt – verwalte den Diff in der Seitenleiste so, wie du es gewohnt bist.**

- Stagen, Unstagen, Verwerfen, Ignorieren, Commit und Commit & Push, mit Baum- und Listenansicht.
- Lass einen beliebigen Agenten die Commit-Nachricht mit einem Klick schreiben – in der Sprache deiner Wahl.

<!-- Screenshot folgt: ../../docs/images/readme/git-changes.png -->

### Herausfinden, wie ein Projekt läuft – und es mit einem Klick starten

**dog.codes analysiert, wie ein Projekt ausgeführt und debuggt wird, und macht aus diesen Befehlen und Skripten anklickbare Starteinträge.**

- Kein Merken der Startbefehle jedes Projekts mehr und kein Terminal nur zum Starten – ein Klick genügt.
- Die Ausgabe wird an einem Ort gesammelt und ist jederzeit einsehbar.

<!-- Screenshot folgt: ../../docs/images/readme/run-project.png -->

### Projekte mit mehreren Ordnern

**Frontend, Backend und Doku in getrennten Ordnern? Übergib sie dem Agenten gemeinsam in einer Unterhaltung.**

- Auch Agenten ohne native Unterstützung für mehrere Ordner können über dog.codes an solchen Projekten arbeiten.

<!-- Screenshot folgt: ../../docs/images/readme/multi-folder.png -->

### Verbrauch pro Runde, glasklar

**Sieh für jede Runde Token-Verbrauch, Kontextbelegung, Cache-Treffer und Cache-Nutzung.**

- Verstehe genau, was die aktuelle Unterhaltung und jede Runde verbraucht haben und wofür deine Tokens draufgingen.

<!-- Screenshot folgt: ../../docs/images/readme/turn-usage.png -->

**Open Source · Kostenlos · Plattformübergreifend**: unter Apache-2.0 lizenziert, für macOS, Windows und Linux gebaut.

<a id="phone"></a>

## Steuere deine Agents vom Smartphone aus

**Lass den Laptop zu Hause. Deine Agents arbeiten auf deinem Computer weiter, während dein Smartphone das Kommando behält.**

Koppel ein Smartphone einmal und erledige alles Folgende von überall. Dein Code und deine Agents bleiben auf deinem Computer.

<!-- Screenshot folgt: ../../docs/images/readme/phone-remote.png -->

- **Gespräche fortsetzen oder neu starten.** Nimm jedes Gespräch von deinem Computer wieder auf oder starte ein neues: Agent, Modell und Denkaufwand wählen, die Ausgabe live mitverfolgen und Sitzungen suchen, anheften, archivieren oder verzweigen.
- **Freigeben und antworten.** Berechtigungsanfragen und Rückfragen der Agents erreichen dein Smartphone mit Push-Benachrichtigungen; ein Tipp, und der Agent macht weiter. Jede Freigabe wird mit dem ursprünglichen Lauf auf deinem Computer abgeglichen.
- **Prüfen und committen.** Sieh die Änderungen in jedem Projektordner, stage Dateien, lass einen Agent die Commit-Nachricht schreiben und committe, pushe, pulle oder synchronisiere dann. Durchsuche den Commit-Verlauf und vergleiche Branches schreibgeschützt.
- **Projekte aus den Ordnern deines PCs hinzufügen.** Durchsuche die Ordner deines Computers und füge einen als Projekt hinzu, ohne dich wieder an den Schreibtisch zu setzen.
- **Befehle im Terminal deines PCs ausführen.** Öffne Terminals in jedem Projektordner und arbeite in der echten Shell deines Computers.
- **Projekt starten, auf dem Smartphone ansehen.** Starte die Startpunkte auf deinem Computer und öffne die laufende Web-App direkt auf dem Smartphone.
- **Dateien ansehen und Anhänge senden.** Lies Projektdateien und hänge Dateien und Bilder an deine Nachrichten an.
- **Remote-Desktop bei Bedarf.** Sieh dir den Bildschirm deines Computers an und tippe, ziehe, scrolle, zoome per Pinch, gib Text ein oder füge Text und Bilder ein – mit einer Qualität, die sich dem Netzwerk anpasst.

<!-- Screenshot folgt: ../../docs/images/readme/remote-desktop.png -->

### So verbindet es sich

- Einmal einen QR-Code scannen, ein Konto ist nicht nötig. Die Verbindung ist Ende-zu-Ende-verschlüsselt. Koppel mehrere Computer, sieh, welche online sind, und widerrufe ein Smartphone jederzeit in der Desktop-App.
- Das Smartphone verbindet sich direkt, wenn es kann, und weicht sonst auf den dog.codes-Relay-Dienst aus, der nur verschlüsselten Datenverkehr weiterleitet.
- Dein Computer bleibt eingeschaltet, mit dog.codes geöffnet oder minimiert. Solange Arbeit oder eine Remote-Sitzung aktiv ist, kann dog.codes den automatischen Ruhezustand des Computers verhindern; das lässt sich abschalten.
- Remote-Desktop funktioniert mit macOS-, Windows- und Linux-Computern (X11 und Wayland); dein Betriebssystem fragt die benötigten Bildschirm- und Eingabeberechtigungen ab.

<a id="getting-started"></a>

## Erste Schritte

Starte die App derzeit aus dem Quellcode. Installiere **Rust 1.97.1**, **Node.js 22+**, **pnpm 11** und die [Tauri-Voraussetzungen für deine Plattform](https://v2.tauri.app/start/prerequisites/). Klone dieses Repository oder lade es herunter und führe im Hauptverzeichnis aus:

```bash
pnpm install
pnpm desktop:dev
```

1. Installiere mindestens einen unterstützten Agenten nach offizieller Anleitung und melde dich an.
2. Öffne dog.codes, verbinde einen verfügbaren Agenten und füge Projektordner hinzu.
3. Starte ein Gespräch, wähle Agent und Modell und beschreibe die Aufgabe.
4. Öffne den Ausführungsbereich, um Projektbefehle zu finden, oder prüfe Änderungen und starte ein Review.
5. Um deine Agents vom Smartphone aus zu steuern, öffne in der Desktop-App **Smartphone verbinden** und scanne den Kopplungscode mit der Smartphone-App.

Gespräche, Projektanalysen, Reviews und die Erstellung von Commit-Nachrichten verwenden das Konto des gewählten Agenten und können dessen Kontingent verbrauchen.

<details>
<summary>An der Entwicklung mitarbeiten</summary>

```bash
cargo test --workspace
pnpm check
```

`pnpm dev` startet eine Browser-Demo mit simulierten Daten. Für echte Agenten und Projektbefehle verwende `pnpm desktop:dev`.

</details>

## Weitere Informationen

Lies vor einem Pull Request [CONTRIBUTING.md](../../CONTRIBUTING.md); Sicherheitslücken meldest du vertraulich wie in [SECURITY.md](../../SECURITY.md) beschrieben.

Die Agenten-Icons stammen aus den bereits vorhandenen Ressourcen dieses Repositorys; siehe [Herkunft und Hinweise](../../apps/desktop/src/assets/licenses/README.md). Produktnamen und Marken gehören ihren jeweiligen Inhabern.
