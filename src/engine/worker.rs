//! Background worker plumbing.
//!
//! The UI never blocks. [`spawn`] hands a request to a fresh OS thread, which
//! reports progress through [`WorkerMsg`] and finishes with the tool's result.
//! Cancellation is a shared [`CancelFlag`]: the UI sets it, the worker polls it
//! through `check_cancel`.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::model::{ToolError, ToolId, ToolOutcome, WorkerMsg};
use crate::tools::{Progress, Tool, ToolRequest, check_cancel};

/// Handle to one running task.
pub struct WorkerHandle {
    pub receiver: Receiver<WorkerMsg>,
    pub cancel: Arc<AtomicBool>,
}

impl WorkerHandle {
    /// Ask the worker to stop. The worker still sends a final `Cancelled` result.
    pub fn request_cancel(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Spawn a tool execution on a background thread.
pub fn spawn(id: ToolId, request: ToolRequest) -> WorkerHandle {
    let (tx, rx): (Sender<WorkerMsg>, Receiver<WorkerMsg>) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = Arc::clone(&cancel);

    let join = thread::Builder::new()
        .name(format!("sparrowpdf-{id:?}"))
        .spawn(move || {
            let tool = crate::tools::tool_for(id);
            let progress = Progress::new(tx.clone());
            let result = run_tool(tool.as_ref(), &request, &progress, &cancel_for_thread);
            let _ = tx.send(WorkerMsg::Finished(result));
        });

    // The join handle is dropped: a worker outliving the UI is harmless
    // because it only reports through the channel.
    let _ = join;

    WorkerHandle { receiver: rx, cancel }
}

/// Run a tool, converting a cancellation request into a cancelled result.
fn run_tool(
    tool: &dyn Tool,
    request: &ToolRequest,
    progress: &Progress,
    cancel: &Arc<AtomicBool>,
) -> Result<ToolOutcome, ToolError> {
    check_cancel(cancel)?;
    tool.execute(request, progress, cancel)
}
