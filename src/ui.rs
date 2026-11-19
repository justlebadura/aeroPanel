//! UI rendering — OpenCode-inspired look.

use crate::app::{App, InputMode, Screen, Status, Tab, FORM_FIELDS, INSTANCE_FIELDS, TABS};
use crate::theme::*;
use chrono::Local;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Sparkline, Wrap};
use ratatui::Frame;

static TERM_W: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(80);
/// segmentos del breadcrumb de archivos: (ruta acumulada, x_ini, x_fin absolutos)
static FILES_SEGS: std::sync::Mutex<Vec<(String, u16, u16)>> = std::sync::Mutex::new(Vec::new());
/// click sobre el breadcrumb de archivos → ruta destino (None si no fue ahí)
pub fn breadcrumb_hit(x: u16) -> Option<String> {
    let segs = FILES_SEGS.lock().ok()?.clone();
    segs.into_iter()
        .find(|(_, x0, x1)| x >= *x0 && x < *x1)
        .map(|(p, _, _)| p)
}
pub fn term_width() -> u16 {
    TERM_W.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn draw(f: &mut Frame, a: &App) {
    TERM_W.store(f.area().width, std::sync::atomic::Ordering::Relaxed);
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(f.area());

    draw_header(f, a, chunks[0]);
    match a.screen {
        Screen::Dashboard => draw_dashboard(f, a, chunks[1]),
        Screen::Detail => draw_detail(f, a, chunks[1]),
    }
    draw_footer(f, a, chunks[2]);

    if a.help {
        draw_help(f, f.area());
    }
    match a.input_mode {
        InputMode::FormAdd => draw_form(f, a, "añadir servidor", &FORM_FIELDS, false),
        InputMode::FormInstance => draw_form(f, a, "nueva instancia de minecraft", &INSTANCE_FIELDS, false),
        InputMode::UploadPath => draw_input(f, "subir archivo → ruta local origen", &a.path_input),
        InputMode::DownloadPath => draw_input(f, "descargar → ruta local destino", &a.path_input),
        InputMode::Editor => draw_editor(f, a, f.area()),
        _ => {}
    }
}

fn panel(focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border(focused))
}

fn draw_header(f: &mut Frame, a: &App, area: Rect) {
    let spinner = SPINNER[a.spinner_frame % SPINNER.len()];
    let title = Line::from(vec![
        Span::styled("* ", Style::default().fg(ACCENT)),
        Span::styled("aeroPanel", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled("  ·  servidores", dim()),
    ]);
    let right = format!("{spinner}  {}", Local::now().format("%H:%M:%S"));
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(BORDER));
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(title), inner);
    let w = right.len() as u16;
    if inner.width > w {
        f.render_widget(
            Paragraph::new(Span::styled(right, dim())).alignment(Alignment::Right),
            Rect::new(inner.x, inner.y, inner.width, inner.height),
        );
    }
}

fn draw_dashboard(f: &mut Frame, a: &App, area: Rect) {
    let cols = Layout::horizontal([Constraint::Percentage(46), Constraint::Percentage(54)]).split(area);
    draw_server_list(f, a, cols[0]);
    draw_summary_card(f, a, cols[1]);
}

fn draw_server_list(f: &mut Frame, a: &App, area: Rect) {
    let items: Vec<ListItem> = a
        .servers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (dot, dotc) = match s.status {
                Status::Up => (DOT, GREEN),
                Status::Running => (DOT, ACCENT),
                Status::Down => (DOT, RED),
                Status::Checking => (SPINNER[a.spinner_frame % SPINNER.len()], BLUE),
            };
            let selected = i == a.selected;
            let name = if selected {
                Span::styled(format!(" {:<18}", s.cfg.name), Style::default().fg(FG).add_modifier(Modifier::BOLD))
            } else {
                Span::styled(format!(" {:<18}", s.cfg.name), Style::default().fg(FG))
            };
            let mode = Span::styled(format!(" {:<4}", s.target.mode), dim());
            let mc = if s.is_mc() { Span::styled(" mc", Style::default().fg(BLUE)) } else { Span::raw("   ") };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{dot} "), Style::default().fg(dotc)),
                name,
                mode,
                mc,
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(panel(true).title(Span::styled(" servidores ", accent())))
        .highlight_style(Style::default().bg(BG_PANEL));
    let mut st = ListState::default().with_selected(Some(a.selected.min(a.servers.len().saturating_sub(1))));
    f.render_stateful_widget(list, area, &mut st);
}

