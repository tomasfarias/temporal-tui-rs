use std::error::Error;
use std::ops;
use std::sync;

use ratatui::{buffer, layout, style, style::palette::tailwind, style::Stylize, text, widgets};
use temporal_client::{self, tonic::Status, WorkflowClientTrait};
use temporal_sdk_core_protos::temporal::api::{
    common::v1::WorkflowType, workflow::v1::WorkflowExecutionInfo,
    workflowservice::v1::ListWorkflowExecutionsResponse,
};
use tokio::sync::mpsc;
use tokio::task;
use tokio::time;

const PALETTES: [tailwind::Palette; 4] = [
    tailwind::BLUE,
    tailwind::EMERALD,
    tailwind::INDIGO,
    tailwind::RED,
];

const ITEM_HEIGHT: usize = 1;

const FOOTER_INFO_TEXT: [&str; 1] = ["(q) quit | (↑/j) move up | (↓/k) move down | (r) reload"];

#[derive(Debug, Clone)]
pub struct WorkflowTableWidget {
    state: sync::Arc<sync::RwLock<WorkflowTableState>>,
    temporal_client: sync::Arc<temporal_client::RetryClient<temporal_client::Client>>,
    sender: sync::Arc<Option<mpsc::Sender<Message>>>,
    page_size: u32,
    colors: TableColors,
    last_reload: sync::Arc<sync::RwLock<Option<time::Instant>>>,
}

#[derive(Debug, Default)]
struct WorkflowTableState {
    workflow_executions: Vec<WorkflowExecutionRow>,
    next_page_token: Option<Vec<u8>>,
    loading_state: LoadingState,
    table_state: widgets::TableState,
    scrollbar_state: widgets::ScrollbarState,
}

#[derive(Debug, Clone)]
struct TableColors {
    buffer_bg: style::Color,
    header_bg: style::Color,
    header_fg: style::Color,
    row_fg: style::Color,
    selected_row_style_fg: style::Color,
    selected_column_style_fg: style::Color,
    selected_cell_style_fg: style::Color,
    normal_row_color: style::Color,
    alt_row_color: style::Color,
    footer_border_color: style::Color,
}

impl TableColors {
    const fn new(color: &tailwind::Palette) -> Self {
        Self {
            buffer_bg: tailwind::SLATE.c950,
            header_bg: color.c900,
            header_fg: tailwind::SLATE.c200,
            row_fg: tailwind::SLATE.c200,
            selected_row_style_fg: color.c400,
            selected_column_style_fg: color.c400,
            selected_cell_style_fg: color.c600,
            normal_row_color: tailwind::SLATE.c950,
            alt_row_color: tailwind::SLATE.c900,
            footer_border_color: color.c400,
        }
    }
}

#[derive(Debug, Default)]
struct WorkflowExecutionRow {
    status: WorkflowExecutionStatus,
    r#type: String,
    workflow_id: String,
    task_queue: String,
    start_time: Option<chrono::DateTime<chrono::Utc>>,
    close_time: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug)]
enum WorkflowExecutionStatus {
    Unspecified,
    Running,
    Completed,
    Failed,
    Canceled,
    Terminated,
    ContinuedAsNew,
    TimedOut,
}

impl TryFrom<i32> for WorkflowExecutionStatus {
    type Error = anyhow::Error;

    fn try_from(status: i32) -> Result<Self, Self::Error> {
        match status {
            0 => Ok(WorkflowExecutionStatus::Unspecified),
            1 => Ok(WorkflowExecutionStatus::Running),
            2 => Ok(WorkflowExecutionStatus::Completed),
            3 => Ok(WorkflowExecutionStatus::Failed),
            4 => Ok(WorkflowExecutionStatus::Canceled),
            5 => Ok(WorkflowExecutionStatus::Terminated),
            6 => Ok(WorkflowExecutionStatus::ContinuedAsNew),
            7 => Ok(WorkflowExecutionStatus::TimedOut),
            i => Err(anyhow::anyhow!("invalid status: {}", i)),
        }
    }
}

