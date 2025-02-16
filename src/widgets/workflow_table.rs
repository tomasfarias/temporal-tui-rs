use std::sync;

use crossterm::event;
use ratatui::{buffer, layout, style, style::Stylize, text, widgets};
use temporal_client::{self, WorkflowClientTrait};
use temporal_sdk_core_protos::temporal::api::workflowservice::v1 as service;
use tokio::sync::mpsc;
use tokio::task;
use tokio::time;

use crate::theme::Theme;
use crate::widgets::common::{LoadingState, Message, WorkflowExecution};
use crate::widgets::workflow::WorkflowWidget;
use crate::widgets::{Keybindable, ViewWidget};

const ITEM_HEIGHT: usize = 1;

/// Modes the [`WorkflowTableWidget`] can be in.
#[derive(Debug, Clone, Copy)]
pub enum Mode {
    /// Default [`Mode`] that allows navigation.
    Normal,
    /// [`Mode`] enabled when taking user input to write a query.
    Query,
}

impl<'m> Mode {
    pub fn as_str(&'m self) -> &'m str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Query => "QUERY",
        }
    }
}

/// A widget to input a query for Temporal.
#[derive(Debug, Clone)]
pub struct QueryInput {
    query: Option<String>,
    placeholder: String,
    cursor: usize,
    theme: Theme,
}

impl Default for QueryInput {
    fn default() -> Self {
        Self {
            query: None,
            placeholder: "Enter a query...".to_string(),
            cursor: 0,
            theme: Theme::default(),
        }
    }
}

impl QueryInput {
    pub fn new(placeholder: &str, theme: Theme) -> Self {
        Self {
            query: None,
            placeholder: placeholder.to_string(),
            cursor: 0,
            theme,
        }
    }

    pub fn query(&self) -> String {
        match &self.query {
            Some(q) => q.trim().to_owned(),
            None => "".to_owned(),
        }
    }
}

impl Keybindable for QueryInput {
    async fn handle_key(&mut self, key: event::KeyEvent) -> Option<ViewWidget> {
        match key {
            event::KeyEvent {
                code: event::KeyCode::Char(c),
                ..
            } => {
                if let Some(query) = self.query.as_mut() {
                    if self.cursor == query.len() - 1 {
                        query.pop();
                        query.push(c);
                        query.push(' ');
                    } else {
                        query.insert(self.cursor, c);
                    }
                    self.cursor += 1;
                } else {
                    let mut query = c.to_string();
                    query.push(' ');
                    self.query = Some(query);
                    self.cursor = 1;
                }
            }
            event::KeyEvent {
                code: event::KeyCode::Backspace,
                ..
            } => {
                if let Some(query) = self.query.as_mut() {
                    if query.len() > 1 {
                        if self.cursor == query.len() - 1 {
                            query.pop();
                            query.pop();
                            query.push(' ');
                        } else {
                            query.remove(self.cursor);
                        }
                        self.cursor -= 1;
                    }
                }
                self.query.take_if(|v| v.len() == 1);
            }
            event::KeyEvent {
                code: event::KeyCode::Left,
                ..
            } => {
                if let Some(_) = self.query.as_ref() {
                    if self.cursor > 0 {
                        self.cursor -= 1;
                    }
                }
            }
            event::KeyEvent {
                code: event::KeyCode::Right,
                ..
            } => {
                if let Some(query) = self.query.as_ref() {
                    if self.cursor < query.len() {
                        self.cursor += 1;
                    }
                }
            }
            _ => {}
        };
        None
    }

    fn keybinds<'k>(&'k self) -> &'k [(&'k str, &'k [&'k str])] {
        &[("Toggle query", &["Ctrl+q"])]
    }
}

