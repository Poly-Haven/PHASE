// Shared UI layout measurements, named by purpose instead of raw numbers.

pub const TOP_BAR_EDGE_PADDING: f32 = 4.0;
pub const TOP_BAR_VERTICAL_PADDING: f32 = 4.0;
pub const TOP_BAR_ACTION_GAP: f32 = 6.0;
pub const TOP_BAR_INTERACT_HEIGHT: f32 = 30.0;
pub const TOP_BAR_ICON_SIZE: f32 = 16.0;

pub const SEARCH_FIELD_WIDTH: f32 = 170.0;
pub const SEARCH_FIELD_HORIZONTAL_PADDING: f32 = 8.0;
pub const SEARCH_CLEAR_ICON_SIZE: f32 = 12.0;
pub const SEARCH_CLEAR_ICON_RIGHT_PADDING: f32 = 8.0;
pub const SEARCH_CLEAR_ICON_TEXT_GAP: f32 = 4.0;

pub const AUTHOR_FILTER_ICON_SIZE: f32 = 12.0;
pub const AUTHOR_FILTER_ICON_LEFT_PADDING: f32 = 4.0;
pub const AUTHOR_FILTER_TEXT_GAP: f32 = 4.0;
pub const AUTHOR_FILTER_COMBO_EXTRA_WIDTH: f32 = 36.0;

pub const ROW_HEIGHT: f32 = 52.0;
pub const ROW_PRIMARY_HEIGHT: f32 = 26.0;
pub const ROW_SECONDARY_HEIGHT: f32 = 24.0;
/// Height of the slim archive/unarchive progress bar on a row's bottom edge.
pub const ROW_PROGRESS_BAR_HEIGHT: f32 = 3.0;
pub const ROW_SECTION_PADDING: f32 = 6.0;
pub const ROW_INTRA_ICON_GAP: f32 = 2.0;
pub const ACTION_ICON_SIZE: f32 = 18.0;
pub const INLINE_ICON_SIZE: f32 = 14.0;
pub const LINK_ICON_SIZE: f32 = 12.0;
/// Tiny superscript-style icon tucked against the slug text (copy-slug button).
pub const TINY_ICON_SIZE: f32 = 11.0;

pub const STATUS_BAR_MARGIN_X: f32 = 4.0;
pub const STATUS_BAR_MARGIN_Y: f32 = 2.0;
pub const STATUS_BAR_ICON_SIZE: f32 = 14.0;

pub const ROW_CONTEXT_POPUP_WIDTH: f32 = 140.0;
pub const ROW_CONTEXT_ICON_INSET: f32 = 1.0;

pub const STATUS_PILL_ICON_SIZE: f32 = 10.0;
pub const STATUS_PILL_PADDING_X: f32 = 8.0;
pub const STATUS_PILL_PADDING_Y: f32 = 3.0;
pub const STATUS_PILL_HEIGHT: f32 = 18.0;
pub const STATUS_OPTION_HEIGHT: f32 = 20.0;
pub const STATUS_OPTION_INSET: f32 = 1.0;
pub const STATUS_OPTION_ROUNDING: f32 = 4.0;
pub const STATUS_OPTION_WIDTH_PADDING: f32 = 24.0;
pub const STATUS_OPTION_MIN_WIDTH: f32 = 120.0;

pub const CONFLICT_DIALOG_WIDTH: f32 = 560.0;
pub const CONFLICT_DIALOG_SCROLL_HEIGHT: f32 = 320.0;
pub const TOKEN_PROMPT_WIDTH: f32 = 460.0;
pub const SETTINGS_DIALOG_WIDTH: f32 = 560.0;
pub const SETTINGS_LOCAL_ROOT_WIDTH: f32 = 420.0;
pub const TRANSFER_FILE_LIST_DIALOG_WIDTH: f32 = 720.0;
pub const TRANSFER_FILE_LIST_SCROLL_HEIGHT: f32 = 360.0;
pub const SCRIPT_OUTPUT_DIALOG_WIDTH: f32 = 620.0;
pub const SCRIPT_OUTPUT_DIALOG_HEIGHT: f32 = 280.0;
pub const CHANGELOG_DIALOG_WIDTH: f32 = 460.0;
pub const CHANGELOG_DIALOG_SCROLL_HEIGHT: f32 = 320.0;
pub const VERIFICATION_DIALOG_WIDTH: f32 = 520.0;

/// "What's new" text sizes, from the per-version heading down to body text.
pub const CHANGELOG_VERSION_SIZE: f32 = 16.0;
pub const CHANGELOG_HEADING_SIZE: f32 = 14.0;
pub const CHANGELOG_SUBHEADING_SIZE: f32 = 13.0;
pub const CHANGELOG_BLANK_LINE_HEIGHT: f32 = 4.0;
pub const CHANGELOG_BULLET_INDENT: f32 = 4.0;

pub const DIALOG_SECTION_SPACING_SMALL: f32 = 6.0;
pub const DIALOG_SECTION_SPACING_MEDIUM: f32 = 8.0;
pub const DIALOG_SECTION_SPACING_LARGE: f32 = 12.0;

pub const SELECTOR_PADDING_X: f32 = 8.4;
pub const SELECTOR_PADDING_Y: f32 = 3.5;
pub const SELECTOR_SEPARATOR_WIDTH: f32 = 1.0;
pub const SELECTOR_OPTION_TOP_INSET: f32 = 1.0;
pub const SELECTOR_OPTION_BOTTOM_INSET: f32 = 1.0;
pub const SELECTOR_SELECTED_OUTER_INSET_X: f32 = 0.5;
pub const SELECTOR_SELECTED_OUTER_INSET_Y: f32 = 1.5;
pub const SELECTOR_SELECTED_LABEL_NUDGE_X: f32 = 0.45;
pub const SELECTOR_ROW_HEIGHT: f32 = 28.0;

// Card ingest screen.
pub const INGEST_REGION_PADDING: f32 = 8.0;
pub const INGEST_HEADER_HEIGHT: f32 = 40.0;
pub const INGEST_CARD_NAME_SIZE: f32 = 17.0;
pub const INGEST_DETAIL_SIZE: f32 = 12.0;
pub const INGEST_ACTION_SIZE: f32 = 13.0;
/// Gap between file squares. The last column and row have no trailing gap.
pub const INGEST_CELL_GAP: f32 = 2.0;
pub const INGEST_CELL_ROUNDING: f32 = 1.0;
/// A square this small is still a visible mark; below it the grid stops being readable.
pub const INGEST_MIN_CELL: f32 = 2.0;
/// Upper bound so a card holding three files does not draw three enormous blocks, but
/// loose enough that a card of a hundred still fills its region rather than huddling in the
/// top-left corner.
pub const INGEST_MAX_CELL: f32 = 48.0;
/// Period of the busy-square pulse.
pub const INGEST_PULSE_SECONDS: f32 = 0.8;
/// How far towards white a busy square goes at the top of its pulse.
pub const INGEST_PULSE_LIGHTEN: f32 = 0.3;
pub const INGEST_BUTTON_TEXT_SIZE: f32 = 12.0;
pub const INGEST_BUTTON_PADDING_X: f32 = 10.0;
/// The back chevron is drawn larger than its label; the glyph sits well below cap height.
pub const INGEST_BACK_CHEVRON_SIZE: f32 = 20.0;
pub const INGEST_BACK_GAP: f32 = 5.0;
