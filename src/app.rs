use std::error;
use std::fs;
use std::io::Read;
use std::sync;
use std::time;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

/// Application.
#[derive(Debug)]
pub struct App {
    /// Is the application running?
    pub running: bool,
    pub temporal_client: sync::Arc<temporal_client::RetryClient<temporal_client::Client>>,
    pub namespace: String,
    current_view: CurrentView,
    pub workflow_table: WorkflowTableWidget,
}

#[derive(Debug)]
pub enum CurrentView {
    WorkflowTable,
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
            workflow_table,
            current_view: CurrentView::WorkflowTable,
        })
    }

    pub async fn run<B: Backend>(mut self, mut terminal: Tui<B>) -> Result<(), anyhow::Error> {
        terminal.init()?;

        self.workflow_table.run(true).await;

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

    /// Handles the tick event of the terminal.
    pub fn tick(&self) {}

    /// Set running to false to quit the application.
    pub fn quit(&mut self) {
        self.running = false;
    }

    pub fn render_current_view(&mut self, frame: &mut Frame) {
        match self.current_view {
            CurrentView::WorkflowTable => self.render_workflow_table(frame),
            _ => panic!("Unsupported view"),
        }
    }

    pub fn render_workflow_table(&mut self, frame: &mut Frame) {
        let vertical =
            &layout::Layout::vertical([layout::Constraint::Length(3), layout::Constraint::Fill(1)]);
        let [title_area, body_area] = vertical.areas(frame.area());

        let block = widgets::Block::bordered()
            .border_type(widgets::BorderType::Double)
            .border_style(style::Style::new().fg(tailwind::BLUE.c400));

        let title_inner_area = block.inner(title_area);
        frame.render_widget(block, title_area);

        let title_horizontal = &layout::Layout::horizontal([
            layout::Constraint::Fill(1),
            layout::Constraint::Fill(1),
            layout::Constraint::Fill(1),
        ]);
        let [left_title_area, center_title_area, right_title_area] =
            title_horizontal.areas(title_inner_area);

        let last_reload_string = match self.workflow_table.get_duration_since_last_reload() {
            Some(duration) => format!("Last reload: {}s ago", duration.as_secs()),
            None => "Last reload: N/A".to_string(),
        };

        let last_reload_title = widgets::Paragraph::new(text::Text::from(last_reload_string))
            .style(style::Style::new().fg(tailwind::SLATE.c200))
            .right_aligned();

        let app_title = widgets::Paragraph::new(text::Text::from(self.title()))
            .style(style::Style::new().fg(tailwind::SLATE.c200))
            .centered();

        frame.render_widget(&last_reload_title, right_title_area);
        frame.render_widget(&app_title, center_title_area);

        frame.render_widget(&self.workflow_table, body_area);
    }

    fn title(&self) -> String {
        format!("Temporal TUI - {}", self.namespace)
    }

    pub async fn scroll_current_view_down(&mut self) {
        match self.current_view {
            CurrentView::WorkflowTable => self.workflow_table.next_row().await,
            CurrentView::ScheduleTable => panic!("not implemented"),
        }
    }

    pub fn is_current_view_at_bottom(&self) -> bool {
        match self.current_view {
            CurrentView::WorkflowTable => self.workflow_table.is_on_last_row(),
            CurrentView::ScheduleTable => panic!("not implemented"),
        }
    }

    pub fn scroll_current_view_up(&mut self) {
        match self.current_view {
            CurrentView::WorkflowTable => self.workflow_table.previous_row(),
            CurrentView::ScheduleTable => panic!("not implemented"),
        }
    }

    pub async fn reload_current_view(&self) {
        match self.current_view {
            CurrentView::WorkflowTable => self.workflow_table.reload().await,
            CurrentView::ScheduleTable => panic!("not implemented"),
        }
    }

    pub fn is_current_view_loading(&self) -> bool {
        match self.current_view {
            CurrentView::WorkflowTable => self.workflow_table.is_loading(),
            CurrentView::ScheduleTable => panic!("not implemented"),
        }
    }

    pub async fn handle_event(&mut self, event: &Event) {
        match event {
            Event::Key(key_event) => {
                match key_event.code {
                    // Exit application on `ESC` or `q`
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.quit();
                    }
                    // Exit application on `Ctrl-C`
                    KeyCode::Char('c') | KeyCode::Char('C') => {
                        if key_event.modifiers == KeyModifiers::CONTROL {
                            self.quit();
                        }
                    }
                    KeyCode::Char('j') | KeyCode::Down => self.scroll_current_view_down().await,
                    KeyCode::Char('k') | KeyCode::Up => self.scroll_current_view_up(),
                    KeyCode::Char('r') | KeyCode::Right => self.reload_current_view().await,
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
