//! Host metrics: CPU%, RAM, uptime, load. One combined probe per refresh.

use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Metrics {
    pub cpu_percent: f32,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub load1: f32,
    pub uptime_str: String,
    pub ok: bool,
    pub error: String,
}

impl Metrics {
    pub fn mem_pct(&self) -> f32 {
        if self.mem_total_mb == 0 { 0.0 } else {
            (self.mem_used_mb as f32 / self.mem_total_mb as f32) * 100.0
        }
    }
}

/// The probe: works over ssh or locally. Parse /proc/meminfo + /proc/uptime + loadavg.
pub const PROBE: &str = r#"
cat /proc/loadavg | cut -d' ' -f1
cat /proc/uptime | cut -d' ' -f1
awk '/MemTotal/{print $2} /MemAvailable/{print $2}' /proc/meminfo
"#;

pub fn parse(out: &str) -> Metrics {
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut m = Metrics { ok: true, ..Default::default() };
    if lines.len() >= 4 {
        m.load1 = lines[0].trim().parse().unwrap_or(0.0);
        let up: f64 = lines[1].trim().parse().unwrap_or(0.0);
        let secs = up as u64;
        m.uptime_str = format!("{}d {}h", secs / 86400, (secs % 86400) / 3600);
        let total: u64 = lines[2].trim().parse().unwrap_or(0) / 1024;
        let avail: u64 = lines[3].trim().parse().unwrap_or(0) / 1024;
        m.mem_total_mb = total;
        m.mem_used_mb = total.saturating_sub(avail);
        // rough CPU% proxy: load relative to core count is unreliable here,
        // sample /proc/stat delta is done by caller when available; keep load-based estimate
        m.cpu_percent = (m.load1 * 100.0).clamp(0.0, 100.0);
    } else {
        m.ok = false;
        m.error = "probe output malformed".into();
    }
    m
}

/// Accurate CPU% requires two /proc/stat samples; caller can refine with this.
pub async fn refine_cpu(t: &crate::connection::Target, m: &mut Metrics) {
    let run = async {
        let snap1 = t.exec("grep '^cpu ' /proc/stat").await?;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let snap2 = t.exec("grep '^cpu ' /proc/stat").await?;
        Ok::<(String, String), anyhow::Error>((snap1, snap2))
    };
    if let Ok((s1, s2)) = run.await {
        let parse = |s: &str| -> Vec<u64> {
            s.split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect()
        };
        let a = parse(&s1);
        let b = parse(&s2);
        if a.len() >= 4 && b.len() >= 4 {
            let d = |i: usize| b[i].saturating_sub(a[i]);
            let total: u64 = (0..a.len().min(b.len())).map(|i| d(i)).sum();
            let idle = d(3) + a.get(4).and_then(|_| b.get(4)).map(|v| v.saturating_sub(a[4])).unwrap_or(0);
            if total > 0 {
                m.cpu_percent = ((total - idle) as f32 / total as f32 * 100.0).clamp(0.0, 100.0);
            }
        }
    }
}

/// Ping-style liveness via short exec
#[allow(dead_code)]
pub async fn reachable(t: &crate::connection::Target) -> bool {
    t.exec("true").await.map(|_| true).unwrap_or(false)
}

#[allow(dead_code)]
#[allow(dead_code)]
pub async fn collect(t: &crate::connection::Target) -> Result<Metrics> {
    let mut m = parse(&t.exec(PROBE).await?);
    refine_cpu(t, &mut m).await;
    Ok(m)
}
