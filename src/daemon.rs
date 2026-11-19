// aeroPanel daemon: dueño de las instancias gestionadas, corre headless
// (aeropanel --daemon, idealmente bajo systemd). La TUI se conecta por
// unix socket y le manda órdenes; el daemon difunde eventos (líneas de
// consola, estado, backups). Además ejecuta el scheduler y las
// notificaciones, y sobrevive a que la TUI se cierre.
use crate::config::{self, Config};
use crate::instances;
use crate::minecraft;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};

pub fn socket_path() -> std::path::PathBuf {
    let mut p = config::config_dir();
    p.push("daemon.sock");
    p
}

pub fn backup_dir(server: &str) -> std::path::PathBuf {
    let mut p = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir);
    p.push("aeropanel");
    p.push("backups");
    p.push(server);
    p
}

pub fn schedule_file() -> std::path::PathBuf {
    let mut p = config::config_dir();
    p.push("schedule.json");
    p
}

// ---------- eventos hacia los clientes ----------

#[derive(Serialize, Clone)]
#[serde(tag = "ev")]
enum Ev {
    Line { server: String, text: String },
    Status { server: String, status: String },
    Msg { msg: String },
    Backup { server: String, path: String },
}

fn emit(ev: &Ev, tx: &broadcast::Sender<Ev>) {
    let _ = tx.send(ev.clone());
}

// ---------- peticiones ----------

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
enum Req {
    Start { server: String },
    Stop { server: String },
    Restart { server: String },
    Console { server: String, text: String },
    Backup { server: String },
    Ping,
}

struct Proc {
    child: Child,
    stdin: Option<tokio::process::ChildStdin>,
}

struct Daemon {
    ev: broadcast::Sender<Ev>,
    procs: Arc<Mutex<HashMap<String, Proc>>>,
}

impl Daemon {
    fn managed_cfg(cfg: &Config, name: &str) -> Option<config::ServerCfg> {
        cfg.server
            .iter().cloned()
            .find(|s| s.name == name && s.minecraft.as_ref().map_or(false, |m| !m.instance_dir.is_empty()))
    }