impl From<&WorkflowExecutionStatus> for String {
    fn from(status: &WorkflowExecutionStatus) -> Self {
        match status {
            WorkflowExecutionStatus::Unspecified => "Unspecified".to_string(),
            WorkflowExecutionStatus::Running => "Running".to_string(),
            WorkflowExecutionStatus::Completed => "Completed".to_string(),
            WorkflowExecutionStatus::Failed => "Failed".to_string(),
            WorkflowExecutionStatus::Canceled => "Canceled".to_string(),
            WorkflowExecutionStatus::Terminated => "Terminated".to_string(),
            WorkflowExecutionStatus::ContinuedAsNew => "Continued-As-New".to_string(),
            WorkflowExecutionStatus::TimedOut => "Timed-Out".to_string(),
        }
    }
}

impl From<&WorkflowExecutionStatus> for widgets::Cell<'_> {
    fn from(status: &WorkflowExecutionStatus) -> Self {
        let s = String::from(status);
        let cell = widgets::Cell::new(s);
        match status {
            WorkflowExecutionStatus::Unspecified => cell.bg(tailwind::GRAY.c300),
            WorkflowExecutionStatus::Running => cell.bg(tailwind::INDIGO.c500),
            WorkflowExecutionStatus::Completed => cell.bg(tailwind::GREEN.c600),
            WorkflowExecutionStatus::Failed => cell.bg(tailwind::RED.c600),
            WorkflowExecutionStatus::Canceled => cell.bg(tailwind::YELLOW.c600),
            WorkflowExecutionStatus::Terminated => cell.fg(tailwind::RED.c600),
            WorkflowExecutionStatus::ContinuedAsNew => cell.bg(tailwind::GRAY.c300),
            WorkflowExecutionStatus::TimedOut => cell.bg(tailwind::RED.c600),
        }
    }
}