fn draw_summary_card(f: &mut Frame, a: &App, area: Rect) {
    let Some(s) = a.sel() else { return };
    let lines = vec![
        Line::from(vec![
            Span::styled("host    ", dim()),
            Span::styled(
                if s.target.mode == "ssh" {
                    format!("{}@{}:{}", s.target.user, s.target.host, s.target.port)
                } else {
                    "localhost".to_string()
                },
                Style::default().fg(FG),
            ),
        ]),
        Line::from(vec![
            Span::styled("estado  ", dim()),
            status_span(s.status),
        ]),
        Line::from(vec![
            Span::styled("cpu     ", dim()),
            Span::styled(format!("{:>5.1} %", s.metrics.cpu_percent), Style::default().fg(if s.metrics.cpu_percent > 85.0 { RED } else { FG })),
        ]),
        Line::from(vec![
            Span::styled("mem     ", dim()),
            Span::styled(
                format!("{:>4} / {} MB", s.metrics.mem_used_mb, s.metrics.mem_total_mb),
                Style::default().fg(FG),
            ),
        ]),
        Line::from(vec![
            Span::styled("uptime  ", dim()),
            Span::styled(s.metrics.uptime_str.clone(), Style::default().fg(FG)),
        ]),
        Line::from(vec![
            Span::styled("tps 1m  ", dim()),
            Span::styled(
                s.health.tps_1m.map(|v| format!("{v:.2}")).unwrap_or_else(|| "—".into()),
                Style::default().fg(tps_color(s.health.tps_1m)),
            ),
        ]),
        Line::from(vec![
            Span::styled("jugador ", dim()),
            Span::styled(format!("{}", s.health.players.len()), Style::default().fg(FG)),
        ]),
        Line::from(""),
        Line::from(Span::styled("cpu (últimos 2m)", dim())),
    ];
    let block = panel(false).title(Span::styled(" resumen ", accent()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let top = Layout::vertical([Constraint::Min(1), Constraint::Length(5)]).split(inner);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), top[0]);
    f.render_widget(
        Sparkline::default()
            .data(&a.sel().map(|s| s.cpu_hist.clone().into_iter().collect::<Vec<u64>>()).unwrap_or_default())
            .style(Style::default().fg(ACCENT)),
        top[1],
    );
}

fn tps_color(v: Option<f64>) -> ratatui::style::Color {
    match v {
        Some(v) if v >= 19.0 => GREEN,
        Some(v) if v >= 15.0 => YELLOW,
        Some(_) => RED,
        None => FG_FAINT,
    }
}

fn status_span(st: Status) -> Span<'static> {
    match st {
        Status::Up => Span::styled("* up", Style::default().fg(GREEN)),
        Status::Running => Span::styled("* running", Style::default().fg(ACCENT)),
        Status::Down => Span::styled("* down", Style::default().fg(RED)),
        Status::Checking => Span::styled("~ verificando…", Style::default().fg(BLUE)),
    }
}

/// Which tab a mouse click at absolute column x hits (tabs row is y=3).
pub fn hit_tab(a: &App, x: u16) -> Option<Tab> {
    let s = a.sel()?;
    let mc = s.is_mc();
    let managed = s.is_managed();
    let mut x0: u16 = 0;
    for t in TABS {
        let valid = match t {
            Tab::Minecraft | Tab::Consola => mc,
            Tab::Gestion => managed,
            _ => true,
        };
        if !valid {
            continue;
        }
        let w = t.label().len() as u16;
        if x >= x0 && x < x0 + w {
            return Some(t);
        }
        x0 += w + 1;
    }
    None
}

