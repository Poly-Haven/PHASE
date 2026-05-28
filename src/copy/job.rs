use log::{info, warn};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
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

/// Shared, lock-free progress state the UI samples each frame.
#[derive(Default)]
pub struct JobProgress {
    pub bytes_done:  AtomicU64,
    pub bytes_total: AtomicU64,
    pub cancel:      AtomicBool,
}

impl JobProgress {
    pub fn fraction(&self) -> f32 {
        let t = self.bytes_total.load(Ordering::Relaxed);
        if t == 0 { return 0.0; }
        (self.bytes_done.load(Ordering::Relaxed) as f64 / t as f64) as f32
    }
}

/// Spawn a worker thread that copies all `New`/`Overwrite` files sequentially.
pub fn spawn(plan: Plan, progress: Arc<JobProgress>, tx: Sender<JobMsg>) -> thread::JoinHandle<()> {
    progress.bytes_total.store(plan.total_bytes_to_copy, Ordering::Relaxed);
    info!("job start: {:?}, {} files, {} bytes",
        plan.direction, plan.files.len(), plan.total_bytes_to_copy);
    thread::spawn(move || {
        for file in plan.files.iter().filter(|f| matches!(f.action, Action::New | Action::Overwrite)) {
            if progress.cancel.load(Ordering::Relaxed) {
                info!("job cancelled");
                let _ = tx.send(JobMsg::Cancelled);
                return;
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
                    info!("job cancelled");
                    let _ = tx.send(JobMsg::Cancelled);
                    return;
                }
                Err(e) => {
                    warn!("file failed {}: {}", file.rel_path.display(), e);
                    let _ = tx.send(JobMsg::FileFailed {
                        rel_path: file.rel_path.to_string_lossy().to_string(),
                        error: e.to_string(),
                    });
                    return;
                }
            }
        }
        info!("job finished");
        let _ = tx.send(JobMsg::Finished);
    })
}
