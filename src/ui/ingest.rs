//! The card-ingest screen: detection, per-card state, and the file grid.
//!
//! The grid draws one square per file, sized so every file fits the card's region at any
//! window size — no scrolling, because the point of the screen is to take in a whole card
//! at a glance.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use super::{colors, layout, table::fmt_bytes, AppState, Screen};
use crate::ingest::job::{CardProgress, CardRun, FileState, IngestMsg, Plan, Pools, Throughput};
use crate::ingest::manifest::Manifest;
use crate::ingest::scan::CardScan;
use crate::ingest::{job, log as card_log, scan};
use crate::removable_media::{self, CardFingerprint};

/// A card scan running in the background: walk the card, read the manifest, and work out
/// what is left to do.
pub struct ScanJob {
    pub rx: Receiver<Prepared>,
    /// Files examined so far, for the loading text on a card with thousands of them.
    pub scanned: Arc<AtomicUsize>,
}

pub struct Prepared {
    pub scan: CardScan,
    pub manifest: Manifest,
    pub plans: Vec<Plan>,
}

impl AppState {
    /// Poll for inserted cards, and drain a finished poll into `removable_cards`.
    pub(super) fn pump_card_detection(&mut self) {
        if !super::CARD_DETECTION_ENABLED {
            return;
        }
        if let Some(cards) = self.card_scan_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.card_scan_rx = None;
            self.next_card_scan_at = Instant::now() + self.card_scan_interval();
            if cards != self.removable_cards {
                self.removable_cards = cards;
                self.reconcile_ingest_runs();
            }
        }
        // One poll at a time: a reader that takes longer than the interval to answer must
        // not stack a new thread every tick.
        if self.card_scan_rx.is_some() || Instant::now() < self.next_card_scan_at {
            return;
        }
        let (tx, rx) = channel();
        thread::spawn(move || {
            // An empty reader slot still reports as removable, and querying it would
            // otherwise pop the shell's "please insert a disk" dialog.
            removable_media::suppress_no_media_dialogs();
            let cards = removable_media::list_removable_drives()
                .into_iter()
                .filter_map(removable_media::fingerprint)
                .collect();
            let _ = tx.send(cards);
        });
        self.card_scan_rx = Some(rx);
    }

    fn card_scan_interval(&self) -> std::time::Duration {
        if self.watcher_was_focused {
            super::CARD_SCAN_INTERVAL_ACTIVE
        } else {
            super::CARD_SCAN_INTERVAL_IDLE
        }
    }

    /// Time until the next detection poll, so the loop wakes even when nothing else is
    /// happening.
    pub(super) fn card_scan_repaint_after(&self) -> std::time::Duration {
        self.next_card_scan_at
            .saturating_duration_since(Instant::now())
    }

    /// Match `ingest_runs` to the cards actually plugged in.
    fn reconcile_ingest_runs(&mut self) {
        // A card that is pulled mid-run keeps its region rather than vanishing under the
        // user; its remaining files will fail with a readable error.
        self.ingest_runs
            .retain(|run| run.started || self.removable_cards.iter().any(|card| card.signature == run.card_signature));

        for card in self.removable_cards.clone() {
            if self.ingest_runs.iter().any(|run| run.card_signature == card.signature) {
                continue;
            }
            self.add_ingest_run(&card);
        }
    }

    fn add_ingest_run(&mut self, card: &CardFingerprint) {
        let id = self.next_card_id;
        self.next_card_id += 1;
        let dest_dir = crate::ingest::card_dir(&self.config.ingest_root, card);
        self.ingest_runs.push(CardRun {
            id,
            drive_letter: card.drive_letter,
            card_name: card.friendly_name.clone(),
            card_signature: card.signature.clone(),
            root: PathBuf::from(format!("{}:\\", card.drive_letter)),
            dest_dir,
            scan: CardScan::default(),
            states: Vec::new(),
            busy_since: Vec::new(),
            errors: HashMap::new(),
            manifest: Manifest::default(),
            progress: Arc::new(CardProgress::default()),
            throughput: Throughput::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            unflushed: 0,
            started: false,
            logged: false,
        });
        self.start_card_scan(card.signature.clone());
    }

    /// Walk a card and work out what is outstanding, off the UI thread.
    pub(super) fn start_card_scan(&mut self, signature: String) {
        if self.ingest_scans.contains_key(&signature) {
            return;
        }
        let Some(run) = self.ingest_runs.iter().find(|run| run.card_signature == signature) else {
            return;
        };
        let (root, dest_dir) = (run.root.clone(), run.dest_dir.clone());
        let (tx, rx) = channel();
        let scanned = Arc::new(AtomicUsize::new(0));
        let counter = scanned.clone();
        thread::spawn(move || {
            removable_media::suppress_no_media_dialogs();
            let scan = scan::scan(&root, &counter);
            let manifest = Manifest::load(&dest_dir);
            // Stat-ing the destination happens here too: on an SMB share that is thousands
            // of round trips, which has no business on the UI thread.
            let plans = job::prepare(&scan, &dest_dir, &manifest);
            let _ = tx.send(Prepared { scan, manifest, plans });
        });
        self.ingest_scans.insert(signature, ScanJob { rx, scanned });
    }

    /// Re-scan every card, e.g. from the header's refresh button.
    pub(super) fn rescan_cards(&mut self) {
        self.ingest_runs.retain(|run| run.is_running());
        self.ingest_scans.clear();
        self.removable_cards.clear();
        self.next_card_scan_at = Instant::now();
    }

    /// Begin ingesting one card.
    pub(super) fn start_card_ingest(&mut self, index: usize) {
        let Some(run) = self.ingest_runs.get(index) else {
            return;
        };
        if run.started || run.scan.files.is_empty() {
            return;
        }

        // Refuse rather than fill the destination and fail every remaining file.
        let needed = run.bytes_outstanding();
        if let Some(free) = free_space(&self.config.ingest_root) {
            if free < needed {
                self.error_banner = Some(format!(
                    "Not enough space for {}: needs {}, {} free on {}",
                    run.card_name,
                    fmt_bytes(needed),
                    fmt_bytes(free),
                    self.config.ingest_root.display(),
                ));
                return;
            }
        }

        if self.ingest_pools.is_none() {
            self.ingest_pools = Some(Pools::start());
        }
        let Some(run) = self.ingest_runs.get_mut(index) else {
            return;
        };
        run.started = true;
        run.cancel.store(false, Ordering::Relaxed);
        run.throughput.reset();
        run.progress
            .bytes_verified
            .store(0, Ordering::Relaxed);
        run.progress.bytes_total.store(needed, Ordering::Relaxed);
        run.progress
            .files_total
            .store(run.scan.files.len(), Ordering::Relaxed);
        run.progress.files_done.store(
            run.states.iter().filter(|s| **s == FileState::Done).count(),
            Ordering::Relaxed,
        );

        let plans: Vec<Plan> = run
            .states
            .iter()
            .map(|state| match state {
                FileState::Done => Plan::AlreadyDone,
                _ => Plan::Copy,
            })
            .collect();
        log::info!("Ingest starting for {} ({} files)", run.card_name, run.scan.files.len());
        if let Some(pools) = &self.ingest_pools {
            pools.submit(run, &plans);
        }
    }

    pub(super) fn cancel_card_ingest(&mut self, index: usize) {
        if let Some(run) = self.ingest_runs.get_mut(index) {
            log::info!("Ingest cancelled for {}", run.card_name);
            run.cancel.store(true, Ordering::Relaxed);
            // Throughput measured before a pause says nothing about the rate after it.
            run.throughput.reset();
            for state in run.states.iter_mut() {
                if !state.is_settled() {
                    *state = FileState::Pending;
                }
            }
            run.started = false;
        }
    }

    pub(super) fn eject_card(&mut self, index: usize) {
        let Some(run) = self.ingest_runs.get(index) else {
            return;
        };
        let (letter, name) = (run.drive_letter, run.card_name.clone());
        match removable_media::safe_eject(letter) {
            Ok(()) => {
                log::info!("Ejected {name} ({letter}:)");
                self.ingest_runs.retain(|run| run.drive_letter != letter);
                self.removable_cards.retain(|card| card.drive_letter != letter);
                self.next_card_scan_at = Instant::now();
            }
            // Plenty of multi-slot card readers simply do not implement media eject, and
            // Windows own "Safely Remove" fails on them too. Say so, and say the part that
            // actually matters: everything is copied and verified, so the card can be
            // pulled regardless. The bare Win32 wording reads like the ingest went wrong.
            Err(err) => {
                log::warn!("Eject failed for {name} ({letter}:): {err}");
                self.error_banner = Some(format!(
                    "Windows would not release {name} ({letter}:) — {err}.                      The ingest is complete and verified, so the card is safe to remove.",
                ));
            }
        }
    }

    /// Drain finished scans and worker messages.
    pub(super) fn pump_ingest(&mut self) {
        self.pump_card_scans();
        self.pump_ingest_messages();
        self.pump_manifest_writes();
        self.sample_ingest_throughput();
    }

    /// Feed each running card's estimator. Sampling here rather than on completion messages
    /// means a card that has stopped producing them still registers as having stalled.
    fn sample_ingest_throughput(&mut self) {
        let now = Instant::now();
        for run in self.ingest_runs.iter_mut().filter(|run| run.is_running()) {
            let verified = run.progress.bytes_verified.load(Ordering::Relaxed);
            // Nothing is recorded until the first file goes green. The pipeline takes a few
            // seconds to fill — files are copying, but none have finished verifying — and
            // averaging that dead time into the rate made the first estimates roughly twice
            // as pessimistic as the run turned out to be.
            if verified == 0 {
                continue;
            }
            let remaining = run.bytes_outstanding();
            run.throughput.record(now, verified, remaining);
        }
    }

    fn pump_card_scans(&mut self) {
        let signatures: Vec<String> = self.ingest_scans.keys().cloned().collect();
        for signature in signatures {
            let Some(prepared) = self
                .ingest_scans
                .get(&signature)
                .and_then(|job| job.rx.try_recv().ok())
            else {
                continue;
            };
            self.ingest_scans.remove(&signature);
            let Some(run) = self
                .ingest_runs
                .iter_mut()
                .find(|run| run.card_signature == signature)
            else {
                continue;
            };
            run.states = prepared.plans.iter().map(Plan::initial_state).collect();
            run.busy_since = vec![None; run.states.len()];
            run.manifest = prepared.manifest;
            run.manifest.card_signature = run.card_signature.clone();
            run.manifest.card_name = run.card_name.clone();
            run.progress
                .files_total
                .store(prepared.scan.files.len(), Ordering::Relaxed);
            run.progress.files_done.store(
                run.states.iter().filter(|s| **s == FileState::Done).count(),
                Ordering::Relaxed,
            );
            run.scan = prepared.scan;
        }
    }

    fn pump_ingest_messages(&mut self) {
        let mut messages = Vec::new();
        if let Some(pools) = &self.ingest_pools {
            while let Ok(msg) = pools.rx.try_recv() {
                messages.push(msg);
            }
        }
        for msg in messages {
            match msg {
                IngestMsg::Stage { card, index, state } => {
                    if let Some(run) = self.ingest_runs.iter_mut().find(|run| run.id == card) {
                        if let Some(slot) = run.states.get_mut(index) {
                            *slot = state;
                        }
                        // Stamped per transition, so copying and verifying each start their
                        // own pulse and the grid never falls into step with itself.
                        if let Some(slot) = run.busy_since.get_mut(index) {
                            *slot = state.is_busy().then(Instant::now);
                        }
                    }
                }
                IngestMsg::Done { card, index, entry } => {
                    if let Some(run) = self.ingest_runs.iter_mut().find(|run| run.id == card) {
                        if let Some(slot) = run.states.get_mut(index) {
                            *slot = FileState::Done;
                        }
                        run.errors.remove(&index);
                        if let Some(file) = run.scan.files.get(index) {
                            let rel_path = file.rel_path.clone();
                            run.manifest.insert(&rel_path, *entry);
                            run.unflushed += 1;
                        }
                    }
                }
                IngestMsg::Failed { card, index, error } => {
                    if let Some(run) = self.ingest_runs.iter_mut().find(|run| run.id == card) {
                        if let Some(slot) = run.states.get_mut(index) {
                            *slot = FileState::Failed;
                        }
                        run.errors.insert(index, error);
                    }
                }
            }
        }
        self.finish_settled_cards();
    }

    /// Flush manifests periodically, and write the card log once a run settles.
    fn finish_settled_cards(&mut self) {
        let mut to_flush: Vec<usize> = Vec::new();
        let mut to_log: Vec<usize> = Vec::new();
        for (index, run) in self.ingest_runs.iter().enumerate() {
            let settled = run.started && run.is_settled();
            if run.unflushed >= MANIFEST_FLUSH_EVERY || (settled && run.unflushed > 0) {
                to_flush.push(index);
            }
            if settled && !run.logged {
                to_log.push(index);
            }
        }
        for index in to_flush {
            self.flush_manifest(index);
        }
        for index in to_log {
            self.write_card_log(index);
        }
    }

    fn flush_manifest(&mut self, index: usize) {
        let Some(run) = self.ingest_runs.get_mut(index) else {
            return;
        };
        // One write per card at a time, so two flushes cannot race each other on the NAS.
        if self.manifest_writes.contains_key(&run.id) {
            return;
        }
        let (id, manifest, dest_dir) = (run.id, run.manifest.clone(), run.dest_dir.clone());
        run.unflushed = 0;
        let (tx, rx) = channel();
        thread::spawn(move || {
            if let Err(err) = manifest.save(&dest_dir) {
                log::warn!("Could not write ingest manifest in {}: {err}", dest_dir.display());
            }
            let _ = tx.send(());
        });
        self.manifest_writes.insert(id, rx);
    }

    fn pump_manifest_writes(&mut self) {
        self.manifest_writes
            .retain(|_, rx| rx.try_recv().is_err());
    }

    fn write_card_log(&mut self, index: usize) {
        let Some(run) = self.ingest_runs.get_mut(index) else {
            return;
        };
        run.logged = true;
        let failures: Vec<(String, String)> = run
            .errors
            .iter()
            .filter_map(|(i, error)| {
                run.scan
                    .files
                    .get(*i)
                    .map(|file| (file.rel_path.to_string_lossy().replace('\\', "/"), error.clone()))
            })
            .collect();
        let ingested = run.done_count();
        let bytes = run
            .scan
            .files
            .iter()
            .zip(run.states.iter())
            .filter(|(_, state)| **state == FileState::Done)
            .map(|(file, _)| file.size)
            .sum();
        let summary = card_log::Summary {
            card_name: run.card_name.clone(),
            card_signature: run.card_signature.clone(),
            destination: run.dest_dir.display().to_string(),
            cameras: run.scan.cameras.clone(),
            ingested,
            bytes,
            skipped: run.scan.skipped,
            failures,
            cancelled: run.cancel.load(Ordering::Relaxed),
        };
        let block = card_log::render(&summary, env!("CARGO_PKG_VERSION"), &card_log::local_timestamp());
        let root = run.root.clone();
        log::info!(
            "Ingest finished for {}: {ingested} done, {} failed",
            run.card_name,
            run.errors.len()
        );
        thread::spawn(move || card_log::append_to_card(&root, &block));
    }

    /// Whether any ingest work is in flight, for the repaint keep-alive.
    pub fn ingest_busy(&self) -> bool {
        !self.ingest_scans.is_empty()
            || !self.manifest_writes.is_empty()
            || self.ingest_runs.iter().any(|run| run.is_running())
    }
}