fn draw_detail(f: &mut Frame, a: &App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    draw_tabs(f, a, rows[0]);
    let body = Block::default().borders(Borders::TOP).border_style(Style::default().fg(BORDER));
    let inner = body.inner(rows[1]);
    f.render_widget(body, rows[1]);
    match a.tab {
        Tab::Overview => draw_overview(f, a, inner),
        Tab::Recursos => draw_recursos(f, a, inner),
        Tab::Logs => draw_logs(f, a, inner),
        Tab::Minecraft => draw_minecraft(f, a, inner),
        Tab::Gestion => draw_gestion(f, a, inner),
        Tab::Archivos => draw_archivos(f, a, inner),
        Tab::Consola => draw_consola(f, a, inner),
    }
}

fn draw_tabs(f: &mut Frame, a: &App, area: Rect) {
    let Some(s) = a.sel() else { return };
    let mc = s.is_mc();
    let managed = s.is_managed();
    let mut x = area.x;
    for t in TABS {
        let valid = match t {
            Tab::Minecraft | Tab::Consola => mc,
            Tab::Gestion => managed,
            _ => true,
        };
        let label = t.label();
        if !valid {
            continue;
        }
        let w = label.len() as u16;
        if x + w > area.x + area.width {
            break;
        }
        let style = if t == a.tab {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        f.render_widget(Paragraph::new(Span::styled(label, style)), Rect::new(x, area.y, w, 1));
        x += w + 1;
    }
}

fn draw_overview(f: &mut Frame, a: &App, area: Rect) {
    let Some(s) = a.sel() else { return };
    let m = &s.metrics;
    let lines = vec![
        Line::from(vec![Span::styled("estado   ", dim()), status_span(s.status)]),
        Line::from(vec![
            Span::styled("cpu      ", dim()),
            Span::styled(format!("{:.1} %", m.cpu_percent), Style::default().fg(if m.cpu_percent > 85.0 { RED } else { FG })),
        ]),
        Line::from(vec![
            Span::styled("memoria  ", dim()),
            Span::styled(format!("{} / {} MB ({:.0} %)", m.mem_used_mb, m.mem_total_mb, m.mem_pct()), Style::default().fg(FG)),
        ]),
        Line::from(vec![Span::styled("uptime   ", dim()), Span::styled(m.uptime_str.clone(), Style::default().fg(FG))]),
        Line::from(""),
        Line::from(vec![Span::styled("error    ", dim()), Span::styled(m.error.clone(), Style::default().fg(RED))]),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_recursos(f: &mut Frame, a: &App, area: Rect) {
    let Some(s) = a.sel() else { return };
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(6),
        Constraint::Min(1),
    ])
    .split(area);
    let cpu = s.metrics.cpu_percent.clamp(0.0, 100.0);
    let mem = s.metrics.mem_pct().clamp(0.0, 100.0);
    f.render_widget(gauge("cpu", cpu, if cpu > 85.0 { RED } else { ACCENT }), rows[0]);
    f.render_widget(gauge("mem", mem, if mem > 90.0 { RED } else { BLUE }), rows[1]);
    f.render_widget(
        Sparkline::default()
            .data(&s.cpu_hist.iter().copied().collect::<Vec<u64>>())
            .style(Style::default().fg(ACCENT))
            .block(panel(false).title(Span::styled(" cpu — últimos 2 min ", dim()))),
        rows[2],
    );
    f.render_widget(
        Sparkline::default()
            .data(&s.mem_hist.iter().copied().collect::<Vec<u64>>())
            .style(Style::default().fg(BLUE))
            .block(panel(false).title(Span::styled(" mem — últimos 2 min ", dim()))),
        rows[3],
    );
}

fn gauge(label: &str, pct: f32, color: ratatui::style::Color) -> Gauge<'static> {
    Gauge::default()
        .label(format!("{label} {:>5.1}%", pct))
        .ratio((pct / 100.0) as f64)
        .gauge_style(Style::default().fg(color).bg(BG_PANEL))
}

