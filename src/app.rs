#![allow(dead_code)]
//! Application state and actions.

use crate::config::{Config, MinecraftCfg, ServerCfg};
use crate::connection::Target;
use crate::metrics::Metrics;
use crate::minecraft::{parse_line, ServerHealth};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::process::Child;
use tokio::sync::Mutex as TMutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Recursos,
    Logs,
    Minecraft,
    Consola,
    Gestion,
    Archivos,
}

pub const TABS: [Tab; 7] = [
    Tab::Overview,
    Tab::Recursos,
    Tab::Logs,
    Tab::Minecraft,
    Tab::Gestion,
    Tab::Archivos,
    Tab::Consola,
];

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Overview => " overview ",
            Tab::Recursos => " recursos ",
            Tab::Logs => " logs ",
            Tab::Minecraft => " minecraft ",
            Tab::Gestion => " gestión ",
            Tab::Archivos => " archivos ",
            Tab::Consola => " consola ",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Checking,
    Up,
    Down,
    /// managed instance process running
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    ConsoleInput,
    FormAdd,
    FormInstance,
    UploadPath,
    DownloadPath,
    Editor,
}

/// Live editor state: lines buffer + cursor, autosave-driven.
#[derive(Default)]
pub struct Editor {
    pub path: String,
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    pub vscroll: usize,
    pub dirty: bool,
    pub saving: bool,
    pub loading: bool,
    pub last_edit: Option<std::time::Instant>,
}

impl Editor {
    pub fn title(&self) -> String {
        let mark = if self.saving {
            " (guardando…)"
        } else if self.dirty {
            " *"
        } else {
            ""
        };
        format!(" editor — {}{}", self.path, mark)
    }
}

pub struct Server {
    pub cfg: ServerCfg,
    pub target: Target,
    pub status: Status,
    pub metrics: Metrics,
    pub cpu_hist: VecDeque<u64>,
    pub mem_hist: VecDeque<u64>,
    pub health: ServerHealth,
    pub log_lines: VecDeque<String>,
    pub console_stdin: Option<Arc<TMutex<Option<tokio::process::ChildStdin>>>>,
    pub child: Option<Arc<TMutex<Option<Child>>>>,
    pub scroll: u16,
}

impl Server {
    pub fn is_mc(&self) -> bool {
        self.cfg.mc_cfg().is_some()
    }
    pub fn is_managed(&self) -> bool {
        self.cfg.mc_cfg().map(|m| !m.instance_dir.is_empty()).unwrap_or(false)
    }
    pub fn files_root(&self) -> String {
        self.cfg
            .mc_cfg()
            .map(|m| m.instance_dir.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| ".".to_string())
    }
}

#[derive(Default, Clone)]
pub struct Form {
    pub fields: Vec<String>,
    pub active: usize,
}

pub struct App {
    pub servers: Vec<Server>,
    pub selected: usize,
    pub screen: Screen,
    pub tab: Tab,
    pub input_mode: InputMode,
    pub console_input: String,
    pub console_history: VecDeque<String>,
    pub history_idx: usize,
    pub form: Form,
    pub path_input: String,
    pub files: Vec<(String, bool, String)>,
    pub files_dir: String,
    pub files_sel: usize,
    pub files_loading: bool,
    pub files_reveal: usize,
    pub files_offset: std::cell::Cell<usize>,
    pub editor: Option<Editor>,
    pub help: bool,
    pub restart_at: Option<(usize, std::time::Instant)>,
    pub spinner_frame: usize,
    pub quit: bool,
    pub line_tx: crate::connection::LineTx,
    /// cliente del daemon (unix socket) — Some ⇒ operaciones por el daemon
    pub daemon_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    /// índice del jugador seleccionado en la pestaña Minecraft
    pub mc_player_sel: usize,
    pub cfg_path: std::path::PathBuf,
}

impl App {
    /// jugador actualmente seleccionado ("") si no hay
    pub fn mc_target(&self) -> String {
        self.sel()
            .and_then(|s| s.health.online.get(self.mc_player_sel).cloned())
            .unwrap_or_default()
    }
}

pub const FORM_FIELDS: [&str; 7] = [
    "name", "mode(local|ssh)", "host", "port", "user", "minecraft(y/n)", "console_command",
];
pub const INSTANCE_FIELDS: [&str; 5] = ["nombre", "core (paper|purpur|fabric|vanilla|neoforge|forge)", "version (ej 1.21.4)", "ram (ej 4G)", "auto_start(y/n)"];

