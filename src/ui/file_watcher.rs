use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

/// Which root a raw filesystem event originated from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WatchSource {
    Local,
    Prod,
}

/// A raw filesystem event forwarded from a `notify` watcher thread into `pump()`.
pub struct RawEvent {
    pub source: WatchSource,
    pub paths: Vec<PathBuf>,
}

/// Activity-aware monitoring mode, derived from how long the user has been
/// inactive in the PHASE window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WatchMode {
    /// < 2 min inactive: real-time `notify` watchers on prod.
    RealTime,
    /// 2–10 min inactive: poll prod every 30s.
    Poll30s,
    /// 10–30 min inactive: poll prod every 2 min.
    Poll2min,
    /// > 30 min inactive: no prod monitoring (focus refresh only).
    Idle,
}

impl WatchMode {
    /// Polling cadence for this mode, or `None` if prod is not polled.
    pub fn poll_interval(self) -> Option<Duration> {
        match self {
            WatchMode::Poll30s => Some(Duration::from_secs(30)),
            WatchMode::Poll2min => Some(Duration::from_secs(120)),
            WatchMode::RealTime | WatchMode::Idle => None,
        }
    }

    /// Whether prod should be watched in real time in this mode.
    pub fn is_real_time(self) -> bool {
        matches!(self, WatchMode::RealTime)
    }

    /// Compute the mode for a given inactivity duration.
    pub fn for_inactivity(inactive: Duration) -> WatchMode {
        if inactive < Duration::from_secs(120) {
            WatchMode::RealTime
        } else if inactive < Duration::from_secs(600) {
            WatchMode::Poll30s
        } else if inactive < Duration::from_secs(1800) {
            WatchMode::Poll2min
        } else {
            WatchMode::Idle
        }
    }

    /// Time until the next mode boundary for a given inactivity duration, used
    /// to schedule a repaint so the mode transition is evaluated even while the
    /// window is unfocused. `None` once fully idle (nothing left to schedule).
    pub fn boundary_after(inactive: Duration) -> Option<Duration> {
        const B1: Duration = Duration::from_secs(120);
        const B2: Duration = Duration::from_secs(600);
        const B3: Duration = Duration::from_secs(1800);
        if inactive < B1 {
            Some(B1 - inactive)
        } else if inactive < B2 {
            Some(B2 - inactive)
        } else if inactive < B3 {
            Some(B3 - inactive)
        } else {
            None
        }
    }
}

/// Owns the `notify` watchers and the channel feeding events into `pump()`.
///
/// The local watcher is always active (cheap local disk). The prod watcher only
/// exists in real-time mode and is dropped otherwise so that network
/// (`ReadDirectoryChangesW` over SMB) handles are released, minimising prod
/// access while the user is inactive.
pub struct FileWatcher {
    ctx: egui::Context,
    tx: Sender<RawEvent>,
    rx: Receiver<RawEvent>,
    local: Option<RecommendedWatcher>,
    local_roots: Vec<PathBuf>,
    prod: Option<RecommendedWatcher>,
    prod_paths: Vec<PathBuf>,
}

