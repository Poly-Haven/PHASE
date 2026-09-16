//! The summary PHASE leaves at the root of a card it has ingested.
//!
//! This exists for a person, months later, holding a card and wondering whether it is safe
//! to format. Nothing else on the card can answer that: the files look the same whether or
//! not they were ever copied anywhere. Re-ingests append rather than overwrite, so the card
//! keeps its whole history.

use std::io::Write;
use std::path::Path;

use super::scan::Skipped;
use crate::ui::table::fmt_bytes;

/// What one ingest run did to one card.
pub struct Summary {
    pub card_name: String,
    pub card_signature: String,
    pub destination: String,
    pub cameras: Vec<String>,
    pub ingested: usize,
    pub bytes: u64,
    pub skipped: Skipped,
    /// Relative path and reason, for anything that never made it.
    pub failures: Vec<(String, String)>,
    /// True when the run was stopped rather than finishing on its own.
    pub cancelled: bool,
}

impl Summary {
    /// The question the log exists to answer.
    pub fn safe_to_format(&self) -> bool {
        self.failures.is_empty() && !self.cancelled && self.ingested > 0
    }
}

/// Append this run's block to the card's log, creating it if needed.
///
/// A write-protected or full card is a warning, never a failed ingest — the files are
/// already safely copied by the time we get here.
pub fn append_to_card(card_root: &Path, block: &str) {
    let path = card_root.join(super::CARD_LOG_NAME);
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(block.as_bytes()));
    match result {
        Ok(()) => log::info!("Wrote ingest summary to {}", path.display()),
        Err(err) => log::warn!("Could not write {}: {err}", path.display()),
    }
}

