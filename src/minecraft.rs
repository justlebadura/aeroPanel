//! Minecraft log parsing + health model.
#![allow(dead_code)]

#[derive(Debug, Clone)]
pub enum Event {
    Join(String),
    Leave(String),
    Chat { who: String, msg: String },
    Death(String),
    Warn(String),
    Error(String),
    TpsReport(f64, f64, f64),
    Mspt(f64),
}

#[derive(Debug, Clone, Default)]
pub struct ServerHealth {
    pub tps_1m: Option<f64>,
    pub tps_5m: Option<f64>,
    pub tps_15m: Option<f64>,
    pub mspt: Option<f64>,
    pub players: Vec<String>,
    /// joined→left diff maintained from log stream
    pub online: Vec<String>,
    pub last_error: Option<String>,
    pub errors_seen: usize,
}

impl ServerHealth {
    pub fn worst_tps(&self) -> Option<f64> {
        [self.tps_1m, self.tps_5m, self.tps_15m]
            .iter()
            .filter_map(|t| *t)
            .reduce(f64::min)
    }
}

/// Parse one log line into an optional event, updating health in place.
pub fn parse_line(line: &str, h: &mut ServerHealth) -> Option<Event> {
    let ev = classify(line);
    let body = msg_body(line);

    match ev {
        Some(Event::Join(ref who)) => {
            if !who.is_empty() && !h.online.contains(who) {
                h.online.push(who.clone());
            }
            if !who.is_empty() && !h.players.contains(who) {
                h.players.push(who.clone());
            }
        }
        Some(Event::Leave(ref who)) => {
            h.online.retain(|p| *p != *who);
        }
        Some(Event::Error(_)) => {
            h.last_error = Some(line.to_string());
            h.errors_seen += 1;
        }
        Some(Event::TpsReport(a, b, c)) => {
            h.tps_1m = Some(a);
            h.tps_5m = Some(b);
            h.tps_15m = Some(c);
        }
        Some(Event::Mspt(v)) => {
            h.mspt = Some(v);
        }
        _ => {}
    }
    let _ = body;
    ev
}

fn msg_body(line: &str) -> &str {
    line.split_once("]: ").map(|(_, b)| b).unwrap_or(line)
}

fn classify(line: &str) -> Option<Event> {
    let body = msg_body(line);

    // Paper: "TPS from last 1m, 5m, 15m: 20.0, 20.0, 20.0"
    if let Some(rest) = body.strip_prefix("TPS from last") {
        if let Some(vals) = rest.split(':').nth(1) {
            let vals: Vec<f64> =
                vals.split(',').filter_map(|v| v.trim().parse().ok()).collect();
            if vals.len() == 3 {
                return Some(Event::TpsReport(vals[0], vals[1], vals[2]));
            }
        }
    }
    // MSPT (Paper 1.21+): "MSPT from last 1m, 5m, 15m: 12.3, ..."
    if body.contains("MSPT from last") {
        if let Some(v) = body
            .split(':')
            .nth(1)
            .and_then(|r| r.split(',').next())
            .and_then(|s| s.trim().parse().ok())
        {
            return Some(Event::Mspt(v));
        }
    }
    // spark: "Average tick time: 12ms"
    if body.contains("Average tick time") {
        if let Some(v) = body
            .split(':')
            .nth(1)
            .and_then(|r| r.trim().trim_end_matches("ms").parse::<f64>().ok())
        {
            return Some(Event::Mspt(v as f64));
        }
    }

    if body.contains(" joined the game") {
        return Some(Event::Join(before(body, " joined the game")));
    }
    if let Some(i) = body.find(" logged in with entity") {
        let who = body[..i].split("[/").next().unwrap_or("").trim().to_string();
        return Some(Event::Join(who));
    }
    if body.contains(" left the game") {
        return Some(Event::Leave(before(body, " left the game")));
    }
    if body.contains(" lost connection: ") {
        return Some(Event::Leave(before(body, " lost connection: ")));
    }
    // chat: "<Player> message"
    if body.starts_with('<') {
        if let Some(end) = body.find('>') {
            return Some(Event::Chat {
                who: body[1..end].trim().to_string(),
                msg: body[end + 1..].trim().to_string(),
            });
        }
    }
    // deaths (common phrases)
    for pat in [
        " was slain by ",
        " was shot by ",
        " was killed by ",
        " fell from a high place",
        " hit the ground too hard",
        " drowned",
    ] {
        if let Some(i) = body.find(pat) {
            if i > 0 && i < 40 {
                return Some(Event::Death(body[..i].trim().to_string()));
            }
        }
    }
    if line.contains("ERROR]") {
        return Some(Event::Error(body.to_string()));
    }
    if line.contains("WARN]") {
        return Some(Event::Warn(body.to_string()));
    }
    None
}

fn before(s: &str, pat: &str) -> String {
    s.split(pat)
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches("* ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::{parse_line, ServerHealth};

    #[test]
    fn parses_paper_tps() {
        let mut h = ServerHealth::default();
        let l = "[12:00:00 INFO]: TPS from last 1m, 5m, 15m: 19.8, 20.0, 20.0";
        let ev = parse_line(l, &mut h);
        assert_eq!(h.tps_1m, Some(19.8));
        assert_eq!(h.tps_15m, Some(20.0));
        assert!(matches!(ev, Some(Event::TpsReport(..))));
    }

    #[test]
    fn parses_join_and_chat() {
        let mut h = ServerHealth::default();
        parse_line("[12:00:00 INFO]: Steve joined the game", &mut h);
        assert_eq!(h.online, vec!["Steve".to_string()]);
        let ev = parse_line("[12:00:05 INFO]: <Steve> hola", &mut h);
        assert!(matches!(ev, Some(Event::Chat { .. })));
    }

    #[test]
    fn parses_leave_and_error() {
        let mut h = ServerHealth::default();
        parse_line("[12:00:00 INFO]: Steve joined the game", &mut h);
        parse_line("[12:00:09 INFO]: Steve left the game", &mut h);
        assert!(h.online.is_empty());
        parse_line("[12:00:10] [Server thread/ERROR]: something broke", &mut h);
        assert_eq!(h.errors_seen, 1);
    }
}