fn line_style(l: &str) -> Style {
    if l.contains("ERROR]") || l.contains("error:") {
        Style::default().fg(RED)
    } else if l.contains("WARN]") || l.contains("warn:") {
        Style::default().fg(YELLOW)
    } else if l.contains("joined the game") {
        Style::default().fg(GREEN)
    } else if l.contains("left the game") {
        Style::default().fg(FG_DIM)
    } else if l.contains("TPS from") || l.contains("MSPT from") || l.contains("Average tick") {
        Style::default().fg(BLUE)
    } else if l.starts_with("[aeroPanel]") {
        accent()
    } else {
        Style::default().fg(FG)
    }
}

fn draw_logs(f: &mut Frame, a: &App, area: Rect) {
    let Some(s) = a.sel() else { return };
    let len = s.log_lines.len() as u16;
    let show = len.min(area.height.max(1));
    let start = s.scroll.min(len.saturating_sub(show));
    let lines: Vec<Line> = s
        .log_lines
        .iter()
        .skip(start as usize)
        .take(show as usize)
        .map(|l| Line::from(Span::styled(l.clone(), line_style(l))))
        .collect();
    let follow = s.scroll == u16::MAX || s.scroll >= len.saturating_sub(show).saturating_sub(2);
    let title = if follow { " logs (siguiendo ↓) " } else { " logs (j/k · g = seguir) " };
    f.render_widget(
        Paragraph::new(lines).block(panel(false).title(Span::styled(title, dim()))),
        area,
    );
}

