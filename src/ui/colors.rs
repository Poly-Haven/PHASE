use egui::Color32;

// ── Interaction ───────────────────────────────────────────────────────────────
/// Tint / text color applied to any clickable element on hover.
pub const HOVER: Color32 = Color32::from_rgb(190, 111, 255);
pub const ACCENT: Color32 = Color32::from_rgb(225, 45, 91);

// ── Text ─────────────────────────────────────────────────────────────────────
/// Primary text, used for active rows and active selector options.
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(238, 238, 238);
/// Disabled text, used for unavailable rows and disabled actions.
pub const TEXT_DISABLED: Color32 = Color32::from_gray(80);

// ── Selector chrome ───────────────────────────────────────────────────────────
pub const PILL_OPTION_BG: Color32 = Color32::from_rgba_premultiplied(26, 26, 26, 26);
pub const PILL_OPTION_BG_HOVER: Color32 = Color32::from_rgba_premultiplied(36, 36, 36, 36);

// ── Row chrome ────────────────────────────────────────────────────────────────
pub const ROW_BACKGROUND: Color32 = Color32::from_black_alpha(60);
/// Progress bar fill while a copy job is active.
pub const PROGRESS_BAR: Color32 = Color32::from_rgb(50, 110, 200);

// ── Banners ───────────────────────────────────────────────────────────────────
/// Error banner text.
pub const ERROR_BANNER: Color32 = Color32::from_rgb(220, 80, 80);

// ── Row message icons ─────────────────────────────────────────────────────────
pub const MSG_INFO: Color32 = Color32::from_rgb(70, 130, 220);
pub const MSG_ERROR: Color32 = Color32::from_rgb(210, 60, 60);
pub const MSG_WARNING: Color32 = Color32::from_rgb(210, 130, 30);
pub const MSG_QUESTION: Color32 = Color32::from_rgb(60, 175, 80);

// ── Asset types ───────────────────────────────────────────────────────────────
pub const ASSET_TYPE_HDRIS: Color32 = Color32::from_rgb(65, 187, 217);
pub const ASSET_TYPE_TEXTURES: Color32 = Color32::from_rgb(243, 130, 55);
pub const ASSET_TYPE_MODELS: Color32 = Color32::from_rgb(161, 208, 77);

// ── Status groups ─────────────────────────────────────────────────────────────
pub const STATUS_TODO: Color32 = ACCENT;
pub const STATUS_IN_PROGRESS: Color32 = Color32::from_rgb(70, 130, 220);
pub const STATUS_COMPLETE: Color32 = Color32::from_rgb(60, 175, 80);

pub fn colored_background(color: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 51)
}

#[cfg(test)]
mod tests {
    #[test]
    fn colored_backgrounds_are_eighty_percent_transparent() {
        let bg = super::colored_background(super::ACCENT);

        assert_eq!(bg.a(), 51);
    }

    #[test]
    fn row_background_is_black_tint() {
        assert_eq!(super::ROW_BACKGROUND, egui::Color32::from_black_alpha(60));
    }
}
