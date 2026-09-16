//! Thumbnail rendering for ingested files.
//!
//! For camera RAW this decodes the **sensor data**, not the embedded preview JPEG.
//! That is the point: a card or network error that corrupts the raw payload leaves the
//! embedded preview perfectly readable, so a preview-based check would happily pass a
//! file whose actual photo is ruined. The 64px thumbnail is the by-product; proving the
//! payload decodes is the job.
//!
//! Decoding 45 MP only to emit 64px would be absurd, so only the expensive part that
//! actually constitutes the integrity check — entropy-decoding the payload — runs in
//! full. Everything after it happens on the downsampled image: we box-average the CFA
//! straight down to thumbnail size, then apply rawler's own white-balance / colour-matrix
//! / sRGB maths to a few thousand pixels instead of tens of millions.

use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};

use image::DynamicImage;
use rawler::cfa::CFAColor;
use rawler::imgop::matrix::{multiply, normalize, pseudo_inverse};
use rawler::imgop::raw::clip_euclidean_norm_avg;
use rawler::imgop::srgb::srgb_apply_gamma;
use rawler::imgop::xyz::{Illuminant, SRGB_TO_XYZ_D65};
use rawler::imgop::{Dim2, Point, Rect};
use rawler::rawimage::{RawImageData, RawPhotometricInterpretation};
use rawler::{Orientation, RawFile, RawImage};

/// Longest edge of a generated thumbnail, in pixels.
pub const THUMB_MAX_EDGE: u32 = 64;

/// JPEG quality for thumbnails. `image` 0.24 only ships the pure-Rust *lossless* WebP
/// encoder, which is PNG-class — the wrong trade for thousands of small files on a NAS.
const THUMB_JPEG_QUALITY: u8 = 85;

// Thumbnails are rendered scene-linear — black/white levels, white balance, the camera's
// colour matrix, sRGB gamma — with no exposure normalisation.
//
// That is a deliberate choice, not an oversight. Auto-exposing each frame would make dark
// frames legible, but these are bracketed HDRI shoots: the dark end of a bracket really
// does peak at under 2% of sensor saturation, and normalising every frame to the same
// apparent brightness would erase exactly the difference that tells one bracket frame from
// the next. A dark square here means the frame is dark.

/// A rendered thumbnail, plus whatever the decoder could tell us about the camera.
pub struct Rendered {
    pub jpeg: Vec<u8>,
    pub camera: Option<String>,
}

/// Render a thumbnail from a file's complete bytes.
///
/// `file_name` only picks the decoder; the bytes are authoritative.
pub fn render(file_name: &str, bytes: Vec<u8>) -> Result<Rendered, String> {
    if super::is_raw_extension(&super::extension_of(file_name)) {
        render_raw(file_name, bytes)
    } else {
        render_standard(bytes)
    }
}

/// Non-RAW stills (JPEG/PNG/TIFF/WebP) go straight through `image`, which does not apply
/// EXIF orientation itself — so we read the hint and apply it, the same as rawler hands us
/// one for RAW.
fn render_standard(bytes: Vec<u8>) -> Result<Rendered, String> {
    let header = super::exif::parse(&bytes[..bytes.len().min(super::exif::HEADER_BYTES)]);
    let decoded = catch_unwind(AssertUnwindSafe(|| image::load_from_memory(&bytes)))
        .map_err(|_| "image decoder panicked".to_string())?
        .map_err(|err| format!("decode failed: {err}"))?;
    let decoded = orient(downscale(decoded), exif_orientation(header.orientation));
    Ok(Rendered {
        jpeg: encode(&decoded)?,
        camera: header.camera(),
    })
}

/// Map an EXIF orientation value onto the same enum rawler reports, so both thumbnail
/// paths rotate through one place.
fn exif_orientation(value: Option<u16>) -> Orientation {
    match value {
        Some(2) => Orientation::HorizontalFlip,
        Some(3) => Orientation::Rotate180,
        Some(4) => Orientation::VerticalFlip,
        Some(5) => Orientation::Transpose,
        Some(6) => Orientation::Rotate90,
        Some(7) => Orientation::Transverse,
        Some(8) => Orientation::Rotate270,
        _ => Orientation::Normal,
    }
}