use super::MANIFEST_FLUSH_EVERY;

/// Free space on the volume holding `path`, if it can be determined.
fn free_space(path: &std::path::Path) -> Option<u64> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            lpDirectoryName: *const u16,
            lpFreeBytesAvailableToCaller: *mut u64,
            lpTotalNumberOfBytes: *mut u64,
            lpTotalNumberOfFreeBytes: *mut std::ffi::c_void,
        ) -> i32;
    }
    // The volume root is what matters, and it exists even when the ingest folder does not.
    let root = path.components().next()?;
    let mut wide: Vec<u16> = std::path::Path::new(&root)
        .join("")
        .as_os_str()
        .encode_wide()
        .collect();
    wide.push(0);
    let mut free = 0u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut())
    };
    (ok != 0).then_some(free)
}

use std::os::windows::ffi::OsStrExt;

/// Draw the ingest screen.
pub fn draw(state: &mut AppState, ui: &mut egui::Ui) {
    if state.ingest_runs.is_empty() {
        draw_empty(state, ui);
        return;
    }

    let available = ui.available_rect_before_wrap();
    let count = state.ingest_runs.len();
    let region_height = (available.height() / count as f32).max(1.0);

    let mut clicked_start: Option<usize> = None;
    let mut clicked_cancel: Option<usize> = None;
    let mut clicked_eject: Option<usize> = None;

    for index in 0..count {
        let top = available.top() + region_height * index as f32;
        let rect = egui::Rect::from_min_size(
            egui::pos2(available.left(), top),
            egui::vec2(available.width(), region_height),
        );
        match draw_card(state, ui, index, rect) {
            Some(CardAction::Start) => clicked_start = Some(index),
            Some(CardAction::Cancel) => clicked_cancel = Some(index),
            Some(CardAction::Eject) => clicked_eject = Some(index),
            None => {}
        }
    }

    // Applied after the loop so the borrow of `state` inside it is already released.
    if let Some(index) = clicked_start {
        state.start_card_ingest(index);
    }
    if let Some(index) = clicked_cancel {
        state.cancel_card_ingest(index);
    }
    if let Some(index) = clicked_eject {
        state.eject_card(index);
    }
}

