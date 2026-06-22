pub fn notion_logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/notion.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "notion_logo", "notion.svg"))
        .clone()
}

pub fn pull_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/box-arrow-in-down.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "pull_icon", "box-arrow-in-down.svg"))
        .clone()
}

pub fn push_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/cloud-upload-fill.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "push_icon", "cloud-upload-fill.svg"))
        .clone()
}

pub fn info_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/info.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "icon_info", "info.svg"))
        .clone()
}

pub fn warn_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/exclamation-triangle.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "icon_warn", "exclamation-triangle.svg"))
        .clone()
}

pub fn error_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/exclamation-diamond.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "icon_error", "exclamation-diamond.svg"))
        .clone()
}

pub fn question_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/question.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "icon_question", "question.svg"))
        .clone()
}

pub fn loading_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/loading.png");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| {
        let image = egui_extras::image::load_image_bytes(BYTES).expect("loading.png");
        ctx.load_texture("loading_spinner", image, egui::TextureOptions::LINEAR)
    })
    .clone()
}

pub fn chevron_down_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/chevron-down.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "chevron_down", "chevron-down.svg"))
        .clone()
}

pub fn list_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/list.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "list_icon", "list.svg"))
        .clone()
}

pub fn x_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/x.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "x_icon", "x.svg"))
        .clone()
}

pub fn external_link_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/box-arrow-up-right.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "external_link", "box-arrow-up-right.svg"))
        .clone()
}

pub fn check_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/check.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "check_icon", "check.svg"))
        .clone()
}

/// The copy icon is drawn very small (superscript beside the slug). The usual
/// fixed 4× oversample backfires here: rasterizing large and letting the GPU
/// minify it down (bilinear, no mipmaps) resamples the image and produces
/// shimmer/aliasing. Instead, rasterize at *exactly* the physical on-screen size
/// so the caller can blit it 1:1, pixel-aligned, with no GPU rescale at all —
/// resvg's coverage anti-aliasing then *is* the final image. Returns the texture
/// and its physical pixel size; re-rendered only when the size (i.e. DPI) changes.
pub fn copy_icon_texture(ctx: &egui::Context, display_pts: f32) -> (egui::TextureHandle, u32) {
    use std::cell::RefCell;
    static BYTES: &[u8] = include_bytes!("../assets/copy.svg");
    thread_local! {
        static CACHE: RefCell<Option<(u32, egui::TextureHandle)>> = const { RefCell::new(None) };
    }
    let target_px = ((display_pts * ctx.pixels_per_point()).round() as u32).max(1);
    let tex = CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((px, tex)) = cache.as_ref() {
            if *px == target_px {
                return tex.clone();
            }
        }
        let tex = load_svg_texture_to_px(ctx, BYTES, "copy_icon", "copy.svg", target_px);
        *cache = Some((target_px, tex.clone()));
        tex
    });
    (tex, target_px)
}

pub fn gear_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/gear-fill.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "gear_icon", "gear-fill.svg"))
        .clone()
}

pub fn folder_fill_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/folder-fill.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "folder_fill_icon", "folder-fill.svg"))
        .clone()
}

pub fn hdd_network_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/hdd-network.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "hdd_network_icon", "hdd-network.svg"))
        .clone()
}

pub fn unarchive_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/box-arrow-in-down-left.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| {
        load_svg_texture(ctx, BYTES, "unarchive_icon", "box-arrow-in-down-left.svg")
    })
    .clone()
}

pub fn file_earmark_zip_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/file-earmark-zip.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "file_earmark_zip_icon", "file-earmark-zip.svg"))
        .clone()
}

pub fn load_svg_texture(
    ctx: &egui::Context,
    bytes: &[u8],
    texture_name: &'static str,
    debug_name: &'static str,
) -> egui::TextureHandle {
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    // Resolve `fill="currentColor"` to white so egui's `tint()` can multiply
    // it down to whatever colour the row needs at draw time.
    opt.style_sheet = Some("svg { color: #ffffff; }".to_string());

    let tree = usvg::Tree::from_data(bytes, &opt).expect(debug_name);
    let size = tree.size().to_int_size();
    // Render at 4x for crisp shrunken icons (the button is ~18px but the
    // source SVG may be 16px or 24px; oversampling avoids aliasing).
    let scale = 4u32;
    let w = size.width() * scale;
    let h = size.height() * scale;
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect(debug_name);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale as f32, scale as f32),
        &mut pixmap.as_mut(),
    );
    ctx.load_texture(
        texture_name,
        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], pixmap.data()),
        egui::TextureOptions::LINEAR,
    )
}

/// Rasterize an SVG so its larger dimension is `target_px` pixels. Use this for
/// icons drawn very small, where rendering near the display size (rather than the
/// fixed oversample in `load_svg_texture`) preserves thin strokes that heavy GPU
/// minification would otherwise drop.
fn load_svg_texture_to_px(
    ctx: &egui::Context,
    bytes: &[u8],
    texture_name: &'static str,
    debug_name: &'static str,
    target_px: u32,
) -> egui::TextureHandle {
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    opt.style_sheet = Some("svg { color: #ffffff; }".to_string());

    let tree = usvg::Tree::from_data(bytes, &opt).expect(debug_name);
    let size = tree.size();
    let native = size.width().max(size.height()).max(1.0);
    let scale = target_px as f32 / native;
    let w = ((size.width() * scale).ceil() as u32).max(1);
    let h = ((size.height() * scale).ceil() as u32).max(1);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect(debug_name);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    ctx.load_texture(
        texture_name,
        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], pixmap.data()),
        egui::TextureOptions::LINEAR,
    )
}

#[cfg(test)]
mod tests {
    /// The copy icon is stroked (not filled) and relies on the loader stylesheet
    /// resolving `currentColor` to white. We can't eyeball the GUI here, so guard
    /// against a malformed path or an unresolved stroke colour (either of which
    /// would render nothing / non-white) by rasterizing it the same way the
    /// loader does and checking for white pixels.
    #[test]
    fn copy_icon_svg_renders_white_strokes() {
        let bytes = include_bytes!("../assets/copy.svg");
        let mut opt = usvg::Options::default();
        opt.style_sheet = Some("svg { color: #ffffff; }".to_string());
        let tree = usvg::Tree::from_data(bytes, &opt).expect("copy.svg should parse");
        let size = tree.size().to_int_size();
        let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).unwrap();
        resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
        let has_white = pixmap
            .pixels()
            .iter()
            .any(|px| px.red() > 0 && px.red() == px.green() && px.green() == px.blue());
        assert!(
            has_white,
            "copy.svg should render white strokes (currentColor → #fff)"
        );
    }
}
