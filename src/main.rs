#![allow(dead_code)]
//! aeroPanel entry point — event loop + background pollers.

mod app;
mod config;
mod connection;
mod daemon;
mod instances;
mod metrics;
mod minecraft;
mod notify;
mod theme;
mod tunnel;
mod ui;

use app::{App, Form, InputMode, Screen, Status, Tab};
use connection::{console_send, Target};
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--daemon") {
        return daemon::run().await;
    }
    let cfg_path = config::config_path();
    let cfg = config::load()?;
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<(usize, String)>();
    let app = Arc::new(tokio::sync::Mutex::new(App::new(cfg, cfg_path.clone(), line_tx.clone())));

    // terminal setup
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    crossterm::execute!(stdout, crossterm::event::EnableMouseCapture)?;
    crossterm::terminal::enable_raw_mode()?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    // cliente del daemon (si hay socket activo): todas las operaciones de
    // ciclo de vida van por él, así las instancias viven aunque se cierre
    // la TUI
    {
        let sock = daemon::socket_path();
        if sock.exists() {
            if let Ok(stream) = tokio::net::UnixStream::connect(&sock).await {
                let (r, mut w) = tokio::io::split(stream);
                let (dtx, mut drx) = tokio::sync::mpsc::unbounded_channel::<String>();
                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    while let Some(s) = drx.recv().await {
                        if w.write_all(format!("{s}\n").as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
                let app2 = app.clone();
                let ltx = line_tx.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let mut lines = tokio::io::BufReader::new(r).lines();
                    while let Ok(Some(l)) = lines.next_line().await {
                        let ev: serde_json::Value = match serde_json::from_str(&l) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let mut a = app2.lock().await;
                        match ev["ev"].as_str() {
                            Some("line") => {
                                if let Some(idx) = a.server_index(ev["server"].as_str().unwrap_or("")) {
                                    let _ = ltx.send((idx, ev["text"].as_str().unwrap_or("").to_string()));
                                }
                            }
                            Some("status") => {
                                if let Some(s) = a.servers.iter_mut().find(|s| s.cfg.name == ev["server"].as_str().unwrap_or("")) {
                                    s.status = if ev["status"].as_str() == Some("running") {
                                        Status::Running
                                    } else {
                                        Status::Down
                                    };
                                }
                            }
                            Some("msg") | Some("backup") => {
                                let text = if ev["ev"] == "backup" {
                                    format!("[aeroPanel] backup listo: {}", ev["path"].as_str().unwrap_or(""))
                                } else {
                                    ev["msg"].as_str().unwrap_or("").to_string()
                                };
                                let idx = a.selected;
                                let _ = a.line_tx.send((idx, text));
                            }
                            _ => {}
                        }
                    }
                });
                let mut a = app.lock().await;
                a.daemon_tx = Some(dtx);
                let _ = a.line_tx.send((0, "[aeroPanel] daemon conectado — las instancias viven fuera de la TUI".into()));
            }
        }
    }

    // background: metrics pollers
    {
        let targets: Vec<(usize, Target, u64)> = {
            let a = app.lock().await;
            a.servers
                .iter()
                .enumerate()
                .map(|(i, s)| (i, s.target.clone(), a.cfg_secs()))
                .collect()
        };
        for (idx, t, refresh) in targets {
            let app = app.clone();
            tokio::spawn(async move {
                loop {
                    match t.exec(metrics::PROBE).await {
                        Ok(out) => {
                            let mut m = metrics::parse(&out);
                            metrics::refine_cpu(&t, &mut m).await;
                            app.lock().await.record_metrics(idx, m);
                        }
                        Err(e) => {
                            app.lock().await.mark_down(idx, e.to_string());
                            tokio::time::sleep(std::time::Duration::from_secs(refresh.max(5))).await;
                            continue;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(refresh)).await;
                }
            });
        }
    }

    // background: auto-start managed instances
    {
        let a = app.lock().await;
        let autos: Vec<usize> = a
            .servers
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_managed() && s.cfg.mc_cfg().map(|m| m.auto_start).unwrap_or(false))
            .map(|(i, _)| i)
            .collect();
        drop(a);
        // con daemon conectado el auto-start lo hace el propio daemon
        if app.lock().await.daemon_tx.is_none() {
            for idx in autos {
                app.lock().await.start_instance(idx);
            }
        }
    }

    // background: external console/log streams for non-managed mc servers
    {
        let a = app.lock().await;
        let mc_servers: Vec<(usize, Target, crate::config::MinecraftCfg)> = a
            .servers
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_mc() && !s.is_managed())
            .map(|(i, s)| (i, s.target.clone(), s.cfg.mc_cfg().unwrap().clone()))
            .collect();
        drop(a);

        for (idx, t, mc) in mc_servers {
            let app2 = app.clone();
            let stream_cmd = if !mc.log_command.is_empty() {
                mc.log_command.clone()
            } else if !mc.console_command.is_empty() {
                mc.console_command.clone()
            } else {
                String::new()
            };
            let is_console_stream = !mc.console_command.is_empty() && stream_cmd == mc.console_command;
            let tx = line_tx.clone();

            if !stream_cmd.is_empty() {
                tokio::spawn(async move {
                    match t.spawn_stream(&stream_cmd).await {
                        Ok((mut child, reader)) => {
                            if is_console_stream {
                                if let Some(stdin) = Target::take_stdin(&mut child) {
                                    let slot = {
                                        let a = app2.lock().await;
                                        a.servers[idx].console_stdin.clone()
                                    };
                                    if let Some(slot) = slot {
                                        *slot.lock().await = Some(stdin);
                                    }
                                }
                            }
                            connection::spawn_line_reader(reader, tx, idx);
                            let _ = child.wait().await;
                        }
                        Err(e) => {
                            let _ = tx.send((idx, format!("[aeroPanel] stream error: {e}")));
                        }
                    }
                });
            }

            // TPS poller
            if mc.send_tps_cmd && is_console_stream {
                let app2 = app.clone();
                let interval = mc.tps_interval_secs;
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                        let a = app2.lock().await;
                        let Some(slot) = a.servers[idx].console_stdin.clone() else { continue };
                        let mut guard = slot.lock().await;
                        if let Some(stdin) = guard.as_mut() {
                            let _ = console_send(stdin, "tps").await;
                        }
                    }
                });
            }
        }
    }

    // main loop
    let mut events = crossterm::event::EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(120));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    while !app.lock().await.quit {
        tokio::select! {
            _ = ticker.tick() => {
                {
                    let mut a = app.lock().await;
                    a.spinner_frame = a.spinner_frame.wrapping_add(1);
                    // restart timer
                    if let Some((_idx, at)) = a.restart_at {
                        if std::time::Instant::now() >= at {
                            a.restart_at = None;
                            let name = a.sel().map(|s| s.cfg.name.clone()).unwrap_or_default();
                            let sel = a.selected;
                            if !daemon_send(&a, &name, "start", serde_json::json!({})) {
                                a.start_instance(sel);
                            }
                        }
                    }
                    // reveal animation for the files tab
                    if a.screen == Screen::Detail && a.tab == Tab::Archivos && a.input_mode == InputMode::Normal {
                        let want = a.files.len();
                        if a.files_reveal < want {
                            a.files_reveal = (a.files_reveal + 2).min(want);
                        }
                    }
                    // live editor autosave (2s after last edit)
                    let due = a.input_mode == InputMode::Editor
                        && a.editor.as_ref().map(|ed| {
                            ed.dirty && !ed.saving && !ed.loading
                                && ed.last_edit.map(|t| t.elapsed() > std::time::Duration::from_secs(2)).unwrap_or(false)
                        }).unwrap_or(false);
                    if due {
                        drop(a);
                        save_editor(app.clone()).await;
                        continue;
                    }
                }
                let a = app.lock().await;
                terminal.draw(|f| ui::draw(f, &a))?;
                drop(a);
            }
            Some(ev) = events.next() => {
                match ev {
                    Ok(crossterm::event::Event::Key(k)) if k.kind == crossterm::event::KeyEventKind::Press => {
                        handle_key(app.clone(), k).await;
                    }
                    Ok(crossterm::event::Event::Mouse(m)) => handle_mouse(app.clone(), m).await,
                    _ => {}
                }
            }
            Some((idx, line)) = line_rx.recv() => {
                app.lock().await.ingest_line(idx, line);
            }
        }
    }

    // teardown
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    Ok(())
}

