//! Command execution abstraction: local or SSH (delegates to the user's
//! OpenSSH — free key-agent support, no extra deps).

use anyhow::{anyhow, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

/// Shared feed for log/console lines: (server_idx, line)
pub type LineTx = tokio::sync::mpsc::UnboundedSender<(usize, String)>;

#[derive(Debug, Clone)]
pub struct Target {
    #[allow(dead_code)]
    pub name: String,
    pub mode: String, // "local" | "ssh"
    pub host: String,
    pub port: u16,
    pub user: String,
    pub key: String,
}

impl Target {
    pub fn ssh_arg(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    /// Build an ssh Command with BatchMode + no TTY
    fn ssh_cmd(&self) -> Result<Command> {
        if self.mode != "ssh" {
            return Err(anyhow!("not an ssh target"));
        }
        let mut c = Command::new("ssh");
        c.arg("-p").arg(self.port.to_string());
        if !self.key.is_empty() {
            c.arg("-i").arg(&self.key);
        }
        c.args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=6",
            "-o", "StrictHostKeyChecking=accept-new",
            // multiplexing: reutiliza una sola conexión TCP para todos los
            // execs/streams → los comandos vuelan (1-2s → ~ms)
            "-o", "ControlMaster=auto",
            "-o", "ControlPath=/tmp/aeropanel-ssh-%r@%h-%p",
            "-o", "ControlPersist=10m",
        ]);
        c.arg(self.ssh_arg());
        Ok(c)
    }

    /// Run a shell command, return stdout
    pub async fn exec(&self, shell_cmd: &str) -> Result<String> {
        let out = if self.mode == "ssh" {
            self.ssh_cmd()?.arg(shell_cmd).output().await?
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(shell_cmd);
            c.output().await?
        };
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            if err.trim().is_empty() {
                return Err(anyhow!("command failed: {}", shell_cmd));
            }
            return Err(anyhow!("{}", err.trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Spawn a long-lived streaming process (log tail or console attach).
    /// Returns child + a reader with the initial buffered output.
    pub async fn spawn_stream(&self, shell_cmd: &str) -> Result<(Child, BufReader<tokio::process::ChildStdout>)> {        let mut cmd = if self.mode == "ssh" {
            let mut c = self.ssh_cmd()?;
            c.arg("-T"); // no tty, pipes work
            c.arg(shell_cmd);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(shell_cmd);
            c
        };
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        Ok((child, BufReader::new(stdout)))
    }

    /// Spawn a local process directly (managed instances) — sh -c wrapper.
    pub async fn local_spawn_stream(shell_cmd: &str) -> Result<(Child, BufReader<tokio::process::ChildStdout>)> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(shell_cmd);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        Ok((child, BufReader::new(stdout)))
    }

    /// Try to get the child's stdin for console input
    pub fn take_stdin(child: &mut Child) -> Option<tokio::process::ChildStdin> {
        child.stdin.take()
    }
}

/// Write a line to a console stdin handle
pub async fn console_send(stdin: &mut tokio::process::ChildStdin, line: &str) -> Result<()> {
    stdin.write_all(format!("{line}\n").as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

/// Spawn a background reader task that pushes lines into the shared feed
pub fn spawn_line_reader(
    reader: BufReader<tokio::process::ChildStdout>,
    tx: LineTx,
    idx: usize,
) {
    tokio::spawn(async move {
        let mut lines = reader.lines();
        loop {
            match lines.next_line().await {
                Ok(Some(l)) => {
                    if tx.send((idx, l)).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
    });
}

/// Copy a file TO the target's directory (upload).
pub async fn upload(t: &Target, local_path: &str, dest_dir: &str) -> Result<()> {
    if t.mode == "ssh" {
        let mut c = Command::new("scp");
        c.arg("-P").arg(t.port.to_string());
        if !t.key.is_empty() {
            c.arg("-i").arg(&t.key);
        }
        c.arg("-o").arg("StrictHostKeyChecking=accept-new");
        c.arg(local_path).arg(format!("{}:{}/", t.ssh_arg(), dest_dir));
        let out = c.output().await?;
        if !out.status.success() {
            return Err(anyhow!("scp falló: {}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    } else {
        let out = Command::new("cp")
            .arg(local_path)
            .arg(format!("{}/", dest_dir.trim_end_matches('/')))
            .output()
            .await?;
        if !out.status.success() {
            return Err(anyhow!("cp falló: {}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    }
}

/// Read a text file from the target (for the live editor), capped to max bytes.
pub async fn read_file(t: &Target, path: &str, max: usize) -> Result<String> {
    let out = t.exec(&format!("head -c {max} {path:?}")).await?;
    Ok(out)
}

/// Write a text buffer to a file on the target (stdin piping, local and ssh).
pub async fn write_file(t: &Target, path: &str, content: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    if t.mode == "ssh" {
        let mut child = t
            .ssh_cmd()?
            .arg(format!("cat > {path:?}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(mut sin) = child.stdin.take() {
            sin.write_all(content.as_bytes()).await?;
            sin.shutdown().await?;
        }
        let st = child.wait().await?;
        if !st.success() {
            return Err(anyhow!("escritura falló en {path}"));
        }
    } else {
        std::fs::write(path, content)?;
    }
    Ok(())
}

/// Open a path with the system file manager / default app (local servers only —
/// local servers never need ssh tunnels; xdg-open runs on this machine).
pub async fn open_desktop(t: &Target, path: &str) -> Result<()> {
    if t.mode != "local" {
        return Err(anyhow!("solo disponible para servidores locales"));
    }
    let out = Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.trim().is_empty() {
            return Err(anyhow!("xdg-open falló (¿hay explorador de archivos instalado?)"));
        }
        return Err(anyhow!("{}", err.trim()));
    }
    Ok(())
}

/// Copy a file FROM the target (download) into local_path (dir or file path).
pub async fn download(t: &Target, remote_path: &str, local_path: &str) -> Result<()> {
    if t.mode == "ssh" {
        let mut c = Command::new("scp");
        c.arg("-P").arg(t.port.to_string());
        if !t.key.is_empty() {
            c.arg("-i").arg(&t.key);
        }
        c.arg("-o").arg("StrictHostKeyChecking=accept-new");
        c.arg(format!("{}:{}", t.ssh_arg(), remote_path)).arg(local_path);
        let out = c.output().await?;
        if !out.status.success() {
            return Err(anyhow!("scp falló: {}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    } else {
        let out = Command::new("cp")
            .arg(remote_path)
            .arg(local_path)
            .output()
            .await?;
        if !out.status.success() {
            return Err(anyhow!("cp falló: {}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    }
}

/// List a directory on the target: returns (name, is_dir, size_str) triplets
pub async fn list_dir(t: &Target, dir: &str) -> Result<Vec<(String, bool, String)>> {
    // ls -A1ap: un nombre por línea, "/" al final si es dir — robusto con
    // espacios, sin depender del locale ni de columnas de ls -la
    let out = t
        .exec(&format!("ls -A1ap -- {dir:?}"))
        .await?;
    let mut v = Vec::new();
    for line in out.lines() {
        if line.is_empty() {
            continue;
        }
        let is_dir = line.ends_with('/');
        let name = line.trim_end_matches('/');
        // algunos ls incluyen ./ y ../ incluso con -A: nunca listarlos
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        v.push((name.to_string(), is_dir, String::new()));
    }
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    Ok(v)
}
