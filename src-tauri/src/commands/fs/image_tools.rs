use super::*;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType, ImageFormat};
use ravif::{BitDepth, Encoder as AvifEncoder, Img, RGBA8};
use webp::Encoder as WebpEncoder;

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ImageConvertFormat {
    Avif,
    Png,
    Jpeg,
    Webp,
    Tiff,
    Bmp,
    Ico,
}

impl ImageConvertFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Avif => "avif",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Ico => "ico",
        }
    }

    fn image_format(self) -> ImageFormat {
        match self {
            Self::Avif => ImageFormat::Avif,
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Webp => ImageFormat::WebP,
            Self::Tiff => ImageFormat::Tiff,
            Self::Bmp => ImageFormat::Bmp,
            Self::Ico => ImageFormat::Ico,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Avif => "AVIF",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WebP",
            Self::Tiff => "TIFF",
            Self::Bmp => "BMP",
            Self::Ico => "ICO",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ImageConvertConflictPolicy {
    #[default]
    KeepBoth,
    Replace,
    Skip,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageConvertOptions {
    pub format: ImageConvertFormat,
    #[serde(default)]
    pub quality: Option<u8>,
    #[serde(default)]
    pub conflict: ImageConvertConflictPolicy,
    #[serde(default)]
    pub destination_directory: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageConvertResult {
    pub source_path: String,
    pub output_path: Option<String>,
    pub output_name: Option<String>,
    pub status: String,
    pub message: Option<String>,
    pub input_bytes: Option<u64>,
    pub output_bytes: Option<u64>,
}

#[derive(Clone)]
enum ImageOutputParent {
    Local(PathBuf),
    Remote(RemotePath),
}

struct ImageConvertSource {
    original_path: String,
    local_path: PathBuf,
    name: String,
    source_parent: ImageOutputParent,
    input_bytes: Option<u64>,
}

#[tauri::command]
pub async fn convert_images(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    paths: Vec<String>,
    options: ImageConvertOptions,
    job_id: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<ImageConvertResult>, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());

    if paths.is_empty() {
        return Err(FsError::new(
            "image_convert_no_paths",
            "Choose at least one image to convert.",
            None,
        ));
    }

    for path in &paths {
        if archive::is_archive_uri(path) {
            return Err(archive_read_only_error(path));
        }
    }

    let workspace = TemporaryWorkspace::new("image-convert")?;
    let destination_parent = match options.destination_directory.as_deref() {
        Some(destination) => Some(resolve_image_output_parent(destination)?),
        None => None,
    };
    let mut results = Vec::with_capacity(paths.len());
    let mut planned_names = HashMap::<String, HashSet<String>>::new();
    let mut remote_name_cache = HashMap::<String, HashSet<String>>::new();
    let total_entries = paths.len() as u64;
    let mut processed_bytes = 0_u64;

    for (index, path) in paths.into_iter().enumerate() {
        operation_state.checkpoint(&job_id, None)?;
        emit_file_operation_progress(
            &app,
            &job_id,
            "image-convert",
            "running",
            ProgressSnapshot {
                processed_bytes,
                processed_entries: index as u64,
                total_entries,
                current_path: Some(path.clone()),
                ..ProgressSnapshot::default()
            },
        );

        let source = materialize_image_convert_source(
            &remotes,
            &workspace,
            &path,
            destination_parent.clone(),
        )
        .await?;
        let output_parent = destination_parent
            .clone()
            .unwrap_or_else(|| source.source_parent.clone());
        let seed_name = image_output_name(&source.name, options.format);
        let destination = resolve_image_destination(
            &remotes,
            output_parent,
            &seed_name,
            options.conflict,
            &mut planned_names,
            &mut remote_name_cache,
        )
        .await?;

        let Some(destination) = destination else {
            results.push(ImageConvertResult {
                source_path: source.original_path,
                output_path: None,
                output_name: Some(seed_name),
                status: "skipped".to_string(),
                message: Some("Output already exists.".to_string()),
                input_bytes: source.input_bytes,
                output_bytes: None,
            });
            emit_file_operation_progress(
                &app,
                &job_id,
                "image-convert",
                "running",
                ProgressSnapshot {
                    processed_bytes,
                    processed_entries: index as u64 + 1,
                    total_entries,
                    ..ProgressSnapshot::default()
                },
            );
            continue;
        };

        let converted = convert_image_to_destination(
            &remotes,
            &workspace,
            &source.local_path,
            destination,
            options.format,
            image_quality(options.quality),
        )
        .await?;

        if let Some(bytes) = source.input_bytes {
            processed_bytes = processed_bytes.saturating_add(bytes);
        }

        results.push(ImageConvertResult {
            source_path: source.original_path,
            output_path: Some(converted.output_path),
            output_name: Some(converted.output_name),
            status: "converted".to_string(),
            message: Some(format!("Converted to {}", options.format.label())),
            input_bytes: source.input_bytes,
            output_bytes: Some(converted.output_bytes),
        });

        emit_file_operation_progress(
            &app,
            &job_id,
            "image-convert",
            "running",
            ProgressSnapshot {
                processed_bytes,
                processed_entries: index as u64 + 1,
                total_entries,
                ..ProgressSnapshot::default()
            },
        );
    }

    Ok(results)
}

fn resolve_image_output_parent(path: &str) -> FsResult<ImageOutputParent> {
    if archive::is_archive_uri(path) {
        return Err(archive_read_only_error(path));
    }

    if let Some(remote_path) = parse_remote_path(path) {
        return Ok(ImageOutputParent::Remote(remote_path));
    }

    Ok(ImageOutputParent::Local(expand_local_path(path)?))
}

async fn materialize_image_convert_source(
    remotes: &RemoteVolumeState,
    workspace: &TemporaryWorkspace,
    path: &str,
    destination_parent: Option<ImageOutputParent>,
) -> FsResult<ImageConvertSource> {
    if let Some(remote_path) = parse_remote_path(path) {
        let name = remote_leaf_name(&remote_path, "image");
        let local_path = workspace.unique_child_path(&name);
        copy_remote_to_local_item(remotes, remote_path.clone(), &local_path, true).await?;
        let input_bytes = fs::metadata(&local_path)
            .ok()
            .map(|metadata| metadata.len());

        return Ok(ImageConvertSource {
            original_path: path.to_string(),
            local_path,
            name,
            source_parent: destination_parent
                .unwrap_or_else(|| ImageOutputParent::Remote(remote_parent_path(&remote_path))),
            input_bytes,
        });
    }

    let local_path = expand_local_path(path)?;
    let metadata = fs::metadata(&local_path)
        .map_err(|error| FsError::io("Unable to read image metadata", &local_path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "image_convert_not_file",
            "Only image files can be converted.",
            Some(path.to_string()),
        ));
    }

    let name = local_path
        .file_name()
        .unwrap_or_else(|| OsStr::new("image"))
        .to_string_lossy()
        .into_owned();
    let source_parent = destination_parent.unwrap_or_else(|| {
        ImageOutputParent::Local(
            local_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
        )
    });

    Ok(ImageConvertSource {
        original_path: path.to_string(),
        local_path,
        name,
        source_parent,
        input_bytes: Some(metadata.len()),
    })
}

fn remote_parent_path(path: &RemotePath) -> RemotePath {
    let trimmed = path.path.trim_matches('/');
    let parent = trimmed
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
        .to_string();

    RemotePath {
        volume_id: path.volume_id.clone(),
        path: parent,
    }
}

fn image_quality(value: Option<u8>) -> u8 {
    value.unwrap_or(85).clamp(1, 100)
}

async fn resolve_image_destination(
    remotes: &RemoteVolumeState,
    parent: ImageOutputParent,
    seed_name: &str,
    conflict: ImageConvertConflictPolicy,
    planned_names: &mut HashMap<String, HashSet<String>>,
    remote_name_cache: &mut HashMap<String, HashSet<String>>,
) -> FsResult<Option<ImageDestination>> {
    match parent {
        ImageOutputParent::Local(parent) => {
            resolve_local_image_destination(parent, seed_name, conflict, planned_names)
        }
        ImageOutputParent::Remote(parent) => {
            resolve_remote_image_destination(
                remotes,
                parent,
                seed_name,
                conflict,
                planned_names,
                remote_name_cache,
            )
            .await
        }
    }
}

enum ImageDestination {
    Local {
        path: PathBuf,
        name: String,
        overwrite: bool,
    },
    Remote {
        path: RemotePath,
        name: String,
        overwrite: bool,
    },
}

struct ConvertedImage {
    output_path: String,
    output_name: String,
    output_bytes: u64,
}

fn resolve_local_image_destination(
    parent: PathBuf,
    seed_name: &str,
    conflict: ImageConvertConflictPolicy,
    planned_names: &mut HashMap<String, HashSet<String>>,
) -> FsResult<Option<ImageDestination>> {
    let key = format!("local:{}", parent.to_string_lossy());
    let planned = planned_names.entry(key).or_default();
    let mut name = seed_name.to_string();
    let planned_exists = planned.contains(&name.to_lowercase());
    let exists = parent.join(&name).exists();

    if planned_exists {
        name = unique_image_output_name(seed_name, |candidate| {
            parent.join(candidate).exists() || planned.contains(&candidate.to_lowercase())
        });
    } else if exists {
        match conflict {
            ImageConvertConflictPolicy::Skip => return Ok(None),
            ImageConvertConflictPolicy::Replace => {}
            ImageConvertConflictPolicy::KeepBoth => {
                name = unique_image_output_name(seed_name, |candidate| {
                    parent.join(candidate).exists() || planned.contains(&candidate.to_lowercase())
                });
            }
        }
    }

    planned.insert(name.to_lowercase());
    Ok(Some(ImageDestination::Local {
        path: parent.join(&name),
        name,
        overwrite: conflict == ImageConvertConflictPolicy::Replace,
    }))
}

async fn resolve_remote_image_destination(
    remotes: &RemoteVolumeState,
    parent: RemotePath,
    seed_name: &str,
    conflict: ImageConvertConflictPolicy,
    planned_names: &mut HashMap<String, HashSet<String>>,
    remote_name_cache: &mut HashMap<String, HashSet<String>>,
) -> FsResult<Option<ImageDestination>> {
    let key = format_remote_uri(&parent.volume_id, &parent.path);
    let existing_names = if let Some(names) = remote_name_cache.get(&key) {
        names.clone()
    } else {
        let names = list_remote_directory(remotes, parent.clone())
            .await?
            .into_iter()
            .map(|entry| entry.name.to_lowercase())
            .collect::<HashSet<_>>();
        remote_name_cache.insert(key.clone(), names.clone());
        names
    };
    let planned = planned_names.entry(format!("remote:{key}")).or_default();
    let mut name = seed_name.to_string();
    let planned_exists = planned.contains(&name.to_lowercase());
    let exists = existing_names.contains(&name.to_lowercase());

    if planned_exists {
        name = unique_image_output_name(seed_name, |candidate| {
            let key = candidate.to_lowercase();
            existing_names.contains(&key) || planned.contains(&key)
        });
    } else if exists {
        match conflict {
            ImageConvertConflictPolicy::Skip => return Ok(None),
            ImageConvertConflictPolicy::Replace => {}
            ImageConvertConflictPolicy::KeepBoth => {
                name = unique_image_output_name(seed_name, |candidate| {
                    let key = candidate.to_lowercase();
                    existing_names.contains(&key) || planned.contains(&key)
                });
            }
        }
    }

    planned.insert(name.to_lowercase());
    let object_path = join_remote_object_path(&parent.path, &name);
    Ok(Some(ImageDestination::Remote {
        path: RemotePath {
            volume_id: parent.volume_id,
            path: object_path,
        },
        name,
        overwrite: conflict == ImageConvertConflictPolicy::Replace,
    }))
}

async fn convert_image_to_destination(
    remotes: &RemoteVolumeState,
    workspace: &TemporaryWorkspace,
    source_path: &Path,
    destination: ImageDestination,
    format: ImageConvertFormat,
    quality: u8,
) -> FsResult<ConvertedImage> {
    match destination {
        ImageDestination::Local {
            path,
            name,
            overwrite,
        } => {
            let temp_path = local_temp_image_output_path(&path)?;
            convert_local_image(source_path, &temp_path, format, quality)?;
            move_converted_local_image(&temp_path, &path, overwrite)?;
            let output_bytes = fs::metadata(&path)
                .map_err(|error| {
                    FsError::io("Unable to read converted image metadata", &path, error)
                })?
                .len();

            Ok(ConvertedImage {
                output_path: path.to_string_lossy().into_owned(),
                output_name: name,
                output_bytes,
            })
        }
        ImageDestination::Remote {
            path,
            name,
            overwrite,
        } => {
            let temp_path = workspace.unique_child_path(&name);
            convert_local_image(source_path, &temp_path, format, quality)?;
            let output_bytes = fs::metadata(&temp_path)
                .map_err(|error| {
                    FsError::io("Unable to read converted image metadata", &temp_path, error)
                })?
                .len();
            copy_local_to_remote_item(
                remotes,
                &temp_path,
                path.clone(),
                overwrite,
                operations::SymlinkMode::Preserve,
            )
            .await?;

            Ok(ConvertedImage {
                output_path: format_remote_uri(&path.volume_id, &path.path),
                output_name: name,
                output_bytes,
            })
        }
    }
}

fn convert_local_image(
    source_path: &Path,
    output_path: &Path,
    format: ImageConvertFormat,
    quality: u8,
) -> FsResult<()> {
    let image = image::open(source_path)
        .map_err(|error| image_error("Unable to read image", source_path, error))?;

    if format == ImageConvertFormat::Jpeg {
        return save_jpeg_image(&image, output_path, quality);
    }

    if format == ImageConvertFormat::Avif {
        return save_avif_image(&image, output_path, quality);
    }

    if format == ImageConvertFormat::Webp {
        return save_webp_image(&image, output_path, quality);
    }

    image
        .save_with_format(output_path, format.image_format())
        .map_err(|error| image_error("Unable to write converted image", output_path, error))
}

fn save_avif_image(image: &DynamicImage, output_path: &Path, quality: u8) -> FsResult<()> {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let pixels = rgba
        .as_raw()
        .chunks_exact(4)
        .map(|pixel| RGBA8::new(pixel[0], pixel[1], pixel[2], pixel[3]))
        .collect::<Vec<_>>();
    let encoded = AvifEncoder::new()
        .with_quality(f32::from(quality))
        .with_alpha_quality(f32::from(quality))
        .with_speed(4)
        .with_bit_depth(BitDepth::Eight)
        .encode_rgba(Img::new(&pixels, width as usize, height as usize))
        .map_err(|error| {
            FsError::new(
                "image_convert_failed",
                format!("Unable to write converted image: {error}"),
                Some(output_path.to_string_lossy().into_owned()),
            )
        })?;

    fs::write(output_path, encoded.avif_file)
        .map_err(|error| FsError::io("Unable to write converted image", output_path, error))
}

fn save_jpeg_image(image: &DynamicImage, output_path: &Path, quality: u8) -> FsResult<()> {
    let rgb = image.to_rgb8();
    let (width, height) = rgb.dimensions();
    let file = fs::File::create(output_path)
        .map_err(|error| FsError::io("Unable to create converted image", output_path, error))?;
    let mut encoder = JpegEncoder::new_with_quality(file, quality);

    encoder
        .encode(rgb.as_raw(), width, height, ExtendedColorType::Rgb8)
        .map_err(|error| image_error("Unable to write converted image", output_path, error))
}

fn save_webp_image(image: &DynamicImage, output_path: &Path, quality: u8) -> FsResult<()> {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let encoder = WebpEncoder::from_rgba(rgba.as_raw(), width, height);
    let encoded = encoder
        .encode_simple(false, f32::from(quality))
        .map_err(|error| {
            FsError::new(
                "image_convert_failed",
                format!("Unable to write converted image: {error:?}"),
                Some(output_path.to_string_lossy().into_owned()),
            )
        })?;

    fs::write(output_path, &*encoded)
        .map_err(|error| FsError::io("Unable to write converted image", output_path, error))
}

fn move_converted_local_image(from: &Path, to: &Path, overwrite: bool) -> FsResult<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| FsError::io("Unable to create image output folder", parent, error))?;
    }

    if to.exists() {
        if !overwrite {
            let _ = fs::remove_file(from);
            return Err(FsError::new(
                "image_output_exists",
                "A file already exists with that converted image name.",
                Some(to.to_string_lossy().into_owned()),
            ));
        }

        if to.is_dir() {
            let _ = fs::remove_file(from);
            return Err(FsError::new(
                "image_output_is_directory",
                "A folder already exists with that converted image name.",
                Some(to.to_string_lossy().into_owned()),
            ));
        }

        fs::remove_file(to)
            .map_err(|error| FsError::io("Unable to replace converted image", to, error))?;
    }

    fs::rename(from, to).map_err(|error| {
        let _ = fs::remove_file(from);
        FsError::io("Unable to place converted image", to, error)
    })
}

