//! aeroPanel visual theme — modeled after OpenCode: near-black canvas,
//! rounded borders, restrained accent palette (orange primary, blue secondary).

use ratatui::style::{Color, Modifier, Style};

pub const BG: Color = Color::Rgb(17, 17, 17);
pub const BG_PANEL: Color = Color::Rgb(24, 24, 24);
pub const FG: Color = Color::Rgb(220, 220, 220);
pub const FG_DIM: Color = Color::Rgb(115, 115, 115);
#[allow(dead_code)]
pub const FG_FAINT: Color = Color::Rgb(70, 70, 70);
pub const BORDER: Color = Color::Rgb(55, 55, 55);
pub const BORDER_ACTIVE: Color = Color::Rgb(130, 130, 130);

pub const ACCENT: Color = Color::Rgb(255, 138, 61); // aero orange
pub const BLUE: Color = Color::Rgb(80, 180, 255);
pub const GREEN: Color = Color::Rgb(120, 220, 130);
pub const RED: Color = Color::Rgb(240, 90, 90);
pub const YELLOW: Color = Color::Rgb(240, 200, 90);

pub const fn base() -> Style {
    Style::new().fg(FG).bg(BG)
}

pub const fn dim() -> Style {
    Style::new().fg(FG_DIM).bg(BG)
}

pub const fn accent() -> Style {
    Style::new().fg(ACCENT).bg(BG)
}

pub const fn panel_border(focused: bool) -> Style {
    Style::new().fg(if focused { BORDER_ACTIVE } else { BORDER }).bg(BG)
}

pub const fn selected() -> Style {
    Style::new().bg(BG_PANEL).fg(FG)
}

pub const fn bold_accent() -> Style {
    Style::new().fg(ACCENT).bg(BG).add_modifier(Modifier::BOLD)
}

/// OpenCode-style spinner frames
pub const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// Status dot characters
pub const DOT: &str = "*";
