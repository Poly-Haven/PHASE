//! The ingest pipeline: two stages, shared worker pools, any number of cards.
//!
//! Stage A copies a file off the card and hashes the source as it streams past. Stage B
//! reads the written file back, checks that hash, and decodes the image — one pass over the
//! destination doing both jobs, because proving the bytes landed and proving the photo
//! survived want the same read.
//!
//! The pools are shared rather than per-card. Two cards plugged in at once should take
//! turns over one network link, not double the thread count and halve each other's
//! throughput.
//!
//! Unlike `copy::job`, a failed file does **not** stop the run. A card is usually formatted
//! afterwards, so abandoning 2,000 good files because one sector went bad would be the
//! worst possible response; each file retries a few times and then goes red on its own.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use filetime::FileTime;

use super::manifest::{self, Entry, Freshness, Manifest};
use super::scan::{CardScan, IngestFile};
use super::thumb;
use crate::copy::engine;

/// Copy workers.
///
/// Four is enough. Both stages share one gigabit link, and mixed read+write traffic tops
/// out at roughly 120 MB/s aggregate on this hardware whatever the worker count — measured
/// at 4 and 8 workers per stage, which differ by less than the run-to-run noise. Raising
/// these only costs memory.
const COPY_WORKERS: usize = 4;

/// Verify workers. Each holds a whole file in memory plus the decoder's own buffers —
/// roughly 300 MB at peak for a 45 MP RAW — so this is a memory budget as much as a
/// parallelism one.
const VERIFY_WORKERS: usize = 4;

/// How far stage A may run ahead of stage B.
///
/// Kept small on purpose. Both stages compete for the same network link, so letting the
/// copy stage sprint ahead buys nothing — and it makes the grid lie, since a deep queue
/// shows up as a crowd of files that look like they are being worked on when they are only
/// waiting.
const VERIFY_QUEUE_DEPTH: usize = COPY_WORKERS;

const COPY_ATTEMPTS: usize = 3;
const VERIFY_ATTEMPTS: usize = 2;
const RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// Identifies a card within a session.
pub type CardId = u64;

/// Remembers directories already created, so they are created once rather than once per file.
///
/// A card holds a handful of directories and thousands of files, but the obvious code asks
/// for the parent directory before every single write. On a local disk that is free; over
/// SMB each call is a network round trip, measured at several milliseconds — thousands of
/// them, spent confirming something that was already true.
#[derive(Default)]
struct DirCache {
    known: Mutex<std::collections::HashSet<PathBuf>>,
}

impl DirCache {
    fn ensure(&self, dir: &Path) -> std::io::Result<()> {
        if let Ok(known) = self.known.lock() {
            if known.contains(dir) {
                return Ok(());
            }
        }
        std::fs::create_dir_all(dir)?;
        if let Ok(mut known) = self.known.lock() {
            known.insert(dir.to_path_buf());
        }
        Ok(())
    }
}

/// What the grid draws for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    Pending,
    /// A `.partial` from a run that was killed rather than cancelled. Shown as a warning
    /// until it is retried, so a half-finished previous run is visible rather than silent.
    Incomplete,
    Copying,
    /// Being read back, hash-checked and decoded.
    Verifying,
    Done,
    Failed,
}

impl FileState {
    pub fn is_busy(self) -> bool {
        matches!(self, FileState::Copying | FileState::Verifying)
    }

    /// Whether this file still has work outstanding.
    pub fn is_settled(self) -> bool {
        matches!(self, FileState::Done | FileState::Failed)
    }
}

/// Progress for one card, sampled by the UI each frame.
#[derive(Default)]
pub struct CardProgress {
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
    pub files_done: AtomicUsize,
    pub files_total: AtomicUsize,
    /// Bytes belonging to files that have gone all the way to green.
    ///
    /// Separate from `bytes_done`, which counts the copy stage only. The estimate has to be
    /// built on work that is actually *finished*: copying runs ahead of verifying, so a
    /// prediction based on copied bytes would be confidently wrong at both ends of a run.
    pub bytes_verified: AtomicU64,
}

/// What a worker tells the UI. Drained in `pump()`.
pub enum IngestMsg {
    /// A file moved into a new non-terminal state.
    Stage {
        card: CardId,
        index: usize,
        state: FileState,
    },
    Done {
        card: CardId,
        index: usize,
        entry: Box<Entry>,
    },
    Failed {
        card: CardId,
        index: usize,
        error: String,
    },
}

struct CopyTask {
    card: CardId,
    index: usize,
    file: IngestFile,
    dst: PathBuf,
    thumb_abs: PathBuf,
    thumb_rel: String,
    /// The hash the manifest recorded, when this file is only being re-checked after a
    /// timestamp shift rather than re-copied.
    recheck: Option<String>,
    cancel: Arc<AtomicBool>,
    progress: Arc<CardProgress>,
}

struct VerifyTask {
    card: CardId,
    index: usize,
    file: IngestFile,
    dst: PathBuf,
    thumb_abs: PathBuf,
    thumb_rel: String,
    src_hash: String,
    cancel: Arc<AtomicBool>,
    progress: Arc<CardProgress>,
}

/// The shared pools. Created once, on the first ingest, and reused for every card after.
pub struct Pools {
    copy_tx: SyncSender<CopyTask>,
    pub rx: Receiver<IngestMsg>,
}

