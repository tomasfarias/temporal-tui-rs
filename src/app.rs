use std::error;
use std::fs;
use std::io::Read;
use std::sync;
use std::time;

use crossterm::event;
use ratatui::{
    backend::Backend, layout, style, style::Stylize, symbols, text, widgets, widgets::Widget, Frame,
};
use temporal_client::{self, ClientOptionsBuilder};
use tokio::task;
use url::Url;

use crate::{
    event::Event, settings::Settings, theme::Theme, tui::Tui, widgets::keybinds::KeybindsWidget,
    widgets::workflow::WorkflowWidget, widgets::workflow_table::WorkflowTableWidget,
    widgets::Keybindable, widgets::ViewWidget,
};

const FOOTER_INFO_TEXT: [&str; 1] = ["(q) quit | (↑/j) move up | (↓/k) move down | (r) reload"];

/// Application result type.
pub type AppResult<T> = std::result::Result<T, anyhow::Error>;

/// The main Temporal TUI application.
#[derive(Debug)]
pub struct App {
    /// Is the application running?
    running: bool,
    temporal_client: sync::Arc<temporal_client::RetryClient<temporal_client::Client>>,
    /// Temporal namespace we are connected to.
    namespace: String,
    /// The current [`ViewWidget`] being displayed.
    view: ViewWidget,
    /// The [`App`]'s [`Theme`] defines its colors.
    theme: Theme,
}

impl App {
    /// Constructs a new instance of [`App`].
    pub async fn new(settings: &Settings) -> Result<Self, anyhow::Error> {
        let theme = settings.theme()?;
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

        let workflow_table = WorkflowTableWidget::new(&temporal_client, theme, 48);

        Ok(App {
            running: true,
            temporal_client,
            namespace,
            view: ViewWidget::WorkflowTable(workflow_table),
            theme,
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
        self.view.run().await;
    }

    /// Handles the tick event of the terminal.
    pub fn tick(&self) {}

    /// Set running to false to quit the application.
    pub fn quit(&mut self) {
        self.running = false;
    }

    /// Render the current view on display with a header and footer.
    pub fn render_view(&mut self, frame: &mut Frame) {
        let app_block = widgets::Block::bordered()
            .title(
                text::Line::from(self.title())
                    .centered()
                    .fg(self.theme.foreground),
            )
            .border_type(widgets::BorderType::Rounded)
            .border_style(self.theme.border)
            .bg(self.theme.background);

        let app_area = app_block.inner(frame.area());
        frame.render_widget(&app_block, frame.area());

        let vertical =
            &layout::Layout::vertical([layout::Constraint::Fill(1), layout::Constraint::Length(2)]);
        let [body_area, footer_area] = vertical.areas(app_area);

        frame.render_widget(&self.view, body_area);

        let footer_horizontal = &layout::Layout::horizontal([
            layout::Constraint::Length(10),
            layout::Constraint::Fill(1),
            // Just to allow aligning the keybinds in the center, currently this third area is not used.
            layout::Constraint::Length(10),
        ]);
        let [_, footer_center_area, _] = footer_horizontal.areas(footer_area);

        let mut keybinds = KeybindsWidget::new(self.view.keybinds(), self.theme);
        keybinds.push(("Quit", &["Ctrl+c"]));
        // let mode_footer = widgets::Paragraph::new(text::Line::from(self.mode.as_str()))
        //     .style(
        //         style::Style::new()
        //             .fg(self.theme.footer_foreground)
        //             .bg(self.theme.footer_background),
        //     )
        //     .centered()
        //     .block(widgets::Block::bordered().borders(widgets::Borders::NONE));
        // frame.render_widget(&mode_footer, footer_left_area);

        // let kebyinds_footer = widgets::Paragraph::new(text::Text::from_iter(FOOTER_INFO_TEXT))
        //     .style(
        //         style::Style::new()
        //             .fg(self.theme.footer_foreground)
        //             .bg(self.theme.footer_background),
        //     )
        //     .centered()
        //     .block(widgets::Block::bordered().borders(widgets::Borders::NONE));
        frame.render_widget(&keybinds, footer_center_area);
    }

    fn title(&self) -> String {
        format!("Temporal TUI - {}", self.namespace)
    }

    pub async fn handle_event(&mut self, event: &Event) {
        match event {
            Event::Key(key_event) => {
                if let event::KeyEvent {
                    code: event::KeyCode::Char('c'),
                    modifiers: event::KeyModifiers::CONTROL,
                    ..
                } = key_event
                {
                    self.quit()
                } else {
                    self.handle_key(*key_event).await
                }
            }
            _ => {}
        }
    }

    pub async fn handle_key(&mut self, key: event::KeyEvent) {
        let should_change_view = match &mut self.view {
            ViewWidget::WorkflowTable(workflow_table) => workflow_table.handle_key(key).await,
            ViewWidget::Workflow(workflow) => workflow.handle_key(key).await,
        };

        if let Some(view) = should_change_view {
            self.view = view;
            self.run_view().await;
        }
    }
}
