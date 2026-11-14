//! Bounded work queue for CPU-heavy per-item jobs.
//!
//! Two tools have the same shape: a list of independent items (page images to
//! encode, PDF pages to rasterise), each slow enough to dominate the run and
//! each safe to do in isolation. [`run`] is the one scheduler they share, so
//! the threading, cancellation and failure rules stay identical.
//!
//! Design notes:
//!
//! * **Dynamic distribution.** Workers claim the next index with a
//!   fetch-and-increment; a slow item never leaves the others waiting the way
//!   a fixed chunk per thread does.
//! * **Bounded hand-off.** A [`sync_channel`] with `workers` slots makes a
//!   finished item block until the caller takes it, so payloads in flight stay
//!   constant instead of growing with the item count.
//! * **One coordinator.** The calling thread receives results, so progress
//!   reporting and bookkeeping stay single-threaded and ordered; it also acts
//!   as the join barrier.
//! * **Failure is data.** A task error or panic is delivered as a result for
//!   that index; the first failure aborts the remaining items, which are then
//!   drained rather than silently dropped.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender};

use crate::model::ToolError;

/// What a worker sends back: the item index and its outcome.
type Message<T> = (usize, Result<T, ToolError>);

/// Whether one failing item ends the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnError {
    /// Abandon the remaining items: the caller cannot use a partial result
    /// (one unreadable scan would produce a PDF with a hole in it).
    Stop,
    /// Keep going: each item is independent and the caller reports the
    /// failures it saw (rasterising pages, one bad page is skippable).
    Continue,
}

/// Hard ceiling on worker threads, whatever the machine offers.
///
/// Past this point these jobs are memory-bandwidth bound and the extra
/// threads only raise the peak without shortening the run.
pub const MAX_WORKERS: usize = 8;

/// How many workers this machine can carry for `items` jobs.
///
/// Three limits, smallest wins: cores minus one (the coordinator and the UI
/// have to keep running), a third of physical memory divided by
/// `per_worker_bytes`, and [`MAX_WORKERS`]. A machine whose memory cannot be
/// read falls back to the ceiling rather than guessing low.
pub fn worker_count(items: usize, per_worker_bytes: u64) -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let by_cpu = cores.saturating_sub(1).max(1);
    let by_memory = match crate::engine::sysinfo::total_memory_bytes() {
        Some(bytes) => (bytes / 3 / per_worker_bytes.max(1)).max(1) as usize,
        None => MAX_WORKERS,
    };
    by_cpu
        .min(by_memory)
        .min(MAX_WORKERS)
        .min(items.max(1))
        .max(1)
}

/// Run `task` for every index in `0..items` on at most `workers` threads.
///
/// * `task` runs on a worker thread and must not touch shared mutable state.
/// * `collect` runs on the calling thread for each finished item, in
///   completion order; returning `Err` stops the run (the remaining items are
///   abandoned, not finished).
/// * Cancellation (`cancel` set by the UI) aborts between items and ends with
///   [`ToolError::Cancelled`].
/// * `workers` is clamped to at least 1, so an empty list still returns.
pub fn run<T, F, C>(
    items: usize,
    workers: usize,
    cancel: &AtomicBool,
    on_error: OnError,
    task: F,
    mut collect: C,
) -> Result<(), ToolError>
where
    T: Send,
    F: Fn(usize) -> Result<T, ToolError> + Sync,
    C: FnMut(usize, Result<T, ToolError>) -> Result<(), ToolError>,
{
    if items == 0 {
        return Ok(());
    }
    let workers = workers.clamp(1, items.max(1));
    let cursor = AtomicUsize::new(0);
    let abort = AtomicBool::new(false);
    let mut first_error: Option<ToolError> = None;
    let mut delivered = 0usize;

    {
        let (tx, rx) = mpsc::sync_channel::<Message<T>>(workers);
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers);
            for _ in 0..workers {
                let tx = tx.clone();
                let cursor = &cursor;
                let abort = &abort;
                let task = &task;
                handles.push(
                    scope.spawn(move || {
                        worker_loop(items, cursor, abort, cancel, on_error, task, &tx)
                    }),
                );
            }
            drop(tx);

            for (index, result) in rx {
                delivered += 1;
                let failed = result.is_err();
                if let Err(err) = collect(index, result) {
                    abort.store(true, Ordering::Relaxed);
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                } else if failed && on_error == OnError::Stop && first_error.is_none() {
                    // Keep draining after a failure so nobody is stuck on a
                    // full queue; the caller reports the item it saw.
                    abort.store(true, Ordering::Relaxed);
                }
            }

            for handle in handles {
                if handle.join().is_err() && first_error.is_none() {
                    first_error = Some(ToolError::Internal {
                        detail: "工作线程异常退出".to_string(),
                    });
                }
            }
        });
    }

    if let Some(err) = first_error {
        return Err(err);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(ToolError::Cancelled);
    }
    // Every item must be accounted for: a short count means a worker died
    // without reporting (a panic that escaped the per-item guard).
    if delivered != items {
        return Err(ToolError::Internal {
            detail: format!("只完成 {delivered}/{items} 项"),
        });
    }
    Ok(())
}

/// Claim items until the queue is drained, aborted or the receiver hung up.
fn worker_loop<T, F>(
    items: usize,
    cursor: &AtomicUsize,
    abort: &AtomicBool,
    cancel: &AtomicBool,
    on_error: OnError,
    task: &F,
    tx: &SyncSender<Message<T>>,
) where
    T: Send,
    F: Fn(usize) -> Result<T, ToolError> + Sync,
{
    loop {
        if abort.load(Ordering::Relaxed) {
            return;
        }
        let index = cursor.fetch_add(1, Ordering::Relaxed);
        if index >= items {
            return;
        }
        if cancel.load(Ordering::Relaxed) {
            abort.store(true, Ordering::Relaxed);
            return;
        }

        // One bad item (or a bug in a decoder) must fail that item, not the
        // process: the unwind is caught here and reported as an ordinary
        // error the caller can show and skip.
        let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(index))) {
            Ok(result) => result,
            Err(panic) => Err(ToolError::Internal {
                detail: format!(
                    "处理第 {} 项时发生内部错误：{}",
                    index + 1,
                    crate::engine::worker::panic_detail(panic.as_ref())
                ),
            }),
        };
        let failed = result.is_err();
        // A closed channel means the coordinator gave up; stop quietly.
        if tx.send((index, result)).is_err() {
            abort.store(true, Ordering::Relaxed);
            return;
        }
        if failed && on_error == OnError::Stop {
            abort.store(true, Ordering::Relaxed);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_count_follows_the_machine_and_stays_bounded() {
        // No items still yields one worker, so the pipeline has no empty case
        // and can never hang on an empty channel.
        assert_eq!(worker_count(0, 400 * 1024 * 1024), 1);
        assert_eq!(worker_count(1, 400 * 1024 * 1024), 1);
        // More work than a machine can carry clamps to the ceiling.
        let many = worker_count(4096, 1024);
        assert!((1..=MAX_WORKERS).contains(&many), "{many}");
        // A machine with endless memory is still bounded by the core count and
        // the ceiling, never by the item count alone.
        assert!(worker_count(4096, 1) <= MAX_WORKERS);
    }
}