    async fn spawn_server(self: &Arc<Self>, name: &str) -> Result<()> {
        {
            let procs = self.procs.lock().await;
            if procs.contains_key(name) {
                return Err(anyhow!("'{name}' ya está corriendo"));
            }
        }
        let s = Self::managed_cfg(&config::load().unwrap_or_default(), name)
            .ok_or_else(|| anyhow!("'{name}' no es una instancia gestionada"))?;
        let mc = s.minecraft.clone().unwrap();
        let dir = mc.instance_dir.clone();
        let jar = std::path::Path::new(&dir).join(format!("paper-{}.jar", mc.version));
        if !jar.exists() {
            emit(&Ev::Msg { msg: format!("a {name} le falta el jar ({:?}); descárgalo desde la TUI", jar) }, &self.ev);
            return Err(anyhow!("falta {}", jar.display()));
        }
        let cmd = instances::java_cmd(std::path::Path::new(&dir), &mc.version, &mc.memory, &mc.core);
        let mut child = Command::new("sh").arg("-c").arg(&cmd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn {name}"))?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        self.procs.lock().await.insert(name.to_string(), Proc { child, stdin: Some(stdin) });
        emit(&Ev::Status { server: name.into(), status: "running".into() }, &self.ev);
        let _ = notify(&self.ev, &format!("▲ {name} arrancado"));
        // lector de stdout → eventos de línea + parse TPS para avisos
        let d = self.clone();
        let n = name.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let tps_below = config::load().unwrap_or_default().notify.tps_below.unwrap_or(0.0);
                if tps_below > 0.0 {
                    let mut h = minecraft::ServerHealth::default();
                    if let Some(minecraft::Event::TpsReport(t1, _, _)) = minecraft::parse_line(&l, &mut h) {
                        if t1 > 0.0 && t1 < tps_below {
                            let _ = notify(&d.ev, &format!("⚠ {n}: TPS 1m = {t1}"));
                        }
                    }
                }
                emit(&Ev::Line { server: n.clone(), text: l }, &d.ev);
            }
        });
        let d = self.clone();
        let n2 = name.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                emit(&Ev::Line { server: n2.clone(), text: l }, &d.ev);
            }
        });
        // vigilante de salida
        let d = self.clone();
        let n3 = name.to_string();
        let procs = self.procs.clone();
        tokio::spawn(async move {
            // espera hasta que el hijo desaparezca
            loop {
                let exited = {
                    let mut procs = procs.lock().await;
                    match procs.get_mut(&n3) {
                        Some(p) => matches!(p.child.try_wait(), Ok(Some(_))),
                        None => true,
                    }
                };
                if exited { break; }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            procs.lock().await.remove(&n3);
            emit(&Ev::Status { server: n3.clone(), status: "stopped".into() }, &d.ev);
            let _ = notify(&d.ev, &format!("▼ {n3} se detuvo"));
        });
        Ok(())
    }

    async fn stop_server(self: &Arc<Self>, name: &str, wait_secs: u64) -> Result<()> {
        let mut stdin = {
            let mut procs = self.procs.lock().await;
            let p = procs.get_mut(name).ok_or_else(|| anyhow!("'{name}' no está corriendo"))?;
            p.stdin.take()
        };
        match stdin.as_mut() {
            Some(si) => {
                let _ = si.write_all(b"stop\n").await;
                let _ = si.flush().await;
                drop(stdin); // soltar para que cierre al morir el hijo
                for _ in 0..wait_secs * 2 {
                    let running = { self.procs.lock().await.contains_key(name) };
                    if !running { return Ok(()); }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
            None => {}
        }
        // a la fuerza
        if let Some(mut p) = self.procs.lock().await.remove(name) {
            let _ = p.child.kill().await;
            emit(&Ev::Status { server: name.into(), status: "stopped".into() }, &self.ev);
        }
        Ok(())
    }

    async fn handle(self: &Arc<Self>, req: Req) -> Result<String> {
        match req {
            Req::Ping => Ok("aeroPanel daemon".into()),
            Req::Start { server } => {
                self.spawn_server(&server).await?;
                Ok(format!("{server} arrancando"))
            }
            Req::Stop { server } => {
                self.stop_server(&server, 30).await?;
                Ok(format!("{server} detenido"))
            }
            Req::Restart { server } => {
                let _ = self.stop_server(&server, 30).await;
                self.spawn_server(&server).await?;
                Ok(format!("{server} reiniciado"))
            }
            Req::Console { server, text } => {
                let mut procs = self.procs.lock().await;
                let p = procs.get_mut(&server).ok_or_else(|| anyhow!("'{server}' no está corriendo"))?;
                let si = p.stdin.as_mut().ok_or_else(|| anyhow!("stdin cerrado"))?;
                si.write_all(format!("{text}\n").as_bytes()).await?;
                si.flush().await?;
                Ok(String::new())
            }
            Req::Backup { server } => {
                let s = Self::managed_cfg(&config::load().unwrap_or_default(), &server)
                    .ok_or_else(|| anyhow!("'{server}' no es gestionada"))?;
                let dir = s.minecraft.map(|m| m.instance_dir).unwrap_or_default();
                let path = do_backup(&server, &dir).await?;
                let msg = format!("backup de {server}: {}", path.display());
                emit(&Ev::Backup { server: server.clone(), path: path.display().to_string() }, &self.ev);
                let _ = notify(&self.ev, &msg);
                Ok(msg)
            }
        }
    }
}

async fn notify(ev: &broadcast::Sender<Ev>, text: &str) -> Result<()> {
    emit(&Ev::Msg { msg: text.to_string() }, ev);
    crate::notify::send(text).await
}

pub async fn do_backup(server: &str, dir: &str) -> Result<std::path::PathBuf> {
    let d = backup_dir(server);
    tokio::fs::create_dir_all(&d).await?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let out = d.join(format!("{ts}.tar.gz"));
    let parent = std::path::Path::new(dir).parent().unwrap_or(std::path::Path::new("/"));
    let base = std::path::Path::new(dir).file_name().unwrap_or_default().to_string_lossy();
    let st = tokio::process::Command::new("tar")
        .args([
            "czf",
            out.to_string_lossy().as_ref(),
            "-C",
            parent.to_string_lossy().as_ref(),
            base.as_ref(),
        ])
        .output()
        .await?;
    if !st.status.success() {
        return Err(anyhow!("tar falló: {}", String::from_utf8_lossy(&st.stderr)));
    }
    // retención: conservar las 5 más nuevas
    let mut baks: Vec<_> = std::fs::read_dir(&d)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |x| x == "gz"))
        .map(|e| e.path())
        .collect();
    baks.sort();
    while baks.len() > 5 {
        let old = baks.remove(0);
        let _ = tokio::fs::remove_file(old).await;
    }
    Ok(out)
}