impl widgets::Widget for &QueryInput {
    fn render(self, area: layout::Rect, buf: &mut buffer::Buffer) {
        let input_block = widgets::Block::bordered()
            .borders(widgets::Borders::ALL)
            .border_type(widgets::BorderType::Rounded)
            .border_style(style::Style::new().fg(self.theme.border));

        let query_str = match self.query.as_ref() {
            Some(q) => q.as_str(),
            None => self.placeholder.as_str(),
        };

        let [query_start, cursor_char, query_end]: [&str; 3] = [
            &query_str[..self.cursor],
            &query_str[self.cursor..self.cursor + 1],
            &query_str[self.cursor + 1..],
        ];
        let query_start_span = text::Span::from(query_start);
        let cursor_char_span = text::Span::from(cursor_char).underlined();
        let query_end_span = text::Span::from(query_end);
        let input_text = widgets::Paragraph::new(text::Line::from_iter([
            query_start_span,
            cursor_char_span,
            query_end_span,
        ]))
        .fg(self.theme.foreground)
        .block(input_block);

        widgets::Widget::render(input_text, area, buf);
    }
}

#[derive(Debug, Clone)]
pub struct WorkflowTableWidget {
    state: sync::Arc<sync::RwLock<WorkflowTableState>>,
    temporal_client: sync::Arc<temporal_client::RetryClient<temporal_client::Client>>,
    sender: sync::Arc<Option<mpsc::Sender<Message>>>,
    page_size: u32,
    mode: Mode,
    theme: Theme,
    last_reload: sync::Arc<sync::RwLock<Option<time::Instant>>>,
    query: sync::Arc<sync::RwLock<QueryInput>>,
}

#[derive(Debug, Default)]
struct WorkflowTableState {
    workflow_executions: Vec<WorkflowExecution>,
    next_page_token: Option<Vec<u8>>,
    loading_state: LoadingState,
    table_state: widgets::TableState,
    scrollbar_state: widgets::ScrollbarState,
}

impl WorkflowTableWidget {
    pub fn new(
        temporal_client: &sync::Arc<temporal_client::RetryClient<temporal_client::Client>>,
        theme: Theme,
        page_size: u32,
    ) -> Self {
        Self {
            state: sync::Arc::new(sync::RwLock::new(WorkflowTableState::default())),
            temporal_client: temporal_client.clone(),
            sender: sync::Arc::new(None),
            page_size,
            theme,
            mode: Mode::Normal,
            last_reload: sync::Arc::new(sync::RwLock::new(None)),
            query: sync::Arc::new(sync::RwLock::new(QueryInput {
                theme,
                ..QueryInput::default()
            })),
        }
    }

    pub fn run(&mut self) {
        let (tx, rx) = mpsc::channel(32);
        *sync::Arc::get_mut(&mut self.sender).unwrap() = Some(tx);

        let this = self.clone(); // clone the widget to pass to the background task
        tokio::spawn(this.fetch_workflows(rx));
    }

    async fn fetch_workflows(mut self, mut receiver: mpsc::Receiver<Message>) {
        log::debug!(widget = "WorkflowTableWidget"; "Starting fetch_workflows loop");
        while let Some(message) = receiver.recv().await {
            match message {
                Message::Reload => {
                    log::debug!(widget = "WorkflowTableWidget"; "Reloading");
                    self.set_loading_state(LoadingState::Loading);
                    let query = self.query.read().unwrap().query();
                    let list_workflow_executions_result = self
                        .temporal_client
                        .list_workflow_executions(self.page_size as i32, Vec::new(), query)
                        .await;

                    match list_workflow_executions_result {
                        Ok(response) => self.on_reload(response),
                        Err(e) => self.on_err(anyhow::anyhow!(
                            "list workflow executions request failed: {}",
                            e.to_string()
                        )),
                    }
                }
                Message::LoadPage { page_token } => {
                    log::debug!(widget = "WorkflowTableWidget"; "Loading page {:?}", page_token);
                    self.set_loading_state(LoadingState::Loading);
                    let query = self.query.read().unwrap().query();
                    let list_workflow_executions_result = self
                        .temporal_client
                        .list_workflow_executions(self.page_size as i32, page_token, query)
                        .await;

                    match list_workflow_executions_result {
                        Ok(response) => self.on_page_load(response),
                        Err(e) => self.on_err(anyhow::anyhow!(
                            "list workflow executions request failed: {}",
                            e.to_string()
                        )),
                    }
                }
            }
        }
    }

