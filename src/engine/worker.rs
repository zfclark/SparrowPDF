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
            // A panicking tool must not silently swallow the failure (the UI
            // would wait for a Finished message that never arrives): catch
            // the unwind, log it and turn it into a visible error.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_tool(tool.as_ref(), &request, &progress, &cancel_for_thread)
            }))
            .unwrap_or_else(|panic| {
                let detail = panic_detail(panic.as_ref());
                crate::engine::logging::log(
                    "ERROR",
                    &format!("工作线程崩溃（{}）：{detail}", id),
                );
                Err(crate::model::ToolError::Internal {
                    detail: format!("处理过程中发生内部错误，已中断（详情见日志）：{detail}"),
                })
            });
            match &result {
                Ok(outcome) => {
                    crate::engine::logging::log("INFO", &format!("任务完成（{id}）：{}", outcome.summary));
                    for output in &outcome.outputs {
                        crate::engine::logging::log("INFO", &format!("输出文件：{}", output.display()));
                    }
                }
                Err(err) => {
                    crate::engine::logging::log("ERROR", &format!("任务失败（{id}）：{err}"));
                }
            }
            let _ = tx.send(WorkerMsg::Finished(result));
        });

    // The join handle is dropped: a worker outliving the UI is harmless
    // because it only reports through the channel.
    let _ = join;

    WorkerHandle { receiver: rx, cancel }
}

/// Best-effort panic payload extraction for logging.
fn panic_detail(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
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
