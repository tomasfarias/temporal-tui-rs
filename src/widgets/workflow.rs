use std::str;

use crossterm::event;
use ratatui::{
    buffer, layout, prelude::StatefulWidget, prelude::Widget, style, style::Stylize, text, widgets,
};
use std::collections;
use std::sync;
use temporal_client::WorkflowClientTrait;
use temporal_sdk_core_protos::temporal::api::{
    common::v1 as temporal_common, enums::v1 as enums, history::v1 as history, sdk::v1 as sdk,
    workflow::v1 as workflow, workflowservice::v1 as service,
};
use tokio::sync::mpsc;
use tokio::task;
use tokio::time;

use crate::theme::Theme;
use crate::widgets::common::{LoadingState, Message, WorkflowExecution};
use crate::widgets::workflow_table::WorkflowTableWidget;
use crate::widgets::{Keybindable, ViewWidget};

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
    title: Option<String>,
}

impl Payload {
    fn with_title(mut self, title: &str) -> Self {
        self.title = Some(title.to_owned());
        self
    }

    fn to_string_pretty(&self) -> String {
        let data = str::from_utf8(&self.data).unwrap();
        let metadata: collections::HashMap<&str, &str> = collections::HashMap::from_iter(
            self.metadata
                .iter()
                .map(|(key, val)| (key.as_str(), str::from_utf8(&val).unwrap())),
        );
        let dumped = serde_json::json!({
            "metadata": metadata,
            "data": data,
        });

        serde_json::to_string_pretty(&dumped).unwrap()
    }
}

impl From<temporal_common::Payload> for Payload {
    fn from(p: temporal_common::Payload) -> Self {
        Self {
            metadata: collections::HashMap::from_iter(p.metadata),
            data: p.data,
            title: None,
        }
    }
}

impl From<&temporal_common::Payload> for Payload {
    fn from(p: &temporal_common::Payload) -> Self {
        Self {
            metadata: collections::HashMap::from_iter(p.metadata.clone()),
            data: p.data.clone(),
            title: None,
        }
    }
}

impl widgets::Widget for &Payload {
    fn render(self, area: layout::Rect, buf: &mut buffer::Buffer) {
        let mut payload_block =
            widgets::Block::bordered().border_type(widgets::BorderType::Rounded);

        if let Some(title) = self.title.as_ref() {
            payload_block = payload_block.title(title.as_str());
        }

        widgets::Paragraph::new(self.to_string_pretty())
            .block(payload_block)
            .wrap(widgets::Wrap { trim: false })
            .render(area, buf);
    }
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
    state: enums::PendingActivityState,
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

impl TryFrom<workflow::PendingActivityInfo> for PendingActivity {
    type Error = anyhow::Error;