fn draw_empty(state: &AppState, ui: &mut egui::Ui) {
    ui.centered_and_justified(|ui| {
        let text = if state.card_scan_rx.is_some() {
            "Looking for memory cards…"
        } else {
            "No memory cards detected.\nInsert a card, or press the refresh button."
        };
        ui.colored_label(colors::TEXT_DISABLED, text);
    });
}

enum CardAction {
    Start,
    Cancel,
    Eject,
}

fn draw_card(
    state: &AppState,
    ui: &mut egui::Ui,
    index: usize,
    rect: egui::Rect,
) -> Option<CardAction> {
    let run = state.ingest_runs.get(index)?;
    let scanning = state.ingest_scans.contains_key(&run.card_signature);
    let connected = state
        .removable_cards
        .iter()
        .any(|card| card.signature == run.card_signature);

    let painter = ui.painter_at(rect);
    let header_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(layout::INGEST_REGION_PADDING, layout::INGEST_REGION_PADDING),
        egui::vec2(
            rect.width() - layout::INGEST_REGION_PADDING * 2.0,
            layout::INGEST_HEADER_HEIGHT,
        ),
    );

    // Card name.
    painter.text(
        header_rect.left_top(),
        egui::Align2::LEFT_TOP,
        &run.card_name,
        egui::FontId::proportional(layout::INGEST_CARD_NAME_SIZE),
        colors::TEXT_PRIMARY,
    );

    // Contents and cameras underneath.
    let mut detail = Vec::new();
    if scanning {
        let examined = state
            .ingest_scans
            .get(&run.card_signature)
            .map(|job| job.scanned.load(Ordering::Relaxed))
            .unwrap_or(0);
        detail.push(format!("Scanning… {examined} files"));
    } else {
        let contents = run.scan.describe_contents();
        if !contents.is_empty() {
            detail.push(contents);
        }
        if !run.scan.cameras.is_empty() {
            detail.push(format!("Cameras: {}", run.scan.cameras.join(", ")));
        }
        if let Some(skipped) = run.scan.skipped.describe() {
            detail.push(skipped);
        }
        let failed = run.failed_count();
        if failed > 0 {
            detail.push(format!("{failed} failed"));
        }
    }
    if !connected {
        detail.push("card removed".to_string());
    }
    painter.text(
        header_rect.left_top() + egui::vec2(0.0, layout::INGEST_CARD_NAME_SIZE + 6.0),
        egui::Align2::LEFT_TOP,
        detail.join("   "),
        egui::FontId::proportional(layout::INGEST_DETAIL_SIZE),
        colors::TEXT_DISABLED,
    );

    let action = draw_header_action(ui, run, header_rect, scanning);

    // The grid fills whatever is left of the region.
    let grid_rect = egui::Rect::from_min_max(
        egui::pos2(
            rect.left() + layout::INGEST_REGION_PADDING,
            header_rect.bottom() + layout::INGEST_REGION_PADDING,
        ),
        egui::pos2(
            rect.right() - layout::INGEST_REGION_PADDING,
            rect.bottom() - layout::INGEST_REGION_PADDING,
        ),
    );
    if grid_rect.width() > 0.0 && grid_rect.height() > 0.0 {
        draw_grid(ui, run, grid_rect);
    }

    // A divider between stacked cards.
    if index + 1 < state.ingest_runs.len() {
        painter.hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, colors::ROW_BACKGROUND),
        );
    }
    action
}

