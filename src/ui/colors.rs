use egui::Color32;

// ── Interaction ───────────────────────────────────────────────────────────────
/// Tint / text color applied to any clickable element on hover.
pub const HOVER:         Color32 = Color32::from_rgb(190, 111, 255);

// ── Row text ─────────────────────────────────────────────────────────────────
/// Slug / author text for rows whose Prod folder is present.
pub const SLUG_ACTIVE:   Color32 = Color32::from_rgb(238, 238, 238);
/// Slug / author text for rows whose Prod folder is absent.
pub const SLUG_MISSING:  Color32 = Color32::from_gray(80);

// ── Row chrome ────────────────────────────────────────────────────────────────
/// Progress bar fill while a copy job is active.
pub const PROGRESS_BAR:  Color32 = Color32::from_rgb(50, 110, 200);

// ── Banners ───────────────────────────────────────────────────────────────────
/// Error banner text.
pub const ERROR_BANNER:  Color32 = Color32::from_rgb(220, 80, 80);

// ── Row message icons ─────────────────────────────────────────────────────────
pub const MSG_INFO:      Color32 = Color32::from_rgb(70, 130, 220);
pub const MSG_ERROR:     Color32 = Color32::from_rgb(210, 60, 60);
pub const MSG_WARNING:   Color32 = Color32::from_rgb(210, 130, 30);
pub const MSG_QUESTION:  Color32 = Color32::from_rgb(60, 175, 80);
