use std::error;
use std::fs;
use std::io::Read;
use std::sync;
use std::time;

use crossterm::event;
use ratatui::{
    backend::Backend,
    layout, style,
    style::{palette::material, palette::tailwind, Color, Modifier, Style, Stylize},
    symbols, text, widgets, Frame,
};
use temporal_client::{self, Client, ClientOptions, ClientOptionsBuilder, RetryClient};
use tokio::task;
use url::Url;

use crate::{
    event::Event, settings::Settings, tui::Tui, widgets::workflow_table::WorkflowTableWidget,
};

const FOOTER_INFO_TEXT: [&str; 1] = ["(q) quit | (↑/j) move up | (↓/k) move down | (r) reload"];
const BORDER_STYLE: Style = Style::new()
    .fg(material::INDIGO.a100)
    .bg(material::GRAY.c500);
const ROW_BG: Color = material::GRAY.c500;
const SELECTED_STYLE: Style = Style::new()
    .bg(material::GRAY.c50)
    .add_modifier(Modifier::BOLD);
const TEXT_FG_COLOR: Color = material::WHITE;

/// Application result type.
pub type AppResult<T> = std::result::Result<T, anyhow::Error>;

#[derive(Debug)]
pub enum Mode {
    Normal,
    Insert,
}

/// Application.
#[derive(Debug)]
pub struct App {
    /// Is the application running?
    running: bool,
    temporal_client: sync::Arc<temporal_client::RetryClient<temporal_client::Client>>,
    namespace: String,
    view: View,
    mode: Mode,
}

#[derive(Debug)]
pub enum View {
    WorkflowTable(WorkflowTableWidget),
    ScheduleTable,
}

impl App {
    /// Constructs a new instance of [`App`].
    pub async fn new(settings: &Settings) -> Result<Self, anyhow::Error> {
        let mut temporal_url = Url::parse(&settings.host)?;
        temporal_url
            .set_port(Some(settings.port))
            .expect("failed to set port");

        log::debug!("Connecting to: {}", temporal_url);

        let mut client_cert_file = fs::File::open(&settings.client_cert)?;
        let mut client_cert = Vec::new();
        client_cert_file.read_to_end(&mut client_cert)?;

        let mut private_key_file = fs::File::open(&settings.client_private_key)?;
        let mut client_private_key = Vec::new();
        private_key_file.read_to_end(&mut client_private_key)?;

        let mut server_root_ca_cert_file = fs::File::open(&settings.server_root_ca_cert)?;
        let mut server_root_ca_cert = Vec::new();
        server_root_ca_cert_file.read_to_end(&mut server_root_ca_cert)?;

        let client_tls_config = temporal_client::ClientTlsConfig {
            client_cert,
            client_private_key,
        };
        let tls_config = temporal_client::TlsConfig {
            server_root_ca_cert: Some(server_root_ca_cert),
            client_tls_config: Some(client_tls_config),
            domain: None,
        };

        let client_options = ClientOptionsBuilder::default()
            .target_url(temporal_url)
            .client_name("temporaltui-rs")
            .client_version("1.0.0")
            .tls_cfg(tls_config)
            .build()?;

        let namespace = settings.namespace.clone();
        let temporal_client = sync::Arc::new(client_options.connect(&namespace, None).await?);

        let workflow_table = WorkflowTableWidget::new(&temporal_client, 48);

        Ok(App {
            running: true,
            temporal_client,
            namespace,
            view: View::WorkflowTable(workflow_table),
            mode: Mode::Normal,
        })
    }

    pub async fn run<B: Backend>(mut self, mut terminal: Tui<B>) -> Result<(), anyhow::Error> {
        terminal.init()?;

        self.run_view().await;

        let period = time::Duration::from_secs_f32(1.0 / 60.0);
        let mut interval = tokio::time::interval(period);

        while self.running {
            tokio::select! {
                _ = interval.tick() => { terminal.draw(&mut self)?; },
                Ok(event) = terminal.events.next() => self.handle_event(&event).await,
            }
        }

        terminal.exit()?;
        Ok(())
    }

    pub async fn run_view(&mut self) {
        match &mut self.view {
            View::WorkflowTable(workflow_table) => {
                workflow_table.run();
                workflow_table.reload().await;
            }
            View::ScheduleTable => panic!("not implemented"),
        }
    }

    /// Handles the tick event of the terminal.
    pub fn tick(&self) {}

    /// Set running to false to quit the application.
    pub fn quit(&mut self) {
        self.running = false;
    }

    pub fn render_view(&mut self, frame: &mut Frame) {
        let app_block = widgets::Block::bordered()
            .title(text::Line::from(self.title()).centered())
            .borders(widgets::Borders::NONE);

        let app_area = app_block.inner(frame.area());
        frame.render_widget(&app_block, frame.area());

        let vertical =
            &layout::Layout::vertical([layout::Constraint::Fill(1), layout::Constraint::Length(2)]);
        let [body_area, footer_area] = vertical.areas(app_area);

        match &self.view {
            View::WorkflowTable(workflow_table) => frame.render_widget(workflow_table, body_area),
            _ => panic!("Unsupported view"),
        };

        let footer = widgets::Paragraph::new(text::Text::from_iter(FOOTER_INFO_TEXT))
            .style(
                style::Style::new()
                    .fg(tailwind::SLATE.c200)
                    .bg(tailwind::SLATE.c950),
            )
            .centered()
            .block(widgets::Block::bordered().borders(widgets::Borders::NONE));
        frame.render_widget(&footer, footer_area);
    }

    fn title(&self) -> String {
        format!("Temporal TUI - {}", self.namespace)
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub async fn handle_event(&mut self, event: &Event) {
        match self.mode {
            Mode::Insert => match event {
                Event::Key(key_event) => match key_event.code {
                    // Switch to Normal mode
                    event::KeyCode::Esc => {
                        self.set_mode(Mode::Normal);
                    }
                    // Exit application on `Ctrl-C`
                    event::KeyCode::Char('c') | event::KeyCode::Char('C') => {
                        if key_event.modifiers == event::KeyModifiers::CONTROL {
                            self.quit();
                        } else {
                            self.handle_insert_key(key_event.code);
                        }
                    }
                    key => {
                        self.handle_insert_key(key);
                    }
                },
                _ => {}
            },
            Mode::Normal => {
                match event {
                    Event::Key(key_event) => {
                        match key_event.code {
                            // Switch to Insert mode
                            event::KeyCode::Char('i') => {
                                self.set_mode(Mode::Insert);
                            }
                            // Exit application on `ESC` or `q`
                            event::KeyCode::Esc => {
                                self.quit();
                            }
                            // Exit application on `Ctrl-C`
                            event::KeyCode::Char('c') | event::KeyCode::Char('C') => {
                                if key_event.modifiers == event::KeyModifiers::CONTROL {
                                    self.quit();
                                } else {
                                    self.handle_normal_key(key_event.code).await;
                                }
                            }
                            key => {
                                self.handle_normal_key(key).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn handle_insert_key(&mut self, key: event::KeyCode) {
        match &mut self.view {
            View::WorkflowTable(workflow_table) => workflow_table.handle_insert_key(key),
            View::ScheduleTable => panic!("not implemented"),
        }
    }

    pub async fn handle_normal_key(&mut self, key: event::KeyCode) {
        match &mut self.view {
            View::WorkflowTable(workflow_table) => workflow_table.handle_normal_key(key).await,
            View::ScheduleTable => panic!("not implemented"),
        }
    }
}