/// mouse: scroll = mover selección, click = seleccionar (2º click abre)
async fn save_editor(app: Arc<tokio::sync::Mutex<App>>) {
    let (idx, target, path, content) = {
        let mut a = app.lock().await;
        let Some(ed) = a.editor.as_mut() else { return };
        if !ed.dirty || ed.saving || ed.loading {
            return;
        }
        ed.saving = true;
        let path = ed.path.clone();
        let content = ed.lines.join("\n");
        let idx = a.selected;
        let target = a.sel().map(|s| s.target.clone());
        (idx, target, path, content)
    };
    let tx = app.lock().await.line_tx.clone();
    let app2 = app.clone();
    tokio::spawn(async move {
        let r = async {
            let t = target.ok_or_else(|| anyhow::anyhow!("sin servidor"))?;
            connection::write_file(&t, &path, &content).await
        }.await;
        let mut a = app2.lock().await;
        if let Some(ed) = a.editor.as_mut() {
            ed.saving = false;
            match r {
                Ok(()) => {
                    ed.dirty = false;
                    let _ = tx.send((idx, format!("[aeroPanel] guardado {path}")));
                }
                Err(e) => {
                    let _ = tx.send((idx, format!("[aeroPanel] error guardando: {e}")));
                }
            }
        }
    });
}