fn render_raw(file_name: &str, bytes: Vec<u8>) -> Result<Rendered, String> {
    // Never hand rawler something that is not recognisably a RAW container: it will sniff
    // arbitrary bytes into some format and trust the dimensions it finds there. See
    // `exif::looks_like_raw`.
    if !super::exif::looks_like_raw(&bytes) {
        return Err("not a recognisable RAW file".into());
    }

    // rawler `.unwrap()`s in a few decoders (cr2 on its embedded JPEG, for one), so a
    // truncated file can panic rather than error. Catching that is precisely why the
    // release profile no longer sets `panic = "abort"`.
    let decoded = catch_unwind(AssertUnwindSafe(|| {
        let mut raw_file = RawFile::new(file_name, Cursor::new(bytes));
        rawler::decode(&mut raw_file, Default::default())
    }))
    .map_err(|_| "RAW decoder panicked".to_string())?;

    let mut raw = decoded.map_err(|err| format!("RAW decode failed: {err}"))?;
    let camera = describe_camera(&raw);

    // Black/white levels, giving f32 in 0..=1.
    raw.apply_scaling()
        .map_err(|err| format!("RAW rescale failed: {err}"))?;

    let image = catch_unwind(AssertUnwindSafe(|| develop_small(&raw)))
        .map_err(|_| "RAW develop panicked".to_string())??;

    Ok(Rendered {
        jpeg: encode(&image)?,
        camera,
    })
}

fn describe_camera(raw: &RawImage) -> Option<String> {
    let model = first_non_empty(&raw.clean_model, &raw.model)?;
    let make = first_non_empty(&raw.clean_make, &raw.make).unwrap_or_default();
    Some(super::format_camera(make, model))
}

fn first_non_empty<'a>(preferred: &'a str, fallback: &'a str) -> Option<&'a str> {
    [preferred.trim(), fallback.trim()]
        .into_iter()
        .find(|value| !value.is_empty())
}

