use crossterm::event;
use ratatui::{buffer, layout, style, text, widgets};

use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct KeybindsWidget {
    inner: Vec<(String, Vec<String>)>,
    theme: Theme,
}

impl KeybindsWidget {
    pub fn new(keybinds: &[(&'_ str, &'_ [&'_ str])], theme: Theme) -> Self {
        Self {
            inner: keybinds
                .iter()
                .map(|(s, k)| (s.to_string(), k.iter().map(|s| s.to_string()).collect()))
                .collect(),
            theme,
        }
    }

    pub fn push(&mut self, keybind: (&'_ str, &'_ [&'_ str])) {
        self.inner.push((
            keybind.0.to_string(),
            keybind.1.iter().map(|s| s.to_string()).collect(),
        ));
    }
}

impl widgets::Widget for &KeybindsWidget {
    fn render(self, area: layout::Rect, buf: &mut buffer::Buffer) {
        let mut spans = Vec::new();
        spans.extend(self.inner.iter().flat_map(|(action, keys)| {
            keys.into_iter()
                .enumerate()
                .flat_map(|(i, key)| {
                    if i > 0 {
                        vec![
                            text::Span::from("/"),
                            text::Span::from(format!("{}", key)).style(
                                style::Style::new()
                                    .fg(self.theme.selection_foreground)
                                    .bg(self.theme.selection_background),
                            ),
                        ]
                        .into_iter()
                    } else {
                        vec![
                            text::Span::from("<"),
                            text::Span::from(format!("{}", key)).style(
                                style::Style::new()
                                    .fg(self.theme.selection_foreground)
                                    .bg(self.theme.selection_background),
                            ),
                        ]
                        .into_iter()
                    }
                })
                .chain(
                    vec![
                        text::Span::from(": "),
                        text::Span::from(action),
                        text::Span::from("> "),
                    ]
                    .into_iter(),
                )
        }));

        let keybinds = widgets::Paragraph::new(text::Line::from(spans))
            .centered()
            .style(
                style::Style::new()
                    .fg(self.theme.footer_foreground)
                    .bg(self.theme.footer_background),
            );

        widgets::Widget::render(&keybinds, area, buf);
    }
}
