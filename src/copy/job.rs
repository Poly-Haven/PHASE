use log::{info, warn};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::copy::engine::{copy_one_file, CopyError};
use crate::copy::plan::{Action, Plan};

/// Updates the UI receives while a job runs.
#[allow(dead_code)]
pub enum JobMsg {
    FileDone { rel_path: String },
    FileFailed { rel_path: String, error: String },
    Finished,
    Cancelled,
}

/// Shared progress state the UI samples each frame.
#[derive(Default)]
pub struct JobProgress {
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
    pub cancel: AtomicBool,
    pub current_file: Mutex<Option<String>>,
}

impl JobProgress {
    pub fn fraction(&self) -> f32 {
        let t = self.bytes_total.load(Ordering::Relaxed);
        if t == 0 {
            return 0.0;
        }
        (self.bytes_done.load(Ordering::Relaxed) as f64 / t as f64) as f32
    }
}

pub fn copy_worker_count(copyable_files: usize) -> usize {
    copyable_files.clamp(1, 4)
}

/// Spawn worker threads that copy all `New`/`Overwrite` files.
pub fn spawn(plan: Plan, progress: Arc<JobProgress>, tx: Sender<JobMsg>) -> thread::JoinHandle<()> {
    spawn_with_worker_count(plan, progress, tx, None)
}

pub(crate) fn spawn_with_worker_count(
    plan: Plan,
    progress: Arc<JobProgress>,
    tx: Sender<JobMsg>,
    worker_count: Option<usize>,
) -> thread::JoinHandle<()> {
    progress
        .bytes_total
        .store(plan.total_bytes_to_copy, Ordering::Relaxed);
    let copyable_files: Vec<_> = plan
        .files
        .into_iter()
        .filter(|f| matches!(f.action, Action::New | Action::Overwrite))
        .collect();
    info!(
        "job start: {:?}, {} files, {} bytes",
        plan.direction,
        copyable_files.len(),
        plan.total_bytes_to_copy
    );
    thread::spawn(move || {
        let files = Arc::new(copyable_files);
        let next_index = Arc::new(AtomicUsize::new(0));
        let terminal_sent = Arc::new(AtomicBool::new(false));
        let worker_count = worker_count
            .unwrap_or_else(|| copy_worker_count(files.len()))
            .clamp(1, files.len().max(1));
        let mut handles = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let files = files.clone();
            let next_index = next_index.clone();
            let progress = progress.clone();
            let tx = tx.clone();
            let terminal_sent = terminal_sent.clone();
            handles.push(thread::spawn(move || loop {
                if progress.cancel.load(Ordering::Relaxed) {
                    if terminal_sent
                        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                    {
                        info!("job cancelled");
                        let _ = tx.send(JobMsg::Cancelled);
                    }
                    return;
                }

                let index = next_index.fetch_add(1, Ordering::Relaxed);
                let Some(file) = files.get(index) else {
                    return;
                };

                if let Ok(mut current_file) = progress.current_file.lock() {
                    *current_file = Some(file.rel_path.to_string_lossy().to_string());
                }
                let res = copy_one_file(
                    &file.src_abs,
                    &file.dst_abs,
                    &progress.bytes_done,
                    &progress.cancel,
                );
                match res {
                    Ok(()) => {
                        let _ = tx.send(JobMsg::FileDone {
                            rel_path: file.rel_path.to_string_lossy().to_string(),
                        });
                    }
                    Err(CopyError::Cancelled) => {
                        if terminal_sent
                            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                            .is_ok()
                        {
                            info!("job cancelled");
                            let _ = tx.send(JobMsg::Cancelled);
                        }
                        return;
                    }
                    Err(e) => {
                        progress.cancel.store(true, Ordering::Relaxed);
                        if terminal_sent
                            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                            .is_ok()
                        {
                            warn!("file failed {}: {}", file.rel_path.display(), e);
                            let _ = tx.send(JobMsg::FileFailed {
                                rel_path: file.rel_path.to_string_lossy().to_string(),
                                error: e.to_string(),
                            });
                        }
                        return;
                    }
                };
            }));
        }

        for handle in handles {
            let _ = handle.join();
        }

        if !terminal_sent.load(Ordering::Relaxed) && !progress.cancel.load(Ordering::Relaxed) {
            if let Ok(mut current_file) = progress.current_file.lock() {
                *current_file = None;
            }
            info!("job finished");
            let _ = tx.send(JobMsg::Finished);
        }
    })
}