/// Box-average the CFA down to thumbnail size, then colour-correct the small result.
fn develop_small(raw: &RawImage) -> Result<DynamicImage, String> {
    let RawImageData::Float(ref pixels) = raw.data else {
        return Err("RAW data was not rescaled to float".into());
    };

    let roi = raw.crop_area.or(raw.active_area).unwrap_or_else(|| {
        Rect::new(Point::new(0, 0), Dim2::new(raw.width, raw.height))
    });
    let (x0, y0) = (roi.x(), roi.y());
    let (w, h) = (roi.width(), roi.height());
    if w == 0 || h == 0 || raw.cpp == 0 {
        return Err("RAW has an empty crop area".into());
    }

    let cfa = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(config) if raw.cpp == 1 && config.cfa.is_rgb() => {
            Some(&config.cfa)
        }
        // Four-colour CFAs (RGBE, CYGM) and already-demosaiced data take the generic
        // path below, which averages whatever planes are there.
        _ => None,
    };

    let (out_w, out_h) = fit(w as u32, h as u32, THUMB_MAX_EDGE);
    let (out_w, out_h) = (out_w as usize, out_h as usize);

    // Accumulate each output pixel from its source block. For a CFA we bucket by the
    // colour the sensor actually recorded at each site — a box-filtered debayer, which at
    // 64px is more than good enough and never invents colour the way interpolation can.
    let mut sums = vec![[0f32; 3]; out_w * out_h];
    let mut counts = vec![[0u32; 3]; out_w * out_h];

    for row in 0..h {
        let out_y = row * out_h / h;
        let src_row = (y0 + row) * raw.width * raw.cpp;
        for col in 0..w {
            let slot = out_y * out_w + col * out_w / w;
            match cfa {
                Some(cfa) => {
                    let channel = match cfa.cfa_color_at(y0 + row, x0 + col) {
                        CFAColor::RED => 0,
                        CFAColor::BLUE => 2,
                        // GREEN and FUJI_GREEN both land here; `is_rgb()` gated out
                        // everything else.
                        _ => 1,
                    };
                    if let Some(&value) = pixels.get(src_row + x0 + col) {
                        sums[slot][channel] += value;
                        counts[slot][channel] += 1;
                    }
                }
                None => {
                    let base = src_row + (x0 + col) * raw.cpp;
                    for channel in 0..3 {
                        // A monochrome sensor repeats its single plane across RGB.
                        let index = base + channel.min(raw.cpp - 1);
                        if let Some(&value) = pixels.get(index) {
                            sums[slot][channel] += value;
                            counts[slot][channel] += 1;
                        }
                    }
                }
            }
        }
    }

    // The same maths as rawler's own `map_3ch_to_rgb`, which is `pub(crate)` — but run
    // over a few thousand pixels rather than the full frame.
    let xyz_to_cam = xyz_to_cam(raw);
    let rgb_to_cam = normalize(multiply(&xyz_to_cam, &SRGB_TO_XYZ_D65));
    let cam_to_rgb = pseudo_inverse(rgb_to_cam);
    let wb = if raw.wb_coeffs[0].is_nan() || raw.wb_coeffs[0] == 0.0 {
        [1.0; 4]
    } else {
        raw.wb_coeffs
    };

    let mut linear: Vec<[f32; 3]> = Vec::with_capacity(out_w * out_h);
    for (sum, count) in sums.iter().zip(counts.iter()) {
        let mut cam = [0f32; 3];
        for (channel, value) in cam.iter_mut().enumerate() {
            if count[channel] > 0 {
                *value = sum[channel] / count[channel] as f32 * wb[channel];
            }
        }
        linear.push([
            cam_to_rgb[0][0] * cam[0] + cam_to_rgb[0][1] * cam[1] + cam_to_rgb[0][2] * cam[2],
            cam_to_rgb[1][0] * cam[0] + cam_to_rgb[1][1] * cam[1] + cam_to_rgb[1][2] * cam[2],
            cam_to_rgb[2][0] * cam[0] + cam_to_rgb[2][1] * cam[1] + cam_to_rgb[2][2] * cam[2],
        ]);
    }

    let mut out = Vec::with_capacity(out_w * out_h * 3);
    for pixel in &linear {
        for channel in clip_euclidean_norm_avg(pixel) {
            out.push((srgb_apply_gamma(channel).clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }

    let buffer = image::RgbImage::from_raw(out_w as u32, out_h as u32, out)
        .ok_or_else(|| "thumbnail buffer size mismatch".to_string())?;
    Ok(orient(DynamicImage::ImageRgb8(buffer), raw.orientation))
}

/// The D65 camera matrix, falling back to whatever illuminant the file does carry —
/// rawler's own `develop_params` makes the same concession.
fn xyz_to_cam(raw: &RawImage) -> [[f32; 3]; 4] {
    let mut xyz_to_cam = [[0f32; 3]; 4];
    let matrix = raw
        .color_matrix
        .get(&Illuminant::D65)
        .or_else(|| raw.color_matrix.values().next());
    match matrix {
        Some(flat) if flat.len() >= 9 => {
            for (i, row) in xyz_to_cam.iter_mut().enumerate() {
                for (j, cell) in row.iter_mut().enumerate() {
                    if let Some(value) = flat.get(i * 3 + j) {
                        *cell = *value;
                    }
                }
            }
        }
        // No usable matrix: identity, so the thumbnail is at least recognisable.
        _ => {
            for (i, row) in xyz_to_cam.iter_mut().enumerate().take(3) {
                row[i] = 1.0;
            }
        }
    }
    xyz_to_cam
}

fn orient(image: DynamicImage, orientation: Orientation) -> DynamicImage {
    match orientation {
        Orientation::Normal | Orientation::Unknown => image,
        Orientation::HorizontalFlip => image.fliph(),
        Orientation::Rotate180 => image.rotate180(),
        Orientation::VerticalFlip => image.flipv(),
        Orientation::Transpose => image.rotate90().fliph(),
        Orientation::Rotate90 => image.rotate90(),
        Orientation::Transverse => image.rotate270().fliph(),
        Orientation::Rotate270 => image.rotate270(),
    }
}

/// Fit `(w, h)` inside a `max_edge` box, preserving aspect and never upscaling.
pub fn fit(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    if w == 0 || h == 0 || max_edge == 0 {
        return (1, 1);
    }
    if w <= max_edge && h <= max_edge {
        return (w, h);
    }
    let scale = |value: u32, from: u32| {
        ((value as u64 * max_edge as u64) / from as u64).max(1) as u32
    };
    if w >= h {
        (max_edge, scale(h, w))
    } else {
        (scale(w, h), max_edge)
    }
}

fn downscale(image: DynamicImage) -> DynamicImage {
    let (w, h) = fit(image.width(), image.height(), THUMB_MAX_EDGE);
    if w == image.width() && h == image.height() {
        return image;
    }
    image.resize_exact(w, h, image::imageops::FilterType::Lanczos3)
}

fn encode(image: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    image
        .to_rgb8()
        .write_to(&mut out, image::ImageOutputFormat::Jpeg(THUMB_JPEG_QUALITY))
        .map_err(|err| format!("thumbnail encode failed: {err}"))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_preserves_aspect_and_never_upscales() {
        assert_eq!(fit(8256, 5504, 64), (64, 42));
        assert_eq!(fit(5504, 8256, 64), (42, 64));
        assert_eq!(fit(64, 64, 64), (64, 64));
        // Already inside the box: left alone rather than blown up.
        assert_eq!(fit(40, 30, 64), (40, 30));
        // Degenerate inputs must not divide by zero.
        assert_eq!(fit(0, 10, 64), (1, 1));
        assert_eq!(fit(10, 0, 64), (1, 1));
        // Extreme aspect ratios still produce at least one pixel.
        assert_eq!(fit(10_000, 1, 64), (64, 1));
    }

    /// Garbage must be rejected *before* it reaches rawler. 4 KB of zeroes was enough to
    /// send the decoder past 6 GB of allocation, so these cases are guarding against an
    /// out-of-memory abort, not merely an ugly error message.
    #[test]
    fn garbage_bytes_are_rejected_without_reaching_the_decoder() {
        for (name, bytes) in [
            ("junk.nef", vec![0u8; 4096]),
            ("junk.jpg", vec![0u8; 4096]),
            ("junk.nef", Vec::new()),
            ("junk.arw", vec![0xFFu8; 256 * 1024]),
            ("junk.cr2", vec![0x49u8; 256 * 1024]),
            ("junk.nef", vec![0u8; 256 * 1024]),
        ] {
            assert!(render(name, bytes).is_err(), "{name} should not decode");
        }
    }

    #[test]
    fn only_real_raw_containers_pass_the_gate() {
        let pad = |magic: &[u8]| {
            let mut bytes = magic.to_vec();
            bytes.resize(128 * 1024, 0);
            bytes
        };
        for magic in [
            &b"II\x2a\x00"[..],
            b"MM\x00\x2a",
            b"FUJIFILM",
            b"FOVb",
            b"\x00MRM",
        ] {
            assert!(super::super::exif::looks_like_raw(&pad(magic)));
        }
        for magic in [&b"\x00\x00\x00\x00"[..], b"\xff\xff\xff\xff", b"RIFF", b"\x89PNG"] {
            assert!(!super::super::exif::looks_like_raw(&pad(magic)));
        }
        // A real container that is too small to be a real photo is still rejected.
        assert!(!super::super::exif::looks_like_raw(b"II\x2a\x00some short file"));
    }

    /// Render real camera files and write the results somewhere you can look at them —
    /// colour correctness is not something an assertion can judge. Point `PHASE_RAW_FIXTURES`
    /// at a directory of RAW files and `PHASE_RAW_OUT` at where the thumbnails should land:
    /// `cargo test -- --ignored --nocapture renders_real_camera_files`.
    #[test]
    #[ignore]
    fn renders_real_camera_files() {
        let fixtures = std::env::var("PHASE_RAW_FIXTURES").expect("set PHASE_RAW_FIXTURES");
        let out_dir = std::env::var("PHASE_RAW_OUT").expect("set PHASE_RAW_OUT");
        std::fs::create_dir_all(&out_dir).unwrap();

        let mut rendered = 0;
        for entry in std::fs::read_dir(&fixtures).unwrap().flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !super::super::is_ingestable_extension(&super::super::extension_of(&name)) {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let size_mb = bytes.len() as f64 / 1_048_576.0;

            let started = std::time::Instant::now();
            match render(&name, bytes) {
                Ok(result) => {
                    let thumb = image::load_from_memory(&result.jpeg).unwrap();
                    println!(
                        "{name}: {size_mb:.1} MB -> {}x{} ({} bytes) in {:.2}s  camera={:?}",
                        thumb.width(),
                        thumb.height(),
                        result.jpeg.len(),
                        started.elapsed().as_secs_f32(),
                        result.camera
                    );
                    std::fs::write(
                        std::path::Path::new(&out_dir).join(format!("{name}.jpg")),
                        &result.jpeg,
                    )
                    .unwrap();
                    rendered += 1;
                }
                Err(err) => println!("{name}: FAILED — {err}"),
            }
        }
        assert!(rendered > 0, "no fixtures rendered from {fixtures}");
    }

    #[test]
    fn a_real_jpeg_renders_to_a_small_jpeg() {
        let source = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(300, 200, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        }));
        let mut encoded = Cursor::new(Vec::new());
        source
            .write_to(&mut encoded, image::ImageOutputFormat::Jpeg(90))
            .unwrap();

        let rendered = render("shot.jpg", encoded.into_inner()).unwrap();
        let thumb = image::load_from_memory(&rendered.jpeg).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (64, 42));
    }
}