    fn set_loading_state(&mut self, loading_state: LoadingState) {
        match loading_state {
            LoadingState::Reloaded => {
                let mut last_reload = self.last_reload.write().unwrap();
                *last_reload = Some(time::Instant::now());
            }
            _ => {}
        };
        let mut state = self.state.write().unwrap();
        state.loading_state = loading_state;
    }

    fn on_reload(&mut self, response: service::ListWorkflowExecutionsResponse) {
        self.on_load(response, true);
        self.set_loading_state(LoadingState::Reloaded);
        log::debug!(widget = "WorkflowTableWidget", method = "on_reload"; "Reloaded");
    }

    fn on_page_load(&mut self, response: service::ListWorkflowExecutionsResponse) {
        self.on_load(response, false);
        self.set_loading_state(LoadingState::PageLoaded);
        log::debug!(widget = "WorkflowTableWidget", method = "on_page_load"; "Loaded next page");
    }

    fn on_load(&mut self, response: service::ListWorkflowExecutionsResponse, clear: bool) {
        let executions: Vec<WorkflowExecution> = match response
            .executions
            .into_iter()
            .map(TryInto::try_into)
            .collect()
        {
            Ok(v) => v,
            Err(e) => {
                self.on_err(anyhow::anyhow!(
                    "invalid workflow execution: {}",
                    e.to_string()
                ));
                return;
            }
        };
        let mut state = self.state.write().unwrap();
        state.next_page_token = Some(response.next_page_token);

        if clear {
            state.workflow_executions.clear();
        }

        state.workflow_executions.extend(executions);

        if !state.workflow_executions.is_empty() && clear {
            state.table_state.select(Some(0));
        }
    }

    fn on_err(&mut self, err: anyhow::Error) {
        self.set_loading_state(LoadingState::Error(err.to_string()));
        panic!("error");
    }

    pub async fn reload(&self) {
        let sender = self.sender.as_ref().clone();
        sender.unwrap().send(Message::Reload).await.unwrap();
    }

    pub fn is_loading(&self) -> bool {
        let state = self.state.read().unwrap();
        match state.loading_state {
            LoadingState::Loading => true,
            _ => false,
        }
    }

    pub fn is_error(&self) -> (bool, Option<String>) {
        let state = self.state.read().unwrap();
        match &state.loading_state {
            LoadingState::Error(s) => (true, Some(s.to_owned())),
            _ => (false, None),
        }
    }

    pub async fn load_next_page(&self) {
        let state = self.state.read().unwrap();
        let next_page_token = state.next_page_token.as_ref().cloned();
        if let Some(page_token) = next_page_token {
            let sender = self.sender.as_ref().clone();
            sender
                .unwrap()
                .send(Message::LoadPage { page_token })
                .await
                .unwrap();
        }
    }

