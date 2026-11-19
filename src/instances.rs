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

/// Download URL + jar name for the chosen core; installer step for cores
/// that need `--installServer`.
async fn resolve_core(
    core: &str,
    version: &str,
    dir: &std::path::Path,
    send: &(dyn Fn(String) + Send + Sync),
) -> Result<()> {
    let t = Target { name: "net".into(), mode: "local".into(), host: String::new(), port: 0, user: String::new(), key: String::new() };
    match core {
        "paper" => {
            send(format!("resolviendo paper {version}…"));
            let (_build, url) = paper_latest(version).await?;
            let jar = dir.join(format!("paper-{version}.jar"));
            send(format!("descargando {url}"));
            t.exec(&format!("curl -fSL -o {jar:?} {url}")).await?;
        }
        "purpur" => {
            send(format!("resolviendo purpur {version}…"));
            let out = t.exec(&format!("curl -fsSL https://api.purpurmc.org/v2/purpur/{version}")).await?;
            let build = out
                .match_indices("\"latest\":")
                .next()
                .and_then(|(i, _)| {
                    out[i + 9..].trim_start_matches('"')
                        .split(|c: char| !c.is_ascii_digit())
                        .next()
                        .map(|s| s.to_string())
                })
                .ok_or_else(|| anyhow!("sin build purpur para {version}"))?;
            let jar = dir.join("purpur.jar");
            send(format!("descargando purpur {version} build {build}"));
            t.exec(&format!(
                "curl -fSL -o {jar:?} https://api.purpurmc.org/v2/purpur/{version}/{build}/download"
            ))
            .await?;
        }
        "vanilla" => {
            send("resolviendo vanilla…".into());
            let mf = t.exec("curl -fsSL https://piston-meta.mojang.com/mc/game/version_manifest_v2.json").await?;
            // busca "id": "<version>" y saca la url del bloque
            let anchor = format!("\"id\":\"{version}\"");
            let mut url = String::new();
            for (i, _) in mf.match_indices(&anchor) {
                if let Some(u) = json_str_after(&mf[i..], "url") {
                    url = u;
                    break;
                }
            }
            anyhow::ensure!(!url.is_empty(), "versión {version} no encontrada");
            let vjson = t.exec(&format!("curl -fsSL {url:?}")).await?;
            // "server": { ... "url": "..." }
            let s = vjson.find("\"server\"").ok_or_else(|| anyhow!("sin server en el manifest"))?;
            let surl = json_str_after(&vjson[s..], "url").ok_or_else(|| anyhow!("sin url del server"))?;
            let jar = dir.join("vanilla.jar");
            send(format!("descargando vanilla {version}"));
            t.exec(&format!("curl -fSL -o {jar:?} {surl}")).await?;
        }
        "fabric" => {
            let jar = dir.join("fabric.jar");
            send(format!("descargando fabric loader para {version}…"));
            t.exec(&format!(
                "curl -fsSL -o {jar:?} https://meta.fabricmc.net/v2/versions/loader/{version}/stable/1.0.0/server/jar"
            ))
            .await?;
            let ok = std::fs::metadata(&jar).map(|m| m.len() > 10_000).unwrap_or(false);
            anyhow::ensure!(ok, "fabric {version}: la descarga falló (¿versión existe?)");
        }
        "neoforge" | "forge" => {
            let (url, jarname) = if core == "neoforge" {
                send("resolviendo neoforge…".into());
                let out = t.exec("curl -fsSL https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge").await?;
                // versiones tipo "21.4.47" para MC 1.21.4 → prefijo sin el 1.
                let prefix = version.strip_prefix("1.").ok_or_else(|| anyhow!("versión inválida"))?;
                let prefix = format!("{prefix}.");
                let mut best = String::new();
                for (i, _) in out.match_indices(&format!("\"{prefix}")) {
                    let rest = &out[i + 1..];
                    let v = rest.split(|c: char| !(c.is_ascii_digit() || c == '.')).next().unwrap_or("");
                    if v.split('.').count() >= 3 && v > best.as_str() {
                        best = v.to_string();
                    }
                }
                anyhow::ensure!(!best.is_empty(), "sin build neoforge para {version}");
                (
                    format!("https://maven.neoforged.net/releases/net/neoforged/neoforge/{best}/neoforge-{best}-installer.jar"),
                    format!("neoforge-{best}-installer.jar"),
                )
            } else {
                send("resolviendo forge…".into());
                let out = t.exec("curl -fsSL https://files.minecraftforge.net/maven/net/minecraftforge/forge/promotions_pretty.json").await?;
                let anchor = format!("\"{version}-latest\":");
                let i = out.find(&anchor).ok_or_else(|| anyhow!("sin build forge para {version}"))?;
                let build = out[i + anchor.len()..]
                    .trim_start_matches(|c: char| c == '"' || c == ':' || c == ' ')
                    .split('"')
                    .next()
                    .unwrap_or("")
                    .to_string();
                anyhow::ensure!(!build.is_empty(), "build forge vacío");
                (
                    format!("https://maven.minecraftforge.net/net/minecraftforge/forge/{version}-{build}/forge-{version}-{build}-installer.jar"),
                    format!("forge-installer.jar"),
                )
            };
            let jar = dir.join(&jarname);
            send(format!("descargando instalador {jarname}"));
            t.exec(&format!("curl -fSL -o {jar:?} {url}")).await?;
            send("ejecutando instalador (1-2 min)…".into());
            let cmd = format!("cd {dir:?} && java -jar {jarname} --installServer > install.log 2>&1");
            let st = tokio::process::Command::new("sh").arg("-c").arg(&cmd).output().await?;
            anyhow::ensure!(st.status.success(), "instalador falló: {}", String::from_utf8_lossy(&st.stderr));
            send("instalador terminado".into());
        }
        other => return Err(anyhow!("core desconocido: {other} (paper|purpur|fabric|vanilla|neoforge|forge)")),
    }
    Ok(())
}