async fn handle_mouse(app: Arc<tokio::sync::Mutex<App>>, m: crossterm::event::MouseEvent) {
    use crossterm::event::{MouseButton, MouseEventKind as MK};
    let (cx, cy) = (m.column, m.row);
    let mut a = app.lock().await;
    match m.kind {
        MK::ScrollUp => match (a.screen, a.tab) {
            (Screen::Dashboard, _) => a.selected = a.selected.saturating_sub(1),
            (Screen::Detail, Tab::Logs) => scroll_sel(&mut a, -3),
            (Screen::Detail, Tab::Archivos) => a.files_sel = a.files_sel.saturating_sub(1),
            _ => {}
        },
        MK::ScrollDown => match (a.screen, a.tab) {
            (Screen::Dashboard, _) => {
                if a.selected + 1 < a.servers.len() {
                    a.selected += 1;
                }
            }
            (Screen::Detail, Tab::Logs) => scroll_sel(&mut a, 3),
            (Screen::Detail, Tab::Archivos) => {
                a.files_sel = (a.files_sel + 1).min(a.files.len().saturating_sub(1));
            }
            _ => {}
        },
        MK::Down(MouseButton::Left) => match a.screen {
            Screen::Dashboard => {
                // lista en el panel izquierdo (46% del body): items desde y=4
                let list_w = ((f_width(&a) * 46) / 100).max(20);
                if cx < list_w && cy >= 4 {
                    let row = (cy - 4) as usize;
                    if row < a.servers.len() {
                        if row == a.selected {
                            a.screen = Screen::Detail;
                            a.tab = Tab::Overview;
                        } else {
                            a.selected = row;
                        }
                    }
                }
            }
            Screen::Detail => {
                if cy == 3 {
                    if let Some(t) = crate::ui::hit_tab(&a, cx) {
                        a.tab = t;
                        if t == Tab::Archivos {
                            drop(a);
                            load_files(app).await;
                        }
                    }
                } else if a.tab == Tab::Archivos {
                    // fila 5 = borde superior del panel = breadcrumb de la ruta
                    if cy == 5 {
                        let dir = a.files_dir.clone();
                        if let Some(dest) = ui::breadcrumb_hit(cx) {
                            if dest != dir {
                                a.files_dir = dest;
                                drop(a);
                                load_files(app).await;
                                return;
                            }
                        }
                    }
                    // el panel tiene borde en y=5; los items arrancan en y=6
                    if cy >= 6 {
                    let reveal = a.files_reveal.min(a.files.len());
                    let row = (cy - 6) as usize + a.files_offset.get();
                    if row < reveal {
                        if row == a.files_sel {
                            if let Some((name, true, _)) = a.files.get(row).cloned() {
                                let dir = a.files_dir.clone();
                                a.files_dir = join_dir(&dir, &name);
                                drop(a);
                                load_files(app).await;
                            }
                        } else {
                            a.files_sel = row;
                        }
                    }
                }
                }
            }
        },
        _ => {}
    }
}

fn f_width(_a: &App) -> u16 {
    crate::ui::term_width()
}

