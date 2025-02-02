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
        SOLARIZED_DARK_HIGH_CONTRAST
    }
}

pub const SOLARIZED_DARK_HIGH_CONTRAST: Theme = Theme {
    background: style::Color::from_u32(0x002b36),
    foreground: style::Color::from_u32(0xfdf6e3),
    alt_background: style::Color::from_u32(0x073642),
    selection_foreground: style::Color::from_u32(0xfdf6e3),
    selection_background: style::Color::from_u32(0x586e75),
    header_foreground: style::Color::from_u32(0xfdf6e3),
    header_background: style::Color::from_u32(0x073642),
    footer_foreground: style::Color::from_u32(0xfdf6e3),
    footer_background: style::Color::from_u32(0x073642),
    border: style::Color::from_u32(0x2aa198),
    success_background: style::Color::from_u32(0x354725),
    failure_background: style::Color::from_u32(0x582b29),
    running_background: style::Color::from_u32(0x004363),
    cancelled_background: style::Color::from_u32(0x928374),
};
