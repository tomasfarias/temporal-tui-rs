use std::time;

use crate::theme::Theme;
use ratatui::style;
use temporal_sdk_core_protos::temporal::api::{enums::v1 as enums, workflow::v1 as workflow};

pub struct Keybind {
    keys: Vec<String>,
    operation: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LoadingState {
    #[default]
    Idle,
    Reloaded,
    Loading,
    PageLoaded,
    Error(String),
}

#[derive(Debug)]
pub enum Message {
    Reload,
    LoadPage { page_token: Vec<u8> },
}

#[derive(Debug, Default, Clone)]
pub struct WorkflowExecution {
    pub status: enums::WorkflowExecutionStatus,
    pub r#type: String,
    pub workflow_id: String,
    pub run_id: String,
    pub task_queue: String,
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub close_time: Option<chrono::DateTime<chrono::Utc>>,
    pub execution_time: Option<chrono::DateTime<chrono::Utc>>,
    pub execution_duration: Option<time::Duration>,
    pub history_size_bytes: u64,
}

impl WorkflowExecution {
    pub fn start_time_as_string(&self) -> String {
        match self.start_time {
            Some(dt) => format!("{}", dt.format("%y-%m-%d %H:%M:%S %Z")),
            None => "-".to_owned(),
        }
    }

    pub fn close_time_as_string(&self) -> String {
        match self.close_time {
            Some(dt) => format!("{}", dt.format("%y-%m-%d %H:%M:%S %Z")),
            None => "-".to_owned(),
        }
    }

    pub fn execution_time_as_string(&self) -> String {
        match self.execution_time {
            Some(dt) => format!("{}", dt.format("%y-%m-%d %H:%M:%S %Z")),
            None => "-".to_owned(),
        }
    }

    pub fn execution_duration_as_string(&self) -> String {
        match self.execution_duration {
            Some(dur) => format!("{}s", dur.as_secs()),
            None => "-".to_owned(),
        }
    }

    pub fn status_color_from_theme(&self, theme: Theme) -> style::Color {
        match self.status {
            enums::WorkflowExecutionStatus::Unspecified => theme.cancelled_background,
            enums::WorkflowExecutionStatus::Running => theme.running_background,
            enums::WorkflowExecutionStatus::Completed => theme.success_background,
            enums::WorkflowExecutionStatus::Failed => theme.failure_background,
            enums::WorkflowExecutionStatus::Canceled => theme.cancelled_background,
            enums::WorkflowExecutionStatus::Terminated => theme.failure_background,
            enums::WorkflowExecutionStatus::ContinuedAsNew => theme.cancelled_background,
            enums::WorkflowExecutionStatus::TimedOut => theme.failure_background,
        }
    }

    pub fn status_as_string(&self) -> String {
        match self.status {
            enums::WorkflowExecutionStatus::Unspecified => "Unspecified".to_owned(),
            enums::WorkflowExecutionStatus::Running => "Running".to_owned(),
            enums::WorkflowExecutionStatus::Completed => "Completed".to_owned(),
            enums::WorkflowExecutionStatus::Failed => "Failed".to_owned(),
            enums::WorkflowExecutionStatus::Canceled => "Canceled".to_owned(),
            enums::WorkflowExecutionStatus::Terminated => "Terminated".to_owned(),
            enums::WorkflowExecutionStatus::ContinuedAsNew => "ContinuedAsNew".to_owned(),
            enums::WorkflowExecutionStatus::TimedOut => "TimedOut".to_owned(),
        }
    }
}

impl TryFrom<workflow::WorkflowExecutionInfo> for WorkflowExecution {
    type Error = anyhow::Error;

    fn try_from(execution_info: workflow::WorkflowExecutionInfo) -> Result<Self, Self::Error> {
        let execution = execution_info
            .execution
            .ok_or(anyhow::anyhow!("workflow has no execution"))?;
        let execution_duration = if let Some(execution_duration) = execution_info.execution_duration
        {
            let duration = time::Duration::try_from(execution_duration)?;
            Some(duration)
        } else {
            None
        };

        let status = enums::WorkflowExecutionStatus::try_from(execution_info.status)?;

        Ok(WorkflowExecution {
            status,
            r#type: execution_info
                .r#type
                .ok_or(anyhow::anyhow!("workflow execution has no type"))?
                .name,
            workflow_id: execution.workflow_id,
            run_id: execution.run_id,
            task_queue: execution_info.task_queue,
            start_time: execution_info.start_time.and_then(|start_time| {
                chrono::DateTime::from_timestamp(start_time.seconds, start_time.nanos as u32)
            }),
            close_time: execution_info.close_time.and_then(|close_time| {
                chrono::DateTime::from_timestamp(close_time.seconds, close_time.nanos as u32)
            }),
            execution_time: execution_info.execution_time.and_then(|execution_time| {
                chrono::DateTime::from_timestamp(
                    execution_time.seconds,
                    execution_time.nanos as u32,
                )
            }),
            execution_duration,
            history_size_bytes: execution_info.history_size_bytes as u64,
        })
    }
}
