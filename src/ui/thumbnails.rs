use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use crate::config::Config;

use super::{layout, AssetType, RowKey};

const THUMBNAIL_SOURCE_RELATIVE_PATH: &[&str] = &["staging", "renders", "thumbnail.png"];
const THUMBNAIL_TARGET_HEIGHT: u32 = layout::ROW_HEIGHT as u32;
const THUMBNAIL_CACHE_MAX_AGE: Duration = Duration::from_secs(60 * 60 * 24 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailFormat {
    WebP,
    Jpeg,
    Png,
}

impl ThumbnailFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::WebP => "webp",
            Self::Jpeg => "jpg",
            Self::Png => "png",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailSignature {
    pub asset_type: AssetType,
    pub slug: String,
    pub source_mtime: u64,
    pub source_size: u64,
}

pub struct ThumbnailJob {
    pub revision: u64,
    pub rx: Receiver<Result<ThumbnailJobResult, String>>,
    pub completed: Arc<(Mutex<bool>, Condvar)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailJobResult {
    pub revision: u64,
    pub signature: ThumbnailSignature,
    pub cache_path: PathBuf,
}

pub fn thumbnail_source_path(config: &Config, key: &RowKey) -> Option<PathBuf> {
    thumbnail_source_path_from_roots(&config.local_root, &config.prod_root, key)
}

pub fn thumbnail_source_path_from_roots(
    local_root: &Path,
    prod_root: &Path,
    key: &RowKey,
) -> Option<PathBuf> {
    let local_source = source_path(local_root, key.asset_type.folder(), &key.slug);
    if local_source.is_file() {
        return Some(local_source);
    }
    let prod_source = source_path(prod_root, key.asset_type.folder(), &key.slug);
    if prod_source.is_file() {
        return Some(prod_source);
    }
    None
}

pub fn thumbnail_signature(
    source_path: &Path,
    asset_type: AssetType,
    slug: &str,
) -> Result<ThumbnailSignature, String> {
    let metadata = fs::metadata(source_path).map_err(|err| err.to_string())?;
    let modified = metadata
        .modified()
        .map_err(|err| err.to_string())?
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|err| err.to_string())?
        .as_secs();
    Ok(ThumbnailSignature {
        asset_type,
        slug: slug.to_string(),
        source_mtime: modified,
        source_size: metadata.len(),
    })
}

pub fn thumbnail_cache_path(
    cache_root: &Path,
    signature: &ThumbnailSignature,
    format: ThumbnailFormat,
) -> PathBuf {
    thumbnail_cache_asset_root(cache_root, signature.asset_type)
        .join(&signature.slug)
        .join(thumbnail_cache_file_name(signature, format))
}

pub fn thumbnail_cache_file_name(
    signature: &ThumbnailSignature,
    format: ThumbnailFormat,
) -> String {
    format!("{}-{}.{}", signature.source_mtime, signature.source_size, format.extension())
}

pub fn thumbnail_cache_asset_root(cache_root: &Path, asset_type: AssetType) -> PathBuf {
    cache_root.join(asset_type.cache_name())
}

pub fn thumbnail_cache_asset_dir(cache_root: &Path, signature: &ThumbnailSignature) -> PathBuf {
    thumbnail_cache_asset_root(cache_root, signature.asset_type).join(&signature.slug)
}

pub fn prune_thumbnail_cache(cache_root: &Path) -> Result<usize, String> {
    prune_thumbnail_cache_older_than(cache_root, THUMBNAIL_CACHE_MAX_AGE)
}

pub fn prune_thumbnail_cache_older_than(
    cache_root: &Path,
    max_age: Duration,
) -> Result<usize, String> {
    if !cache_root.exists() {
        return Ok(0);
    }
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0;
    prune_thumbnail_cache_dir(cache_root, cutoff, &mut removed)?;
    Ok(removed)
}

pub fn spawn_thumbnail_job(
    cache_root: PathBuf,
    local_root: PathBuf,
    prod_root: PathBuf,
    key: RowKey,
    revision: u64,
    revision_state: Arc<AtomicU64>,
) -> ThumbnailJob {
    let (tx, rx) = channel();
    let completed = Arc::new((Mutex::new(false), Condvar::new()));
    let completed_thread = Arc::clone(&completed);
    thread::spawn(move || {
        let result = render_thumbnail(&cache_root, &local_root, &prod_root, &key, revision, &revision_state);
        let _ = tx.send(result);
        let (lock, cvar) = &*completed_thread;
        if let Ok(mut finished) = lock.lock() {
            *finished = true;
            cvar.notify_all();
        }
    });
    ThumbnailJob {
        revision,
        rx,
        completed,
    }
}

pub fn load_thumbnail_texture(
    ctx: &egui::Context,
    cache_path: &Path,
    texture_name: &str,
) -> Result<egui::TextureHandle, String> {
    let bytes = fs::read(cache_path).map_err(|err| err.to_string())?;
    let decoded = image::load_from_memory(&bytes).map_err(|err| err.to_string())?;
    let rgba = decoded.to_rgba8();
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    );
    Ok(ctx.load_texture(texture_name, image, egui::TextureOptions::LINEAR))
}