impl Pools {
    pub fn start() -> Pools {
        let (msg_tx, rx) = channel::<IngestMsg>();
        let (copy_tx, copy_rx) = sync_channel::<CopyTask>(VERIFY_QUEUE_DEPTH);
        let (verify_tx, verify_rx) = sync_channel::<VerifyTask>(VERIFY_QUEUE_DEPTH);

        // A shared receiver behind a mutex is the std-only way to fan one queue out to
        // several workers. The lock is held only long enough to take the next task.
        let copy_rx = Arc::new(Mutex::new(copy_rx));
        for worker in 0..COPY_WORKERS {
            let copy_rx = copy_rx.clone();
            let verify_tx = verify_tx.clone();
            let msg_tx = msg_tx.clone();
            thread::Builder::new()
                .name(format!("ingest-copy-{worker}"))
                .spawn(move || copy_loop(&copy_rx, &verify_tx, &msg_tx))
                .expect("spawn ingest copy worker");
        }
        drop(verify_tx);

        let verify_rx = Arc::new(Mutex::new(verify_rx));
        let dirs = Arc::new(DirCache::default());
        for worker in 0..VERIFY_WORKERS {
            let verify_rx = verify_rx.clone();
            let msg_tx = msg_tx.clone();
            let dirs = dirs.clone();
            thread::Builder::new()
                .name(format!("ingest-verify-{worker}"))
                .spawn(move || verify_loop(&verify_rx, &msg_tx, &dirs))
                .expect("spawn ingest verify worker");
        }

        log::info!("Ingest pools started: {COPY_WORKERS} copy, {VERIFY_WORKERS} verify");
        Pools { copy_tx, rx }
    }
}

fn next_task<T>(rx: &Mutex<Receiver<T>>) -> Option<T> {
    // Take the lock, take one task, release. Holding it across the work would serialise
    // the whole pool.
    let task = rx.lock().ok()?.recv().ok()?;
    Some(task)
}

fn copy_loop(
    rx: &Mutex<Receiver<CopyTask>>,
    verify_tx: &SyncSender<VerifyTask>,
    msg_tx: &Sender<IngestMsg>,
) {
    while let Some(task) = next_task(rx) {
        if task.cancel.load(Ordering::Relaxed) {
            continue;
        }
        let _ = msg_tx.send(IngestMsg::Stage {
            card: task.card,
            index: task.index,
            state: FileState::Copying,
        });

        match run_copy(&task) {
            Ok(src_hash) => {
                let verify = VerifyTask {
                    card: task.card,
                    index: task.index,
                    file: task.file,
                    dst: task.dst,
                    thumb_abs: task.thumb_abs,
                    thumb_rel: task.thumb_rel,
                    src_hash,
                    cancel: task.cancel,
                    progress: task.progress,
                };
                // Deliberately *not* marked Verifying here: the file is only queued, and a
                // square that claims to be verifying while it waits its turn misrepresents
                // how much work is actually in flight. The verify worker stamps it.
                if verify_tx.send(verify).is_err() {
                    return;
                }
            }
            Err(error) => {
                if !task.cancel.load(Ordering::Relaxed) {
                    log::warn!("Ingest copy failed for {}: {error}", task.file.rel_path.display());
                    let _ = msg_tx.send(IngestMsg::Failed {
                        card: task.card,
                        index: task.index,
                        error,
                    });
                }
            }
        }
    }
}

/// Copy with retries, returning the source hash in hex.
fn run_copy(task: &CopyTask) -> Result<String, String> {
    // A file whose timestamps merely shifted (a DST change on a FAT card) is confirmed by
    // hashing the source, not by re-copying ~50 MB across the network.
    if let Some(expected) = &task.recheck {
        match engine::hash_file(&task.file.src_abs) {
            Ok(hash) if hash.to_hex().to_string() == *expected => return Ok(expected.clone()),
            Ok(_) => log::info!(
                "{} changed since it was ingested; re-copying",
                task.file.rel_path.display()
            ),
            Err(err) => log::warn!("Re-check read failed for {}: {err}", task.file.rel_path.display()),
        }
    }

    let mut last_error = String::from("copy did not run");
    for attempt in 1..=COPY_ATTEMPTS {
        if task.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        // Each attempt re-copies from the start, so its bytes must not be counted twice.
        let before = task.progress.bytes_done.load(Ordering::Relaxed);
        match engine::copy_one_file_hashed(
            &task.file.src_abs,
            &task.dst,
            &task.progress.bytes_done,
            &task.cancel,
        ) {
            Ok(hash) => return Ok(hash.to_hex().to_string()),
            Err(err) => {
                task.progress.bytes_done.store(before, Ordering::Relaxed);
                last_error = err.to_string();
                if task.cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                if attempt < COPY_ATTEMPTS {
                    thread::sleep(RETRY_BACKOFF);
                }
            }
        }
    }
    Err(last_error)
}

fn verify_loop(rx: &Mutex<Receiver<VerifyTask>>, msg_tx: &Sender<IngestMsg>, dirs: &DirCache) {
    while let Some(task) = next_task(rx) {
        if task.cancel.load(Ordering::Relaxed) {
            continue;
        }
        let _ = msg_tx.send(IngestMsg::Stage {
            card: task.card,
            index: task.index,
            state: FileState::Verifying,
        });
        let mut last_error = String::from("verify did not run");
        let mut settled = false;
        for attempt in 1..=VERIFY_ATTEMPTS {
            match verify_and_thumbnail(&task, dirs) {
                Ok(entry) => {
                    task.progress.files_done.fetch_add(1, Ordering::Relaxed);
                    task.progress
                        .bytes_verified
                        .fetch_add(task.file.size, Ordering::Relaxed);
                    let _ = msg_tx.send(IngestMsg::Done {
                        card: task.card,
                        index: task.index,
                        entry: Box::new(entry),
                    });
                    settled = true;
                    break;
                }
                Err(err) => {
                    last_error = err;
                    if task.cancel.load(Ordering::Relaxed) {
                        settled = true;
                        break;
                    }
                    if attempt < VERIFY_ATTEMPTS {
                        thread::sleep(RETRY_BACKOFF);
                    }
                }
            }
        }
        if !settled {
            log::warn!("Ingest verify failed for {}: {last_error}", task.file.rel_path.display());
            let _ = msg_tx.send(IngestMsg::Failed {
                card: task.card,
                index: task.index,
                error: last_error,
            });
        }
    }
}

