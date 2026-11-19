//! Managed local Minecraft instances: create (download jar), start/stop/restart.

use anyhow::{anyhow, Result};
use crate::connection::Target;

pub const INSTANCES_DIR: &str = "instances"; // under config dir

pub fn instances_root() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("aeropanel")
        .join(INSTANCES_DIR)
}

/// Resolve latest paper build for a version; returns (build, jar_url)
async fn paper_latest(version: &str) -> Result<(u32, String)> {
    let t = Target { name: "net".into(), mode: "local".into(), host: String::new(), port: 0, user: String::new(), key: String::new() };
    let out = t
        .exec(&format!(
            "curl -fsSL https://api.papermc.io/v2/projects/paper/versions/{version}/builds"
        ))
        .await?;
    let build: u32 = out
        .match_indices("\"build\":")
        .last()
        .and_then(|(i, _)| {
            out[i + 8..]
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|s| s.parse().ok())
        })
        .ok_or_else(|| anyhow!("no se encontró build para {version}"))?;
    Ok((
        build,
        format!("https://api.papermc.io/v2/projects/paper/versions/{version}/builds/{build}/downloads/paper-{version}-{build}.jar"),
    ))
}

/// Create a managed instance: download paper jar, write eula.txt.
/// Returns the instance dir. `on_line` gets progress messages.
pub async fn create(
    name: &str,
    version: &str,
    memory: &str,
    tx: &crate::connection::LineTx,
    idx: usize,
) -> Result<std::path::PathBuf> {
    let dir = instances_root().join(name);
    if dir.exists() {
        return Err(anyhow!("la instancia {} ya existe", name));
    }
    std::fs::create_dir_all(&dir)?;
    let send = |m: String| {
        let _ = tx.send((idx, format!("[aeroPanel] {m}")));
    };

    send(format!("resolviendo paper {version}…"));
    let (_build, url) = paper_latest(version).await?;
    send(format!("descargando {url}"));
    let t = Target { name: "net".into(), mode: "local".into(), host: String::new(), port: 0, user: String::new(), key: String::new() };
    let jar_path = dir.join(format!("paper-{version}.jar"));
    let jar = jar_path.clone();
    t.exec(&format!(
        "curl -fSL -o {jar:?} {url}"
    ))
    .await?;
    let mb = std::fs::metadata(&jar_path).map(|m| m.len() / 1024 / 1024).unwrap_or(0);
    send(format!("jar descargado ({} MB)", mb));

    std::fs::write(dir.join("eula.txt"), "eula=true\n")?;
    std::fs::write(
        dir.join("server.properties"),
        "motd=aeroPanel instance\nonline-mode=true\n",
    )?;
    let _ = memory;
    send("instancia creada ✔  (eula=true ya aceptada)".into());
    Ok(dir)
}

/// Java launch command for a managed instance
pub fn java_cmd(dir: &std::path::Path, version: &str, memory: &str) -> String {
    let mem = if memory.is_empty() { "2G" } else { memory };
    let jar = format!("paper-{version}.jar");
    format!(
        "cd {dir:?} && exec java -Xms{mem} -Xmx{mem} -jar {jar} nogui"
    )
}

/// Guess version from jar name in dir
pub fn detect_version(dir: &std::path::Path) -> Option<String> {
    let _ = dir;
    None
}