    fn try_from(info: workflow::PendingActivityInfo) -> Result<Self, Self::Error> {
        let state = enums::PendingActivityState::try_from(info.state)?;
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
                    .map(|p| Payload::from(p).with_title("Heartbeat details"))
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
pub struct Attributes {
    inner: Option<history::history_event::Attributes>,
    table_state: widgets::TableState,
    theme: Theme,
}

#[derive(Debug, Clone)]
pub struct EventWidget {
    id: i64,
    time: Option<chrono::DateTime<chrono::Utc>>,
    r#type: enums::EventType,
    attributes: Option<history::history_event::Attributes>,
}

impl EventWidget {
    pub fn time_as_string(&self) -> String {
        match self.time {
            Some(dt) => format!("{}", dt.format("%y-%m-%d %H:%M:%S %Z")),
            None => "-".to_owned(),
        }
    }

    pub fn type_as_string(&self) -> String {
        self.r#type
            .as_str_name()
            .replace("_", " ")
            .split_inclusive(" ")
            .map(|s| {
                s.to_lowercase()
                    .char_indices()
                    .map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c })
                    .collect::<String>()
            })
            .filter(|s| s.as_str() != "Event " && s.as_str() != "Type ")
            .collect::<String>()
    }
}

impl widgets::Widget for &EventWidget {
    fn render(self, area: layout::Rect, buf: &mut buffer::Buffer) {
        if let Some(inner) = self.attributes.as_ref() {
            match inner {
                history::history_event::Attributes::WorkflowExecutionStartedEventAttributes(
                    attrs,
                ) => {
                    let areas = layout::Layout::vertical([
                        layout::Constraint::Length(9),
                        layout::Constraint::Fill(1),
                    ])
                    .split(area);

                    let lines = vec![
                        text::Line::from(vec![
                            "Workflow type name: ".into(),
                            text::Span::from(&attrs.workflow_type.as_ref().unwrap().name),
                        ]),
                        text::Line::from(vec![
                            "Task queue name: ".into(),
                            text::Span::from(&attrs.task_queue.as_ref().unwrap().name),
                        ]),
                        text::Line::from(vec![
                            "Task queue kind: ".into(),
                            text::Span::from(match attrs.task_queue.as_ref().unwrap().kind {
                                1 => enums::TaskQueueKind::Unspecified {}.as_str_name(),
                                2 => enums::TaskQueueKind::Normal {}.as_str_name(),
                                _ => enums::TaskQueueKind::Sticky {}.as_str_name(),
                            }),
                        ]),
                        text::Line::from(vec![
                            "Workflow task timeout: ".into(),
                            text::Span::from(match attrs.workflow_task_timeout {
                                Some(dur) => {
                                    format!("{}s", time::Duration::try_from(dur).unwrap().as_secs())
                                }
                                None => "-".to_owned(),
                            }),
                        ]),
                        text::Line::from(vec![
                            "Attempt: ".into(),
                            text::Span::from(format!("{}", attrs.attempt)),
                        ]),
                        text::Line::from(vec![
                            "Original execution run ID: ".into(),
                            text::Span::from(&attrs.original_execution_run_id),
                        ]),
                        text::Line::from(vec![
                            "Identity: ".into(),
                            text::Span::from(&attrs.identity),
                        ]),
                        text::Line::from(vec![
                            "First execution run ID: ".into(),
                            text::Span::from(&attrs.first_execution_run_id),
                        ]),
                        text::Line::from(vec![
                            "Workflow ID: ".into(),
                            text::Span::from(&attrs.workflow_id),
                        ]),
                    ];

                    widgets::Paragraph::new(lines).render(areas[0], buf);

                    if let Some(payloads) = attrs.input.as_ref() {
                        for p in payloads.payloads.iter() {
                            let payload = Payload::from(p).with_title("Input");
                            payload.render(areas[1], buf);
                        }
                    }
                }
                history::history_event::Attributes::WorkflowTaskScheduledEventAttributes(attrs) => {
                    let lines = vec![
                        text::Line::from(vec![
                            "Task queue name: ".into(),
                            text::Span::from(&attrs.task_queue.as_ref().unwrap().name),
                        ]),
                        text::Line::from(vec![
                            "Task queue kind: ".into(),
                            text::Span::from(match attrs.task_queue.as_ref().unwrap().kind {
                                1 => enums::TaskQueueKind::Unspecified {}.as_str_name(),
                                2 => enums::TaskQueueKind::Normal {}.as_str_name(),
                                _ => enums::TaskQueueKind::Sticky {}.as_str_name(),
                            }),
                        ]),
                        text::Line::from(vec![
                            "Start to close timeout: ".into(),
                            text::Span::from(if let Some(dur) = attrs.start_to_close_timeout {
                                format!("{}s", time::Duration::try_from(dur).unwrap().as_secs())
                            } else {
                                "-".to_owned()
                            }),
                        ]),
                        text::Line::from(vec![
                            "Attempt: ".into(),
                            text::Span::from(format!("{}", attrs.attempt)),
                        ]),
                    ];
                    widgets::Paragraph::new(lines).render(area, buf);
                }
                history::history_event::Attributes::WorkflowTaskStartedEventAttributes(attrs) => {
                    let lines = vec![
                        text::Line::from(vec![
                            "Scheduled event ID: ".into(),
                            text::Span::from(format!("{}", attrs.scheduled_event_id)),
                        ]),
                        text::Line::from(vec![
                            "Identity: ".into(),
                            text::Span::from(&attrs.identity),
                        ]),
                        text::Line::from(vec![
                            "Request ID: ".into(),
                            text::Span::from(&attrs.request_id),
                        ]),
                        text::Line::from(vec![
                            "History size bytes: ".into(),
                            text::Span::from(format!("{}", &attrs.history_size_bytes)),
                        ]),
                        text::Line::from(vec![
                            "Worker version: ".into(),
                            text::Span::from(match &attrs.worker_version {
                                Some(ts) => &ts.build_id,
                                None => "-",
                            }),
                        ]),
                    ];
                    widgets::Paragraph::new(lines).render(area, buf);
                }
                history::history_event::Attributes::WorkflowTaskCompletedEventAttributes(attrs) => {
                    let lines = vec![
                        text::Line::from(vec![
                            "Scheduled event ID: ".into(),
                            text::Span::from(format!("{}", attrs.scheduled_event_id)),
                        ]),
                        text::Line::from(vec![
                            "Started event ID: ".into(),
                            text::Span::from(format!("{}", attrs.started_event_id)),
                        ]),
                        text::Line::from(vec![
                            "Identity: ".into(),
                            text::Span::from(&attrs.identity),
                        ]),
                        text::Line::from(vec![
                            "Worker version: ".into(),
                            text::Span::from(if let Some(ts) = &attrs.worker_version {
                                &ts.build_id
                            } else {
                                "-"
                            }),
                        ]),
                    ];
                    widgets::Paragraph::new(lines).render(area, buf);
                }
                history::history_event::Attributes::ActivityTaskScheduledEventAttributes(attrs) => {
                    let areas = layout::Layout::vertical([
                        layout::Constraint::Length(10),
                        layout::Constraint::Fill(1),
                        layout::Constraint::Fill(1),
                        layout::Constraint::Fill(1),
                    ])
                    .split(area);

                    let lines = vec![
                        text::Line::from(vec![
                            "Activity ID: ".into(),
                            text::Span::from(&attrs.activity_id),
                        ]),
                        text::Line::from(vec![
                            "Activity type: ".into(),
                            text::Span::from(if let Some(activity_type) = &attrs.activity_type {
                                &activity_type.name
                            } else {
                                "-"
                            }),
                        ]),
                        text::Line::from(vec![
                            "Task queue name: ".into(),
                            text::Span::from(&attrs.task_queue.as_ref().unwrap().name),
                        ]),
                        text::Line::from(vec![
                            "Task queue kind: ".into(),
                            text::Span::from(match attrs.task_queue.as_ref().unwrap().kind {
                                1 => enums::TaskQueueKind::Unspecified {}.as_str_name(),
                                2 => enums::TaskQueueKind::Normal {}.as_str_name(),
                                _ => enums::TaskQueueKind::Sticky {}.as_str_name(),
                            }),
                        ]),
                        text::Line::from(vec![
                            "Start to close timeout: ".into(),
                            text::Span::from(if let Some(dur) = attrs.start_to_close_timeout {
                                format!("{}s", time::Duration::try_from(dur).unwrap().as_secs())
                            } else {
                                "-".to_owned()
                            }),
                        ]),
                        text::Line::from(vec![
                            "Workflow task completed event ID: ".into(),
                            text::Span::from(format!("{}", attrs.workflow_task_completed_event_id)),
                        ]),
                        text::Line::from(vec![
                            "Use workflow build ID: ".into(),
                            text::Span::from(format!("{}", attrs.use_workflow_build_id)),
                        ]),
                        text::Line::from(vec![
                            "Retry policy initial interval: ".into(),
                            text::Span::from(
                                if let Some(retry_policy) = attrs.retry_policy.as_ref() {
                                    if let Some(initial_interval) = retry_policy.initial_interval {
                                        format!("{}", initial_interval)
                                    } else {
                                        "-".to_owned()
                                    }
                                } else {
                                    "-".to_owned()
                                },
                            ),
                        ]),
                        text::Line::from(vec![
                            "Retry policy backoff coefficient: ".into(),
                            text::Span::from(
                                if let Some(retry_policy) = attrs.retry_policy.as_ref() {
                                    format!("{}", retry_policy.backoff_coefficient)
                                } else {
                                    "-".to_owned()
                                },
                            ),
                        ]),
                        text::Line::from(vec![
                            "Retry policy maximum interval: ".into(),
                            text::Span::from(
                                if let Some(retry_policy) = attrs.retry_policy.as_ref() {
                                    if let Some(maximum_interval) = retry_policy.maximum_interval {
                                        format!("{}", maximum_interval)
                                    } else {
                                        "-".to_owned()
                                    }
                                } else {
                                    "-".to_owned()
                                },
                            ),
                        ]),
                    ];
                    widgets::Paragraph::new(lines).render(areas[0], buf);

                    if let Some(retry_policy) = attrs.retry_policy.as_ref() {
                        // Using `collections::BTreeMap` for consistent order.
                        let non_retryable_error_types: collections::BTreeMap<String, String> =
                            retry_policy
                                .non_retryable_error_types
                                .iter()
                                .enumerate()
                                .map(|(i, e)| (format!("{}", i), e.to_string()))
                                .collect();
                        let pretty_non_retryable_error_types =
                            serde_json::to_string_pretty(&non_retryable_error_types).unwrap();

                        widgets::Paragraph::new(pretty_non_retryable_error_types)
                            .block(
                                widgets::Block::bordered()
                                    .border_type(widgets::BorderType::Rounded)
                                    .title("Retry policy non retryable error types"),
                            )
                            .wrap(widgets::Wrap { trim: false })
                            .render(areas[1], buf);
                    }

                    if let Some(header) = attrs.header.as_ref() {
                        let headers: collections::HashMap<String, String> = header
                            .fields
                            .iter()
                            .map(|(k, v)| {
                                let payload = Payload::from(v);
                                (k.to_string(), payload.to_string_pretty())
                            })
                            .collect();
                        let pretty_header = serde_json::to_string_pretty(&headers).unwrap();

                        widgets::Paragraph::new(pretty_header)
                            .block(
                                widgets::Block::bordered()
                                    .border_type(widgets::BorderType::Rounded)
                                    .title("Header"),
                            )
                            .wrap(widgets::Wrap { trim: false })
                            .render(areas[2], buf);
                    }

                    if let Some(payloads) = attrs.input.as_ref() {
                        for p in payloads.payloads.iter() {
                            let payload = Payload::from(p).with_title("Input");
                            payload.render(areas[3], buf);
                        }
                    }
                }
                history::history_event::Attributes::ActivityTaskStartedEventAttributes(attrs) => {
                    let lines = vec![
                        text::Line::from(vec![
                            "Scheduled event ID: ".into(),
                            text::Span::from(attrs.scheduled_event_id.to_string()),
                        ]),
                        text::Line::from(vec![
                            "Identity: ".into(),
                            text::Span::from(&attrs.identity),
                        ]),
                        text::Line::from(vec![
                            "Request ID: ".into(),
                            text::Span::from(&attrs.request_id),
                        ]),
                        text::Line::from(vec![
                            "Attempt: ".into(),
                            text::Span::from(format!("{}", attrs.attempt)),
                        ]),
                        text::Line::from(vec![
                            "Worker version: ".into(),
                            text::Span::from(match &attrs.worker_version {
                                Some(ts) => &ts.build_id,
                                None => "-",
                            }),
                        ]),
                    ];
                    widgets::Paragraph::new(lines).render(area, buf);
                }
                history::history_event::Attributes::ActivityTaskCompletedEventAttributes(attrs) => {
                    let areas = layout::Layout::vertical([
                        layout::Constraint::Length(3),
                        layout::Constraint::Fill(1),
                    ])
                    .split(area);

                    let lines = vec![
                        text::Line::from(vec![
                            "Scheduled event ID: ".into(),
                            text::Span::from(attrs.scheduled_event_id.to_string()),
                        ]),
                        text::Line::from(vec![
                            "Scheduled event ID: ".into(),
                            text::Span::from(attrs.started_event_id.to_string()),
                        ]),
                        text::Line::from(vec![
                            "Identity: ".into(),
                            text::Span::from(&attrs.identity),
                        ]),
                    ];
                    widgets::Paragraph::new(lines).render(areas[0], buf);

                    if let Some(payloads) = attrs.result.as_ref() {
                        for p in payloads.payloads.iter() {
                            let payload = Payload::from(p).with_title("Result");
                            payload.render(areas[1], buf);
                        }
                    }
                }
                history::history_event::Attributes::ActivityTaskFailedEventAttributes(attrs) => {
                    let areas = layout::Layout::vertical([
                        layout::Constraint::Length(4),
                        layout::Constraint::Fill(1),
                        layout::Constraint::Fill(1),
                    ])
                    .split(area);

                    let retry_state = match attrs.retry_state {
                        1 => enums::RetryState::InProgress,
                        2 => enums::RetryState::NonRetryableFailure,
                        3 => enums::RetryState::Timeout,
                        4 => enums::RetryState::MaximumAttemptsReached,
                        5 => enums::RetryState::RetryPolicyNotSet,
                        6 => enums::RetryState::InternalServerError,
                        7 => enums::RetryState::CancelRequested,
                        _ => enums::RetryState::Unspecified,
                    };

                    let lines = vec![
                        text::Line::from(vec![
                            "Identity: ".into(),
                            text::Span::from(&attrs.identity),
                        ]),
                        text::Line::from(vec![
                            "Retry state: ".into(),
                            text::Span::from(retry_state.as_str_name()),
                        ]),
                        text::Line::from(vec![
                            "Scheduled event ID: ".into(),
                            text::Span::from(attrs.scheduled_event_id.to_string()),
                        ]),
                        text::Line::from(vec![
                            "Started event ID: ".into(),
                            text::Span::from(attrs.started_event_id.to_string()),
                        ]),
                    ];
                    widgets::Paragraph::new(lines).render(areas[0], buf);
                }
                _ => {}
            }
        };
    }
}

impl TryFrom<history::HistoryEvent> for EventWidget {
    type Error = anyhow::Error;