/// One pass over the written file: hash it, decode it, write its thumbnail.
fn verify_and_thumbnail(task: &VerifyTask, dirs: &DirCache) -> Result<Entry, String> {
    let bytes = read_whole(&task.dst, &task.cancel)?;

    let dst_hash = blake3::hash(&bytes).to_hex().to_string();
    if dst_hash != task.src_hash {
        return Err("hash mismatch — what landed is not what was read".into());
    }

    let metadata = std::fs::metadata(&task.dst).map_err(|err| format!("stat failed: {err}"))?;
    let file_name = task
        .file
        .rel_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();

    // Decoding is the other half of the integrity check, so a decode failure fails the
    // file — that is the whole reason we decode rather than lift the embedded preview.
    let rendered = thumb::render(&file_name, bytes)?;
    if let Some(parent) = task.thumb_abs.parent() {
        dirs.ensure(parent)
            .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
    }
    std::fs::write(&task.thumb_abs, &rendered.jpeg)
        .map_err(|err| format!("could not write thumbnail: {err}"))?;

    Ok(Entry {
        src_size: task.file.size,
        src_mtime: task.file.mtime,
        dst_size: metadata.len(),
        dst_mtime: FileTime::from_last_modification_time(&metadata).unix_seconds(),
        blake3: dst_hash,
        thumb: Some(task.thumb_rel.clone()),
        camera: rendered.camera,
    })
}

fn read_whole(path: &Path, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path).map_err(|err| format!("open failed: {err}"))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut bytes = Vec::with_capacity(size as usize);
    let mut chunk = vec![0u8; 8 * 1024 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(err) => return Err(format!("read failed: {err}")),
        }
    }
    Ok(bytes)
}

/// How far back the throughput estimate looks.
///
/// Long enough to ride out the lumpiness of individual 35-50 MB files finishing, short
/// enough that starting or finishing a second card is reflected within a few seconds rather
/// than being averaged away over the whole run.
const THROUGHPUT_WINDOW: Duration = Duration::from_secs(20);

/// Samples closer together than this are not worth keeping.
const THROUGHPUT_SAMPLE_GAP: Duration = Duration::from_millis(250);

/// No estimate is offered until the window spans at least this long, so the first seconds
/// of a run do not produce a wild number that immediately corrects itself.
const THROUGHPUT_MIN_SPAN: Duration = Duration::from_secs(4);

/// Time constant for smoothing the reported estimate.
///
/// The raw rate is honest but lumpy — individual files finish in bursts — and on a long run
/// that turned into an estimate visibly flapping between 46 and 54 minutes. The text is
/// redrawn every frame, so that reads as flicker. Eight seconds is short enough to follow a
/// real change (a second card joining) within the same handful of seconds the window itself
/// takes to notice, and long enough to sit still otherwise.
const ETA_SMOOTHING_TAU: f64 = 8.0;

/// A sliding window of completed-bytes samples, used to estimate the rate of progress.
#[derive(Default)]
pub struct Throughput {
    samples: std::collections::VecDeque<(Instant, u64)>,
    /// Last update time and the smoothed estimate in seconds.
    smoothed: Option<(Instant, f64)>,
}

impl Throughput {
    /// Record cumulative verified bytes and how much is left. Cheap to call every frame.
    pub fn record(&mut self, now: Instant, verified_bytes: u64, remaining_bytes: u64) {
        if let Some((last, _)) = self.samples.back() {
            if now.saturating_duration_since(*last) < THROUGHPUT_SAMPLE_GAP {
                return;
            }
        }
        self.samples.push_back((now, verified_bytes));
        while let Some((oldest, _)) = self.samples.front() {
            if now.saturating_duration_since(*oldest) > THROUGHPUT_WINDOW {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        self.update_smoothed(now, remaining_bytes);
    }

    fn update_smoothed(&mut self, now: Instant, remaining_bytes: u64) {
        let Some(raw) = self.raw_eta_secs(remaining_bytes) else {
            return;
        };
        self.smoothed = Some(match self.smoothed {
            Some((last, previous)) => {
                let dt = now.saturating_duration_since(last).as_secs_f64();
                // Exponential moving average, framed in real time so it behaves the same
                // whatever rate the caller happens to sample at.
                let alpha = 1.0 - (-dt / ETA_SMOOTHING_TAU).exp();
                (now, previous + (raw - previous) * alpha)
            }
            None => (now, raw),
        });
    }

    /// Bytes per second across the window, or `None` while there is too little to go on.
    pub fn bytes_per_sec(&self) -> Option<f64> {
        let (first, first_bytes) = *self.samples.front()?;
        let (last, last_bytes) = *self.samples.back()?;
        let span = last.saturating_duration_since(first);
        if span < THROUGHPUT_MIN_SPAN {
            return None;
        }
        let moved = last_bytes.checked_sub(first_bytes)?;
        if moved == 0 {
            return None;
        }
        Some(moved as f64 / span.as_secs_f64())
    }

    fn raw_eta_secs(&self, remaining: u64) -> Option<f64> {
        if remaining == 0 {
            return None;
        }
        let rate = self.bytes_per_sec()?;
        let secs = remaining as f64 / rate;
        secs.is_finite().then_some(secs)
    }

    /// The smoothed time remaining, or `None` while there is nothing to go on.
    pub fn eta(&self, remaining: u64) -> Option<Duration> {
        if remaining == 0 {
            return None;
        }
        let (_, secs) = self.smoothed?;
        Duration::try_from_secs_f64(secs).ok()
    }

    /// Discard history, e.g. when a run is cancelled and later resumed.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.smoothed = None;
    }
}

/// Everything a card contributes to a session.
pub struct CardRun {
    pub id: CardId,
    pub drive_letter: char,
    pub card_name: String,
    pub card_signature: String,
    pub root: PathBuf,
    pub dest_dir: PathBuf,
    pub scan: CardScan,
    pub states: Vec<FileState>,
    /// When each file last entered a busy state, so the grid can give every square its own
    /// pulse phase instead of flashing them all in lockstep.
    pub busy_since: Vec<Option<Instant>>,
    pub errors: HashMap<usize, String>,
    pub manifest: Manifest,
    pub progress: Arc<CardProgress>,
    pub throughput: Throughput,
    pub cancel: Arc<AtomicBool>,
    /// Entries written since the manifest was last flushed.
    pub unflushed: usize,
    pub started: bool,
    pub logged: bool,
}

impl CardRun {
    pub fn file_count(&self) -> usize {
        self.scan.files.len()
    }