async fn handle_key(app: Arc<tokio::sync::Mutex<App>>, k: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode as K;
    let mut a = app.lock().await;
    let code = k.code;

    // help overlay: any key closes
    if a.help {
        a.help = false;
        return;
    }

    match a.input_mode {
        InputMode::Normal => match (a.screen, code) {
            (_, K::Char('?')) => a.help = true,
            (_, K::Char('q')) => a.quit = true,
            (Screen::Dashboard, K::Char('j') | K::Down) => {
                let n = a.servers.len().max(1);
                a.selected = (a.selected + 1) % n;
            }
            (Screen::Dashboard, K::Char('k') | K::Up) => {
                let n = a.servers.len().max(1);
                a.selected = (a.selected + n - 1) % n;
            }
            (Screen::Dashboard, K::Enter) if !a.servers.is_empty() => {
                a.screen = Screen::Detail;
                a.tab = Tab::Overview;
            }
            (Screen::Dashboard, K::Char('a')) => {
                a.input_mode = InputMode::FormAdd;
                a.form = Form { fields: vec![String::new(); 7], active: 0 };
            }
            (Screen::Dashboard, K::Char('n')) => {
                a.input_mode = InputMode::FormInstance;
                a.form = Form {
                    fields: vec![String::new(), "1.21.4".into(), "4G".into(), "n".into()],
                    active: 0,
                };
            }
            (Screen::Dashboard, K::Char('d')) if !a.servers.is_empty() => {
                let idx = a.selected;
                a.servers.remove(idx);
                if a.selected >= a.servers.len() {
                    a.selected = a.servers.len().saturating_sub(1);
                }
                let _ = a.save_config();
            }
            (Screen::Detail, K::Esc) => {
                a.screen = Screen::Dashboard;
                a.input_mode = InputMode::Normal;
            }
            (Screen::Detail, K::Tab | K::Right | K::Char('l')) => {
                let valid = valid_tabs(&a);
                let cur = valid.iter().position(|t| *t == a.tab).unwrap_or(0);
                a.tab = valid[(cur + 1) % valid.len()];
                if a.tab == Tab::Archivos {
                    drop(a);
                    load_files(app).await;
                }
            }
            (Screen::Detail, K::Left | K::Char('h')) => {
                let valid = valid_tabs(&a);
                let cur = valid.iter().position(|t| *t == a.tab).unwrap_or(0);
                a.tab = valid[(cur + valid.len() - 1) % valid.len()];
                if a.tab == Tab::Archivos {
                    drop(a);
                    load_files(app).await;
                }
            }
            (Screen::Detail, K::Char('i')) if a.tab == Tab::Consola => {
                a.input_mode = InputMode::ConsoleInput;
            }
            // gestión de jugadores en la pestaña Minecraft
            (Screen::Detail, K::Char('j')) if a.tab == Tab::Minecraft => {
                let n = a.sel().map(|s| s.health.online.len()).unwrap_or(0);
                if n > 0 { a.mc_player_sel = (a.mc_player_sel + 1).min(n - 1); }
            }
            (Screen::Detail, K::Char('k')) if a.tab == Tab::Minecraft => {
                a.mc_player_sel = a.mc_player_sel.saturating_sub(1);
            }
            (Screen::Detail, K::Char('K')) if a.tab == Tab::Minecraft => {
                let (cmd, p) = (format!("kick {}", a.mc_target()), a.mc_target().clone());
                if !p.is_empty() { a.mc_player_sel = 0; drop(a); send_console(app, cmd).await; }
            }
            (Screen::Detail, K::Char('B')) if a.tab == Tab::Minecraft => {
                let (cmd, p) = (format!("ban {}", a.mc_target()), a.mc_target().clone());
                if !p.is_empty() { a.mc_player_sel = 0; drop(a); send_console(app, cmd).await; }
            }
            (Screen::Detail, K::Char('o')) if a.tab == Tab::Minecraft => {
                let (cmd, p) = (format!("op {}", a.mc_target()), a.mc_target().clone());
                if !p.is_empty() { drop(a); send_console(app, cmd).await; }
            }
            (Screen::Detail, K::Char('j')) if a.tab == Tab::Logs => scroll_sel(&mut a, 3),
            (Screen::Detail, K::Char('k')) if a.tab == Tab::Logs => scroll_sel(&mut a, -3),
            (Screen::Detail, K::Char('g')) if a.tab == Tab::Logs => a.sel_mut().map(|s| s.scroll = u16::MAX).unwrap_or(()),
            // gestión: start/stop/restart (+ backup por el daemon)
            (Screen::Detail, K::Char('S')) if a.tab == Tab::Gestion => {
                let sel = a.selected;
                let name = a.sel().map(|s| s.cfg.name.clone()).unwrap_or_default();
                if !daemon_send(&a, &name, "start", serde_json::json!({})) {
                    a.start_instance(sel);
                }
            }
            (Screen::Detail, K::Char('X')) if a.tab == Tab::Gestion => {
                let sel = a.selected;
                let name = a.sel().map(|s| s.cfg.name.clone()).unwrap_or_default();
                if !daemon_send(&a, &name, "stop", serde_json::json!({})) {
                    a.stop_instance(sel, 3);
                }
            }
            (Screen::Detail, K::Char('R')) if a.tab == Tab::Gestion => {
                let sel = a.selected;
                let name = a.sel().map(|s| s.cfg.name.clone()).unwrap_or_default();
                if !daemon_send(&a, &name, "restart", serde_json::json!({})) {
                    a.restart_instance(sel);
                }
            }
            (Screen::Detail, K::Char('B')) if a.tab == Tab::Gestion => {
                let name = a.sel().map(|s| s.cfg.name.clone()).unwrap_or_default();
                if !daemon_send(&a, &name, "backup", serde_json::json!({})) {
                    let _ = a.line_tx.send((a.selected, "[aeroPanel] backup necesita el daemon (aeropanel --daemon)".into()));
                }
            }
            (Screen::Detail, K::Char('N')) if a.tab == Tab::Gestion => {
                let idx = a.selected;
                let tx = a.line_tx.clone();
                drop(a);
                tokio::spawn(async move {
                    match tunnel::net_info().await {
                        Ok((public, local, cgnat)) => {
                            let _ = tx.send((idx, format!("[aeroPanel] ip publica: {public}")));
                            let _ = tx.send((idx, format!("[aeroPanel] ip local:   {local}")));
                            if cgnat {
                                let _ = tx.send((idx, "[aeroPanel] ⚠ CGNAT detectado: abrir puertos NO funciona → usa un túnel (T)".into()));
                            } else {
                                let _ = tx.send((idx, "[aeroPanel] ip pública propia: puedes abrir el 25565 en el router".into()));
                            }
                        }
                        Err(e) => {
                            let _ = tx.send((idx, format!("[aeroPanel] {e}")));
                        }
                    }
                });
            }
            (Screen::Detail, K::Char('T')) if a.tab == Tab::Gestion => {
                let idx = a.selected;
                let tx = a.line_tx.clone();
                let child_slot = a.sel().and_then(|s| s.child.clone());
                drop(a);
                tokio::spawn(async move {
                    match tunnel::ensure_playit(idx, &tx).await {
                        Ok(mut child) => {
                            if let Some(out) = child.stdout.take() {
                                crate::connection::spawn_line_reader(
                                    tokio::io::BufReader::new(out),
                                    tx,
                                    idx,
                                );
                            }
                            let _ = child.wait().await;
                        }
                        Err(e) => {
                            let _ = tx.send((idx, format!("[aeroPanel] túnel: {e:#}")));
                        }
                    }
                });
                let _ = child_slot;
            }
            // archivos: navigation
            (Screen::Detail, K::Char('j') | K::Down) if a.tab == Tab::Archivos => {
                a.files_sel = (a.files_sel + 1).min(a.files.len().saturating_sub(1));
            }
            (Screen::Detail, K::Char('k') | K::Up) if a.tab == Tab::Archivos => {
                a.files_sel = a.files_sel.saturating_sub(1);
            }
            (Screen::Detail, K::Enter) if a.tab == Tab::Archivos => {
                let dir = a.files_dir.clone();
                if let Some((name, is_dir, _)) = a.files.get(a.files_sel) {
                    if *is_dir {
                        let newdir = join_dir(&dir, &name);
                        a.files_dir = newdir;
                        drop(a);
                        load_files(app).await;
                    }
                }
            }
            (Screen::Detail, K::Backspace | K::Char('u')) if a.tab == Tab::Archivos => {
                // go up
                let dir = a.files_dir.clone();
                let p = parent_of(&dir);
                let root = a.sel().map(|s| s.files_root()).unwrap_or_default();
                a.files_dir = if !root.is_empty() && p.len() < root.len() && p != "." && root != "." && root != "/" {
                    root
                } else {
                    p
                };
                drop(a);
                load_files(app).await;
            }
            (Screen::Detail, K::Char('p')) if a.tab == Tab::Archivos => {
                a.input_mode = InputMode::UploadPath;
                a.path_input.clear();
            }
            (Screen::Detail, K::Char('e')) if a.tab == Tab::Archivos => {
                a.input_mode = InputMode::DownloadPath;
                a.path_input.clear();
            }
            (Screen::Detail, K::Char('r')) if a.tab == Tab::Archivos => {
                drop(a);
                load_files(app).await;
            }
            (Screen::Detail, K::Char('o')) if a.tab == Tab::Archivos => {
                let (idx, target, path, tx) = (
                    a.selected,
                    a.sel().map(|s| s.target.clone()),
                    if a.files_dir.is_empty() { ".".to_string() } else { a.files_dir.clone() },
                    a.line_tx.clone(),
                );
                drop(a);
                tokio::spawn(async move {
                    match target {
                        Some(t) => match connection::open_desktop(&t, &path).await {
                            Ok(()) => {
                                let _ = tx.send((idx, format!("[aeroPanel] abriendo {path} en el explorador")));
                            }
                            Err(e) => {
                                let _ = tx.send((idx, format!("[aeroPanel] {e}")));
                            }
                        },
                        None => {}
                    }
                });
            }
            (Screen::Detail, K::Char('v')) if a.tab == Tab::Archivos => {
                let file = a.files.get(a.files_sel).filter(|(_, is_dir, _)| !*is_dir).map(|(n, _, _)| n.clone());
                if let Some(name) = file {
                    let dir = a.files_dir.clone();
                    let full = join_dir(&dir, &name);
                    a.input_mode = InputMode::Editor;
                    a.editor = Some(app::Editor {
                        path: full.clone(),
                        loading: true,
                        ..Default::default()
                    });
                    let idx = a.selected;
                    let target = a.sel().map(|s| s.target.clone());
                    drop(a);
                    tokio::spawn(async move {
                        let r = async {
                            let t = target.ok_or_else(|| anyhow::anyhow!("sin servidor"))?;
                            connection::read_file(&t, &full, 512_000).await
                        }.await;
                        let mut a = app.lock().await;
                        if a.input_mode == InputMode::Editor {
                            match r {
                                Ok(content) => {
                                    if let Some(ed) = a.editor.as_mut() {
                                        ed.lines = content.lines().map(|l| l.to_string()).collect();
                                        if ed.lines.is_empty() {
                                            ed.lines.push(String::new());
                                        }
                                        ed.loading = false;
                                    }
                                }
                                Err(e) => {
                                    a.input_mode = InputMode::Normal;
                                    a.editor = None;
                                    let _ = a.line_tx.send((idx, format!("[aeroPanel] no se pudo abrir: {e}")));
                                }
                            }
                        }
                    });
                }
            }
            _ => {}
        },
        InputMode::ConsoleInput => match code {
            K::Esc => a.input_mode = InputMode::Normal,
            K::Enter => {
                let cmd = a.console_input.clone();
                a.console_input.clear();
                if !cmd.trim().is_empty() {
                    a.console_history.push_back(cmd.clone());
                    if a.console_history.len() > 50 {
                        a.console_history.pop_front();
                    }
                    a.history_idx = a.console_history.len();
                }
                drop(a);
                send_console(app, cmd).await;
            }
            K::Up => {
                if a.history_idx > 0 {
                    a.history_idx -= 1;
                    a.console_input = a.console_history.get(a.history_idx).cloned().unwrap_or_default();
                }
            }
            K::Down => {
                if a.history_idx + 1 < a.console_history.len() {
                    a.history_idx += 1;
                    a.console_input = a.console_history[a.history_idx].clone();
                } else {
                    a.history_idx = a.console_history.len();
                    a.console_input.clear();
                }
            }
            K::Backspace => { a.console_input.pop(); }
            K::Char(c) => a.console_input.push(c),
            _ => {}
        },
        InputMode::Editor => {
            use crossterm::event::KeyModifiers;
            let Some(ed) = a.editor.as_mut() else { a.input_mode = InputMode::Normal; return };
            if ed.loading {
                if let K::Esc = code {
                    a.input_mode = InputMode::Normal;
                    a.editor = None;
                }
                return;
            }
            match code {
                K::Char('s') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    drop(a);
                    save_editor(app).await;
                }
                K::Esc => {
                    let was_dirty = ed.dirty;
                    let path = ed.path.clone();
                    a.input_mode = InputMode::Normal;
                    a.editor = None;
                    if was_dirty {
                        let _ = a.line_tx.send((a.selected, format!("[aeroPanel] editor cerrado sin guardar: {path}")));
                    }
                }
                K::Up => {
                    if ed.row > 0 {
                        ed.row -= 1;
                        let len = ed.lines.get(ed.row).map(|l| l.chars().count()).unwrap_or(0);
                        ed.col = ed.col.min(len);
                    }
                }
                K::Down => {
                    if ed.row + 1 < ed.lines.len() {
                        ed.row += 1;
                        let len = ed.lines.get(ed.row).map(|l| l.chars().count()).unwrap_or(0);
                        ed.col = ed.col.min(len);
                    }
                }
                K::Left => {
                    if ed.col > 0 {
                        ed.col -= 1;
                    } else if ed.row > 0 {
                        ed.row -= 1;
                        ed.col = ed.lines.get(ed.row).map(|l| l.chars().count()).unwrap_or(0);
                    }
                }
                K::Right => {
                    let len = ed.lines.get(ed.row).map(|l| l.chars().count()).unwrap_or(0);
                    if ed.col < len {
                        ed.col += 1;
                    } else if ed.row + 1 < ed.lines.len() {
                        ed.row += 1;
                        ed.col = 0;
                    }
                }
                K::Home => ed.col = 0,
                K::End => {
                    ed.col = ed.lines.get(ed.row).map(|l| l.chars().count()).unwrap_or(0);
                }
                K::Enter => {
                    let rest: String = ed.lines.get(ed.row).map(|l| l.chars().skip(ed.col).collect()).unwrap_or_default();
                    let head: String = ed.lines.get(ed.row).map(|l| l.chars().take(ed.col).collect()).unwrap_or_default();
                    ed.lines.insert(ed.row, head);
                    if let Some(l) = ed.lines.get_mut(ed.row + 1) {
                        *l = rest;
                    } else {
                        ed.lines.push(rest);
                    }
                    ed.row += 1;
                    ed.col = 0;
                    ed.dirty = true;
                    ed.last_edit = Some(std::time::Instant::now());
                }
                K::Backspace => {
                    if ed.col > 0 {
                        if let Some(l) = ed.lines.get_mut(ed.row) {
                            let idx = l.chars().count().min(ed.col);
                            let head: String = l.chars().take(idx - 1).collect();
                            let tail: String = l.chars().skip(idx).collect();
                            *l = format!("{head}{tail}");
                            ed.col = idx - 1;
                        }
                    } else if ed.row > 0 {
                        let cur = ed.lines.remove(ed.row);
                        let prev_len = ed.lines.get(ed.row - 1).map(|l| l.chars().count()).unwrap_or(0);
                        if let Some(p) = ed.lines.get_mut(ed.row - 1) {
                            p.push_str(&cur);
                        }
                        ed.row -= 1;
                        ed.col = prev_len;
                    }
                    ed.dirty = true;
                    ed.last_edit = Some(std::time::Instant::now());
                }
                K::Char(c) => {
                    if let Some(l) = ed.lines.get_mut(ed.row) {
                        let idx = l.chars().count().min(ed.col);
                        let head: String = l.chars().take(idx).collect();
                        let tail: String = l.chars().skip(idx).collect();
                        *l = format!("{head}{c}{tail}");
                        ed.col = idx + 1;
                    }
                    ed.dirty = true;
                    ed.last_edit = Some(std::time::Instant::now());
                }
                _ => {}
            }
        }
        InputMode::FormAdd => handle_form(app.clone(), &mut a, code, 7).await,
        InputMode::FormInstance => handle_form(app.clone(), &mut a, code, 4).await,
        InputMode::UploadPath | InputMode::DownloadPath => match code {
            K::Esc => a.input_mode = InputMode::Normal,
            K::Enter => {
                let path = a.path_input.clone();
                a.path_input.clear();
                let is_upload = a.input_mode == InputMode::UploadPath;
                let dir = a.files_dir.clone();
                let idx = a.selected;
                let tx = a.line_tx.clone();
                a.input_mode = InputMode::Normal;
                drop(a);
                let appc = app.clone();
                tokio::spawn(async move {
                    let t = {
                        let a = appc.lock().await;
                        a.servers[idx].target.clone()
                    };
                    let _ = tx.send((idx, if is_upload { format!("[aeroPanel] subiendo {path}…") } else { format!("[aeroPanel] descargando…") }));
                    let r = if is_upload {
                        connection::upload(&t, &path, &dir).await
                    } else {
                        connection::download(&t, &dir, &path).await
                    };
                    match r {
                        Ok(()) => {
                            let _ = tx.send((idx, if is_upload {
                                format!("[aeroPanel] subido a {dir} ")
                            } else {
                                format!("[aeroPanel] guardado en {path} ")
                            }));
                        }
                        Err(e) => {
                            let _ = tx.send((idx, format!("[aeroPanel] error: {e}")));
                        }
                    }
                });
            }
            K::Backspace => { a.path_input.pop(); }
            K::Char(c) => a.path_input.push(c),
            _ => {}
        },
    }
}