    fn try_from(history_event: history::HistoryEvent) -> Result<Self, Self::Error> {
        let event_type = enums::EventType::try_from(history_event.event_type)?;
        Ok(Self {
            id: history_event.event_id,
            time: history_event
                .event_time
                .and_then(|t| chrono::DateTime::from_timestamp(t.seconds, t.nanos as u32)),
            r#type: event_type,
            attributes: history_event.attributes,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct HistoryWidget {
    events: Vec<EventWidget>,
    next_page_token: Option<Vec<u8>>,
    theme: Theme,
    display_event: Option<usize>,
}

impl HistoryWidget {
    fn clear(&mut self) {
        self.events.clear();
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    fn len(&self) -> usize {
        self.events.len()
    }

    fn extend_from_history(&mut self, history: history::History) {
        for history_event in history.events.into_iter() {
            if let Ok(event) = EventWidget::try_from(history_event) {
                self.events.push(event);
            }
        }
    }

    fn display_event_at(&mut self, index: usize) {
        self.display_event = Some(index);
    }

    fn clear_display_event(&mut self) {
        self.display_event = None;
    }

    fn is_displaying_event(&self) -> bool {
        match self.display_event {
            Some(_) => true,
            None => false,
        }
    }
}

impl widgets::StatefulWidget for &HistoryWidget {
    type State = widgets::TableState;

    fn render(self, area: layout::Rect, buf: &mut buffer::Buffer, state: &mut Self::State) {
        let event_history_block = widgets::Block::bordered()
            .border_type(widgets::BorderType::Rounded)
            .title(text::Span::from("Event history".fg(self.theme.foreground)));

        let selected_row_style = style::Style::default()
            .add_modifier(style::Modifier::REVERSED)
            .fg(self.theme.selection_background);

        match self.display_event {
            Some(index) => {
                let displaying_event = self.events.get(index).unwrap();

                let inner_area = event_history_block.inner(area);
                widgets::Widget::render(event_history_block, area, buf);

                let header = [
                    widgets::Cell::new(displaying_event.time_as_string()),
                    widgets::Cell::new(displaying_event.type_as_string()),
                ]
                .into_iter()
                .map(widgets::Cell::from)
                .collect::<widgets::Row>()
                .style(selected_row_style)
                .height(1);

                let single_row_table = widgets::Table::default()
                    .widths([
                        layout::Constraint::Length(24),
                        layout::Constraint::Length(32),
                    ])
                    .row_highlight_style(selected_row_style)
                    .bg(self.theme.background)
                    .highlight_spacing(widgets::HighlightSpacing::Always)
                    .header(header);

                let vertical = &layout::Layout::vertical([
                    layout::Constraint::Length(1),
                    layout::Constraint::Fill(1),
                ]);
                let [table_area, attributes_area] = vertical.areas(inner_area);

                widgets::Widget::render(single_row_table, table_area, buf);

                displaying_event.render(attributes_area, buf);
            }
            None => {
                let rows = self
                    .events
                    .iter()
                    .enumerate()
                    .map(|(i, event)| {
                        let color = match i % 2 {
                            0 => self.theme.background,
                            _ => self.theme.alt_background,
                        };
                        widgets::Row::new(vec![
                            widgets::Cell::new(format!("{}", event.id)),
                            widgets::Cell::new(event.time_as_string()),
                            widgets::Cell::new(event.type_as_string()),
                        ])
                        .style(style::Style::new().fg(self.theme.foreground).bg(color))
                        .height(1)
                    })
                    .collect::<Vec<widgets::Row>>();
                let event_history_table = widgets::Table::new(
                    rows,
                    [
                        layout::Constraint::Length(5),
                        layout::Constraint::Length(24),
                        layout::Constraint::Length(32),
                    ],
                )
                .block(event_history_block)
                .row_highlight_style(selected_row_style)
                .bg(self.theme.background)
                .highlight_spacing(widgets::HighlightSpacing::Always);

                widgets::StatefulWidget::render(event_history_table, area, buf, state);
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Workflow {
    pending_activities: Vec<PendingActivity>,
    execution: Option<WorkflowExecution>,
    history: HistoryWidget,
    history_state: sync::Arc<sync::RwLock<widgets::TableState>>,
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
    workflow: sync::Arc<sync::RwLock<Workflow>>,
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
            workflow: sync::Arc::new(sync::RwLock::new(Workflow::default())),
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

                    let get_workflow_execution_history_result = self
                        .temporal_client
                        .get_workflow_execution_history(
                            self.workflow_id.clone(),
                            self.run_id.clone(),
                            Vec::new(),
                        )
                        .await;

                    match (
                        describe_workflow_execution_result,
                        get_workflow_execution_history_result,
                    ) {
                        (Ok(r1), Ok(r2)) => self.on_reload(r1, r2),
                        (Err(e1), Err(e2)) => self.on_err(anyhow::anyhow!(
                            "fetch workflow requests failed: {}, {}",
                            e1.to_string(),
                            e2.to_string()
                        )),
                        (Err(e1), _) => self.on_err(anyhow::anyhow!(
                            "describe workflow execution request failed: {}",
                            e1.to_string(),
                        )),
                        (_, Err(e2)) => self.on_err(anyhow::anyhow!(
                            "get workflow execution history request failed: {}",
                            e2.to_string()
                        )),
                    }
                }
                _ => {}
            }
            match message {
                Message::LoadPage { page_token } => {
                    log::debug!(widget = "WorfklowWidget"; "Loading page {:?}", page_token);
                    self.set_loading_state(LoadingState::Loading);

                    let get_workflow_execution_history_result = self
                        .temporal_client
                        .get_workflow_execution_history(
                            self.workflow_id.clone(),
                            self.run_id.clone(),
                            page_token,
                        )
                        .await;

                    match get_workflow_execution_history_result {
                        Ok(response) => self.on_workflow_history_load(response, false),
                        Err(e) => self.on_err(anyhow::anyhow!(
                            "get workflow execution history request failed: {}",
                            e.to_string()
                        )),
                    }
                }
                _ => {}
            }
        }
    }

    fn on_reload(
        &mut self,
        describe_workflow_response: service::DescribeWorkflowExecutionResponse,
        get_workflow_history_response: service::GetWorkflowExecutionHistoryResponse,
    ) {
        self.on_workflow_execution_load(describe_workflow_response);
        self.on_workflow_history_load(get_workflow_history_response, true);
        self.set_loading_state(LoadingState::Reloaded);
        log::debug!(widget = "WorkflowWidget"; "Reloaded");
    }

    fn on_workflow_execution_load(
        &mut self,
        describe_workflow_response: service::DescribeWorkflowExecutionResponse,
    ) {
        let execution = match describe_workflow_response.workflow_execution_info {
            Some(info) => match WorkflowExecution::try_from(info) {
                Ok(e) => e,
                Err(e) => {
                    self.on_err(anyhow::anyhow!(
                        "invalid workflow execution: {}",
                        e.to_string()
                    ));
                    return;
                }
            },
            None => {
                self.on_err(anyhow::anyhow!("unknown workflow execution"));
                return;
            }
        };

        let pending_activities: Vec<PendingActivity> = match describe_workflow_response
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

        let mut workflow = self.workflow.write().unwrap();
        workflow.execution = Some(execution);
        workflow.pending_activities = pending_activities;
    }

    fn on_workflow_history_load(
        &mut self,
        get_workflow_history_response: service::GetWorkflowExecutionHistoryResponse,
        clear: bool,
    ) {
        let mut workflow = self.workflow.write().unwrap();

        if clear {
            log::debug!(
                widget = "WorkflowWidget",
                method = "on_workflow_history_load";
                "Clearing workflow history and next page token",
            );
            workflow.history.clear();
            workflow.history.next_page_token = None;
        }

        if !get_workflow_history_response.next_page_token.is_empty() {
            workflow.history.next_page_token = Some(get_workflow_history_response.next_page_token);
        }

        if let Some(history) = get_workflow_history_response.history {
            workflow.history.extend_from_history(history);
        }

        if !workflow.history.is_empty() && clear {
            workflow.history_state.write().unwrap().select(Some(0));
        }
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

    pub fn get_selected_history_event(&self) -> Option<usize> {
        let workflow = self.workflow.read().unwrap();
        let selected = workflow.history_state.read().unwrap().selected();
        selected
    }

    pub async fn next_row(&mut self) {
        let on_last_row = self.is_on_last_row();
        let loading_next = if on_last_row {
            self.load_next_page().await
        } else {
            false
        };
        log::debug!(widget = "WorkflowWidget"; "Will load next {}", loading_next);

        loop {
            let on_last_row = self.is_on_last_row();
            if !on_last_row || !loading_next {
                break;
            }
            task::yield_now().await;
        }

        let history_state_selected = self.get_selected_history_event();

        let workflow = self.workflow.read().unwrap();
        let i = match history_state_selected {
            Some(i) => {
                if i >= workflow.history.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }

            None => 0,
        };

        let mut history_state = workflow.history_state.write().unwrap();
        history_state.select(Some(i));
        // state.scrollbar_state = state.scrollbar_state.position(i * ITEM_HEIGHT);
    }

    pub fn is_on_last_row(&self) -> bool {
        log::debug!(widget = "WorkflowWidget", method = "is_on_last_row"; "Requesting read workflow lock");
        let workflow = self.workflow.read().unwrap();
        log::debug!(widget = "WorkflowWidget", method = "is_on_last_row"; "Read workflow lock obtained");
        let history_state_selected = self.get_selected_history_event();
        match history_state_selected {
            Some(i) => {
                if i >= workflow.history.len() - 1 {
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    pub async fn load_next_page(&self) -> bool {
        let workflow = self.workflow.read().unwrap();
        let next_page_token = workflow.history.next_page_token.as_ref().cloned();
        if let Some(page_token) = next_page_token {
            log::debug!(
                widget = "WorkflowWidget",
                method = "load_next_page";
                "Loading next page with token {:?}", &page_token
            );

            let sender = self.sender.as_ref().clone();
            sender
                .unwrap()
                .send(Message::LoadPage { page_token })
                .await
                .unwrap();
            true
        } else {
            false
        }
    }

    pub fn previous_row(&mut self) {
        let history_state_selected = self.get_selected_history_event();

        let workflow = self.workflow.read().unwrap();
        let i = match history_state_selected {
            Some(i) => {
                if i == 0 {
                    workflow.history.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };

        let mut history_state = workflow.history_state.write().unwrap();
        history_state.select(Some(i));
        // state.scrollbar_state = state.scrollbar_state.position(i * ITEM_HEIGHT);
    }

    pub fn is_displaying_history_event(&self) -> bool {
        let workflow = self.workflow.read().unwrap();
        workflow.history.is_displaying_event()
    }
}

impl widgets::Widget for &WorkflowWidget {
    fn render(self, area: layout::Rect, buf: &mut buffer::Buffer) {
        let vertical =
            &layout::Layout::vertical([layout::Constraint::Length(9), layout::Constraint::Fill(1)]);
        let [header_area, body_area] = vertical.areas(area);

        let workflow = self.workflow.read().unwrap();

        if workflow.execution.is_none() {
            return;
        }

        let workflow_execution = workflow.execution.as_ref().unwrap();

        let (status, status_color) = (
            workflow_execution.status_as_string(),
            workflow_execution.status_color_from_theme(self.theme),
        );

        let header_block = widgets::Block::bordered()
            .border_type(widgets::BorderType::Rounded)
            .title(text::Span::from(
                status.bg(status_color).fg(self.theme.foreground),
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

        let [start_time, end_time, execution_duration, workflow_run_id, workflow_type, task_queue, history_size_bytes] = [
            workflow_execution.start_time_as_string(),
            workflow_execution.close_time_as_string(),
            workflow_execution.execution_duration_as_string(),
            workflow_execution.run_id.clone(),
            workflow_execution.r#type.clone(),
            workflow_execution.task_queue.clone(),
            format!("{}", workflow_execution.history_size_bytes),
        ];

        let right_values = widgets::Paragraph::new(vec![
            text::Line::raw(start_time).right_aligned(),
            text::Line::raw(end_time).right_aligned(),
            text::Line::raw(execution_duration).right_aligned(),
            text::Line::raw(workflow_run_id).right_aligned(),
            text::Line::raw(workflow_type).right_aligned(),
            text::Line::raw(task_queue).right_aligned(),
            text::Line::raw(history_size_bytes).right_aligned(),
        ]);

        widgets::Widget::render(left_keys, header_left_area, buf);
        widgets::Widget::render(right_values, header_right_area, buf);

        let mut history_state = workflow.history_state.write().unwrap();
        workflow.history.render(body_area, buf, &mut history_state);
    }
}

impl Keybindable for WorkflowWidget {
    async fn handle_key(&mut self, key: event::KeyEvent) -> Option<ViewWidget> {
        match key {
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
            // Reload history table
            event::KeyEvent {
                code: event::KeyCode::Char('r'),
                modifiers: event::KeyModifiers::CONTROL,
                ..
            } => self.reload().await,
            event::KeyEvent {
                code: event::KeyCode::Enter,
                ..
            } => {
                let history_state_selected = self.get_selected_history_event();
                let mut workflow = self.workflow.write().unwrap();

                match history_state_selected {
                    Some(u) => workflow.history.display_event_at(u),
                    _ => {}
                }
            }
            event::KeyEvent {
                code: event::KeyCode::Esc,
                ..
            } => {
                let is_displaying_history_event = self.is_displaying_history_event();
                if is_displaying_history_event {
                    let mut workflow = self.workflow.write().unwrap();
                    workflow.history.clear_display_event();
                } else {
                    return Some(ViewWidget::WorkflowTable(WorkflowTableWidget::new(
                        &self.temporal_client,
                        self.theme,
                        48,
                    )));
                }
            }
            _ => {}
        };
        None
    }

    fn keybinds<'k>(&'k self) -> &'k [(&'k str, &'k [&'k str])] {
        &[
            ("Up", &["j", "↑"]),
            ("Down", &["k", "↓"]),
            ("Expand event", &["Enter"]),
            ("Reload", &["Ctrl+r"]),
        ]
    }
}