fn draw_minecraft(f: &mut Frame, a: &App, area: Rect) {
    let Some(s) = a.sel() else { return };
    let h = &s.health;
    let stability = |tps: Option<f64>| -> (&'static str, ratatui::style::Color) {
        match tps {
            Some(v) if v >= 19.0 => ("estable", GREEN),
            Some(v) if v >= 15.0 => ("parcial", YELLOW),
            Some(_) => ("inestable", RED),
            None => ("—", FG_FAINT),
        }
    };
    let (st_label, st_color) = stability(h.worst_tps());
    let players: Vec<Line> = if h.players.is_empty() {
        vec![Line::from(Span::styled("  (nadie conectado)", dim()))]
    } else {
        h.players.iter().map(|p| Line::from(Span::styled(format!("  * {p}"), Style::default().fg(GREEN)))).collect()
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("tps 1m/5m/15m  ", dim()),
            Span::styled(
                format!(
                    "{}/{}/{}",
                    h.tps_1m.map(|v| format!("{v:.2}")).unwrap_or("—".into()),
                    h.tps_5m.map(|v| format!("{v:.2}")).unwrap_or("—".into()),
                    h.tps_15m.map(|v| format!("{v:.2}")).unwrap_or("—".into())
                ),
                Style::default().fg(FG),
            ),
        ]),
        Line::from(vec![
            Span::styled("mspt           ", dim()),
            Span::styled(h.mspt.map(|v| format!("{v:.1} ms")).unwrap_or_else(|| "—".into()), Style::default().fg(FG)),
        ]),
        Line::from(vec![
            Span::styled("estabilidad    ", dim()),
            Span::styled(format!("* {st_label}"), Style::default().fg(st_color)),
        ]),
        Line::from(vec![
            Span::styled("errores vistos ", dim()),
            Span::styled(format!("{}", h.errors_seen), Style::default().fg(if h.errors_seen > 0 { RED } else { FG })),
        ]),
        Line::from(""),
        Line::from(Span::styled("jugadores", dim())),
    ];
    lines.extend(players);
    if let Some(e) = &h.last_error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!("último error: {e}"), Style::default().fg(RED))));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_gestion(f: &mut Frame, a: &App, area: Rect) {
    let Some(s) = a.sel() else { return };
    let mc = s.cfg.mc_cfg();
    let restarting = a
        .restart_at
        .map(|(i, _)| i == a.selected)
        .unwrap_or(false);
    let mut lines = vec![
        Line::from(vec![Span::styled("instancia  ", dim()), Span::styled(
            mc.map(|m| m.instance_dir.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| "—".into()),
            Style::default().fg(FG),
        )]),
        Line::from(vec![Span::styled("versión    ", dim()), Span::styled(mc.map(|m| m.version.clone()).unwrap_or_default(), Style::default().fg(FG))]),
        Line::from(vec![Span::styled("ram        ", dim()), Span::styled(mc.map(|m| m.memory.clone()).unwrap_or_default(), Style::default().fg(FG))]),
        Line::from(vec![Span::styled("auto_start ", dim()), Span::styled(
            if mc.map(|m| m.auto_start).unwrap_or(false) { "sí" } else { "no" },
            Style::default().fg(FG),
        )]),
        Line::from(vec![Span::styled("proceso    ", dim()), status_span(s.status)]),
        Line::from(""),
    ];
    if restarting {
        lines.push(Line::from(Span::styled("~ reiniciando…", Style::default().fg(BLUE))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  S  iniciar    X  detener    R  reiniciar", accent())));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_archivos(f: &mut Frame, a: &App, area: Rect) {
    if a.files_loading {
        f.render_widget(Paragraph::new(Span::styled(format!("{} cargando…", SPINNER[a.spinner_frame % SPINNER.len()]), dim())), area);
        return;
    }
    // reveal animation: items aparecen progresivamente al abrir una carpeta
    let visible = a.files_reveal.min(a.files.len());
    let items: Vec<ListItem> = a
        .files
        .iter()
        .enumerate()
        .take(visible)
        .map(|(i, (name, is_dir, size))| {
            let icon = if *is_dir { ">" } else { " " };
            let style = if *is_dir { Style::default().fg(BLUE) } else { Style::default().fg(FG) };
            let sel = i == a.files_sel;
            let name_style = if sel { style.add_modifier(Modifier::BOLD) } else { style };
            // últimos items revelados entran atenuados (fade-in)
            let fresh = visible.saturating_sub(i) <= 2;
            let base = if fresh { Style::default().fg(FG_DIM) } else { Style::default() };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{icon} "), if fresh { base } else { Style::default().fg(if *is_dir { ACCENT } else { FG_FAINT }) }),
                Span::styled(format!("{name:<40}"), name_style.patch(base)),
                Span::styled(size.clone(), dim().patch(base)),
            ]))
        })
        .collect();
    // gestionamos el scroll manualmente para poder mapear clicks del mouse
    let h = area.height.saturating_sub(2) as usize; // filas útiles dentro del panel
    let mut offset = a.files_offset.get().min(a.files.len().saturating_sub(1));
    if a.files_sel < offset {
        offset = a.files_sel;
    } else if a.files_sel >= offset + h && h > 0 {
        offset = a.files_sel + 1 - h;
    }
    a.files_offset.set(offset);
    // breadcrumb de la ruta: cada segmento es clickable con el mouse
    let path = if a.files_dir.is_empty() { "." } else { &a.files_dir };
    let mut spans: Vec<Span> = Vec::new();
    let mut segs: Vec<(String, u16, u16)> = Vec::new();
    let mut col: u16 = 1; // +1: el título arranca tras el borde izquierdo del panel
    let title_x = area.x;
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    for (i, p) in parts.iter().enumerate() {
        let cur = if *p == "." || i == parts.len() - 1 { accent() } else { Style::default().fg(BLUE) };
        let label = if i == parts.len() - 1 { p.to_string() } else { format!("{p}/") };
        let start = title_x + col;
        spans.push(Span::styled(label.clone(), cur));
        col += label.chars().count() as u16;
        segs.push((parts[..=i].join("/"), start, title_x + col));
    }
    FILES_SEGS.lock().unwrap().clone_from(&segs);
    let list = List::new(items)
        .block(
            panel(false).title(Line::from(spans)),
        )
        .highlight_style(Style::default().bg(BG_PANEL));
    let mut st = ListState::default()
        .with_selected(Some(a.files_sel.min(a.files.len().saturating_sub(1))))
        .with_offset(offset);
    f.render_stateful_widget(list, area, &mut st);
    if visible < a.files.len() {
        f.render_widget(
            Paragraph::new(Span::styled(format!("{} desplegando…", SPINNER[a.spinner_frame % SPINNER.len()]), dim()))
                .alignment(Alignment::Right),
            Rect::new(area.x, area.y + area.height.saturating_sub(1), area.width, 1),
        );
    }
}