fn valid_tabs(a: &App) -> Vec<Tab> {
    let Some(s) = a.sel() else { return vec![Tab::Overview] };
    let mc = s.is_mc();
    let managed = s.is_managed();
    app::TABS
        .iter()
        .copied()
        .filter(|t| match t {
            Tab::Minecraft | Tab::Consola => mc,
            Tab::Gestion => managed,
            _ => true,
        })
        .collect()
}

async fn handle_form(app: Arc<tokio::sync::Mutex<App>>, a: &mut App, code: crossterm::event::KeyCode, n: usize) {
    use crossterm::event::KeyCode as K;
    match code {
        K::Esc => a.input_mode = InputMode::Normal,
        K::Tab | K::Down => a.form.active = (a.form.active + 1) % n,
        K::Up => a.form.active = (a.form.active + n - 1) % n,
        K::Enter => {
            let is_instance = a.input_mode == InputMode::FormInstance;
            if is_instance {
                let (name, core, version, memory, auto) = a.instance_to_cfg();
                if !name.is_empty() {
                    let tx = a.line_tx.clone();
                    a.input_mode = InputMode::Normal;
                    // placeholder entry so we can stream progress lines to it
                    a.add_instance_server(&name, std::path::Path::new("…"), &version, &memory, auto);
                    if let Some(s) = a.servers.last_mut() {
                        if let Some(mc) = s.cfg.minecraft.as_mut() {
                            mc.core = core.clone();
                        }
                    }
                    let idx = a.servers.len() - 1;
                    let _ = tx.send((idx, format!("[aeroPanel] creando instancia {name} ({core} {version}, {memory})…")));
                    let app_state = app;
                    tokio::spawn(async move {
                        let r = crate::instances::create(&name, &version, &memory, &core, &tx, idx).await;
                        let mut a = app_state.lock().await;
                        match r {
                            Ok(dir) => {
                                let _ = tx.send((idx, format!("[aeroPanel] instancia creada en {dir:?} ")));
                                if let Some(s) = a.servers.get_mut(idx) {
                                    if let Some(mc) = s.cfg.minecraft.as_mut() {
                                        mc.instance_dir = dir.to_string_lossy().to_string();
                                    }
                                }
                                let _ = a.save_config();
                            }
                            Err(e) => {
                                let _ = tx.send((idx, format!("[aeroPanel] ERROR: {e}")));
                            }
                        }
                    });
                }
            } else {
                let cfg = a.form_to_cfg();
                if !cfg.name.is_empty() {
                    let new_server = new_server(cfg);
                    let idx = a.servers.len();
                    a.servers.push(new_server);
                    a.selected = idx;
                    a.input_mode = InputMode::Normal;
                    a.screen = Screen::Dashboard;
                    let _ = a.save_config();
                }
            }
        }
        K::Backspace => {
            let act = a.form.active;
            if let Some(f) = a.form.fields.get_mut(act) { f.pop(); }
        }
        K::Char(c) => {
            let act = a.form.active;
            if let Some(f) = a.form.fields.get_mut(act) { f.push(c); }
        }
        _ => {}
    }
}