    pub fn count_of(&self, state: FileState) -> usize {
        self.states.iter().filter(|s| **s == state).count()
    }

    pub fn done_count(&self) -> usize {
        self.count_of(FileState::Done)
    }

    pub fn failed_count(&self) -> usize {
        self.count_of(FileState::Failed)
    }

    /// Whether every file has reached a terminal state.
    pub fn is_settled(&self) -> bool {
        !self.states.is_empty() && self.states.iter().all(|state| state.is_settled())
    }

    /// Fully ingested with nothing outstanding — the card can be ejected.
    pub fn is_complete(&self) -> bool {
        !self.states.is_empty() && self.states.iter().all(|state| *state == FileState::Done)
    }

    pub fn is_running(&self) -> bool {
        self.started && !self.is_settled() && !self.cancel.load(Ordering::Relaxed)
    }

    /// Time until this card finishes, at the rate it is currently going.
    ///
    /// Derived from measured throughput rather than any assumption about the hardware, so
    /// it accounts for free: a slow card, a busy network, and — the case that matters most
    /// here — a second card sharing the same worker pools and the same link.
    pub fn eta(&self) -> Option<Duration> {
        self.throughput.eta(self.bytes_outstanding())
    }

    /// Bytes still to copy, for the free-space check and the progress bar.
    pub fn bytes_outstanding(&self) -> u64 {
        self.scan
            .files
            .iter()
            .zip(self.states.iter())
            .filter(|(_, state)| **state != FileState::Done)
            .map(|(file, _)| file.size)
            .sum()
    }
}

/// Work out, without touching the network more than a stat per file, what a card still
/// needs. Files already ingested and untouched come back as `Done`.
pub fn prepare(scan: &CardScan, dest_dir: &Path, manifest: &Manifest) -> Vec<Plan> {
    scan.files
        .iter()
        .map(|file| {
            let dst = dest_dir.join(&file.rel_path);
            let dst_meta = std::fs::metadata(&dst)
                .ok()
                .map(|m| (m.len(), FileTime::from_last_modification_time(&m).unix_seconds()));
            match manifest.freshness(file, dst_meta) {
                Freshness::Current => Plan::AlreadyDone,
                Freshness::Recheck => Plan::Recheck(
                    manifest
                        .get(&file.rel_path)
                        .map(|entry| entry.blake3.clone())
                        .unwrap_or_default(),
                ),
                Freshness::Stale => {
                    if partial_path(&dst).exists() {
                        Plan::Incomplete
                    } else {
                        Plan::Copy
                    }
                }
            }
        })
        .collect()
}

/// What `prepare` decided for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    AlreadyDone,
    /// Timestamps shifted but sizes match; confirm with a hash of the source.
    Recheck(String),
    /// A `.partial` is lying around from an interrupted run.
    Incomplete,
    Copy,
}

impl Plan {
    pub fn initial_state(&self) -> FileState {
        match self {
            Plan::AlreadyDone => FileState::Done,
            Plan::Incomplete => FileState::Incomplete,
            Plan::Recheck(_) | Plan::Copy => FileState::Pending,
        }
    }
}

fn partial_path(dst: &Path) -> PathBuf {
    let mut raw = dst.as_os_str().to_owned();
    raw.push(".partial");
    PathBuf::from(raw)
}

/// Queue every outstanding file of a card onto the shared pools.
///
/// Runs on its own thread because the bounded queue makes `send` block once the pools are
/// saturated — which is the point, but must never happen on the UI thread.
fn enqueue(pools_tx: SyncSender<CopyTask>, tasks: Vec<CopyTask>, cancel: Arc<AtomicBool>) {
    thread::spawn(move || {
        for task in tasks {
            if cancel.load(Ordering::Relaxed) || pools_tx.send(task).is_err() {
                return;
            }
        }
    });
}