/// extrae "value" de un campo `"key": "value"` después de `s`
fn json_str_after<'a>(s: &'a str, key: &str) -> Option<String> {
    let i = s.find(&format!("\"{key}\""))?;
    let rest = &s[i + key.len() + 2..];
    let j = rest.find('"')?;
    let k = rest[j + 1..].find('"')?;
    Some(rest[j + 1..j + 1 + k].to_string())
}

/// Create a managed instance: download the chosen core, write eula.txt.
/// Returns the instance dir. `on_line` gets progress messages.
pub async fn create(
    name: &str,
    version: &str,
    _memory: &str,
    core: &str,
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

    resolve_core(core, version, &dir, &send).await?;
    let mb = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |x| x == "jar"))
            .filter_map(|e| std::fs::metadata(e.path()).ok())
            .map(|m| m.len())
            .sum::<u64>() / 1024 / 1024,
        Err(_) => 0,
    };
    send(format!("archivos listos ({} MB)", mb));

    std::fs::write(dir.join("eula.txt"), "eula=true\n")?;
    std::fs::write(
        dir.join("server.properties"),
        "motd=aeroPanel instance\nonline-mode=true\n",
    )?;
    send(format!("instancia creada ✔  (core {core}, eula=true ya aceptada)").into());
    Ok(dir)
}

/// Java launch command for a managed instance
pub fn java_cmd(dir: &std::path::Path, version: &str, memory: &str, core: &str) -> String {
    let mem = if memory.is_empty() { "2G" } else { memory };
    // forge/neoforge generan run.sh con sus libs; úsalo si existe
    if dir.join("run.sh").exists() {
        return format!("cd {dir:?} && exec ./run.sh nogui");
    }
    let jar = match core {
        "paper" | "" => format!("paper-{version}.jar"),
        c => format!("{c}.jar"),
    };
    format!(
        "cd {dir:?} && exec java -Xms{mem} -Xmx{mem} -jar {jar} nogui"
    )
}

/// Guess version from jar name in dir
pub fn detect_version(dir: &std::path::Path) -> Option<String> {
    let _ = dir;
    None
}