/// The right-hand action: start, progress + cancel, or eject once finished.
fn draw_header_action(
    ui: &mut egui::Ui,
    run: &CardRun,
    header_rect: egui::Rect,
    scanning: bool,
) -> Option<CardAction> {
    if scanning {
        return None;
    }

    // The three states are colour-coded by what the click does, not by mood: blue to start a
    // copy (the same blue Pull uses on the main screen), red to stop one, green when the card
    // is finished with.
    let (label, color, action) = if run.is_complete() {
        (
            format!("Safely remove {}", run.card_name),
            colors::STATUS_COMPLETE,
            CardAction::Eject,
        )
    } else if run.is_running() {
        let done = run.progress.files_done.load(Ordering::Relaxed);
        let total = run.progress.files_total.load(Ordering::Relaxed);
        let mut text = format!(
            "{done} / {total} · {}",
            fmt_bytes(run.progress.bytes_done.load(Ordering::Relaxed))
        );
        if let Some(eta) = run.eta() {
            text.push_str(" · ");
            text.push_str(&fmt_eta(eta));
        }
        // ACCENT rather than MSG_ERROR: this is a stop control, and the error red is spoken
        // for by failed files in the grid below.
        (text, colors::ACCENT, CardAction::Cancel)
    } else {
        let outstanding = run.states.iter().filter(|s| **s != FileState::Done).count();
        if outstanding == 0 {
            return None;
        }
        (
            format!(
                "Ingest {} · {}",
                plural(outstanding, "file"),
                fmt_bytes(run.bytes_outstanding())
            ),
            colors::PULL,
            CardAction::Start,
        )
    };

    // The leading mark is a real icon from the set the rest of the app uses, rather than a
    // font glyph, which sits on the text baseline at the wrong weight and size.
    let icon = match action {
        CardAction::Eject => Some(super::check_texture(ui.ctx())),
        CardAction::Cancel => Some(super::x_icon_texture(ui.ctx())),
        CardAction::Start => None,
    };
    let icon_size = layout::INLINE_ICON_SIZE;
    let icon_width = icon
        .as_ref()
        .map(|_| icon_size + layout::ROW_INTRA_ICON_GAP * 2.0)
        .unwrap_or(0.0);

    let font = egui::FontId::proportional(layout::INGEST_ACTION_SIZE);
    let text_size = ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(label.clone(), font.clone(), egui::Color32::WHITE)
            .rect
            .size()
    });
    let button_rect = egui::Rect::from_min_size(
        egui::pos2(
            header_rect.right() - text_size.x - icon_width,
            header_rect.top(),
        ),
        egui::vec2(text_size.x + icon_width, text_size.y.max(icon_size)),
    );
    let response = ui.interact(
        button_rect,
        ui.id().with(("ingest_action", &run.card_signature)),
        egui::Sense::click(),
    );
    let tint = if response.hovered() { colors::HOVER } else { color };
    if let Some(icon) = icon {
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(button_rect.left() + icon_size / 2.0, button_rect.center().y),
            egui::vec2(icon_size, icon_size),
        );
        ui.painter().image(
            icon.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
    }
    // Painted with `text` rather than a pre-laid galley: a galley carries the colour it was
    // laid out with, so tinting it at paint time would be silently ignored.
    ui.painter().text(
        egui::pos2(button_rect.left() + icon_width, button_rect.center().y),
        egui::Align2::LEFT_CENTER,
        &label,
        font,
        tint,
    );
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.clicked().then_some(action)
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Largest square cell size that fits `count` cells into `rect`.
///
/// `cols * (cell + gap) - gap <= width` is the real constraint — the last column and row
/// have no trailing gap — which is why the `+ gap` appears on both sides.
pub fn cell_size(width: f32, height: f32, count: usize, gap: f32) -> f32 {
    if count == 0 || width <= 0.0 || height <= 0.0 {
        return 0.0;
    }
    let fits = |cell: f32| {
        let cols = ((width + gap) / (cell + gap)).floor().max(0.0) as u64;
        let rows = ((height + gap) / (cell + gap)).floor().max(0.0) as u64;
        cols.saturating_mul(rows) >= count as u64
    };
    // Monotone in `cell`, so a bisection converges quickly and exactly enough for pixels.
    let (mut low, mut high) = (layout::INGEST_MIN_CELL, layout::INGEST_MAX_CELL);
    if !fits(low) {
        return 0.0;
    }
    for _ in 0..24 {
        let mid = (low + high) / 2.0;
        if fits(mid) {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}

fn draw_grid(ui: &mut egui::Ui, run: &CardRun, rect: egui::Rect) {
    let count = run.states.len();
    if count == 0 {
        return;
    }
    let gap = layout::INGEST_CELL_GAP;
    let cell = cell_size(rect.width(), rect.height(), count, gap);
    if cell <= 0.0 {
        return;
    }
    let cols = (((rect.width() + gap) / (cell + gap)).floor() as usize).max(1);

    // Busy squares pulse, which needs a repaint even when nothing else is going on. The
    // existing keep-alive ticks too slowly to read as a pulse rather than a stutter.
    let pulsing = run.states.iter().any(|state| state.is_busy());
    if pulsing {
        ui.ctx().request_repaint();
    }
    let now = Instant::now();

    let painter = ui.painter_at(rect);
    let cell_rect = |index: usize| {
        let (col, row) = (index % cols, index / cols);
        egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + col as f32 * (cell + gap),
                rect.top() + row as f32 * (cell + gap),
            ),
            egui::vec2(cell, cell),
        )
    };

    for (index, file_state) in run.states.iter().enumerate() {
        let square = cell_rect(index);
        if square.bottom() > rect.bottom() {
            break;
        }
        let mut color = state_color(*file_state);
        if file_state.is_busy() {
            // Timed from when each square became busy, so a grid of them shimmers rather
            // than blinking in unison.
            let elapsed = run
                .busy_since
                .get(index)
                .and_then(|started| *started)
                .map(|started| now.saturating_duration_since(started).as_secs_f32())
                .unwrap_or(0.0);
            color = lighten(color, pulse_amount(elapsed, index));
        }
        painter.rect_filled(square, layout::INGEST_CELL_ROUNDING, color);
    }

    // One interaction for the whole grid: 2,000 individually-sensed rects would mean 2,000
    // egui ids per frame for no benefit, when the hovered cell is just arithmetic.
    let response = ui.interact(
        rect,
        ui.id().with(("ingest_grid", &run.card_signature)),
        egui::Sense::click(),
    );
    let Some(pointer) = response.hover_pos() else {
        return;
    };
    let col = ((pointer.x - rect.left()) / (cell + gap)).floor();
    let row = ((pointer.y - rect.top()) / (cell + gap)).floor();
    if col < 0.0 || row < 0.0 || col >= cols as f32 {
        return;
    }
    let index = row as usize * cols + col as usize;
    let Some(file) = run.scan.files.get(index) else {
        return;
    };
    // Only inside the square itself, not the gap around it.
    if !cell_rect(index).contains(pointer) {
        return;
    }

    let mut tooltip = file.rel_path.to_string_lossy().to_string();
    if let Some(error) = run.errors.get(&index) {
        tooltip.push('\n');
        tooltip.push_str(error);
    }
    // A per-cell id keeps the tooltip from going stale as the pointer moves between cells.
    egui::show_tooltip_at_pointer(
        ui.ctx(),
        ui.id().with(("ingest_cell", run.id, index)),
        |ui| ui.label(tooltip),
    );
    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);

    if response.clicked() {
        let dst = run.dest_dir.join(&file.rel_path);
        reveal(if dst.exists() { &dst } else { &file.src_abs });
    }
}