impl App {
    pub fn new(cfg: Config, cfg_path: std::path::PathBuf, line_tx: crate::connection::LineTx) -> Self {
        let tail = cfg.general.log_tail_lines.max(50);
        let servers = cfg
            .server
            .iter()
            .map(|c| Server {
                target: Target {
                    name: c.name.clone(),
                    mode: c.mode.clone(),
                    host: c.host.clone(),
                    port: c.port,
                    user: c.user.clone(),
                    key: c.key.clone(),
                },
                cfg: c.clone(),
                status: Status::Checking,
                metrics: Metrics::default(),
                cpu_hist: VecDeque::with_capacity(120),
                mem_hist: VecDeque::with_capacity(120),
                health: ServerHealth::default(),
                log_lines: VecDeque::with_capacity(tail),
                console_stdin: c
                    .mc_cfg()
                    .filter(|mc| !mc.console_command.is_empty())
                    .map(|_| Arc::new(TMutex::new(None))),
                child: None,
                scroll: 0,
            })
            .collect();
        App {
            servers,
            selected: 0,
            screen: Screen::Dashboard,
            tab: Tab::Overview,
            input_mode: InputMode::Normal,
            console_input: String::new(),
            console_history: VecDeque::new(),
            history_idx: 0,
            form: Form { fields: vec![String::new(); 7], active: 0 },
            path_input: String::new(),
            files: Vec::new(),
            files_dir: String::new(),
            files_sel: 0,
            files_loading: false,
            files_reveal: 0,
            files_offset: std::cell::Cell::new(0),
            editor: None,
            help: false,
            restart_at: None,
            spinner_frame: 0,
            quit: false,
            line_tx,
            daemon_tx: None,
            mc_player_sel: 0,
            cfg_path,
        }
    }

    pub fn cfg_secs(&self) -> u64 {
        3
    }

    pub fn server_index(&self, name: &str) -> Option<usize> {
        self.servers.iter().position(|s| s.cfg.name == name)
    }

    pub fn sel(&self) -> Option<&Server> {
        self.servers.get(self.selected)
    }

    pub fn sel_mut(&mut self) -> Option<&mut Server> {
        self.servers.get_mut(self.selected)
    }

    pub fn ingest_line(&mut self, server_idx: usize, line: String) {
        let Some(s) = self.servers.get_mut(server_idx) else { return };
        let max = 300usize;
        if s.log_lines.len() >= max {
            s.log_lines.pop_front();
        }
        if s.is_mc() {
            parse_line(&line, &mut s.health);
        }
        s.log_lines.push_back(line);
    }

    pub fn record_metrics(&mut self, idx: usize, m: Metrics) {
        let Some(s) = self.servers.get_mut(idx) else { return };
        s.metrics = m.clone();
        s.status = if s.is_managed() && s.status == Status::Running {
            Status::Running
        } else if m.ok {
            Status::Up
        } else {
            Status::Down
        };
        if s.cpu_hist.len() >= 120 {
            s.cpu_hist.pop_front();
        }
        s.cpu_hist.push_back(m.cpu_percent as u64);
        if s.mem_hist.len() >= 120 {
            s.mem_hist.pop_front();
        }
        s.mem_hist.push_back(m.mem_pct() as u64);
    }

    pub fn mark_down(&mut self, idx: usize, err: String) {
        let Some(s) = self.servers.get_mut(idx) else { return };
        if s.is_managed() && s.status == Status::Running {
            return; // local process running; host metrics failure irrelevant
        }
        s.status = Status::Down;
        s.metrics.ok = false;
        s.metrics.error = err;
    }

    /// Start a managed instance: spawn java, capture console, attach reader.
    pub fn start_instance(&mut self, idx: usize) {
        let (dir, version, memory) = {
            let Some(s) = self.servers.get(idx) else { return };
            let Some(mc) = s.cfg.mc_cfg() else { return };
            (
                mc.instance_dir.clone(),
                mc.version.clone(),
                mc.memory.clone(),
            )
        };
        if dir.is_empty() {
            return;
        }
        let dirp = std::path::PathBuf::from(&dir);
        let cmd_str = crate::instances::java_cmd(&dirp, &version, &memory, self.servers[idx].cfg.mc_cfg().map(|m| m.core.as_str()).unwrap_or("paper"));
        let _ = self.line_tx.send((idx, "[aeroPanel] iniciando instancia…".into()));
        let tx = self.line_tx.clone();
        let slot_in = self.servers[idx].console_stdin.get_or_insert_with(|| Arc::new(TMutex::new(None))).clone();
        let slot_child = self.servers[idx].child.get_or_insert_with(|| Arc::new(TMutex::new(None))).clone();
        self.servers[idx].status = Status::Running;
        tokio::spawn(async move {
            match Target::local_spawn_stream(&cmd_str).await {
                Ok((child, reader)) => {
                    *slot_child.lock().await = Some(child);
                    if let Some(stdin) = slot_child.lock().await.as_mut().and_then(|c| c.stdin.take()) {
                        *slot_in.lock().await = Some(stdin);
                    }
                    crate::connection::spawn_line_reader(reader, tx, idx);
                }
                Err(e) => {
                    let _ = tx.send((idx, format!("[aeroPanel] error iniciando: {e}")));
                }
            }
        });
    }