#[cfg(test)]
mod diag {
    use super::*;
    use rawler::rawimage::RawImageData;

    /// Dump the decoder's own numbers for a file, to diagnose exposure/colour issues.
    /// `PHASE_RAW_FIXTURES=<dir> cargo test --release -- --ignored --nocapture dump_raw_stats`
    #[test]
    #[ignore]
    fn dump_raw_stats() {
        let dir = std::env::var("PHASE_RAW_FIXTURES").expect("set PHASE_RAW_FIXTURES");
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !super::super::is_raw_extension(&super::super::extension_of(&name)) {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let mut raw = {
                let mut rf = RawFile::new(&name, Cursor::new(bytes));
                rawler::decode(&mut rf, Default::default()).unwrap()
            };
            println!("\n== {name} ==");
            println!("  wb_coeffs   {:?}", raw.wb_coeffs);
            println!("  whitelevel  {:?}", raw.whitelevel.0);
            println!("  blacklevel  {:?}", raw.blacklevel.levels);
            println!("  cpp={} bps={} {}x{}", raw.cpp, raw.bps, raw.width, raw.height);
            raw.apply_scaling().unwrap();
            if let RawImageData::Float(ref px) = raw.data {
                let n = px.len() as f64;
                let mean = px.iter().map(|v| *v as f64).sum::<f64>() / n;
                let mut sorted: Vec<f32> = px.iter().step_by(97).copied().collect();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
                println!(
                    "  scaled CFA: mean={mean:.4} p50={:.4} p99={:.4} p99.9={:.4} max={:.4}",
                    pct(0.50), pct(0.99), pct(0.999), sorted[sorted.len() - 1]
                );
            }
        }
    }
}
