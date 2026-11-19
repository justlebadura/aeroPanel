# ◆ aeroPanel

TUI en Rust para administrar servidores con estilo OpenCode: canvas casi negro,
bordes redondeados, acentos naranja/azul, spinners animados y sparklines.
Soporte de primera clase para servidores de **Minecraft** (TPS, jugadores, estabilidad, consola).

## Características

- **Dashboard**: lista de servidores con estado (online/down), CPU, RAM, uptime, sparkline de CPU.
- **Local + SSH**: cada servidor corre en modo `local` o `ssh` (usa tu OpenSSH y ssh-agent: llaves, claves por defecto, todo gratis).
- **Minecraft** (por servidor):
  - Jugadores online detectados del log (joins/leaves).
  - TPS 1m/5m/15m (Paper) y MSPT/spark, con auto-poll del comando `tps`.
  - Panel de **estabilidad** (estable / parcial / inestable según TPS).
  - Errores vistos y último error del log.
  - **Consola interactiva**: escribe comandos al servidor (con historial ↑↓).
  - Logs en vivo coloreados (errores rojos, warns, chat, TPS).
- **Añadir/borrar servidores desde la TUI** (se persiste en el TOML).
- **Crear instancias de Minecraft**: `n` descarga el jar de Paper (versión a elegir), crea el directorio en
  `~/.config/aeropanel/instances/<nombre>/` con `eula.txt`, `server.properties` y lo añade como servidor gestionado.
- **Gestión de instancias**: iniciar / detener (graceful `stop`) / reiniciar con `S` / `X` / `R`, auto-start opcional.
- **Explorador de archivos**: navega el directorio de la instancia, sube (`p`) y descarga (`e`) archivos.

## Instalación

```bash
cargo install --path .
```

## Configuración

Copia `config.example.toml` a `~/.config/aeropanel/config.toml` y edita tus servidores.
Cada servidor minecraft necesita un `console_command` cuyo stdin sea la consola del servidor, ej:

```toml
[[server]]
name = "mc-main"
mode = "ssh"
host = "192.168.1.10"
user = "lebadura"

  [server.minecraft]
  console_command = "docker exec -i mc-main rcon-cli"
  # o, sin rcon, adjunta screen/tmux o usa mcrcon
  log_command = "tail -F -n 100 /opt/mc/logs/latest.log"  # opcional, logs cuando la consola es de solo-lectura
  send_tps_cmd = true
```

## Atajos

| Contexto | Teclas |
|---|---|
| Dashboard | `j/k` mover · `enter` abrir · `n` nueva instancia · `a` añadir · `d` borrar · `?` ayuda · `q` salir |
| Detalle | `tab/h` pestañas · `esc` atrás |
| Gestión | `S` iniciar · `X` detener · `R` reiniciar |
| Archivos | `j/k` mover · `enter` abrir carpeta · `u` subir dir · `p` subir archivo · `e` descargar · `r` refrescar |
| Logs | `j/k` scroll · `g` ir al tail |
| Consola | `i` escribir · `enter` enviar · `↑↓` historial · `esc` salir del input |
| Siempre | `?` overlay de ayuda |

## Instancias gestionadas

Las instancias creadas con `n` aparecen con estado `running` cuando el proceso java vive. aeroPanel
les habla por stdin (comandos `tps` automáticos según `tps_interval_secs`) y al detener envía `stop`
antes de matar el proceso. El jar va a `~/.config/aeropanel/instances/`, todo declarado en el config.

## Estructura

```
src/
├── main.rs         # event loop, pollers de métricas, streams de log, poller TPS
├── app.rs          # estado, formularios, persistencia de config
├── config.rs       # config TOML (serde)
├── connection.rs   # ejecución local/SSH, spawn de streams, consola, subir/descargar, ls
├── instances.rs    # creación de instancias Paper (jar, eula, properties, launch)
├── metrics.rs      # probe /proc (loadavg, uptime, meminfo, CPU por doble muestreo)
├── minecraft.rs    # parser de logs (TPS/MSPT, joins/leaves/chat/muertes/errores)
├── theme.rs        # paleta estilo OpenCode
└── ui.rs           # render ratatui (dashboard, tabs, gauges, sparklines, consola)
```