fn join_dir(dir: &str, name: &str) -> String {
    // ls -A1ap devuelve los dirs con "/" final; lo limpiamos
    let name = name.trim_end_matches('/');
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

fn parent_of(dir: &str) -> String {
    // recorta la última componente; respeta "/" y "./" (no escapa del root)
    if dir == "/" || dir.is_empty() {
        return dir.to_string();
    }
    let trimmed = dir.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) if trimmed[..i].is_empty() => "/".to_string(),
        Some(i) => trimmed[..i].to_string(),
        None => {
            if trimmed == "." || trimmed.starts_with("./") {
                ".".to_string()
            } else {
                ".".to_string()
            }
        }
    }
}

fn scroll_sel(a: &mut App, delta: i16) {
    if let Some(s) = a.sel_mut() {
        let max = s.log_lines.len().saturating_sub(1) as u16;
        s.scroll = s.scroll.saturating_add_signed(delta).min(max.saturating_sub(2));
    }
}

fn new_server(cfg: crate::config::ServerCfg) -> app::Server {
    app::Server {
        target: Target {
            name: cfg.name.clone(),
            mode: cfg.mode.clone(),
            host: cfg.host.clone(),
            port: cfg.port,
            user: cfg.user.clone(),
            key: cfg.key.clone(),
        },
        cfg,
        status: Status::Checking,
        metrics: metrics::Metrics::default(),
        cpu_hist: Default::default(),
        mem_hist: Default::default(),
        health: Default::default(),
        log_lines: Default::default(),
        console_stdin: None,
        child: None,
        scroll: 0,
    }
}

