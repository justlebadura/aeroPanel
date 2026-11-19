// Notificaciones: Discord webhook y/o Telegram. Sin dependencias nuevas —
// llamamos a curl (ya lo usamos en instances.rs).
use crate::config;

pub async fn send(text: &str) -> anyhow::Result<()> {
    let n = config::load().unwrap_or_default().notify;
    let mut errs = Vec::new();
    if !n.discord_webhook.is_empty() {
        let body = serde_json::json!({ "content": format!("[aeroPanel] {text}") });
        let out = tokio::process::Command::new("curl")
            .args(["-s", "-o", "/dev/null", "-X", "POST", "-H", "Content-Type: application/json"])
            .arg("--data")
            .arg(serde_json::to_string(&body).unwrap_or_default())
            .arg(&n.discord_webhook)
            .output()
            .await?;
        if !out.status.success() {
            errs.push("discord falló");
        }
    }
    if !n.telegram_token.is_empty() && !n.telegram_chat.is_empty() {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            n.telegram_token
        );
        let out = tokio::process::Command::new("curl")
            .args(["-s", "-o", "/dev/null"])
            .arg("--data-urlencode")
            .arg(format!("chat_id={}", n.telegram_chat))
            .arg("--data-urlencode")
            .arg(format!("text=[aeroPanel] {text}"))
            .arg(&url)
            .output()
            .await?;
        if !out.status.success() {
            errs.push("telegram falló");
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(errs.join(", ")))
    }
}
