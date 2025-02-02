use serde_derive::Deserialize;

use ratatui::style;

#[derive(Debug, Deserialize, Copy, Clone)]
pub struct Theme {
    pub background: style::Color,
    pub foreground: style::Color,
    pub alt_background: style::Color,
    pub selection_foreground: style::Color,
    pub selection_background: style::Color,
    pub header_foreground: style::Color,
    pub header_background: style::Color,
    pub footer_foreground: style::Color,
    pub footer_background: style::Color,
    pub border: style::Color,
    pub success_background: style::Color,
    pub failure_background: style::Color,
    pub running_background: style::Color,
    pub cancelled_background: style::Color,
}

impl Default for Theme {
    fn default() -> Self {
        NORD_DARK
    }
}

pub const SOLARIZED_DARK_HIGH_CONTRAST: Theme = Theme {
    background: style::Color::from_u32(0x00002b36),
    foreground: style::Color::from_u32(0x00fdf6e3),
    alt_background: style::Color::from_u32(0x00073642),
    selection_foreground: style::Color::from_u32(0x00fdf6e3),
    selection_background: style::Color::from_u32(0x00586e75),
    header_foreground: style::Color::from_u32(0x00fdf6e3),
    header_background: style::Color::from_u32(0x00073642),
    footer_foreground: style::Color::from_u32(0x00fdf6e3),
    footer_background: style::Color::from_u32(0x00073642),
    border: style::Color::from_u32(0x00002aa198),
    success_background: style::Color::from_u32(0x00354725),
    failure_background: style::Color::from_u32(0x00582b29),
    running_background: style::Color::from_u32(0x00004363),
    cancelled_background: style::Color::from_u32(0x00928374),
};

pub const NORD_DARK: Theme = Theme {
    background: style::Color::from_u32(0x002e3440),
    foreground: style::Color::from_u32(0x00eceff4),
    alt_background: style::Color::from_u32(0x003b4252),
    selection_foreground: style::Color::from_u32(0x003b4252),
    selection_background: style::Color::from_u32(0x00d8dee9),
    header_foreground: style::Color::from_u32(0x00eceff4),
    header_background: style::Color::from_u32(0x002e3440),
    footer_foreground: style::Color::from_u32(0x00eceff4),
    footer_background: style::Color::from_u32(0x002e3440),
    border: style::Color::from_u32(0x0081a1c1),
    success_background: style::Color::from_u32(0x00a3be8c),
    failure_background: style::Color::from_u32(0x00bf616a),
    running_background: style::Color::from_u32(0x005e81ac),
    cancelled_background: style::Color::from_u32(0x004c566a),
};