fn render_thumbnail(
    cache_root: &Path,
    local_root: &Path,
    prod_root: &Path,
    key: &RowKey,
    revision: u64,
    revision_state: &AtomicU64,
) -> Result<ThumbnailJobResult, String> {
    if revision_state.load(Ordering::Acquire) != revision {
        return Err("thumbnail refresh superseded".into());
    }
    let Some(source_path) = thumbnail_source_path_from_roots(local_root, prod_root, key) else {
        return Err(format!(
            "Missing thumbnail source for {}/{}",
            key.asset_type.folder(),
            key.slug
        ));
    };
    let signature = thumbnail_signature(&source_path, key.asset_type, &key.slug)?;
    fs::create_dir_all(thumbnail_cache_asset_dir(cache_root, &signature))
        .map_err(|err| err.to_string())?;
    if revision_state.load(Ordering::Acquire) != revision {
        return Err("thumbnail refresh superseded".into());
    }
    if let Some(path) = existing_cache_path(cache_root, &signature) {
        return Ok(ThumbnailJobResult {
            revision,
            signature: signature.clone(),
            cache_path: path,
        });
    }

    remove_stale_cache_files(cache_root, &signature)?;
    let source_bytes = fs::read(&source_path).map_err(|err| err.to_string())?;
    if revision_state.load(Ordering::Acquire) != revision {
        return Err("thumbnail refresh superseded".into());
    }
    let current_signature =
        thumbnail_signature(&source_path, signature.asset_type, &signature.slug)?;
    if current_signature != signature {
        return Err("thumbnail source changed while rendering".into());
    }
    let decoded = image::load_from_memory(&source_bytes).map_err(|err| err.to_string())?;
    let resized = resize_for_thumbnail(decoded);
    if revision_state.load(Ordering::Acquire) != revision {
        return Err("thumbnail refresh superseded".into());
    }
    let current_signature =
        thumbnail_signature(&source_path, signature.asset_type, &signature.slug)?;
    if current_signature != signature {
        return Err("thumbnail source changed while rendering".into());
    }
    let cache_path = write_thumbnail(cache_root, &signature, &resized)?;
    Ok(ThumbnailJobResult {
        revision,
        signature: signature.clone(),
        cache_path,
    })
}

fn resize_for_thumbnail(image: image::DynamicImage) -> image::DynamicImage {
    if image.height() <= THUMBNAIL_TARGET_HEIGHT {
        return image;
    }
    let width = ((image.width() as u64 * THUMBNAIL_TARGET_HEIGHT as u64)
        / image.height().max(1) as u64)
        .max(1) as u32;
    image.resize_exact(
        width,
        THUMBNAIL_TARGET_HEIGHT,
        image::imageops::FilterType::Lanczos3,
    )
}

fn write_thumbnail(
    cache_root: &Path,
    signature: &ThumbnailSignature,
    image: &image::DynamicImage,
) -> Result<PathBuf, String> {
    for format in [
        ThumbnailFormat::WebP,
        ThumbnailFormat::Jpeg,
        ThumbnailFormat::Png,
    ] {
        let path = thumbnail_cache_path(cache_root, signature, format);
        let mut encoded = Cursor::new(Vec::new());
        let write_result = match format {
            ThumbnailFormat::WebP => image.write_to(&mut encoded, image::ImageOutputFormat::WebP),
            ThumbnailFormat::Jpeg => image
                .to_rgb8()
                .write_to(&mut encoded, image::ImageOutputFormat::Jpeg(85)),
            ThumbnailFormat::Png => image.write_to(&mut encoded, image::ImageOutputFormat::Png),
        };
        if let Ok(()) = write_result {
            fs::write(&path, encoded.into_inner()).map_err(|err| err.to_string())?;
            return Ok(path);
        }
    }
    Err("failed to encode thumbnail as webp, jpeg, or png".into())
}

fn existing_cache_path(cache_root: &Path, signature: &ThumbnailSignature) -> Option<PathBuf> {
    [
        ThumbnailFormat::WebP,
        ThumbnailFormat::Jpeg,
        ThumbnailFormat::Png,
    ]
    .into_iter()
    .map(|format| thumbnail_cache_path(cache_root, signature, format))
    .find(|path| path.is_file())
}

fn remove_stale_cache_files(
    cache_root: &Path,
    signature: &ThumbnailSignature,
) -> Result<(), String> {
    let asset_root = thumbnail_cache_asset_dir(cache_root, signature);
    if !asset_root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&asset_root).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if !path.is_file() || !is_thumbnail_cache_file(&path) {
            continue;
        }
        let _ = fs::remove_file(&path);
    }
    Ok(())
}

fn prune_thumbnail_cache_dir(
    cache_root: &Path,
    cutoff: SystemTime,
    removed: &mut usize,
) -> Result<(), String> {
    if !cache_root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(cache_root).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            prune_thumbnail_cache_dir(&path, cutoff, removed)?;
            continue;
        }
        if !path.is_file() || !is_thumbnail_cache_file(&path) {
            continue;
        }
        let modified = entry
            .metadata()
            .map_err(|err| err.to_string())?
            .modified()
            .map_err(|err| err.to_string())?;
        if modified < cutoff {
            fs::remove_file(&path).map_err(|err| err.to_string())?;
            *removed += 1;
        }
    }
    Ok(())
}

fn source_path(root: &Path, asset_folder: &str, slug: &str) -> PathBuf {
    let mut path = root.join(asset_folder).join(slug);
    for part in THUMBNAIL_SOURCE_RELATIVE_PATH {
        path = path.join(part);
    }
    path
}

fn is_thumbnail_cache_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("webp" | "jpg" | "jpeg" | "png")
    ) && name.rsplit_once('-').is_some()
}