// ---------- scheduler ----------

fn load_last_run() -> HashMap<String, i64> {
    std::fs::read_to_string(schedule_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_last_run(m: &HashMap<String, i64>) {
    if let Ok(s) = serde_json::to_string_pretty(m) {
        let _ = std::fs::write(schedule_file(), s);
    }
}

async fn tick_schedule(d: &Arc<Daemon>) {
    let cfg = config::load().unwrap_or_default();
    let mut last = load_last_run();
    let now = chrono::Local::now();
    for job in &cfg.schedule {
        let due = match (&job.every_secs, &job.at) {
            (Some(every), _) => {
                let t = last.get(&job.name).copied().unwrap_or(0);
                now.timestamp() - t >= *every as i64
            }
            (None, Some(at)) => {
                let (h, m) = match at.split_once(':') {
                    Some((h, m)) => (h.parse::<u32>().unwrap_or(99), m.parse::<u32>().unwrap_or(99)),
                    None => (99, 99),
                };
                let key = format!("day:{}", job.name);
                let today = now.format("%F").to_string();
                let not_today = last.get(&key).map_or(true, |d| d.to_string() != today);
                if chrono::Timelike::hour(&now.time()) == h && chrono::Timelike::minute(&now.time()) == m && not_today {
                    last.insert(key, now.timestamp()); // marca el día
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !due { continue; }
        last.insert(job.name.clone(), now.timestamp());
        save_last_run(&last);
        let res = match job.action.as_str() {
            "backup" => d.handle(Req::Backup { server: job.server.clone() }).await,
            "restart" => d.handle(Req::Restart { server: job.server.clone() }).await,
            "command" => d
                .handle(Req::Console { server: job.server.clone(), text: job.command.clone().unwrap_or_default() })
                .await
                .map(|_| format!("comando enviado a {}", job.server)),
            other => Err(anyhow!("acción desconocida: {other}")),
        };
        if let Err(e) = res {
            let _ = notify(&d.ev, &format!("⚠ scheduler '{}' falló: {e}", job.name));
        }
    }
    // los due "at" que usamos arriba escriben claves "day:name"; persistir
    save_last_run(&last);
}

// ---------- servidor de socket ----------

async fn serve_client(d: Arc<Daemon>, stream: UnixStream) {
    let (r, mut w) = tokio::io::split(stream);
    let mut rx = d.ev.subscribe();
    // reenvía eventos en segundo plano
    let ev_task = tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            let mut s = serde_json::to_string(&ev).unwrap_or_default();
            s.push('\n');
            if w.write_all(s.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let req: Req = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = d.ev.send(Ev::Msg { msg: format!("petición inválida: {e}") });
                continue;
            }
        };
        match d.handle(req).await {
            Ok(msg) if !msg.is_empty() => {
                let _ = d.ev.send(Ev::Msg { msg });
            }
            Ok(_) => {}
            Err(e) => {
                let _ = d.ev.send(Ev::Msg { msg: format!("error: {e}") });
            }
        }
    }
    ev_task.abort();
}

pub async fn run() -> Result<()> {
    let cfg = config::load().unwrap_or_default();
    let (ev, _) = broadcast::channel(4096);
    let d = Arc::new(Daemon { ev, procs: Arc::new(Mutex::new(HashMap::new())) });
    // auto-start de las instancias gestionadas con auto_start
    for s in &cfg.server {
        if s.minecraft.as_ref().map_or(false, |m| m.auto_start) {
            if let Err(e) = d.spawn_server(&s.name).await {
                let _ = d.ev.send(Ev::Msg { msg: format!("auto-start {0}: {e}", s.name) });
            }
        }
    }
    // scheduler cada 20s
    let d2 = d.clone();
    tokio::spawn(async move {
        loop {
            tick_schedule(&d2).await;
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        }
    });
    // socket
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path)?;
    let _ = notify(&d.ev, "daemon de aeroPanel iniciado");
    loop {
        let (stream, _) = listener.accept().await?;
        let d = d.clone();
        tokio::spawn(serve_client(d, stream));
    }
}