impl Default for WorkflowExecutionStatus {
    fn default() -> Self {
        Self::Unspecified
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum LoadingState {
    #[default]
    Idle,
    Reloaded,
    Loading,
    PageLoaded,
    Error(String),
}

#[derive(Debug)]
enum Message {
    Reload,
    LoadPage { page_token: Vec<u8> },
}

impl WorkflowTableWidget {
    pub fn new(
        temporal_client: &sync::Arc<temporal_client::RetryClient<temporal_client::Client>>,
        page_size: u32,
    ) -> Self {
        Self {
            state: sync::Arc::new(sync::RwLock::new(WorkflowTableState::default())),
            temporal_client: temporal_client.clone(),
            sender: sync::Arc::new(None),
            page_size,
            colors: TableColors::new(&PALETTES[0]),
            last_reload: sync::Arc::new(sync::RwLock::new(None)),
        }
    }

    pub async fn run(&mut self, reload_immediatelly: bool) {
        let (tx, rx) = mpsc::channel(32);
        *sync::Arc::get_mut(&mut self.sender).unwrap() = Some(tx);

        let this = self.clone(); // clone the widget to pass to the background task
        tokio::spawn(this.fetch_workflows(rx));

        if reload_immediatelly {
            self.reload().await;
        }
    }

    async fn fetch_workflows(mut self, mut receiver: mpsc::Receiver<Message>) {
        log::debug!(widget = "WorkflowTableWidget"; "Starting fetch_workflows loop");
        while let Some(message) = receiver.recv().await {
            match message {
                Message::Reload => {
                    log::debug!(widget = "WorkflowTableWidget"; "Reloading");
                    self.set_loading_state(LoadingState::Loading);
                    let list_workflow_executions_result = self
                        .temporal_client
                        .list_workflow_executions(self.page_size as i32, Vec::new(), "".to_string())
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
                    let list_workflow_executions_result = self
                        .temporal_client
                        .list_workflow_executions(self.page_size as i32, page_token, "".to_string())
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

    fn on_reload(&mut self, response: ListWorkflowExecutionsResponse) {
        self.on_load(response, true);
        self.set_loading_state(LoadingState::Reloaded);
        log::debug!(widget = "WorkflowTableWidget"; "Reloaded");
    }

    fn on_page_load(&mut self, response: ListWorkflowExecutionsResponse) {
        self.on_load(response, false);
        self.set_loading_state(LoadingState::PageLoaded);
        log::debug!(widget = "WorkflowTableWidget"; "Loaded next page");
    }

    fn on_load(&mut self, response: ListWorkflowExecutionsResponse, clear: bool) {
        let executions: Vec<WorkflowExecutionRow> = match response
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
}

impl widgets::Widget for &WorkflowTableWidget {
    fn render(self, area: layout::Rect, buf: &mut buffer::Buffer) {
        let vertical =
            &layout::Layout::vertical([layout::Constraint::Fill(1), layout::Constraint::Length(3)]);
        let rects = vertical.split(area);

        // Render table
        let mut state = self.state.write().unwrap();
        let header_style = style::Style::default()
            .fg(self.colors.header_fg)
            .bg(self.colors.header_bg);
        let selected_row_style = style::Style::default()
            .add_modifier(style::Modifier::REVERSED)
            .fg(self.colors.selected_row_style_fg);
        let selected_col_style = style::Style::default().fg(self.colors.selected_column_style_fg);
        let selected_cell_style = style::Style::default()
            .add_modifier(style::Modifier::REVERSED)
            .fg(self.colors.selected_cell_style_fg);

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

        let rows = state
            .workflow_executions
            .iter()
            .enumerate()
            .map(|(i, execution)| {
                let color = match i % 2 {
                    0 => self.colors.normal_row_color,
                    _ => self.colors.alt_row_color,
                };
                widgets::Row::from(execution)
                    .style(style::Style::new().fg(self.colors.row_fg).bg(color))
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
        .bg(self.colors.buffer_bg)
        .highlight_spacing(widgets::HighlightSpacing::Always);

        widgets::StatefulWidget::render(table, rects[0], buf, &mut state.table_state);

        // Render footer
        let info_footer = widgets::Paragraph::new(text::Text::from_iter(FOOTER_INFO_TEXT))
            .style(
                style::Style::new()
                    .fg(self.colors.row_fg)
                    .bg(self.colors.buffer_bg),
            )
            .centered()
            .block(
                widgets::Block::bordered()
                    .border_type(widgets::BorderType::Double)
                    .border_style(style::Style::new().fg(self.colors.footer_border_color)),
            );
        widgets::Widget::render(info_footer, rects[1], buf);
    }
}

impl From<&WorkflowExecutionRow> for widgets::Row<'_> {
    fn from(execution: &WorkflowExecutionRow) -> Self {
        widgets::Row::new(vec![
            widgets::Cell::from(&execution.status),
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
    }
}

impl TryFrom<WorkflowExecutionInfo> for WorkflowExecutionRow {
    type Error = anyhow::Error;

    fn try_from(execution_info: WorkflowExecutionInfo) -> Result<Self, Self::Error> {
        Ok(WorkflowExecutionRow {
            status: WorkflowExecutionStatus::try_from(execution_info.status)?,
            r#type: execution_info
                .r#type
                .ok_or(anyhow::anyhow!("workflow execution has no type"))?
                .name,
            workflow_id: execution_info
                .execution
                .ok_or(anyhow::anyhow!("workflow execution has no workflow id"))?
                .workflow_id,
            task_queue: execution_info.task_queue,
            start_time: execution_info.start_time.and_then(|start_time| {
                chrono::DateTime::from_timestamp(start_time.seconds, start_time.nanos as u32)
            }),
            close_time: execution_info.close_time.and_then(|close_time| {
                chrono::DateTime::from_timestamp(close_time.seconds, close_time.nanos as u32)
            }),
        })
    }
}