/// Render a remaining-time estimate for the progress line.
///
/// Rounded coarsely on purpose: this is a projection from the last twenty seconds of a
/// network copy, so presenting it to the second would claim a precision it does not have,
/// and a number that visibly churns is worse than a vaguer one that holds still. Sub-minute
/// remainders are still shown in seconds — calibrating against the real 113-file card, the
/// last fifty seconds of a run all fell inside one minute, and collapsing them to a single
/// "<1 minute" told the user nothing for most of the time they were watching.
pub fn fmt_eta(remaining: std::time::Duration) -> String {
    let secs = remaining.as_secs();
    if secs < 60 {
        // To the nearest five seconds, never zero: "0 seconds" alongside a still-running
        // progress count reads as a stall rather than an imminent finish.
        let rounded = ((secs + 2) / 5 * 5).max(5);
        return format!("ETL {rounded} seconds");
    }
    if secs >= 60 * 60 {
        let hours = secs / 3600;
        let minutes = (secs % 3600 + 30) / 60;
        // Rounding the minutes can carry into the next hour.
        let (hours, minutes) = if minutes >= 60 { (hours + 1, 0) } else { (hours, minutes) };
        let hour_label = if hours == 1 { "hour" } else { "hours" };
        return if minutes == 0 {
            format!("ETL {hours} {hour_label}")
        } else {
            format!("ETL {hours} {hour_label} {minutes} min")
        };
    }
    // Nearest minute, not the next one up: ceiling-rounding turned a well-measured 61
    // seconds into "2 minutes", which read as twice the wait actually left.
    let minutes = ((secs + 30) / 60).max(1);
    if minutes == 1 {
        "ETL 1 minute".to_string()
    } else {
        format!("ETL {minutes} minutes")
    }
}

