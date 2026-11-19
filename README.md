# ◆ aeroPanel

> A Rust TUI for managing Minecraft servers — in the style of OpenCode: near-black canvas, rounded borders, orange/blue accents, animated spinners and sparklines.

**aeroPanel** is a terminal UI for administering servers with first-class **Minecraft** support: TPS/MSPT monitoring, player tracking, live colored logs, an interactive console, managed Paper instance creation, file browsing over SSH — all from one dashboard.

## Features

- **Dashboard**: server list with status (online/down), CPU, RAM, uptime, CPU sparkline.
- **Local + SSH**: each server runs in `local` or `ssh` mode (uses your OpenSSH and ssh-agent — keys, defaults, all free).
- **Minecraft** (per server):
  - Online players detected from the log (joins/leaves).
  - TPS 1m/5m/15m (Paper) and MSPT/spark, with auto-poll of the `tps` command.
  - **Stability panel** (stable / partial / unstable based on TPS).
  - Seen errors and last error from the log.
  - **Interactive console**: send commands to the server (with ↑↓ history).
  - Live colored logs (errors red, warns, chat, TPS).
- **Add/remove servers from the TUI** (persisted in the TOML config).
- **Create Minecraft instances**: `n` downloads the Paper jar (version of your choice), creates the directory in
  `~/.config/aeropanel/instances/<name>/` with `eula.txt`, `server.properties`, and adds it as a managed server.
- **Instance management**: start / stop (graceful `stop`) / restart with `S` / `X` / `R`, optional auto-start.
- **File explorer**: browse the instance directory, upload (`p`) and download (`e`) files.

## Install

```bash
cargo install --path .
```

## Configuration

Copy `config.example.toml` to `~/.config/aeropanel/config.toml` and edit your servers.
Each Minecraft server needs a `console_command` whose stdin is the server console, e.g.:

```toml
[[server]]
name = "mc-main"
mode = "ssh"
host = "192.168.1.10"
user = "you"

  [server.minecraft]
  console_command = "docker exec -i mc-main rcon-cli"
  # or, without rcon, attach screen/tmux or use mcrcon
  log_command = "tail -F -n 100 /opt/mc/logs/latest.log"  # optional, for read-only consoles
  send_tps_cmd = true
```

## Keybindings

| Context | Keys |
|---|---|
| Dashboard | `j/k` move · `enter` open · `n` new instance · `a` add · `d` delete · `?` help · `q` quit |
| Detail | `tab/h` tabs · `esc` back |
| Manage | `S` start · `X` stop · `R` restart |
| Files | `j/k` move · `enter` open dir · `u` up dir · `p` upload file · `e` download · `r` refresh |
| Logs | `j/k` scroll · `g` jump to tail |
| Console | `i` write · `enter` send · `↑↓` history · `esc` leave input |
| Always | `?` help overlay |

## Managed instances

Instances created with `n` show as `running` while the java process lives. aeroPanel talks to them via stdin
(automatic `tps` commands per `tps_interval_secs`) and on stop sends `stop` before killing the process. The jar
goes to `~/.config/aeropanel/instances/`, all declared in the config.

## Project layout

```
src/
├── main.rs         # event loop, metric pollers, log streams, TPS poller
├── app.rs          # state, forms, config persistence
├── config.rs       # TOML config (serde)
├── connection.rs   # local/SSH execution, stream spawn, console, upload/download, ls
├── instances.rs    # Paper instance creation (jar, eula, properties, launch)
├── metrics.rs      # /proc probe (loadavg, uptime, meminfo, CPU by double sampling)
├── minecraft.rs    # log parser (TPS/MSPT, joins/leaves/chat/deaths/errors)
├── theme.rs        # OpenCode-style palette
└── ui.rs           # ratatui render (dashboard, tabs, gauges, sparklines, console)
```

## Support

If aeroPanel saves you a terminal window or two, consider [supporting the project](https://ko-fi.com/justlebadura) ☕ — it keeps development going.

## License

MIT © 2026