impl Pools {
    /// Build and queue the outstanding work for a card.
    pub fn submit(&self, run: &CardRun, plans: &[Plan]) {
        let mut tasks = Vec::new();
        for ((index, file), plan) in run.scan.files.iter().enumerate().zip(plans.iter()) {
            let recheck = match plan {
                Plan::AlreadyDone => continue,
                Plan::Recheck(hash) => Some(hash.clone()),
                Plan::Incomplete | Plan::Copy => None,
            };
            let thumb_rel = super::thumb_rel_path(&file.rel_path);
            tasks.push(CopyTask {
                card: run.id,
                index,
                file: file.clone(),
                dst: run.dest_dir.join(&file.rel_path),
                thumb_abs: run.dest_dir.join(&thumb_rel),
                thumb_rel: manifest::key_for(&thumb_rel),
                recheck,
                cancel: run.cancel.clone(),
                progress: run.progress.clone(),
            });
        }
        log::info!(
            "Ingest queued {} files ({} already done) for {}",
            tasks.len(),
            run.file_count() - tasks.len(),
            run.card_name
        );
        enqueue(self.copy_tx.clone(), tasks, run.cancel.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::manifest::Entry;

    fn ingest_file(rel: &str, size: u64, mtime: i64) -> IngestFile {
        IngestFile {
            rel_path: PathBuf::from(rel),
            src_abs: PathBuf::from("I:").join(rel),
            size,
            mtime,
        }
    }

    fn entry(size: u64, mtime: i64) -> Entry {
        Entry {
            src_size: size,
            src_mtime: mtime,
            dst_size: size,
            dst_mtime: mtime,
            blake3: "deadbeef".into(),
            thumb: None,
            camera: None,
        }
    }

    #[test]
    fn at_most_one_file_per_worker_can_look_busy() {
        // The grid should never show more busy squares than there are workers plus the
        // small hand-off queue between the two stages. A deep queue used to make dozens of
        // files look like they were being verified at once.
        assert!(VERIFY_QUEUE_DEPTH <= COPY_WORKERS);
        let most_busy = COPY_WORKERS + VERIFY_QUEUE_DEPTH + VERIFY_WORKERS;
        assert!(most_busy <= 16, "{most_busy} busy squares is too many to read");
    }

    fn at(base: Instant, secs: f64) -> Instant {
        base + Duration::from_secs_f64(secs)
    }

    #[test]
    fn no_estimate_is_offered_until_there_is_something_to_go_on() {
        let base = Instant::now();
        let mut t = Throughput::default();
        assert_eq!(t.bytes_per_sec(), None);
        t.record(base, 0, 100_000_000);
        assert_eq!(t.bytes_per_sec(), None, "one sample is not a rate");
        // Still inside the minimum span.
        t.record(at(base, 1.0), 10_000_000, 90_000_000);
        assert_eq!(t.bytes_per_sec(), None);
        // Past it.
        t.record(at(base, 5.0), 50_000_000, 50_000_000);
        assert!(t.bytes_per_sec().is_some());
    }

    #[test]
    fn the_rate_matches_the_bytes_that_actually_moved() {
        let base = Instant::now();
        let mut t = Throughput::default();
        // 10 MB/s for ten seconds, with a steady 100 MB outstanding so the smoothed
        // estimate has time to settle on the true value.
        for step in 0..=60 {
            t.record(at(base, step as f64 * 0.5), step * 5_000_000, 100_000_000);
        }
        let rate = t.bytes_per_sec().unwrap();
        assert!((rate - 10_000_000.0).abs() < 200_000.0, "rate was {rate}");

        // 100 MB left at 10 MB/s is ten seconds.
        let eta = t.eta(100_000_000).unwrap();
        assert!((eta.as_secs_f64() - 10.0).abs() < 0.5, "eta was {eta:?}");
    }

    #[test]
    fn the_pipeline_fill_period_does_not_drag_the_estimate_down() {
        // Measured against the real 113-file Sony card: the run took ~81s, but for the first
        // ~6s nothing had finished verifying yet. Including that dead time in the average
        // made the estimate at t=10s read "2 minutes" when ~50s actually remained.
        let base = Instant::now();
        let total = 3_890_000_000u64;

        // Only sampled from the first completion onwards, as the caller now does.
        let mut anchored = Throughput::default();
        for step in 12..=20 {
            let t = step as f64 * 0.5;
            let verified = ((t - 6.0) * 48_000_000.0) as u64;
            anchored.record(at(base, t), verified, total - verified);
        }
        let verified = ((10.0 - 6.0) * 48_000_000.0) as u64;
        let eta = anchored.eta(total - verified).unwrap().as_secs_f64();
        assert!(
            (40.0..80.0).contains(&eta),
            "expected roughly the ~50s that really remained, got {eta:.0}s"
        );
    }

    #[test]
    fn the_reported_estimate_does_not_flap_with_every_burst_of_completions() {
        // Files finish in bursts, which on the real 101 GB card made the raw estimate swing
        // between 46 and 54 minutes. Smoothing has to damp that without hiding real change.
        let base = Instant::now();
        let mut t = Throughput::default();
        let remaining = 90_000_000_000u64;
        let mut verified = 0u64;
        let mut reported = Vec::new();
        for step in 0..200 {
            // A lumpy but stationary rate: alternating fast and slow half-seconds.
            verified += if step % 2 == 0 { 30_000_000 } else { 2_000_000 };
            t.record(at(base, step as f64 * 0.5), verified, remaining);
            if step > 60 {
                if let Some(eta) = t.eta(remaining) {
                    reported.push(eta.as_secs_f64());
                }
            }
        }
        let spread = reported.iter().cloned().fold(f64::MIN, f64::max)
            - reported.iter().cloned().fold(f64::MAX, f64::min);
        let mean = reported.iter().sum::<f64>() / reported.len() as f64;
        assert!(
            spread / mean < 0.05,
            "smoothed estimate still swings {:.0}% (spread {spread:.0}s about {mean:.0}s)",
            100.0 * spread / mean
        );
    }

    #[test]
    fn smoothing_still_follows_a_real_change_in_rate() {
        // A second card joining genuinely halves our share; the estimate must actually move,
        // and within a sensible number of seconds rather than eventually.
        let base = Instant::now();
        let mut t = Throughput::default();
        let remaining = 3_000_000_000u64;
        let mut verified = 0u64;
        for step in 0..80 {
            verified += 10_000_000;
            t.record(at(base, step as f64 * 0.25), verified, remaining);
        }
        let before = t.eta(remaining).unwrap().as_secs_f64();

        // Half the rate. Convergence is deliberately gradual: the window has to slide off
        // the old rate *and* the average has to catch up, so the estimate moves promptly but
        // takes the better part of a minute to fully arrive — which is what the real cards
        // did when the second one joined.
        let switch = 20.0;
        let mut at_20s = None;
        for step in 0..200 {
            verified += 5_000_000;
            let now = switch + step as f64 * 0.25;
            t.record(at(base, now), verified, remaining);
            if at_20s.is_none() && now >= switch + 20.0 {
                at_20s = t.eta(remaining).map(|d| d.as_secs_f64());
            }
        }
        let after_20s = at_20s.expect("an estimate 20s after the change");
        let settled = t.eta(remaining).unwrap().as_secs_f64();

        assert!(
            after_20s > before * 1.4,
            "estimate should react within 20s: {before:.0}s -> {after_20s:.0}s"
        );
        assert!(
            (settled - before * 2.0).abs() < before * 0.15,
            "estimate should settle near double: {before:.0}s -> {settled:.0}s"
        );
    }

    #[test]
    fn a_run_that_stalls_reports_no_estimate_rather_than_a_wrong_one() {
        let base = Instant::now();
        let mut t = Throughput::default();
        for step in 0..=20 {
            // Time passes, nothing completes — a card pulled out, or a stuck share.
            t.record(at(base, step as f64 * 0.5), 1_000, 500_000_000);
        }
        assert_eq!(t.bytes_per_sec(), None);
        assert_eq!(t.eta(500_000_000), None);
    }

    #[test]
    fn a_finished_card_has_no_estimate() {
        let base = Instant::now();
        let mut t = Throughput::default();
        for step in 0..=20 {
            t.record(at(base, step as f64 * 0.5), step * 5_000_000, 100_000_000);
        }
        assert_eq!(t.eta(0), None, "nothing left to wait for");
    }

    #[test]
    fn the_window_forgets_old_throughput() {
        let base = Instant::now();
        let mut t = Throughput::default();
        // Fast for ten seconds...
        for step in 0..=20 {
            t.record(at(base, step as f64 * 0.5), step * 10_000_000, 1_000_000_000);
        }
        let fast = t.bytes_per_sec().unwrap();
        // ...then a second card joins and halves our share, for well over the window.
        let slow_start = 10.0;
        let slow_base = 200_000_000u64;
        for step in 1..=60 {
            t.record(
                at(base, slow_start + step as f64 * 0.5),
                slow_base + step * 2_500_000,
                1_000_000_000,
            );
        }
        let slow = t.bytes_per_sec().unwrap();
        assert!(slow < fast / 1.5, "expected the rate to drop: {fast} -> {slow}");
        // Only the recent, slower rate should remain in the window.
        assert!((slow - 5_000_000.0).abs() < 300_000.0, "rate was {slow}");
    }

    #[test]
    fn samples_are_thinned_so_the_window_cannot_grow_without_bound() {
        let base = Instant::now();
        let mut t = Throughput::default();
        // Called every frame at 60fps for a minute.
        for frame in 0..3600 {
            t.record(at(base, frame as f64 / 60.0), frame * 100_000, 5_000_000_000);
        }
        let held = t.samples.len();
        let most = (THROUGHPUT_WINDOW.as_secs_f64() / THROUGHPUT_SAMPLE_GAP.as_secs_f64()) as usize + 2;
        assert!(held <= most, "kept {held} samples, expected at most {most}");
    }

    #[test]
    fn resetting_clears_the_history() {
        let base = Instant::now();
        let mut t = Throughput::default();
        for step in 0..=20 {
            t.record(at(base, step as f64 * 0.5), step * 5_000_000, 100_000_000);
        }
        assert!(t.bytes_per_sec().is_some());
        t.reset();
        assert_eq!(t.bytes_per_sec(), None);
        assert_eq!(t.eta(100_000_000), None);
    }

    /// Time each stage of the pipeline against real hardware, to find out what a card is
    /// actually waiting on rather than guessing.
    ///
    /// ```text
    /// PHASE_BENCH_SRC=I:\DCIM\115NCZ_7 PHASE_BENCH_DEST=P:\_INGEST\_bench \
    ///   cargo test --release -- --ignored --nocapture benchmark_ingest_stages
    /// ```
    #[test]
    #[ignore]
    fn benchmark_ingest_stages() {
        use std::time::Instant as T;

        let src_dir = std::env::var("PHASE_BENCH_SRC").expect("set PHASE_BENCH_SRC");
        let dest_dir = std::env::var("PHASE_BENCH_DEST").expect("set PHASE_BENCH_DEST");
        let count: usize = std::env::var("PHASE_BENCH_FILES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6);
        std::fs::create_dir_all(&dest_dir).unwrap();

        let mut files: Vec<PathBuf> = std::fs::read_dir(&src_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && crate::ingest::is_raw_extension(&crate::ingest::extension_of(
                        &p.file_name().unwrap_or_default().to_string_lossy(),
                    ))
            })
            .collect();
        files.sort();
        files.truncate(count);
        assert!(!files.is_empty(), "no RAW files in {src_dir}");

        let mb = |bytes: u64| bytes as f64 / 1_048_576.0;
        let rate = |bytes: u64, secs: f64| mb(bytes) / secs;

        let (mut read_s, mut copy_s, mut back_s, mut hash_s, mut decode_s) = (0.0, 0.0, 0.0, 0.0, 0.0);
        let mut total_bytes = 0u64;

        for path in &files {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let dst = PathBuf::from(&dest_dir).join(&name);
            let size = std::fs::metadata(path).unwrap().len();
            total_bytes += size;

            // 1. Straight read from the card, as a ceiling on what the reader can do.
            let t = T::now();
            let card_bytes = std::fs::read(path).unwrap();
            read_s += t.elapsed().as_secs_f64();

            // 2. Stage A: card -> NAS, hashing the source on the way past.
            let bytes_done = AtomicU64::new(0);
            let cancel = AtomicBool::new(false);
            let t = T::now();
            let src_hash = engine::copy_one_file_hashed(path, &dst, &bytes_done, &cancel).unwrap();
            copy_s += t.elapsed().as_secs_f64();

            // 3. Stage B: read the written file back off the NAS.
            let t = T::now();
            let back = read_whole(&dst, &cancel).unwrap();
            back_s += t.elapsed().as_secs_f64();

            // 4. Verify it.
            let t = T::now();
            let dst_hash = blake3::hash(&back).to_hex().to_string();
            hash_s += t.elapsed().as_secs_f64();
            assert_eq!(dst_hash, src_hash.to_hex().to_string(), "{name}");

            // 5. Decode it to a thumbnail.
            let t = T::now();
            let rendered = thumb::render(&name, back).unwrap();
            decode_s += t.elapsed().as_secs_f64();
            assert!(!rendered.jpeg.is_empty());

            drop(card_bytes);
            let _ = std::fs::remove_file(&dst);
        }

        let n = files.len() as f64;
        println!("\n{} files, {:.1} MB each\n", files.len(), mb(total_bytes) / n);
        let row = |label: &str, secs: f64| {
            println!(
                "  {label:<26} {:>7.0} ms/file  {:>7.1} MB/s",
                secs / n * 1000.0,
                rate(total_bytes, secs)
            );
        };
        row("card read (ceiling)", read_s);
        row("A: copy card -> NAS", copy_s);
        row("B: read back from NAS", back_s);
        row("B: blake3 verify", hash_s);
        row("B: RAW decode + thumb", decode_s);

        let stage_a = copy_s;
        let stage_b = back_s + hash_s + decode_s;
        println!(
            "\n  stage A total {:>7.0} ms/file      stage B total {:>7.0} ms/file",
            stage_a / n * 1000.0,
            stage_b / n * 1000.0
        );
        // The stages run concurrently, so throughput is set by whichever is slower given its
        // worker count.
        let a_throughput = COPY_WORKERS as f64 / (stage_a / n);
        let b_throughput = VERIFY_WORKERS as f64 / (stage_b / n);
        println!(
            "  with {COPY_WORKERS} copy workers: {a_throughput:.2} files/s   \
with {VERIFY_WORKERS} verify workers: {b_throughput:.2} files/s"
        );
        let bound = if a_throughput < b_throughput { "stage A (copy)" } else { "stage B (verify+decode)" };
        let files_per_sec = a_throughput.min(b_throughput);
        println!(
            "  => bound by {bound}: {files_per_sec:.2} files/s = {:.0} MB/s",
            files_per_sec * mb(total_bytes) / n
        );
        println!(
            "  => a 2120-file, 101 GB card would take about {:.0} minutes",
            2120.0 / files_per_sec / 60.0
        );
    }

    #[test]
    fn a_directory_is_only_created_once_however_many_files_land_in_it() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("thumbs").join("DCIM").join("115NCZ_7");
        let cache = DirCache::default();

        cache.ensure(&dir).unwrap();
        assert!(dir.is_dir());

        // Deleting it behind the cache's back proves the second call really was a no-op
        // rather than quietly recreating it.
        std::fs::remove_dir_all(temp.path().join("thumbs")).unwrap();
        cache.ensure(&dir).unwrap();
        assert!(!dir.exists(), "second ensure should not have touched the filesystem");

        // A directory it has not seen is still created.
        let other = temp.path().join("thumbs").join("DCIM").join("116NCZ_7");
        cache.ensure(&other).unwrap();
        assert!(other.is_dir());
    }

    /// Measure what the cache saves on a real network share:
    /// `PHASE_BENCH_DEST=P:\_INGEST\_bench cargo test --release -- --ignored --nocapture benchmark_dir_cache`
    #[test]
    #[ignore]
    fn benchmark_dir_cache() {
        let dest = std::env::var("PHASE_BENCH_DEST").expect("set PHASE_BENCH_DEST");
        let dir = PathBuf::from(&dest).join("thumbs").join("DCIM").join("115NCZ_7");
        std::fs::create_dir_all(&dir).unwrap();
        let rounds = 300;

        let start = Instant::now();
        for _ in 0..rounds {
            std::fs::create_dir_all(&dir).unwrap();
        }
        let uncached = start.elapsed();

        let cache = DirCache::default();
        let start = Instant::now();
        for _ in 0..rounds {
            cache.ensure(&dir).unwrap();
        }
        let cached = start.elapsed();

        let per_call = |d: Duration| d.as_secs_f64() * 1000.0 / rounds as f64;
        println!(
            "\n  create_dir_all  {:.3} ms/call\n  DirCache::ensure {:.3} ms/call\n  over 2120 files: {:.1}s -> {:.1}s\n",
            per_call(uncached),
            per_call(cached),
            per_call(uncached) * 2120.0 / 1000.0,
            per_call(cached) * 2120.0 / 1000.0,
        );
    }

    /// Isolate stage A: how fast can N workers actually push files card -> NAS?
    ///
    /// ```text
    /// PHASE_BENCH_SRC=I:\DCIM\115NCZ_7 PHASE_BENCH_DEST=P:\_INGEST\_bench \
    ///   cargo test --release -- --ignored --nocapture benchmark_stage_a_concurrency
    /// ```
    #[test]
    #[ignore]
    fn benchmark_stage_a_concurrency() {
        let src_dir = std::env::var("PHASE_BENCH_SRC").expect("set PHASE_BENCH_SRC");
        let dest_dir = PathBuf::from(std::env::var("PHASE_BENCH_DEST").expect("set PHASE_BENCH_DEST"));
        std::fs::create_dir_all(&dest_dir).unwrap();

        let mut all: Vec<PathBuf> = std::fs::read_dir(&src_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("nef")))
            .collect();
        all.sort();
        // Deep into the card, so the OS cache cannot flatter the read side.
        let all: Vec<PathBuf> = all.into_iter().skip(1500).collect();

        println!();
        for workers in [1usize, 2, 4, 6, 8] {
            let files: Vec<PathBuf> = all.iter().skip(workers * 17).take(workers * 2).cloned().collect();
            if files.len() < workers {
                continue;
            }
            let bytes: u64 = files.iter().map(|f| std::fs::metadata(f).unwrap().len()).sum();

            let next = Arc::new(AtomicUsize::new(0));
            let files = Arc::new(files);
            let started = Instant::now();
            let mut handles = Vec::new();
            for _ in 0..workers {
                let next = next.clone();
                let files = files.clone();
                let dest_dir = dest_dir.clone();
                handles.push(thread::spawn(move || {
                    let bytes_done = AtomicU64::new(0);
                    let cancel = AtomicBool::new(false);
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(src) = files.get(index) else { return };
                        let dst = dest_dir.join(format!("bench_{index}.nef"));
                        engine::copy_one_file_hashed(src, &dst, &bytes_done, &cancel).unwrap();
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            let secs = started.elapsed().as_secs_f64();
            let mb = bytes as f64 / 1_048_576.0;
            println!(
                "  stage A workers={workers}  {:>6.1} MB in {:>5.2}s = {:>6.1} MB/s  ({:>5.2} files/s)",
                mb,
                secs,
                mb / secs,
                files.len() as f64 / secs
            );
            for index in 0..files.len() {
                let _ = std::fs::remove_file(dest_dir.join(format!("bench_{index}.nef")));
            }
        }
        println!();
    }

    #[test]
    fn states_classify_as_busy_and_settled_correctly() {
        assert!(FileState::Copying.is_busy());
        assert!(FileState::Verifying.is_busy());
        assert!(!FileState::Pending.is_busy());
        assert!(!FileState::Incomplete.is_busy());

        assert!(FileState::Done.is_settled());
        assert!(FileState::Failed.is_settled());
        for state in [FileState::Pending, FileState::Incomplete, FileState::Copying, FileState::Verifying] {
            assert!(!state.is_settled(), "{state:?} is not settled");
        }
    }

    #[test]
    fn a_file_the_manifest_already_covers_is_not_re_copied() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path();
        std::fs::create_dir_all(dest.join("DCIM")).unwrap();
        std::fs::write(dest.join("DCIM/A.NEF"), b"hello").unwrap();
        let written = std::fs::metadata(dest.join("DCIM/A.NEF")).unwrap();
        let dst_mtime = FileTime::from_last_modification_time(&written).unix_seconds();

        let file = ingest_file("DCIM/A.NEF", 5, dst_mtime);
        let mut manifest = Manifest::default();
        manifest.insert(
            &file.rel_path,
            Entry {
                dst_mtime,
                ..entry(5, dst_mtime)
            },
        );

        let scan = CardScan {
            files: vec![file],
            ..Default::default()
        };
        assert_eq!(prepare(&scan, dest, &manifest), vec![Plan::AlreadyDone]);
        assert_eq!(Plan::AlreadyDone.initial_state(), FileState::Done);
    }

    #[test]
    fn a_file_with_no_destination_is_copied() {
        let temp = tempfile::tempdir().unwrap();
        let scan = CardScan {
            files: vec![ingest_file("DCIM/A.NEF", 5, 100)],
            ..Default::default()
        };
        assert_eq!(prepare(&scan, temp.path(), &Manifest::default()), vec![Plan::Copy]);
        assert_eq!(Plan::Copy.initial_state(), FileState::Pending);
    }

    #[test]
    fn a_leftover_partial_shows_as_incomplete_rather_than_pending() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("DCIM")).unwrap();
        // What a killed run leaves behind.
        std::fs::write(temp.path().join("DCIM/A.NEF.partial"), b"half").unwrap();

        let scan = CardScan {
            files: vec![ingest_file("DCIM/A.NEF", 5, 100)],
            ..Default::default()
        };
        assert_eq!(prepare(&scan, temp.path(), &Manifest::default()), vec![Plan::Incomplete]);
        assert_eq!(Plan::Incomplete.initial_state(), FileState::Incomplete);
    }

    #[test]
    fn a_timestamp_shift_asks_for_a_recheck_carrying_the_recorded_hash() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path();
        std::fs::create_dir_all(dest.join("DCIM")).unwrap();
        std::fs::write(dest.join("DCIM/A.NEF"), b"hello").unwrap();
        let written = std::fs::metadata(dest.join("DCIM/A.NEF")).unwrap();
        let dst_mtime = FileTime::from_last_modification_time(&written).unix_seconds();

        // Same size, source timestamp an hour out: a DST artefact on a FAT card.
        let file = ingest_file("DCIM/A.NEF", 5, dst_mtime + 3600);
        let mut manifest = Manifest::default();
        manifest.insert(&file.rel_path, entry(5, dst_mtime));

        let scan = CardScan {
            files: vec![file],
            ..Default::default()
        };
        assert_eq!(
            prepare(&scan, dest, &manifest),
            vec![Plan::Recheck("deadbeef".into())]
        );
        assert_eq!(Plan::Recheck(String::new()).initial_state(), FileState::Pending);
    }