fn draw_consola(f: &mut Frame, a: &App, area: Rect) {
    let Some(s) = a.sel() else { return };
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let show = rows[0].height;
    let start = s.log_lines.len().saturating_sub(show as usize);
    let lines: Vec<Line> = s
        .log_lines
        .iter()
        .skip(start)
        .map(|l| Line::from(Span::styled(l.clone(), line_style(l))))
        .collect();
    f.render_widget(Paragraph::new(lines), rows[0]);
    let (prompt, input, cursor) = match a.input_mode {
        InputMode::ConsoleInput => ("> ", a.console_input.clone(), Style::default().fg(ACCENT)),
        _ => ("  ", "(i para escribir · ↑↓ historial)".to_string(), dim()),
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(prompt, accent()), Span::styled(input, cursor)])),
        rows[1],
    );
}

fn draw_footer(f: &mut Frame, a: &App, area: Rect) {
    let hint = match (a.screen, a.input_mode) {
        (_, InputMode::ConsoleInput) => " enter: enviar · esc: volver ",
        (_, InputMode::FormAdd) | (_, InputMode::FormInstance) => " tab: sig. campo · enter: guardar · esc: cancelar ",
        (_, InputMode::UploadPath) | (_, InputMode::DownloadPath) => " enter: confirmar · esc: cancelar ",
        (Screen::Dashboard, _) => " j/k: moverse · enter: abrir · n: nueva instancia · a: añadir servidor · d: borrar · ?: ayuda · q: salir ",
        (Screen::Detail, _) => match a.tab {
            Tab::Logs => " j/k: scroll · g: seguir · tab: pestaña · esc: volver ",
            Tab::Archivos => " j/k: moverse · enter: abrir · click en la ruta para saltar · v: editar · o: abrir en explorador · u: subir · p: subir archivo · e: descargar · r: refrescar ",
            Tab::Gestion => " S: iniciar · X: detener · R: reiniciar · tab: pestaña · esc: volver ",
            Tab::Consola => " i: escribir · esc: volver ",
            _ => " tab: pestaña · esc: volver · ?: ayuda ",
        },
    };
    f.render_widget(Paragraph::new(Span::styled(hint, Style::default().bg(BG_PANEL).fg(FG_DIM))), area);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let w = 58.min(area.width.saturating_sub(4));
    let h = 20.min(area.height.saturating_sub(4));
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    let box_area = Rect::new(x, y, w, h);
    f.render_widget(Clear, box_area);
    let lines = vec![
        Line::from(Span::styled("atajos", accent())),
        Line::from(""),
        Line::from(vec![Span::styled(" j/k ", accent()), Span::styled("  moverse", dim())]),
        Line::from(vec![Span::styled(" enter ", accent()), Span::styled("abrir servidor / entrar a carpeta", dim())]),
        Line::from(vec![Span::styled(" u ", accent()), Span::styled("    subir un nivel en archivos", dim())]),
        Line::from(vec![Span::styled(" v ", accent()), Span::styled("    editar un archivo (editor en vivo)", dim())]),
        Line::from(vec![Span::styled(" o ", accent()), Span::styled("    abrir carpeta en el explorador", dim())]),
        Line::from(vec![Span::styled(" n ", accent()), Span::styled("    nueva instancia de minecraft (paper)", dim())]),
        Line::from(vec![Span::styled(" a ", accent()), Span::styled("    añadir servidor (local/ssh)", dim())]),
        Line::from(vec![Span::styled(" d ", accent()), Span::styled("    borrar servidor de la lista", dim())]),
        Line::from(vec![Span::styled(" S/X/R ", accent()), Span::styled(" iniciar / detener / reiniciar instancia", dim())]),
        Line::from(vec![Span::styled(" p/e ", accent()), Span::styled("  subir / descargar archivo", dim())]),
        Line::from(vec![Span::styled(" i ", accent()), Span::styled("    escribir en la consola del servidor", dim())]),
        Line::from(vec![Span::styled(" mouse ", accent()), Span::styled(" clic en pestañas/listas · rueda scrollea", dim())]),
        Line::from(vec![Span::styled(" ? ", accent()), Span::styled("    esta ayuda", dim())]),
        Line::from(vec![Span::styled(" q ", accent()), Span::styled("    salir", dim())]),
        Line::from(""),
        Line::from(Span::styled("cualquier tecla cierra", dim())),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .block(panel(true).title(Span::styled(" aeroPanel ", accent())))
            .wrap(Wrap { trim: false }),
        box_area,
    );
}