/// How far towards white a busy square sits, `elapsed` seconds into its own pulse.
///
/// Timing from when each square became busy is most of the story, but four workers pick up
/// four files within milliseconds of each other, so start times alone still leave whole
/// groups blinking in unison. The index contributes an additional stagger via the golden
/// ratio, whose irrationality keeps neighbouring squares far apart in the cycle rather than
/// falling into a repeating pattern the eye would read as a band.
fn pulse_amount(elapsed: f32, index: usize) -> f32 {
    const GOLDEN_RATIO_CONJUGATE: f32 = 0.618_034;
    let stagger = (index as f32 * GOLDEN_RATIO_CONJUGATE).fract() * layout::INGEST_PULSE_SECONDS;
    let wave =
        ((elapsed + stagger) * std::f32::consts::TAU / layout::INGEST_PULSE_SECONDS).sin();
    // sin spans -1..1; mapped to 0..1 so a square only ever brightens from its base colour.
    layout::INGEST_PULSE_LIGHTEN * (0.5 + 0.5 * wave)
}

/// Blend `color` towards white by `amount` (0..1).
///
/// Brightening rather than dimming keeps the state legible: a darkened blue square reads as
/// a different, duller state, whereas a lighter one reads as the same square, working.
fn lighten(color: egui::Color32, amount: f32) -> egui::Color32 {
    let mix = |channel: u8| {
        (channel as f32 + (255.0 - channel as f32) * amount.clamp(0.0, 1.0)).round() as u8
    };
    egui::Color32::from_rgb(mix(color.r()), mix(color.g()), mix(color.b()))
}

