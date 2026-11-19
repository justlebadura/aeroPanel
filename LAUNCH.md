# Post de lanzamiento — r/admincraft (texto listo para pegar)

## Título

aeroPanel — I built a Rust TUI to manage my Minecraft servers (TPS, players, console, file browser) — v0.1.0

## Cuerpo

I got tired of juggling tmux panes, `tail -F` and rcon-cli across a handful of servers, so I built aeroPanel — a terminal dashboard for managing Minecraft servers, in the style of those modern coding TUIs.

**What it does:**

- One dashboard for all your servers (local or SSH — uses your ssh-agent, nothing extra to install on the host)
- Minecraft-aware: TPS 1m/5m/15m + MSPT with sparklines, players online, server stability at a glance (stable/partial/unstable)
- Live colored log view (errors red, warns yellow, chat and joins highlighted)
- Interactive console: write commands straight to the server (via rcon-cli/screen/whatever your `console_command` is), with ↑↓ history
- Create managed Paper instances from inside the TUI — it downloads the jar, writes eula/properties, done
- Start / graceful-stop / restart with one key; file explorer with upload/download over SSH

**Screenshots/links:**

- Repo: https://github.com/justlebadura/aeroPanel
- Release with prebuilt Linux binary: https://github.com/justlebadura/aeroPanel/releases/tag/v0.1.0
- MIT licensed, Rust + ratatui.

**Feedback I'd love:** What's missing that would make you actually replace your current workflow? Was thinking of adding multi-node metrics rollup and backup commands next.

---

## Notas para mí (no publicar):

1. REGLA CRÍTICA r/admincraft: se permiten promos de herramientas solo los fines de semana (Rule: "server advertisement"... verificar reglas del sub ANTES de postear: los lanzamientos de herramientas van en el hilo semanal o sábado/domingo según regla 3). Verificar en https://reddit.com/r/admincraft/wiki/rules
2. Adjuntar 2–3 capturas (TUI corriendo contra un servidor real) — post sin imágenes muere.
3. Comentar activamente en el primer día (responder dudas rápido sube visibilidad).
4. Repost en: r/rust (Show Saturday), r/MinecraftCommands?, servidores Discord de Minecraft hosting (encontrar 3–4 grandes), HN Show (solo si tracción).

## Seguimiento (si hay tracción)

- Añadir GIF del dashboard en el README (graba la TUI con asciinema + agg).
- Modrinth "Server utility" listing con link a GitHub (alcance gratis a la audiencia exacta).
- Ko-fi link en README + release notes ya insertado (placeholder: ko-fi.com/justlebadura) — CONFIRMAR que la cuenta existe con ese nombre exacto antes de postear.
