use std::io;

use ratatui::{backend::CrosstermBackend, Terminal};
use structured_logger::async_json;

use crate::{
    app::{App, AppResult},
    event::{Event, EventHandler},
    settings::Settings,
    tui::Tui,
};

pub mod app;
pub mod event;
pub mod handler;
pub mod settings;
pub mod theme;
pub mod tui;
pub mod widgets;

#[tokio::main]
async fn main() -> AppResult<()> {
    let settings = Settings::new()?;
    let level = if settings.debug {
        "debug".to_string()
    } else {
        "info".to_string()
    };

    structured_logger::Builder::with_level(&level)
        .with_target_writer(
            "*",
            async_json::new_writer(
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&settings.log_path)
                    .await?,
            ),
        )
        .init();

    // Create an application.
    let app = App::new(&settings).await?;

    // Initialize the terminal user interface.
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    let events = EventHandler::new(250);
    let tui = Tui::new(terminal, events);

    app.run(tui).await?;
    Ok(())
}