    fn run_with(states: Vec<FileState>) -> CardRun {
        let files: Vec<IngestFile> = (0..states.len())
            .map(|i| ingest_file(&format!("A{i}.NEF"), 10, 0))
            .collect();
        CardRun {
            id: 1,
            drive_letter: 'I',
            card_name: "120GB_671658".into(),
            card_signature: "sig".into(),
            root: PathBuf::from("I:\\"),
            dest_dir: PathBuf::from(r"P:\_INGEST\120GB_671658"),
            scan: CardScan {
                files,
                ..Default::default()
            },
            busy_since: vec![None; states.len()],
            states,
            errors: HashMap::new(),
            manifest: Manifest::default(),
            progress: Arc::new(CardProgress::default()),
            throughput: Throughput::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            unflushed: 0,
            started: true,
            logged: false,
        }
    }

    #[test]
    fn a_card_is_complete_only_when_every_file_is_green() {
        let run = run_with(vec![FileState::Done, FileState::Done]);
        assert!(run.is_complete());
        assert!(run.is_settled());
        assert_eq!(run.done_count(), 2);

        // One failure means settled but not complete — no eject offer.
        let run = run_with(vec![FileState::Done, FileState::Failed]);
        assert!(run.is_settled());
        assert!(!run.is_complete());
        assert_eq!(run.failed_count(), 1);

        // Still working.
        let run = run_with(vec![FileState::Done, FileState::Copying]);
        assert!(!run.is_settled());
        assert!(!run.is_complete());
        assert!(run.is_running());

        // A card with no files at all is neither complete nor settled.
        let run = run_with(Vec::new());
        assert!(!run.is_complete());
        assert!(!run.is_settled());
    }

    #[test]
    fn outstanding_bytes_exclude_what_is_already_done() {
        let run = run_with(vec![FileState::Done, FileState::Pending, FileState::Failed]);
        assert_eq!(run.bytes_outstanding(), 20);
    }

    #[test]
    fn a_cancelled_card_is_no_longer_running() {
        let run = run_with(vec![FileState::Copying]);
        assert!(run.is_running());
        run.cancel.store(true, Ordering::Relaxed);
        assert!(!run.is_running());
    }
}