fn local_temp_image_output_path(path: &Path) -> FsResult<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        FsError::new(
            "invalid_image_output",
            "Unable to resolve the image output folder.",
            Some(path.to_string_lossy().into_owned()),
        )
    })?;
    let name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("converted-image"))
        .to_string_lossy();

    Ok(unique_child_path_in(
        parent,
        &format!(".carelo-convert-{}-{name}", random_token(8)),
    ))
}

fn image_output_name(source_name: &str, format: ImageConvertFormat) -> String {
    let source_name = source_name.trim();
    let stem = image_file_stem(source_name);
    let extension = format.extension();
    let candidate = format!("{stem}.{extension}");

    if source_name.eq_ignore_ascii_case(&candidate) {
        format!("{stem} converted.{extension}")
    } else {
        candidate
    }
}

fn image_file_stem(name: &str) -> String {
    let clean_name = name
        .replace(['/', '\\'], " ")
        .trim()
        .trim_matches('.')
        .to_string();

    if clean_name.is_empty() {
        return "Image".to_string();
    }

    Path::new(&clean_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "." && *value != "..")
        .unwrap_or("Image")
        .to_string()
}

fn unique_image_output_name<F>(seed_name: &str, exists: F) -> String
where
    F: Fn(&str) -> bool,
{
    if !exists(seed_name) {
        return seed_name.to_string();
    }

    let path = Path::new(seed_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Image");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();

    for index in 2..1000 {
        let candidate = format!("{stem} {index}{extension}");

        if !exists(&candidate) {
            return candidate;
        }
    }

    format!("{stem} {}{extension}", random_token(8))
}

fn image_error(action: &str, path: &Path, error: image::ImageError) -> FsError {
    FsError::new(
        "image_convert_failed",
        format!("{action}: {error}"),
        Some(path.to_string_lossy().into_owned()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_converted_when_target_name_matches_source() {
        assert_eq!(
            image_output_name("photo.jpg", ImageConvertFormat::Jpeg),
            "photo converted.jpg"
        );
    }

    #[test]
    fn uses_target_extension_for_different_source_format() {
        assert_eq!(
            image_output_name("photo.png", ImageConvertFormat::Webp),
            "photo.webp"
        );
    }

    #[test]
    fn uses_avif_extension() {
        assert_eq!(
            image_output_name("photo.png", ImageConvertFormat::Avif),
            "photo.avif"
        );
    }

    #[test]
    fn keeps_output_names_unique() {
        let existing = HashSet::from(["photo.webp".to_string(), "photo 2.webp".to_string()]);

        assert_eq!(
            unique_image_output_name("photo.webp", |name| existing.contains(name)),
            "photo 3.webp"
        );
    }

    #[test]
    fn converts_local_image_between_formats() {
        let root = std::env::temp_dir().join(format!(
            "carelo-image-convert-test-{}-{}",
            std::process::id(),
            random_token(8)
        ));
        fs::create_dir_all(&root).expect("create test directory");
        let source = root.join("source.png");
        let output = root.join("source.avif");
        let image = image::RgbImage::from_pixel(2, 2, image::Rgb([220, 20, 60]));

        DynamicImage::ImageRgb8(image)
            .save_with_format(&source, ImageFormat::Png)
            .expect("write source image");
        convert_local_image(&source, &output, ImageConvertFormat::Avif, 85).expect("convert image");

        let converted = image::open(&output).expect("read converted image");
        assert_eq!((converted.width(), converted.height()), (2, 2));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn converts_local_image_to_webp_with_quality() {
        let root = std::env::temp_dir().join(format!(
            "carelo-image-convert-webp-test-{}-{}",
            std::process::id(),
            random_token(8)
        ));
        fs::create_dir_all(&root).expect("create test directory");
        let source = root.join("source.png");
        let low_quality_output = root.join("source-low.webp");
        let high_quality_output = root.join("source-high.webp");
        let image = image::RgbaImage::from_fn(48, 32, |x, y| {
            image::Rgba([
                ((x * 5) % 255) as u8,
                ((y * 7) % 255) as u8,
                (((x + y) * 3) % 255) as u8,
                255,
            ])
        });

        DynamicImage::ImageRgba8(image)
            .save_with_format(&source, ImageFormat::Png)
            .expect("write source image");
        convert_local_image(&source, &low_quality_output, ImageConvertFormat::Webp, 30)
            .expect("convert low quality image");
        convert_local_image(&source, &high_quality_output, ImageConvertFormat::Webp, 90)
            .expect("convert high quality image");

        let low_quality_bytes = fs::read(&low_quality_output).expect("read low quality output");
        let high_quality_bytes = fs::read(&high_quality_output).expect("read high quality output");
        assert_ne!(low_quality_bytes, high_quality_bytes);

        let converted = image::open(&high_quality_output).expect("read converted image");
        assert_eq!((converted.width(), converted.height()), (48, 32));

        let _ = fs::remove_dir_all(root);
    }
}