/// Render one run's block. `now` is passed in so this is testable.
pub fn render(summary: &Summary, version: &str, now: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("PHASE ingest — {now} — v{version}\n"));
    out.push_str(&format!(
        "Card:        {}  (signature {})\n",
        summary.card_name, summary.card_signature
    ));
    out.push_str(&format!("Destination: {}\n", summary.destination));
    if !summary.cameras.is_empty() {
        out.push_str(&format!("Cameras:     {}\n", summary.cameras.join(", ")));
    }
    out.push('\n');

    out.push_str(&format!(
        "Ingested:    {} — {} — verified by BLAKE3 and by decoding every image\n",
        plural(summary.ingested, "file"),
        fmt_bytes(summary.bytes),
    ));
    if let Some(skipped) = summary.skipped.describe() {
        // `describe` reads "skipped 113 JPEG with RAW"; the label already says Skipped.
        let detail = skipped.trim_start_matches("skipped ").to_string();
        out.push_str(&format!("Skipped:     {detail}\n"));
    }
    out.push_str(&format!("Failed:      {}\n", summary.failures.len()));
    for (path, reason) in &summary.failures {
        out.push_str(&format!("               {path} — {reason}\n"));
    }
    out.push('\n');

    let verdict = if summary.safe_to_format() {
        "SAFE TO FORMAT: YES"
    } else if summary.cancelled {
        "SAFE TO FORMAT: NO — this run was cancelled before it finished"
    } else if !summary.failures.is_empty() {
        "SAFE TO FORMAT: NO — the files listed above did not copy successfully"
    } else {
        "SAFE TO FORMAT: NO — nothing was ingested"
    };
    out.push_str(verdict);
    out.push_str("\n\n");
    out
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Local wall-clock time as `YYYY-MM-DD HH:MM:SS`.
///
/// Win32 hands this over already broken into fields in the user's own timezone, which is
/// both simpler and more correct than deriving a local date from a UNIX timestamp — and it
/// avoids taking on a date/time crate for one line of a log file.
pub fn local_timestamp() -> String {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(lpSystemTime: *mut SystemTime);
    }

    let mut now = SystemTime::default();
    unsafe { GetLocalTime(&mut now) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        now.year, now.month, now.day, now.hour, now.minute, now.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Summary {
        Summary {
            card_name: "120GB_671658".into(),
            card_signature: "671658a1b2c3d4e5".into(),
            destination: r"P:\_INGEST\120GB_671658".into(),
            cameras: vec!["Nikon Z 7".into()],
            ingested: 2120,
            bytes: 108_500_000_000,
            skipped: Skipped { jpeg_with_raw: 0, other: 1 },
            failures: Vec::new(),
            cancelled: false,
        }
    }

    #[test]
    fn a_clean_run_says_the_card_is_safe_to_format() {
        let text = render(&summary(), "1.8.0", "2026-09-16 14:32:11");
        assert!(text.contains("PHASE ingest — 2026-09-16 14:32:11 — v1.8.0"));
        assert!(text.contains("120GB_671658  (signature 671658a1b2c3d4e5)"));
        assert!(text.contains("Cameras:     Nikon Z 7"));
        assert!(text.contains("Ingested:    2120 files"));
        assert!(text.contains("Skipped:     1 non-image"));
        assert!(text.contains("Failed:      0"));
        assert!(text.contains("SAFE TO FORMAT: YES"));
        assert!(summary().safe_to_format());
    }

    #[test]
    fn failures_are_named_and_flip_the_verdict() {
        let mut summary = summary();
        summary.failures = vec![
            ("DCIM/100/IMG_1.NEF".into(), "hash mismatch".into()),
            ("DCIM/100/IMG_2.NEF".into(), "RAW decode failed".into()),
        ];
        let text = render(&summary, "1.8.0", "now");
        assert!(text.contains("Failed:      2"));
        assert!(text.contains("DCIM/100/IMG_1.NEF — hash mismatch"));
        assert!(text.contains("DCIM/100/IMG_2.NEF — RAW decode failed"));
        assert!(text.contains("SAFE TO FORMAT: NO — the files listed above"));
        assert!(!summary.safe_to_format());
    }

    #[test]
    fn a_cancelled_run_is_never_safe_to_format() {
        let mut summary = summary();
        summary.cancelled = true;
        assert!(render(&summary, "1.8.0", "now").contains("SAFE TO FORMAT: NO — this run was cancelled"));
        assert!(!summary.safe_to_format());
    }

    #[test]
    fn an_empty_run_is_never_safe_to_format() {
        let mut summary = summary();
        summary.ingested = 0;
        assert!(render(&summary, "1.8.0", "now").contains("SAFE TO FORMAT: NO — nothing was ingested"));
        assert!(!summary.safe_to_format());
    }

    #[test]
    fn one_file_is_not_pluralised() {
        let mut summary = summary();
        summary.ingested = 1;
        assert!(render(&summary, "1.8.0", "now").contains("Ingested:    1 file —"));
    }

    #[test]
    fn re_ingesting_appends_rather_than_overwriting() {
        let temp = tempfile::tempdir().unwrap();
        append_to_card(temp.path(), &render(&summary(), "1.8.0", "first run"));
        append_to_card(temp.path(), &render(&summary(), "1.8.1", "second run"));

        let text = std::fs::read_to_string(temp.path().join(super::super::CARD_LOG_NAME)).unwrap();
        assert!(text.contains("first run"));
        assert!(text.contains("second run"));
        assert_eq!(text.matches("SAFE TO FORMAT").count(), 2);
        assert!(text.find("first run") < text.find("second run"));
    }

    #[test]
    fn an_unwritable_card_does_not_panic() {
        // A directory that does not exist stands in for a write-protected card.
        append_to_card(Path::new(r"Z:\definitely\not\here"), "block");
    }

    #[test]
    fn the_timestamp_looks_like_a_timestamp() {
        let now = local_timestamp();
        assert_eq!(now.len(), 19, "{now}");
        assert!(now.starts_with("20"), "{now}");
        let (date, time) = now.split_once(' ').unwrap();
        assert_eq!(date.split('-').count(), 3);
        assert_eq!(time.split(':').count(), 3);
    }
}
