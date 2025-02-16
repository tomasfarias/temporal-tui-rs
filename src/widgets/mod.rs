use crossterm::event;
use ratatui::{buffer, layout, widgets};

mod common;
pub mod keybinds;
pub mod workflow;
pub mod workflow_table;

pub trait Keybindable {
    async fn handle_key(&mut self, key: event::KeyEvent) -> Option<ViewWidget>;
    fn keybinds<'k>(&'k self) -> &'k [(&'k str, &'k [&'k str])];
}

/// Enumeration of potential views the [`App`] can display.
#[derive(Debug, Clone)]
pub enum ViewWidget {
    /// A view of a single workflow execution.
    Workflow(workflow::WorkflowWidget),
    /// A view of all Temporal workflow executions rendered by [`WorkflowTableWidget`].
    WorkflowTable(workflow_table::WorkflowTableWidget),
}

impl ViewWidget {
    pub async fn run(&mut self) {
        match self {
            ViewWidget::WorkflowTable(workflow_table) => {
                workflow_table.run();
                workflow_table.reload().await;
            }
            ViewWidget::Workflow(workflow) => {
                workflow.run();
                workflow.reload().await;
            }
        }
    }
}

impl widgets::Widget for &ViewWidget {
    fn render(self, area: layout::Rect, buf: &mut buffer::Buffer) {
        match self {
            ViewWidget::Workflow(w) => w.render(area, buf),
            ViewWidget::WorkflowTable(t) => t.render(area, buf),
        }
    }
}

impl Keybindable for ViewWidget {
    async fn handle_key(&mut self, key: event::KeyEvent) -> Option<ViewWidget> {
        match self {
            ViewWidget::Workflow(w) => w.handle_key(key).await,
            ViewWidget::WorkflowTable(t) => t.handle_key(key).await,
        }
    }

    fn keybinds<'k>(&'k self) -> &'k [(&'k str, &'k [&'k str])] {
        match self {
            ViewWidget::Workflow(w) => w.keybinds(),
            ViewWidget::WorkflowTable(t) => t.keybinds(),
        }
    }
}