impl FileWatcher {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            ctx,
            tx,
            rx,
            local: None,
            local_roots: Vec::new(),
            prod: None,
            prod_paths: Vec::new(),
        }
    }

    /// Drain all pending raw events without blocking.
    pub fn drain(&self) -> Vec<RawEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            events.push(event);
        }
        events
    }

    /// Ensure the local recursive watch covers exactly `roots`. Recreated only
    /// when the set of roots changes.
    pub fn ensure_local(&mut self, roots: &[PathBuf]) {
        if self.local.is_some() && self.local_roots == roots {
            return;
        }
        match self.make_watcher(WatchSource::Local) {
            Some(mut watcher) => {
                for root in roots {
                    if let Err(err) = watcher.watch(root, RecursiveMode::Recursive) {
                        log::warn!("Local watch failed for {}: {err}", root.display());
                    }
                }
                self.local = Some(watcher);
                self.local_roots = roots.to_vec();
            }
            None => {
                self.local = None;
                self.local_roots.clear();
            }
        }
    }

    /// Set the prod watch to exactly `paths`. An empty list drops the prod
    /// watcher entirely (releasing network handles). Recreated only when the
    /// desired path set changes.
    pub fn set_prod(&mut self, paths: &[(PathBuf, RecursiveMode)]) {
        if paths.is_empty() {
            self.prod = None;
            self.prod_paths.clear();
            return;
        }
        let desired: Vec<PathBuf> = paths.iter().map(|(path, _)| path.clone()).collect();
        if self.prod.is_some() && self.prod_paths == desired {
            return;
        }
        match self.make_watcher(WatchSource::Prod) {
            Some(mut watcher) => {
                for (path, mode) in paths {
                    if let Err(err) = watcher.watch(path, *mode) {
                        // Non-fatal: a missing or unreachable network path simply
                        // won't deliver real-time events; polling/focus cover it.
                        log::warn!("Prod watch failed for {}: {err}", path.display());
                    }
                }
                self.prod = Some(watcher);
                self.prod_paths = desired;
            }
            None => {
                self.prod = None;
                self.prod_paths.clear();
            }
        }
    }

    fn make_watcher(&self, source: WatchSource) -> Option<RecommendedWatcher> {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let handler = move |res: notify::Result<Event>| match res {
            Ok(event) => {
                let _ = tx.send(RawEvent {
                    source,
                    paths: event.paths,
                });
                ctx.request_repaint();
            }
            Err(err) => {
                log::warn!("File watch error ({source:?}): {err}");
            }
        };
        match notify::recommended_watcher(handler) {
            Ok(watcher) => Some(watcher),
            Err(err) => {
                log::warn!("Failed to create {source:?} watcher: {err}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_thresholds_follow_activity_schedule() {
        assert_eq!(
            WatchMode::for_inactivity(Duration::from_secs(0)),
            WatchMode::RealTime
        );
        assert_eq!(
            WatchMode::for_inactivity(Duration::from_secs(119)),
            WatchMode::RealTime
        );
        assert_eq!(
            WatchMode::for_inactivity(Duration::from_secs(120)),
            WatchMode::Poll30s
        );
        assert_eq!(
            WatchMode::for_inactivity(Duration::from_secs(599)),
            WatchMode::Poll30s
        );
        assert_eq!(
            WatchMode::for_inactivity(Duration::from_secs(600)),
            WatchMode::Poll2min
        );
        assert_eq!(
            WatchMode::for_inactivity(Duration::from_secs(1799)),
            WatchMode::Poll2min
        );
        assert_eq!(
            WatchMode::for_inactivity(Duration::from_secs(1800)),
            WatchMode::Idle
        );
    }

    #[test]
    fn poll_interval_only_in_polling_modes() {
        assert_eq!(WatchMode::RealTime.poll_interval(), None);
        assert_eq!(
            WatchMode::Poll30s.poll_interval(),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            WatchMode::Poll2min.poll_interval(),
            Some(Duration::from_secs(120))
        );
        assert_eq!(WatchMode::Idle.poll_interval(), None);
        assert!(WatchMode::RealTime.is_real_time());
        assert!(!WatchMode::Poll30s.is_real_time());
    }

    #[test]
    fn boundary_counts_down_to_next_threshold_then_stops() {
        assert_eq!(
            WatchMode::boundary_after(Duration::from_secs(0)),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            WatchMode::boundary_after(Duration::from_secs(100)),
            Some(Duration::from_secs(20))
        );
        assert_eq!(
            WatchMode::boundary_after(Duration::from_secs(300)),
            Some(Duration::from_secs(300))
        );
        assert_eq!(WatchMode::boundary_after(Duration::from_secs(1800)), None);
        assert_eq!(WatchMode::boundary_after(Duration::from_secs(5000)), None);
    }
}

