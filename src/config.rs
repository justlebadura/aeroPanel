use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct General {
    #[serde(default = "d_refresh")]
    pub refresh_secs: u64,
    #[serde(default = "d_tail")]
    pub log_tail_lines: usize,
}

fn d_refresh() -> u64 { 3 }
fn d_tail() -> usize { 300 }

impl Default for General {
    fn default() -> Self {
        Self { refresh_secs: d_refresh(), log_tail_lines: d_tail() }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MinecraftCfg {
    #[serde(default)]
    pub console_command: String,
    #[serde(default)]
    pub log_command: String,
    #[serde(default = "d_true")]
    pub send_tps_cmd: bool,
    #[serde(default = "d_tps_interval")]
    pub tps_interval_secs: u64,
    #[serde(default = "d_maxtps")]
    pub expected_max_tps: f64,
    // ── managed local instances ──
    #[serde(default)]
    pub instance_dir: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub memory: String,
    #[serde(default)]
    pub auto_start: bool,
}

fn d_true() -> bool { true }
fn d_tps_interval() -> u64 { 20 }
fn d_maxtps() -> f64 { 20.0 }

impl Default for MinecraftCfg {
    fn default() -> Self {
        Self {
            console_command: String::new(),
            log_command: String::new(),
            send_tps_cmd: true,
            tps_interval_secs: d_tps_interval(),
            expected_max_tps: d_maxtps(),
            instance_dir: String::new(),
            version: String::new(),
            memory: String::new(),
            auto_start: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerCfg {
    pub name: String,
    pub mode: String, // "local" | "ssh"
    #[serde(default)]
    pub host: String,
    #[serde(default = "d_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub key: String,
    /// Presence = this server is Minecraft (its config lives here)
    #[serde(default)]
    pub minecraft: Option<MinecraftCfg>,
}

fn d_port() -> u16 { 22 }

impl ServerCfg {
    pub fn mc_cfg(&self) -> Option<&MinecraftCfg> {
        self.minecraft.as_ref()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub server: Vec<ServerCfg>,
}

pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("AEROPANEL_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("aeropanel").join("config.toml")
}

pub fn load() -> anyhow::Result<Config> {
    let path = config_path();
    if !path.exists() {
        return Ok(Config::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    let cfg = toml::from_str(&raw)?;
    Ok(cfg)
}