fn state_color(state: FileState) -> egui::Color32 {
    match state {
        FileState::Pending => colors::TEXT_DISABLED,
        FileState::Incomplete => colors::MSG_WARNING,
        FileState::Copying => colors::PULL,
        FileState::Verifying => colors::PUSH,
        FileState::Done => colors::STATUS_COMPLETE,
        FileState::Failed => colors::MSG_ERROR,
    }
}

/// Open Explorer with the file selected.
///
/// `open::that` can only open a path, not highlight one, so this shells out. Explorer
/// exits with a non-zero status even when it works, so its result is deliberately ignored.
fn reveal(path: &std::path::Path) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

/// Leave the ingest screen.
pub fn back(state: &mut AppState) {
    state.screen = Screen::Main;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_shrink_until_every_file_fits() {
        // 2120 files in a typical window: everything must fit, with nothing left over.
        let cell = cell_size(830.0, 400.0, 2120, 2.0);
        assert!(cell > 0.0);
        let cols = ((830.0 + 2.0) / (cell + 2.0)).floor() as u64;
        let rows = ((400.0 + 2.0) / (cell + 2.0)).floor() as u64;
        assert!(cols * rows >= 2120, "{cols}x{rows} cannot hold 2120");
    }

    #[test]
    fn a_few_files_get_large_cells_but_no_larger_than_the_cap() {
        let cell = cell_size(830.0, 400.0, 4, 2.0);
        assert!(cell <= layout::INGEST_MAX_CELL);
        assert!(cell >= layout::INGEST_MAX_CELL - 0.01);
    }

    #[test]
    fn the_last_column_needs_no_trailing_gap() {
        // Four 10px cells with 2px gaps span 46px, not 48 — the row has three gaps, not
        // four. Asserted as the property rather than the number, because the bisection
        // legitimately lands a hair under a round value.
        let cell = cell_size(46.0, 10.0, 4, 2.0);
        let cols = ((46.0 + 2.0) / (cell + 2.0)).floor() as u64;
        assert_eq!(cols, 4, "46px should hold 4 cells of {cell}px");
        assert!(cell * 4.0 + 2.0 * 3.0 <= 46.0 + 0.01, "cells overflow the row");
    }

    #[test]
    fn degenerate_regions_are_handled_rather_than_dividing_by_zero() {
        assert_eq!(cell_size(0.0, 100.0, 10, 2.0), 0.0);
        assert_eq!(cell_size(100.0, 0.0, 10, 2.0), 0.0);
        assert_eq!(cell_size(100.0, 100.0, 0, 2.0), 0.0);
        assert_eq!(cell_size(-5.0, 100.0, 10, 2.0), 0.0);
        // More files than could ever fit at the minimum cell size.
        assert_eq!(cell_size(20.0, 20.0, 1_000_000, 2.0), 0.0);
    }

    #[test]
    fn a_full_grid_still_fits_at_the_minimum_window_size() {
        // 600x400 is the window minimum; two cards halve the height each.
        let cell = cell_size(590.0, 160.0, 2120, 2.0);
        assert!(cell > 0.0, "2120 files must still fit in a half-height minimum window");
    }

    #[test]
    fn a_busy_square_only_ever_brightens() {
        let base = colors::PULL;
        // Sampled across a couple of full periods; never darker, never more than the cap.
        for step in 0..200 {
            let phase = step as f32 * layout::INGEST_PULSE_SECONDS / 50.0;
            let amount = pulse_amount(phase, step);
            assert!(
                (0.0..=layout::INGEST_PULSE_LIGHTEN + 1e-6).contains(&amount),
                "pulse {amount} out of range at {phase}s"
            );
            let lit = lighten(base, amount);
            assert!(lit.r() >= base.r() && lit.g() >= base.g() && lit.b() >= base.b());
        }
    }

    #[test]
    fn the_pulse_actually_reaches_both_ends_of_its_range() {
        let quarter = layout::INGEST_PULSE_SECONDS / 4.0;
        // Peak at a quarter period, trough at three quarters, for a square with no stagger.
        assert!((pulse_amount(quarter, 0) - layout::INGEST_PULSE_LIGHTEN).abs() < 1e-5);
        assert!(pulse_amount(3.0 * quarter, 0).abs() < 1e-5);
    }

    #[test]
    fn squares_that_started_at_different_times_are_out_of_phase() {
        // A quarter period apart, not a half: half a period lands on the opposite zero
        // crossing of the sine, where the brightness happens to match again.
        let quarter = layout::INGEST_PULSE_SECONDS / 4.0;
        let spread = (pulse_amount(0.0, 0) - pulse_amount(quarter, 0)).abs();
        assert!(spread > 0.1, "quarter-period offset barely differs ({spread})");
    }

    #[test]
    fn neighbouring_squares_shimmer_even_when_they_start_together() {
        // Four workers start four files at effectively the same instant; without the
        // per-index stagger that whole group would blink as one block.
        let together: Vec<f32> = (0..8).map(|i| pulse_amount(0.0, i)).collect();
        for pair in together.windows(2) {
            assert!(
                (pair[0] - pair[1]).abs() > 0.03,
                "adjacent squares are nearly identical: {pair:?}"
            );
        }
        let spread = together.iter().cloned().fold(f32::MIN, f32::max)
            - together.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread > layout::INGEST_PULSE_LIGHTEN * 0.7, "too little spread: {spread}");
    }

    #[test]
    fn lighten_clamps_and_preserves_the_extremes() {
        let c = egui::Color32::from_rgb(10, 128, 250);
        assert_eq!(lighten(c, 0.0), c);
        assert_eq!(lighten(c, 1.0), egui::Color32::WHITE);
        // Out-of-range input must not wrap around or panic.
        assert_eq!(lighten(c, -1.0), c);
        assert_eq!(lighten(c, 5.0), egui::Color32::WHITE);
    }

    #[test]
    fn remaining_time_reads_naturally_at_every_scale() {
        use std::time::Duration;
        assert_eq!(fmt_eta(Duration::from_secs(0)), "ETL 5 seconds");
        assert_eq!(fmt_eta(Duration::from_secs(4)), "ETL 5 seconds");
        assert_eq!(fmt_eta(Duration::from_secs(28)), "ETL 30 seconds");
        assert_eq!(fmt_eta(Duration::from_secs(52)), "ETL 50 seconds");
        assert_eq!(fmt_eta(Duration::from_secs(59)), "ETL 60 seconds");
        assert_eq!(fmt_eta(Duration::from_secs(60)), "ETL 1 minute");
        assert_eq!(fmt_eta(Duration::from_secs(90)), "ETL 2 minutes");
        assert_eq!(fmt_eta(Duration::from_secs(120)), "ETL 2 minutes");
        assert_eq!(fmt_eta(Duration::from_secs(35 * 60)), "ETL 35 minutes");
        assert_eq!(fmt_eta(Duration::from_secs(3600)), "ETL 1 hour");
        assert_eq!(fmt_eta(Duration::from_secs(3600 + 20 * 60)), "ETL 1 hour 20 min");
        assert_eq!(fmt_eta(Duration::from_secs(2 * 3600 + 60)), "ETL 2 hours 1 min");
        // Minute rounding must carry into the hour rather than printing "60 min".
        assert_eq!(fmt_eta(Duration::from_secs(3600 + 59 * 60 + 45)), "ETL 2 hours");
        // Absurd inputs must still format rather than panic.
        assert!(fmt_eta(Duration::from_secs(u32::MAX as u64)).starts_with("ETL "));
    }

    #[test]
    fn a_well_measured_minute_is_not_reported_as_two() {
        use std::time::Duration;
        // Calibrated against the real card: at t=16s the estimator computed ~61s remaining
        // and ~60s really did remain. Ceiling-rounding called that "2 minutes".
        assert_eq!(fmt_eta(Duration::from_secs(61)), "ETL 1 minute");
        assert_eq!(fmt_eta(Duration::from_secs(89)), "ETL 1 minute");
    }

    #[test]
    fn the_estimate_never_counts_down_in_distracting_jumps() {
        use std::time::Duration;
        // Seconds ticking away inside a minute must not change the rendered text.
        assert_eq!(
            fmt_eta(Duration::from_secs(5 * 60 + 1)),
            fmt_eta(Duration::from_secs(5 * 60 + 25))
        );
        // Nor should sub-second jitter move a sub-minute estimate.
        assert_eq!(
            fmt_eta(Duration::from_secs_f64(30.1)),
            fmt_eta(Duration::from_secs_f64(30.9))
        );
    }

    #[test]
    fn every_state_has_its_own_colour() {
        let states = [
            FileState::Pending,
            FileState::Incomplete,
            FileState::Copying,
            FileState::Verifying,
            FileState::Done,
            FileState::Failed,
        ];
        for (i, a) in states.iter().enumerate() {
            for b in &states[i + 1..] {
                assert_ne!(state_color(*a), state_color(*b), "{a:?} and {b:?} look alike");
            }
        }
    }
}