    /// Stop a managed instance: send `stop` via console, else kill the child.
    pub fn stop_instance(&mut self, idx: usize, wait_secs: u64) {
        let slot_in = self.servers[idx].console_stdin.clone();
        let slot_child = self.servers[idx].child.clone();
        let tx = self.line_tx.clone();
        self.servers[idx].status = Status::Up;
        tokio::spawn(async move {
            let _ = tx.send((idx, "[aeroPanel] deteniendo instancia…".into()));
            if let Some(sl) = &slot_in {
                let mut guard = sl.lock().await;
                if let Some(stdin) = guard.as_mut() {
                    let _ = crate::connection::console_send(stdin, "stop").await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
            if let Some(sl) = &slot_child {
                let mut guard = sl.lock().await;
                if let Some(child) = guard.as_mut() {
                    let _ = child.kill().await;
                }
                *guard = None;
            }
            if let Some(sl) = &slot_in {
                *sl.lock().await = None;
            }
            let _ = tx.send((idx, "[aeroPanel] instancia detenida".into()));
        });
    }

    pub fn restart_instance(&mut self, idx: usize) {
        let _ = self.line_tx.send((idx, "[aeroPanel] reiniciando…".into()));
        self.stop_instance(idx, 4);
        self.restart_at = Some((idx, std::time::Instant::now() + std::time::Duration::from_secs(9)));
    }

    /// Build config from current servers and persist
    pub fn save_config(&self) -> anyhow::Result<()> {
        let mut cfg = Config::default();
        cfg.server = self.servers.iter().map(|s| s.cfg.clone()).collect();
        let toml = toml::to_string_pretty(&cfg)?;
        if let Some(parent) = self.cfg_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.cfg_path, toml)?;
        Ok(())
    }

    /// Build a ServerCfg from the add-server form fields
    pub fn form_to_cfg(&self) -> ServerCfg {
        let f = &self.form.fields;
        let minecraft = matches!(
            f.get(5).map(|s| s.trim().to_ascii_lowercase()).as_deref(),
            Some("y") | Some("yes") | Some("si") | Some("sí")
        );
        let mc = MinecraftCfg {
            console_command: f.get(6).cloned().unwrap_or_default().trim().to_string(),
            ..Default::default()
        };
        ServerCfg {
            name: f.first().cloned().unwrap_or_default().trim().to_string(),
            mode: f.get(1).cloned().unwrap_or_default().trim().to_ascii_lowercase(),
            host: f.get(2).cloned().unwrap_or_default().trim().to_string(),
            port: f.get(3).and_then(|p| p.trim().parse().ok()).unwrap_or(22),
            user: f.get(4).cloned().unwrap_or_default().trim().to_string(),
            key: String::new(),
            minecraft: if minecraft { Some(mc) } else { None },
        }
    }

    /// Build a ServerCfg from the create-instance form
    pub fn instance_to_cfg(&self) -> (String, String, String, String, bool) {
        let f = &self.form.fields;
        let name = f.first().cloned().unwrap_or_default().trim().to_string();
        let core = f
            .get(1)
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let core = match core.as_str() {
            "" | "paper" => "paper".to_string(),
            c => c.to_string(),
        };
        let version = f.get(2).cloned().unwrap_or_default().trim().to_string();
        let memory = f.get(3).cloned().unwrap_or_default().trim().to_string();
        let auto = matches!(
            f.get(4).map(|s| s.trim().to_ascii_lowercase()).as_deref(),
            Some("y") | Some("yes") | Some("si") | Some("sí")
        );
        (name, core, version, memory, auto)
    }

    /// Add a server entry for a managed instance
    pub fn add_instance_server(&mut self, name: &str, dir: &std::path::Path, version: &str, memory: &str, auto: bool) {
        let mc = MinecraftCfg {
            instance_dir: dir.to_string_lossy().to_string(),
            version: version.to_string(),
            memory: memory.to_string(),
            auto_start: auto,
            send_tps_cmd: true,
            ..Default::default()
        };
        let cfg = ServerCfg {
            name: name.to_string(),
            mode: "local".into(),
            host: String::new(),
            port: 22,
            user: String::new(),
            key: String::new(),
            minecraft: Some(mc),
        };
        let s = Server {
            target: Target {
                name: name.to_string(),
                mode: "local".into(),
                host: String::new(),
                port: 22,
                user: String::new(),
                key: String::new(),
            },
            cfg,
            status: Status::Up,
            metrics: Metrics::default(),
            cpu_hist: VecDeque::with_capacity(120),
            mem_hist: VecDeque::with_capacity(120),
            health: ServerHealth::default(),
            log_lines: VecDeque::with_capacity(300),
            console_stdin: None,
            child: None,
            scroll: 0,
        };
        self.servers.push(s);
        self.selected = self.servers.len() - 1;
    }
}
