//! Tunnel / network assistant: CGNAT detection + playit.gg agent management.
//!
//! Opening router ports from the app is not possible (router web UI,
//! per-model, ISP credentials). Instead:
//!  - `net_info()` shows public IP vs local IP and detects CGNAT.
//!  - `ensure_playit()` installs the playit agent binary and runs it —
//!    the agent prints a claim URL the user visits once, after which
//!    `miserver.playit.gg` routes to 25565 even behind CGNAT.

use anyhow::{Context, Result};

const PLAYIT_BIN: &str = "playit";
const PLAYIT_URL: &str =
    "https://github.com/playit-cloud/playit-agent/releases/latest/download/playit-linux-amd64";

/// public/local IP + CGNAT detection → (public, local, cgnat)
pub async fn net_info() -> Result<(String, String, bool)> {
    let t = crate::connection::Target {
        name: "net".into(),
        mode: "local".into(),
        host: String::new(),
        port: 0,
        user: String::new(),
        key: String::new(),
    };
    let public = t
        .exec("curl -fsSL --max-time 5 https://api.ipify.org")
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let local = t
        .exec("ip -4 route get 1.1.1.1 2>/dev/null | grep -oP 'src \\K[0-9.]+'")
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let cgnat = is_cgnat(&public);
    Ok((public, local, cgnat))
}

/// CGNAT ranges: 100.64.0.0/10, plus the usual private ranges.
fn is_cgnat(ip: &str) -> bool {
    let oct: Vec<u32> = ip
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();
    if oct.len() != 4 {
        return false;
    }
    let (a, b, _, _) = (oct[0], oct[1], oct[2], oct[3]);
    a == 10 || (a == 172 && b >= 16 && b <= 31) || (a == 192 && b == 168) || (a == 100 && b >= 64 && b <= 127)
}

/// Make sure the playit agent binary exists (download if missing) and
/// spawn it, streaming its output into the console of `idx`.
pub async fn ensure_playit(
    idx: usize,
    tx: &crate::connection::LineTx,
) -> Result<tokio::process::Child> {
    let t = crate::connection::Target {
        name: "net".into(),
        mode: "local".into(),
        host: String::new(),
        port: 0,
        user: String::new(),
        key: String::new(),
    };
    let send = |m: String| {
        let _ = tx.send((idx, format!("[aeroPanel] {m}")));
    };
    let bin = format!("{}/{}", bin_dir(), PLAYIT_BIN);
    let installed = t
        .exec(&format!("test -x {bin:?} && echo ok"))
        .await
        .map(|s| s.trim() == "ok")
        .unwrap_or(false);
    if !installed {
        send(format!("descargando agente playit ({PLAYIT_URL})…"));
        t.exec(&format!(
            "mkdir -p {0:?} && curl -fSL -o {1:?} {PLAYIT_URL} && chmod +x {1:?}",
            bin_dir(),
            bin
        ))
        .await
        .context("descarga de playit falló")?;
        send("agente instalado".into());
    }
    send("arrancando playit…".into());
    send("→ en el primer arranque imprime una URL https://playit.gg/claim/…: ábrela en el navegador y vincula tu cuenta".into());
    // stderr → merge via shell (2>&1) so a single stdout reader suffices
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(format!("exec {bin:?} 2>&1"))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("no se pudo arrancar playit")?;
    Ok(child)
}

fn bin_dir() -> String {
    format!("{}/.local/bin", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
}