fn draw_editor(f: &mut Frame, a: &App, area: Rect) {
    let Some(ed) = a.editor.as_ref() else { return };
    let inner_area = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), area.height.saturating_sub(2));
    f.render_widget(Clear, area);
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner_area);
    if ed.loading {
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("{} cargando {}…", SPINNER[a.spinner_frame % SPINNER.len()], ed.path),
                dim(),
            )),
            inner_area,
        );
        return;
    }
    let h = rows[0].height as usize;
    // keep the cursor line visible (purely computed)
    let vs = ed.row.saturating_sub(h.saturating_sub(1));
    let lines: Vec<Line> = ed
        .lines
        .iter()
        .enumerate()
        .skip(vs)
        .take(h)
        .map(|(i, l)| {
            let n = (i + 1).to_string();
            let gutter = Span::styled(format!("{n:>4} "), dim());
            if i == ed.row {
                let col = ed.col.min(l.chars().count());
                let head: Span = Span::styled(l.chars().take(col).collect::<String>(), Style::default().fg(FG));
                let cur: Span = Span::styled(l.chars().skip(col).take(1).collect::<String>(), Style::default().fg(BG).bg(ACCENT));
                let tail: Span = Span::styled(l.chars().skip(col + 1).collect::<String>(), Style::default().fg(FG));
                let mut spans = vec![gutter, head, cur];
                if col == l.chars().count() {
                    spans[2] = Span::styled(" ", Style::default().fg(BG).bg(ACCENT));
                }
                spans.push(tail);
                Line::from(spans)
            } else {
                Line::from(vec![gutter, Span::styled(l.clone(), Style::default().fg(FG))])
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(
            panel(true)
                .title(Span::styled(ed.title(), accent()))
                .style(Style::default().bg(BG)),
        ),
        inner_area,
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            " ctrl+s: guardar · autosave 2s · esc: cerrar · flechas: mover",
            dim(),
        )),
        rows[1],
    );
}

fn draw_form(f: &mut Frame, a: &App, title: &str, fields: &[&str], _mask: bool) {
    let w = 62.min(f.area().width.saturating_sub(4));
    let h = (fields.len() + 4) as u16;
    let x = f.area().x + (f.area().width - w) / 2;
    let y = f.area().y + (f.area().height - h) / 2;
    let area = Rect::new(x, y, w, h);
    f.render_widget(Clear, area);
    let mut lines = vec![Line::from("")];
    for (i, name) in fields.iter().enumerate() {
        let val = a.form.fields.get(i).cloned().unwrap_or_default();
        let active = i == a.form.active;
        let cursor = if active { "▌" } else { "" };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:<18}", name),
                if active { accent() } else { dim() },
            ),
            Span::styled(val, Style::default().fg(FG)),
            Span::styled(cursor, Style::default().fg(ACCENT)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" tab: siguiente · enter: guardar · esc: cancelar", dim())));
    f.render_widget(
        Paragraph::new(lines)
            .block(panel(true).title(Span::styled(format!(" {title} "), accent()))),
        area,
    );
}

fn draw_input(f: &mut Frame, title: &str, val: &str) {
    let area = centered_rect(f.area(), 62, 5);
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from(Span::styled(title, dim())),
        Line::from(vec![Span::styled("> ", accent()), Span::styled(format!("{val}▌"), Style::default().fg(FG))]),
    ];
    f.render_widget(
        Paragraph::new(lines).block(panel(true).title(Span::styled(" ruta ", accent()))),
        area,
    );
}

fn centered_rect(total: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(total.width.saturating_sub(4));
    let h = h.min(total.height.saturating_sub(2));
    Rect::new(total.x + (total.width - w) / 2, total.y + (total.height - h) / 2, w, h)
}