/// list files of the selected server's root/current dir
async fn load_files(app: Arc<tokio::sync::Mutex<App>>) {
    let (idx, t, dir) = {
        let a = app.lock().await;
        match a.sel() {
            Some(s) => {
                let dir = if a.files_dir.is_empty() { s.files_root() } else { a.files_dir.clone() };
                (a.selected, s.target.clone(), dir)
            }
            None => return,
        }
    };
    {
        let mut a = app.lock().await;
        a.files_dir = dir.clone();
        a.files_loading = true;
    }
    match connection::list_dir(&t, &dir).await {
        Ok(files) => {
            let mut a = app.lock().await;
            if a.files_dir == dir {
                a.files = files;
                a.files_sel = 0;
                a.files_reveal = 0;
                a.files_loading = false;
            }
        }
        Err(e) => {
            let mut a = app.lock().await;
            a.files_loading = false;
            let _ = a.line_tx.send((idx, format!("[aeroPanel] ls error: {e}")));
        }
    }
}

async fn send_console(app: Arc<tokio::sync::Mutex<App>>, cmd: String) {
    let (slot, via_daemon, name, idx) = {
        let a = app.lock().await;
        (
            a.sel().and_then(|s| s.console_stdin.clone()),
            a.daemon_tx.clone(),
            a.sel().map(|s| s.cfg.name.clone()).unwrap_or_default(),
            a.selected,
        )
    };
    if let Some(tx) = via_daemon {
        let _ = tx.send(serde_json::to_string(&serde_json::json!({
            "cmd": "console", "server": name, "text": cmd
        })).unwrap_or_default());
        return;
    }
    let slot = match slot {
        Some(sl) => sl,
        None => return,
    };
    let mut guard = slot.lock().await;
    if let Some(stdin) = guard.as_mut() {
        let _ = console_send(stdin, &cmd).await;
    }
    let _ = idx;
}

/// manda una orden al daemon; false si no hay daemon conectado
fn daemon_send(a: &App, name: &str, op: &str, extra: serde_json::Value) -> bool {
    let Some(tx) = &a.daemon_tx else { return false };
    let mut v = serde_json::json!({ "cmd": op, "server": name });
    if let (Some(o), Some(x)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in x {
            o.insert(k.clone(), val.clone());
        }
    }
    let _ = tx.send(serde_json::to_string(&v).unwrap_or_default());
    true
}
