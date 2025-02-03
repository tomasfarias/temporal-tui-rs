use ratatui::widgets;
use temporal_sdk_core_protos::temporal::api::workflow::v1::WorkflowExecutionInfo;

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

#[derive(Debug, Clone)]
pub enum WorkflowExecutionStatus {
    Unspecified,
    Running,
    Completed,
    Failed,
    Canceled,
    Terminated,
    ContinuedAsNew,
    TimedOut,
}

impl<'m> WorkflowExecutionStatus {
    pub fn as_str(&'m self) -> &'m str {
        match self {
            WorkflowExecutionStatus::Unspecified => "Unspecified",
            WorkflowExecutionStatus::Running => "Running",
            WorkflowExecutionStatus::Completed => "Completed",
            WorkflowExecutionStatus::Failed => "Failed",
            WorkflowExecutionStatus::Canceled => "Canceled",
            WorkflowExecutionStatus::Terminated => "Terminated",
            WorkflowExecutionStatus::ContinuedAsNew => "Continued-As-New",
            WorkflowExecutionStatus::TimedOut => "Timed-Out",
        }
    }
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
        status.as_str().to_owned()
    }
}

impl From<&WorkflowExecutionStatus> for widgets::Cell<'_> {
    fn from(status: &WorkflowExecutionStatus) -> Self {
        let s = String::from(status);
        widgets::Cell::new(s)
    }
}

impl Default for WorkflowExecutionStatus {
    fn default() -> Self {
        Self::Unspecified
    }
}

#[derive(Debug, Default, Clone)]
pub struct WorkflowExecution {
    pub status: WorkflowExecutionStatus,
    pub r#type: String,
    pub workflow_id: String,
    pub task_queue: String,
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub close_time: Option<chrono::DateTime<chrono::Utc>>,
    pub history_size_bytes: u64,
}

impl WorkflowExecution {
    pub fn start_time_string(&self) -> String {
        match self.start_time {
            Some(dt) => format!("{}", dt.format("%y-%m-%d %H:%M:%S %Z")),
            None => "-".to_owned(),
        }
    }

    pub fn close_time_string(&self) -> String {
        match self.close_time {
            Some(dt) => format!("{}", dt.format("%y-%m-%d %H:%M:%S %Z")),
            None => "-".to_owned(),
        }
    }
}

impl TryFrom<WorkflowExecutionInfo> for WorkflowExecution {
    type Error = anyhow::Error;

    fn try_from(execution_info: WorkflowExecutionInfo) -> Result<Self, Self::Error> {
        Ok(WorkflowExecution {
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
            history_size_bytes: execution_info.history_size_bytes as u64,
        })
    }
}
