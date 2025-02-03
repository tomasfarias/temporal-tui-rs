use std::collections;
use std::sync;

use ratatui::{buffer, layout, style, style::Stylize, text, widgets};
use temporal_client::WorkflowClientTrait;
use temporal_sdk_core_protos::temporal::api::{
    workflow::v1::PendingActivityInfo, workflowservice::v1::DescribeWorkflowExecutionResponse,
};
use tokio::sync::mpsc;
use tokio::time;

use crate::theme::Theme;
use crate::widgets::common::{LoadingState, Message, WorkflowExecution, WorkflowExecutionStatus};

#[derive(Debug, Clone)]
pub enum PendingActivityState {
    Unspecified,
    Scheduled,
    Started,
    CancelRequested,
}

impl<'m> PendingActivityState {
    pub fn as_str(&'m self) -> &'m str {
        match self {
            PendingActivityState::Unspecified => "Unspecified",
            PendingActivityState::Scheduled => "Scheduled",
            PendingActivityState::Started => "Started",
            PendingActivityState::CancelRequested => "CancelRequested",
        }
    }
}

impl TryFrom<i32> for PendingActivityState {
    type Error = anyhow::Error;

    fn try_from(status: i32) -> Result<Self, Self::Error> {
        match status {
            0 => Ok(PendingActivityState::Unspecified),
            1 => Ok(PendingActivityState::Scheduled),
            2 => Ok(PendingActivityState::Started),
            3 => Ok(PendingActivityState::CancelRequested),
            i => Err(anyhow::anyhow!("invalid status: {}", i)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Payload {
    metadata: collections::HashMap<String, Vec<u8>>,
    data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Failure {
    message: String,
    source: String,
    stack_trace: String,
}

#[derive(Debug, Clone)]
pub struct PendingActivity {
    id: String,
    r#type: Option<String>,
    state: PendingActivityState,
    heartbeat_details: Option<Vec<Payload>>,
    last_heartbeat_time: Option<chrono::DateTime<chrono::Utc>>,
    last_started_time: Option<chrono::DateTime<chrono::Utc>>,
    attempt: u32,
    maximum_attempts: u32,
    scheduled_time: Option<chrono::DateTime<chrono::Utc>>,
    expiration_time: Option<chrono::DateTime<chrono::Utc>>,
    last_failure: Option<Failure>,
    last_worker_identity: String,
    last_attempt_complete_time: Option<chrono::DateTime<chrono::Utc>>,
    next_attempt_schedule_time: Option<chrono::DateTime<chrono::Utc>>,
}

impl TryFrom<PendingActivityInfo> for PendingActivity {
    type Error = anyhow::Error;

    fn try_from(info: PendingActivityInfo) -> Result<Self, Self::Error> {
        let state = PendingActivityState::try_from(info.state)?;
        let last_failure = if let Some(f) = info.last_failure {
            Some(Failure {
                message: f.message,
                source: f.source,
                stack_trace: f.stack_trace,
            })
        } else {
            None
        };
        let heartbeat_details: Option<Vec<Payload>> = if let Some(payloads) = info.heartbeat_details
        {
            Some(
                payloads
                    .payloads
                    .into_iter()
                    .map(|p| {
                        let metadata = collections::HashMap::from_iter(p.metadata);
                        Payload {
                            metadata,
                            data: p.data,
                        }
                    })
                    .collect(),
            )
        } else {
            None
        };

        Ok(Self {
            id: info.activity_id,
            r#type: info.activity_type.map(|a| a.name),
            state,
            heartbeat_details,
            last_heartbeat_time: info
                .last_heartbeat_time
                .and_then(|t| chrono::DateTime::from_timestamp(t.seconds, t.nanos as u32)),
            last_started_time: info
                .last_heartbeat_time
                .and_then(|t| chrono::DateTime::from_timestamp(t.seconds, t.nanos as u32)),
            attempt: info.attempt as u32,
            maximum_attempts: info.maximum_attempts as u32,
            scheduled_time: info
                .scheduled_time
                .and_then(|t| chrono::DateTime::from_timestamp(t.seconds, t.nanos as u32)),
            last_failure,
            last_worker_identity: info.last_worker_identity,
            expiration_time: info
                .expiration_time
                .and_then(|t| chrono::DateTime::from_timestamp(t.seconds, t.nanos as u32)),
            last_attempt_complete_time: info
                .last_attempt_complete_time
                .and_then(|t| chrono::DateTime::from_timestamp(t.seconds, t.nanos as u32)),
            next_attempt_schedule_time: info
                .next_attempt_schedule_time
                .and_then(|t| chrono::DateTime::from_timestamp(t.seconds, t.nanos as u32)),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Workflow {
    pending_activities: Vec<PendingActivity>,
    execution: Option<WorkflowExecution>,
}

#[derive(Debug, Clone)]
pub struct WorkflowWidget {
    temporal_client: sync::Arc<temporal_client::RetryClient<temporal_client::Client>>,
    sender: sync::Arc<Option<mpsc::Sender<Message>>>,
    theme: Theme,
    /// The ID of the workflow we are displaying.
    workflow_id: String,
    /// The ID of the workflow run we are displaying.
    run_id: Option<String>,
    /// The actual workflow data
    workflow: sync::Arc<sync::RwLock<Option<Workflow>>>,
    last_reload: sync::Arc<sync::RwLock<Option<time::Instant>>>,
    loading_state: sync::Arc<sync::RwLock<LoadingState>>,
}

impl WorkflowWidget {
    pub fn new(
        temporal_client: &sync::Arc<temporal_client::RetryClient<temporal_client::Client>>,
        workflow_id: &str,
        run_id: Option<&str>,
        theme: Theme,
    ) -> Self {
        Self {
            temporal_client: temporal_client.clone(),
            sender: sync::Arc::new(None),
            theme,
            workflow_id: workflow_id.to_owned(),
            run_id: run_id.map(|s| s.to_owned()),
            last_reload: sync::Arc::new(sync::RwLock::new(None)),
            workflow: sync::Arc::new(sync::RwLock::new(None)),
            loading_state: sync::Arc::new(sync::RwLock::new(LoadingState::Idle)),
        }
    }

    pub fn run(&mut self) {
        let (tx, rx) = mpsc::channel(32);
        *sync::Arc::get_mut(&mut self.sender).unwrap() = Some(tx);

        let this = self.clone(); // clone the widget to pass to the background task
        tokio::spawn(this.fetch_workflow(rx));
    }

    async fn fetch_workflow(mut self, mut receiver: mpsc::Receiver<Message>) {
        log::debug!(widget = "WorkflowWidget"; "Starting fetch_workflow loop");
        while let Some(message) = receiver.recv().await {
            match message {
                Message::Reload => {
                    log::debug!(widget = "WorfklowWidget"; "Reloading");
                    self.set_loading_state(LoadingState::Loading);
                    let describe_workflow_execution_result = self
                        .temporal_client
                        .describe_workflow_execution(self.workflow_id.clone(), self.run_id.clone())
                        .await;

                    match describe_workflow_execution_result {
                        Ok(response) => self.on_reload(response),
                        Err(e) => self.on_err(anyhow::anyhow!(
                            "describe workflow executions request failed: {}",
                            e.to_string()
                        )),
                    }
                }
                _ => {}
            }
        }
    }

    fn on_reload(&mut self, response: DescribeWorkflowExecutionResponse) {
        self.on_load(response);
        self.set_loading_state(LoadingState::Reloaded);
        log::debug!(widget = "WorkflowWidget"; "Reloaded");
    }

    fn on_load(&mut self, response: DescribeWorkflowExecutionResponse) {
        let execution = match response.workflow_execution_info {
            Some(info) => match WorkflowExecution::try_from(info) {
                Ok(e) => Some(e),
                Err(e) => {
                    self.on_err(anyhow::anyhow!(
                        "invalid workflow execution: {}",
                        e.to_string()
                    ));
                    return;
                }
            },
            None => None,
        };
        let pending_activities: Vec<PendingActivity> = match response
            .pending_activities
            .into_iter()
            .map(TryInto::try_into)
            .collect()
        {
            Ok(v) => v,
            Err(e) => {
                self.on_err(anyhow::anyhow!(
                    "invalid workflow pending activity: {}",
                    e.to_string()
                ));
                return;
            }
        };
        let workflow = Workflow {
            execution,
            pending_activities,
        };

        let mut workflow_lock = self.workflow.write().unwrap();
        *workflow_lock = Some(workflow);
    }

    fn on_err(&mut self, err: anyhow::Error) {
        self.set_loading_state(LoadingState::Error(err.to_string()));
        panic!("error");
    }

    fn set_loading_state(&mut self, loading_state: LoadingState) {
        match loading_state {
            LoadingState::Reloaded => {
                let mut last_reload = self.last_reload.write().unwrap();
                *last_reload = Some(time::Instant::now());
            }
            _ => {}
        };
        let mut loading_state_lock = self.loading_state.write().unwrap();
        *loading_state_lock = loading_state;
    }

    pub async fn reload(&self) {
        let sender = self.sender.as_ref().clone();
        sender.unwrap().send(Message::Reload).await.unwrap();
    }
}

impl widgets::Widget for &WorkflowWidget {
    fn render(self, area: layout::Rect, buf: &mut buffer::Buffer) {
        let vertical =
            &layout::Layout::vertical([layout::Constraint::Length(9), layout::Constraint::Fill(1)]);
        let [header_area, body_area] = vertical.areas(area);

        let workflow_lock = self.workflow.read().unwrap();
        let workflow_ref = &*workflow_lock;
        let status = if let Some(workflow) = workflow_ref {
            if let Some(execution) = workflow.execution.as_ref() {
                &execution.status
            } else {
                &WorkflowExecutionStatus::Unspecified
            }
        } else {
            &WorkflowExecutionStatus::Unspecified
        };

        let status_color = match status {
            WorkflowExecutionStatus::Unspecified => self.theme.cancelled_background,
            WorkflowExecutionStatus::Running => self.theme.running_background,
            WorkflowExecutionStatus::Completed => self.theme.success_background,
            WorkflowExecutionStatus::Failed => self.theme.failure_background,
            WorkflowExecutionStatus::Canceled => self.theme.cancelled_background,
            WorkflowExecutionStatus::Terminated => self.theme.failure_background,
            WorkflowExecutionStatus::ContinuedAsNew => self.theme.cancelled_background,
            WorkflowExecutionStatus::TimedOut => self.theme.failure_background,
        };

        let header_block = widgets::Block::bordered()
            .title(text::Span::from(
                String::from(status)
                    .bg(status_color)
                    .fg(self.theme.foreground),
            ))
            .title(text::Span::from(self.workflow_id.clone()));

        let inner_header_area = header_block.inner(header_area);

        widgets::Widget::render(header_block, header_area, buf);

        let header_horizontal =
            &layout::Layout::horizontal([layout::Constraint::Fill(1), layout::Constraint::Fill(1)]);
        let [header_left_area, header_right_area] = header_horizontal.areas(inner_header_area);

        let left_keys = widgets::Paragraph::new(vec![
            text::Line::raw("Start").left_aligned(),
            text::Line::raw("End").left_aligned(),
            text::Line::raw("Duration").left_aligned(),
            text::Line::raw("Run ID").left_aligned(),
            text::Line::raw("Workflow Type").left_aligned(),
            text::Line::raw("Task Queue").left_aligned(),
            text::Line::raw("History Size (Bytes)").left_aligned(),
        ]);

        let [start_time, end_time, workflow_type, task_queue, history_size_bytes] =
            if let Some(workflow) = workflow_ref {
                if let Some(execution) = workflow.execution.as_ref() {
                    [
                        execution.start_time_string(),
                        execution.close_time_string(),
                        execution.r#type.clone(),
                        execution.task_queue.clone(),
                        format!("{}", execution.history_size_bytes),
                    ]
                } else {
                    [
                        "-".to_string(),
                        "-".to_string(),
                        "".to_string(),
                        "".to_string(),
                        "0".to_string(),
                    ]
                }
            } else {
                [
                    "-".to_string(),
                    "-".to_string(),
                    "".to_string(),
                    "".to_string(),
                    "0".to_string(),
                ]
            };

        let right_values = widgets::Paragraph::new(vec![
            text::Line::raw(start_time).right_aligned(),
            text::Line::raw(end_time).right_aligned(),
            text::Line::raw("Duration").right_aligned(),
            text::Line::raw("Run ID").right_aligned(),
            text::Line::raw(workflow_type).right_aligned(),
            text::Line::raw(task_queue).right_aligned(),
            text::Line::raw(history_size_bytes).right_aligned(),
        ]);

        widgets::Widget::render(left_keys, header_left_area, buf);
        widgets::Widget::render(right_values, header_right_area, buf);
    }
}