    pub async fn next_row(&mut self) {
        let on_last_row = self.is_on_last_row();
        if on_last_row {
            self.load_next_page().await;
            task::yield_now().await;
        }

        loop {
            let on_last_row = self.is_on_last_row();
            if !on_last_row {
                break;
            }
            task::yield_now().await;
        }

        let mut state = self.state.write().unwrap();
        let i = match state.table_state.selected() {
            Some(i) => {
                if i >= state.workflow_executions.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        state.table_state.select(Some(i));
        state.scrollbar_state = state.scrollbar_state.position(i * ITEM_HEIGHT);
    }

    pub fn is_on_last_row(&self) -> bool {
        let state = self.state.read().unwrap();
        match state.table_state.selected() {
            Some(i) => {
                if i >= state.workflow_executions.len() - 1 {
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    pub fn previous_row(&mut self) {
        let mut state = self.state.write().unwrap();
        let i = match state.table_state.selected() {
            Some(i) => {
                if i == 0 {
                    state.workflow_executions.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        state.table_state.select(Some(i));
        state.scrollbar_state = state.scrollbar_state.position(i * ITEM_HEIGHT);
    }

    pub fn get_duration_since_last_reload(&self) -> Option<time::Duration> {
        match self.last_reload.try_read() {
            Ok(last_reload) => match *last_reload {
                Some(instant) => time::Instant::now().checked_duration_since(instant),
                None => None,
            },
            Err(_) => None,
        }
    }

    pub fn get_selected_workflow_id(&self) -> Option<String> {
        let state = self.state.read().unwrap();
        match state.table_state.selected() {
            Some(i) => Some(state.workflow_executions[i].workflow_id.clone()),
            None => None,
        }
    }

    pub async fn handle_query_key(&mut self, key: event::KeyEvent) {
        match key {
            // Mode switch
            event::KeyEvent {
                code: event::KeyCode::Char('q'),
                modifiers: event::KeyModifiers::CONTROL,
                ..
            } => self.set_mode(Mode::Normal),
            // Reload workflow table
            event::KeyEvent {
                code: event::KeyCode::Char('r'),
                modifiers: event::KeyModifiers::CONTROL,
                ..
            } => self.reload().await,
            // Pass along to `QueryInput`
            event::KeyEvent {
                code: event::KeyCode::Char(_),
                ..
            }
            | event::KeyEvent {
                code: event::KeyCode::Backspace,
                ..
            }
            | event::KeyEvent {
                code: event::KeyCode::Left,
                ..
            }
            | event::KeyEvent {
                code: event::KeyCode::Right,
                ..
            } => {
                let mut query_input = self.query.write().unwrap();
                query_input.handle_key(key).await;
            }
            _ => {}
        }
    }

    pub async fn handle_normal_key(&mut self, key: event::KeyEvent) -> Option<WorkflowWidget> {
        match key {
            // Mode switch
            event::KeyEvent {
                code: event::KeyCode::Char('q'),
                modifiers: event::KeyModifiers::CONTROL,
                ..
            } => self.set_mode(Mode::Query),
            // Navigation
            event::KeyEvent {
                code: event::KeyCode::Char('j'),
                ..
            }
            | event::KeyEvent {
                code: event::KeyCode::Down,
                ..
            } => self.next_row().await,
            event::KeyEvent {
                code: event::KeyCode::Char('k'),
                ..
            }
            | event::KeyEvent {
                code: event::KeyCode::Up,
                ..
            } => self.previous_row(),
            // Reload workflow table
            event::KeyEvent {
                code: event::KeyCode::Char('r'),
                modifiers: event::KeyModifiers::CONTROL,
                ..
            } => self.reload().await,
            // Select workflow and switch to workflow widget
            event::KeyEvent {
                code: event::KeyCode::Enter,
                ..
            } => {
                if let Some(workflow_id) = self.get_selected_workflow_id() {
                    let workflow_widget =
                        WorkflowWidget::new(&self.temporal_client, &workflow_id, None, self.theme);
                    return Some(workflow_widget);
                }
            }
            _ => {}
        };
        None
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }
}

impl Keybindable for WorkflowTableWidget {
    async fn handle_key(&mut self, key: event::KeyEvent) -> Option<ViewWidget> {
        match self.mode {
            Mode::Query => {
                if let event::KeyEvent {
                    code: event::KeyCode::Char('r'),
                    modifiers: event::KeyModifiers::CONTROL,
                    ..
                } = key
                {
                    self.reload().await;
                    None
                } else {
                    self.handle_query_key(key).await;
                    None
                }
            }
            Mode::Normal => {
                if let Some(workflow_widget) = self.handle_normal_key(key).await {
                    Some(ViewWidget::Workflow(workflow_widget))
                } else {
                    None
                }
            }
        }
    }

    fn keybinds<'k>(&'k self) -> &'k [(&'k str, &'k [&'k str])] {
        match self.mode {
            Mode::Query => &[("Toggle query", &["Ctrl+q"]), ("Reload", &["Ctrl+r"])],
            Mode::Normal => &[
                ("Up", &["j", "↑"]),
                ("Down", &["k", "↓"]),
                ("View workflow", &["Enter"]),
                ("Toggle query", &["Ctrl+q"]),
                ("Reload", &["Ctrl+r"]),
            ],
        }
    }
}

impl widgets::Widget for &WorkflowTableWidget {
    fn render(self, area: layout::Rect, buf: &mut buffer::Buffer) {
        let vertical =
            &layout::Layout::vertical([layout::Constraint::Length(3), layout::Constraint::Fill(1)]);
        let [header_area, body_area] = vertical.areas(area);

        let header_horizontal = &layout::Layout::horizontal([
            layout::Constraint::Fill(1),
            layout::Constraint::Percentage(20),
        ]);
        let [header_left_area, header_right_area] = header_horizontal.areas(header_area);

        let last_reload_string = match self.get_duration_since_last_reload() {
            Some(duration) => format!("Last reload: {}s ago", duration.as_secs()),
            None => "Last reload: N/A".to_string(),
        };

        let last_reload_title = widgets::Paragraph::new(text::Text::from(last_reload_string))
            .style(style::Style::new().fg(self.theme.foreground))
            .right_aligned();

        let query_input = self.query.read().unwrap();
        widgets::Widget::render(&(*query_input), header_left_area, buf);
        widgets::Widget::render(last_reload_title, header_right_area, buf);

        let table_block = widgets::Block::bordered()
            .title(
                text::Line::from("Workflows")
                    .left_aligned()
                    .fg(self.theme.header_foreground)
                    .bold(),
            )
            .border_type(widgets::BorderType::Rounded)
            .border_style(style::Style::new().fg(self.theme.border))
            .bg(self.theme.background);

        let header_style = style::Style::default()
            .fg(self.theme.header_foreground)
            .bg(self.theme.header_background);
        let selected_row_style = style::Style::default()
            .add_modifier(style::Modifier::REVERSED)
            .fg(self.theme.selection_background);
        let selected_col_style = style::Style::default().fg(self.theme.selection_background);
        let selected_cell_style = style::Style::default()
            .add_modifier(style::Modifier::REVERSED)
            .fg(self.theme.selection_background);

        let header = [
            "Status",
            "Type",
            "Workflow ID",
            "Task Queue",
            "Start Time",
            "Close Time",
        ]
        .into_iter()
        .map(widgets::Cell::from)
        .collect::<widgets::Row>()
        .style(header_style)
        .height(1);

        let mut state = self.state.write().unwrap();

        let rows = state
            .workflow_executions
            .iter()
            .enumerate()
            .map(|(i, execution)| {
                let color = match i % 2 {
                    0 => self.theme.background,
                    _ => self.theme.alt_background,
                };
                let status_color = execution.status_color_from_theme(self.theme);

                widgets::Row::new(vec![
                    widgets::Cell::from(execution.status_as_string()).bg(status_color),
                    widgets::Cell::new(execution.r#type.clone()),
                    widgets::Cell::new(execution.workflow_id.clone()),
                    widgets::Cell::new(execution.task_queue.clone()),
                    widgets::Cell::new(
                        execution
                            .start_time
                            .and_then(|dt| Some(format!("{}", dt.format("%y-%m-%d %H:%M:%S %Z"))))
                            .unwrap_or("".to_string()),
                    ),
                    widgets::Cell::new(
                        execution
                            .close_time
                            .and_then(|dt| Some(format!("{}", dt.format("%y-%m-%d %H:%M:%S %Z"))))
                            .unwrap_or("".to_string()),
                    ),
                ])
                .style(style::Style::new().fg(self.theme.foreground).bg(color))
                .height(1)
            });
        let bar = " █ ";
        let table = widgets::Table::new(
            rows,
            [
                layout::Constraint::Length(18),
                layout::Constraint::Length(32),
                layout::Constraint::Length(64),
                layout::Constraint::Length(32),
                layout::Constraint::Length(32),
                layout::Constraint::Length(32),
            ],
        )
        .block(table_block)
        .header(header)
        .row_highlight_style(selected_row_style)
        .column_highlight_style(selected_col_style)
        .cell_highlight_style(selected_cell_style)
        .highlight_symbol(text::Text::from(vec![
            "".into(),
            bar.into(),
            bar.into(),
            "".into(),
        ]))
        .bg(self.theme.background)
        .highlight_spacing(widgets::HighlightSpacing::Always);

        widgets::StatefulWidget::render(table, body_area, buf, &mut state.table_state);
    }
}
