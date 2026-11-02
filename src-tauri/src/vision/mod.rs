use serde_json::Value;
use std::{
    env,
    path::{Path, PathBuf},
    sync::OnceLock,
};

mod ocr;

pub(crate) use ocr::recognize_image_rgba as ocr_recognize_image_rgba;

/// Bundled-resource root that contains the `.models/` directory. Set once at
/// startup from the Tauri resource dir (see `register_resource_models_dir`),
/// because in a packaged app the models live under the app bundle, not relative
/// to the process working directory. Model lookups add this as a candidate root
/// so the SAME path-resolution code works in dev and in a shipped build.
static RESOURCE_MODELS_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Register the directory that holds the bundled `.models/` tree (the Tauri
/// resource dir). Call once during app setup. Idempotent / first-write-wins.
pub(crate) fn register_resource_models_dir(dir: PathBuf) {
    let _ = RESOURCE_MODELS_ROOT.set(dir);
}

const TSR_MODEL_ENV: &str = "MARKDOWNNOTE_TSR_MODEL";
const TSR_IMAGE_ENV: &str = "MARKDOWNNOTE_TSR_IMAGE";
const PDFIUM_DIR_ENV: &str = "MARKDOWNNOTE_PDFIUM_DIR";
/// Override for the OCR model directory (must contain the det/rec ONNX files +
/// dictionary). Defaults to the bundled `.models/ocr`.
const OCR_MODEL_DIR_ENV: &str = "MARKDOWNNOTE_OCR_MODEL_DIR";
const DEFAULT_OCR_MODEL_DIR: &str = ".models/ocr";

/// Resolve the OCR model directory: env override first, else the bundled
/// `.models/ocr` located via the shared resource/project candidate roots.
pub(crate) fn ocr_model_dir_from_env_or_default() -> Option<PathBuf> {
    env::var(OCR_MODEL_DIR_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| find_project_relative_dir(DEFAULT_OCR_MODEL_DIR))
}
#[cfg(feature = "tsr-onnx")]
const DEFAULT_TSR_MODEL_PATH: &str = ".models/tsr/slanet_1m.onnx";
const DEFAULT_PDFIUM_DIR: &str = ".models/pdfium";
const PDF_PAGE_RENDER_TARGET_WIDTH: i32 = 1600;
const TSR_MODEL_INPUT_SIZE_FOR_BBOX: f64 = 488.0;
#[cfg(feature = "tsr-onnx")]
const SLANET_INPUT_SIZE: usize = 488;
#[cfg(feature = "tsr-onnx")]
const PDF_RENDER_TARGET_WIDTH: i32 = 1800;
#[cfg(feature = "tsr-onnx")]
const CROP_PADDING_RATIO: f64 = 0.012;
#[cfg(feature = "tsr-onnx")]
const SLANET_STRUCTURE_TOKENS: [&str; 28] = [
    "<thead>",
    "<tr>",
    "<td>",
    "</td>",
    "</tr>",
    "</thead>",
    "<tbody>",
    "</tbody>",
    "<td",
    " colspan=\"5\"",
    ">",
    " colspan=\"2\"",
    " colspan=\"3\"",
    " rowspan=\"2\"",
    " colspan=\"4\"",
    " colspan=\"6\"",
    " rowspan=\"3\"",
    " colspan=\"9\"",
    " colspan=\"10\"",
    " colspan=\"7\"",
    " rowspan=\"4\"",
    " rowspan=\"5\"",
    " rowspan=\"9\"",
    " colspan=\"8\"",
    " rowspan=\"8\"",
    " rowspan=\"6\"",
    " rowspan=\"7\"",
    " rowspan=\"10\"",
];

#[derive(Clone)]
pub(crate) struct VisualTableAsset {
    pub id: String,
    pub page_no: u32,
    pub asset_type: String,
    pub caption: String,
    pub bbox: Value,
    pub image_path: String,
}

#[derive(Debug)]
pub(crate) struct TableRecognitionResult {
    pub cells: Vec<TableRecognitionCell>,
    pub html: String,
    pub markdown: String,
    pub ocr_text: String,
    pub crop_path: String,
    pub confidence: f64,
    pub source: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TableRecognitionCell {
    pub row_index: u32,
    pub col_index: u32,
    pub row_span: u32,
    pub col_span: u32,
    pub text: String,
    pub bbox: Value,
    pub is_header: bool,
    pub confidence: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct PdfTextLine {
    pub text: String,
    pub bbox: Value,
}

pub(crate) fn recognize_table(
    pdf_path: &Path,
    asset: &VisualTableAsset,
) -> Result<Option<TableRecognitionResult>, String> {
    if asset.asset_type != "table" {
        return Ok(None);
    }
    let Some(config) = TsrConfig::from_env()? else {
        return Ok(None);
    };
    recognize_table_with_config(pdf_path, asset, &config)
}

pub(crate) fn render_visual_asset_crop(
    pdf_path: &Path,
    asset: &VisualTableAsset,
) -> Result<Option<String>, String> {
    #[cfg(not(feature = "tsr-onnx"))]
    {
        let _ = pdf_path;
        let _ = asset;
        Ok(None)
    }
    #[cfg(feature = "tsr-onnx")]
    {
        let pdfium_dir = pdfium_dir_from_env_or_default();
        render_pdf_asset_crop(pdf_path, asset, pdfium_dir.as_deref())
            .map(|path| Some(path.display().to_string()))
    }
}

/// Serializes ALL pdfium access process-wide. pdfium (the C library) is NOT
/// thread-safe: concurrent calls from the text-index and visual-index worker
/// threads (both run on the blocking thread pool, each binding its own pdfium)
/// intermittently fail or crash — the "occasional red error" when indexing a
/// document. Every function that binds pdfium must hold this guard for the whole
/// duration of its pdfium work. Poison is recovered (the guarded unit carries no
/// invariant), so a single panicking indexing job can't wedge all future indexing.
pub(crate) fn pdfium_lock() -> std::sync::MutexGuard<'static, ()> {
    static PDFIUM_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    PDFIUM_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn render_pdf_page_image(pdf_path: &Path, page_no: u32) -> Result<PathBuf, String> {
    use pdfium_render::prelude::{PdfRenderConfig, Pdfium};
    let _pdfium_guard = pdfium_lock();

    if !pdf_path.is_file() {
        return Err(format!(
            "Cannot render PDF page because PDF is missing: {}",
            pdf_path.display()
        ));
    }
    let pdfium_dir = pdfium_dir_from_env_or_default();
    let bindings = match pdfium_dir.as_deref() {
        Some(dir) => Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(dir))
            .or_else(|_| Pdfium::bind_to_system_library()),
        None => Pdfium::bind_to_system_library(),
    }
    .map_err(|err| {
        let hint = pdfium_dir
            .as_deref()
            .map(|dir| format!(" from {}", dir.display()))
            .unwrap_or_else(|| " from the system library path".to_string());
        format!("Failed to bind Pdfium{hint} for page rendering: {err}")
    })?;
    let pdfium = Pdfium::new(bindings);
    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|err| format!("Failed to load PDF for page rendering: {err}"))?;
    let page_index = page_no
        .checked_sub(1)
        .ok_or_else(|| "Cannot render PDF page 0".to_string())?;
    let page = document
        .pages()
        .get(
            u16::try_from(page_index)
                .map_err(|_| format!("Page {page_no} is outside Pdfium's supported page range"))?,
        )
        .map_err(|err| format!("Failed to open PDF page {page_no}: {err}"))?;
    let rendered = page
        .render_with_config(
            &PdfRenderConfig::new()
                .set_target_width(PDF_PAGE_RENDER_TARGET_WIDTH)
                .render_annotations(false)
                .render_form_data(false),
        )
        .map_err(|err| format!("Failed to render PDF page {page_no}: {err}"))?
        .as_image();
    let output_path = page_image_output_path(pdf_path, page_no)?;
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Failed to create PDF page image directory {}: {err}",
                parent.display()
            )
        })?;
    }
    rendered.save(&output_path).map_err(|err| {
        format!(
            "Failed to save PDF page image {}: {err}",
            output_path.display()
        )
    })?;
    Ok(output_path)
}

pub(crate) fn attach_pdf_text_to_table_cells(
    result: &mut TableRecognitionResult,
    asset: &VisualTableAsset,
    lines: &[PdfTextLine],
) {
    let Some(asset_rect) = normalized_rect_from_bbox_list(&asset.bbox) else {
        return;
    };
    let line_rects = lines
        .iter()
        .filter_map(|line| {
            let text = line.text.trim();
            if text.is_empty() {
                return None;
            }
            normalized_rect_from_bbox_list(&line.bbox).map(|rect| (text.to_string(), rect))
        })
        .collect::<Vec<_>>();
    if line_rects.is_empty() {
        return;
    }
    for cell in &mut result.cells {
        let Some(cell_rect) = cell_rect_to_page_rect(&cell.bbox, asset_rect) else {
            continue;
        };
        cell.bbox = normalized_rect_to_bbox_json(cell_rect);
        if !cell.text.trim().is_empty() {
            continue;
        }
        let mut matches = line_rects
            .iter()
            .filter_map(|(text, line_rect)| {
                let overlap = normalized_rect_intersection_area(cell_rect, *line_rect);
                let line_area = line_rect.area();
                let center_inside =
                    cell_rect.contains_point(line_rect.center_x(), line_rect.center_y());
                if center_inside || (line_area > 0.0 && overlap / line_area >= 0.35) {
                    Some((line_rect.y1, line_rect.x1, text.as_str()))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    left.1
                        .partial_cmp(&right.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        cell.text = matches
            .into_iter()
            .map(|(_, _, text)| text)
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
    }
    result.markdown = structure_cells_to_markdown(&result.cells);
    result.ocr_text = result
        .cells
        .iter()
        .map(|cell| cell.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
}

fn recognize_table_with_config(
    pdf_path: &Path,
    asset: &VisualTableAsset,
    config: &TsrConfig,
) -> Result<Option<TableRecognitionResult>, String> {
    #[cfg(not(feature = "tsr-onnx"))]
    {
        let _ = pdf_path;
        let _ = asset;
        Err(format!(
            "Rust TSR model is configured for {}, but this binary was built without the `tsr-onnx` feature",
            config.model_path.display()
        ))
    }
    #[cfg(feature = "tsr-onnx")]
    {
        recognize_table_with_onnx(pdf_path, config, asset)
    }
}

#[cfg(feature = "tsr-onnx")]
fn recognize_table_with_onnx(
    pdf_path: &Path,
    config: &TsrConfig,
    asset: &VisualTableAsset,
) -> Result<Option<TableRecognitionResult>, String> {
    let mut session = ort::session::Session::builder()
        .map_err(|err| format!("Failed to create Rust TSR ONNX session builder: {err}"))?
        .commit_from_file(&config.model_path)
        .map_err(|err| {
            format!(
                "Failed to load Rust TSR ONNX model {}: {err}",
                config.model_path.display()
            )
        })?;
    log::debug!(
        "Loaded Rust TSR ONNX model {} with {} inputs and {} outputs",
        config.model_path.display(),
        session.inputs().len(),
        session.outputs().len()
    );
    let crop_path;
    let image_path = if let Some(image_path) = &config.image_path {
        image_path.as_path()
    } else if !asset.image_path.trim().is_empty() && Path::new(asset.image_path.trim()).is_file() {
        Path::new(asset.image_path.trim())
    } else {
        crop_path = render_pdf_asset_crop(pdf_path, asset, config.pdfium_dir.as_deref())?;
        crop_path.as_path()
    };
    let inference = run_image_inference(&mut session, image_path)?;
    let result = decode_slanet_outputs(inference.outputs, inference.geometry, image_path, asset)?;
    Ok(Some(result))
}

#[cfg(feature = "tsr-onnx")]
fn render_pdf_asset_crop(
    pdf_path: &Path,
    asset: &VisualTableAsset,
    pdfium_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    use pdfium_render::prelude::{PdfRenderConfig, Pdfium};
    let _pdfium_guard = pdfium_lock();

    if !pdf_path.is_file() {
        return Err(format!(
            "Cannot render Rust visual crop because PDF is missing: {}",
            pdf_path.display()
        ));
    }
    let bindings = match pdfium_dir {
        Some(dir) => Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(dir))
            .or_else(|_| Pdfium::bind_to_system_library()),
        None => Pdfium::bind_to_system_library(),
    }
    .map_err(|err| {
        let hint = pdfium_dir
            .map(|dir| format!(" from {}", dir.display()))
            .unwrap_or_else(|| " from the system library path".to_string());
        format!("Failed to bind Pdfium{hint} for Rust visual crop rendering: {err}")
    })?;
    let pdfium = Pdfium::new(bindings);
    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|err| format!("Failed to load PDF for Rust visual crop rendering: {err}"))?;
    let page_index = asset
        .page_no
        .checked_sub(1)
        .ok_or_else(|| "Rust TSR asset has invalid page number 0".to_string())?;
    let page = document
        .pages()
        .get(u16::try_from(page_index).map_err(|_| {
            format!(
                "Rust TSR asset page {} is outside Pdfium's supported page range",
                asset.page_no
            )
        })?)
        .map_err(|err| {
            format!(
                "Failed to open page {} for Rust visual crop rendering: {err}",
                asset.page_no
            )
        })?;
    let rendered = page
        .render_with_config(
            &PdfRenderConfig::new()
                .set_target_width(PDF_RENDER_TARGET_WIDTH)
                .render_annotations(false)
                .render_form_data(false),
        )
        .map_err(|err| {
            format!(
                "Failed to render page {} for Rust visual crop: {err}",
                asset.page_no
            )
        })?
        .as_image();
    let rect = bbox_to_pixel_rect(
        &asset.bbox,
        rendered.width(),
        rendered.height(),
        CROP_PADDING_RATIO,
    )?;
    let crop = rendered.crop_imm(rect.x, rect.y, rect.width, rect.height);
    let output_path = table_crop_output_path(pdf_path, asset)?;
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Failed to create Rust visual crop directory {}: {err}",
                parent.display()
            )
        })?;
    }
    crop.save(&output_path).map_err(|err| {
        format!(
            "Failed to save Rust visual crop {}: {err}",
            output_path.display()
        )
    })?;
    Ok(output_path)
}

#[cfg(feature = "tsr-onnx")]
#[derive(Debug, PartialEq, Eq)]
struct PixelRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[cfg(feature = "tsr-onnx")]
fn bbox_to_pixel_rect(
    bbox: &Value,
    image_width: u32,
    image_height: u32,
    padding_ratio: f64,
) -> Result<PixelRect, String> {
    let rects = bbox
        .as_array()
        .ok_or_else(|| "Rust TSR asset bbox is not an array".to_string())?;
    let mut union: Option<(f64, f64, f64, f64)> = None;
    for item in rects {
        let Some(values) = item.as_array() else {
            continue;
        };
        if values.len() < 4 {
            continue;
        }
        let coords = [
            values[0].as_f64(),
            values[1].as_f64(),
            values[2].as_f64(),
            values[3].as_f64(),
        ];
        let [Some(x1), Some(y1), Some(x2), Some(y2)] = coords else {
            continue;
        };
        let current = (x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2));
        union = Some(match union {
            Some((ux1, uy1, ux2, uy2)) => (
                ux1.min(current.0),
                uy1.min(current.1),
                ux2.max(current.2),
                uy2.max(current.3),
            ),
            None => current,
        });
    }
    let Some((mut x1, mut y1, mut x2, mut y2)) = union else {
        return Err("Rust TSR asset bbox has no usable rectangles".to_string());
    };
    let width = (x2 - x1).max(0.0);
    let height = (y2 - y1).max(0.0);
    let pad_x = width.max(0.01) * padding_ratio;
    let pad_y = height.max(0.01) * padding_ratio;
    x1 = (x1 - pad_x).clamp(0.0, 1.0);
    y1 = (y1 - pad_y).clamp(0.0, 1.0);
    x2 = (x2 + pad_x).clamp(0.0, 1.0);
    y2 = (y2 + pad_y).clamp(0.0, 1.0);
    if x2 <= x1 || y2 <= y1 {
        return Err(format!("Rust TSR asset bbox is empty: {bbox}"));
    }
    let pixel_x1 = (x1 * f64::from(image_width)).floor() as u32;
    let pixel_y1 = (y1 * f64::from(image_height)).floor() as u32;
    let pixel_x2 = (x2 * f64::from(image_width)).ceil() as u32;
    let pixel_y2 = (y2 * f64::from(image_height)).ceil() as u32;
    let x = pixel_x1.min(image_width.saturating_sub(1));
    let y = pixel_y1.min(image_height.saturating_sub(1));
    let max_width = image_width.saturating_sub(x).max(1);
    let max_height = image_height.saturating_sub(y).max(1);
    let width = pixel_x2.saturating_sub(x).clamp(1, max_width);
    let height = pixel_y2.saturating_sub(y).clamp(1, max_height);
    Ok(PixelRect {
        x,
        y,
        width,
        height,
    })
}

#[cfg(feature = "tsr-onnx")]
fn table_crop_output_path(pdf_path: &Path, asset: &VisualTableAsset) -> Result<PathBuf, String> {
    let stem = pdf_path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(sanitize_path_component)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "document".to_string());
    let file = format!(
        "p{}-{}-{}.png",
        asset.page_no,
        sanitize_path_component(&asset.asset_type),
        sanitize_path_component(&asset.id)
    );
    Ok(env::temp_dir()
        .join("markdownnote-tsr-crops")
        .join(stem)
        .join(file))
}

fn page_image_output_path(pdf_path: &Path, page_no: u32) -> Result<PathBuf, String> {
    let stem = pdf_path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(sanitize_path_component)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "document".to_string());
    Ok(env::temp_dir()
        .join("markdownnote-page-images")
        .join(format!("{stem}-{}", path_fingerprint(pdf_path)))
        .join(format!("p{page_no}.png")))
}

fn path_fingerprint(path: &Path) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(feature = "tsr-onnx")]
#[derive(Debug)]
struct RawOnnxOutput {
    name: String,
    shape: Vec<usize>,
    data: Vec<f32>,
}

#[cfg(feature = "tsr-onnx")]
#[derive(Clone, Copy, Debug)]
struct ImagePreprocessGeometry {
    resized_width: usize,
    resized_height: usize,
}

#[cfg(feature = "tsr-onnx")]
#[derive(Debug)]
struct ImageInferenceOutput {
    outputs: Vec<RawOnnxOutput>,
    geometry: ImagePreprocessGeometry,
}

#[cfg(feature = "tsr-onnx")]
fn run_image_inference(
    session: &mut ort::session::Session,
    image_path: &Path,
) -> Result<ImageInferenceOutput, String> {
    use ort::value::{Tensor, TensorElementType, ValueType};

    let input = session
        .inputs()
        .first()
        .ok_or_else(|| "Rust TSR ONNX model has no inputs".to_string())?;
    let (ty, shape) = match input.dtype() {
        ValueType::Tensor { ty, shape, .. } => (ty, shape),
        other => {
            return Err(format!(
                "Rust TSR ONNX input must be a tensor, got {other:?}"
            ))
        }
    };
    if *ty != TensorElementType::Float32 {
        return Err(format!("Rust TSR ONNX input must be float32, got {ty:?}"));
    }
    let tensor_shape = normalize_image_tensor_shape(shape)?;
    let tensor = image_to_tensor_data(image_path, tensor_shape)?;
    let outputs = session
        .run(ort::inputs![Tensor::<f32>::from_array((
            tensor_shape.to_vec(),
            tensor.data
        ))
        .map_err(|err| format!(
            "Failed to build Rust TSR input tensor: {err}"
        ))?])
        .map_err(|err| format!("Rust TSR ONNX image smoke inference failed: {err}"))?;
    let outputs = outputs
        .into_iter()
        .map(|(name, value)| {
            let (shape, data) = value
                .try_extract_tensor::<f32>()
                .map_err(|err| format!("Failed to extract Rust TSR output {name}: {err}"))?;
            Ok(RawOnnxOutput {
                name: name.to_string(),
                shape: shape
                    .iter()
                    .map(|value| usize::try_from(*value).unwrap_or(0))
                    .collect(),
                data: data.to_vec(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ImageInferenceOutput {
        outputs,
        geometry: tensor.geometry,
    })
}

#[cfg(feature = "tsr-onnx")]
fn normalize_image_tensor_shape(shape: &[i64]) -> Result<[usize; 4], String> {
    if shape.len() != 4 {
        return Err(format!(
            "Rust TSR ONNX image input must be rank 4, got shape {shape:?}"
        ));
    }
    let mut dims = [1usize; 4];
    for (index, dim) in shape.iter().enumerate() {
        dims[index] = match (*dim, index) {
            (value, _) if value > 0 => value as usize,
            (_, 0) => 1,
            (_, 1) => 3,
            (_, 2) => SLANET_INPUT_SIZE,
            (_, 3) => SLANET_INPUT_SIZE,
            _ => SLANET_INPUT_SIZE,
        };
    }
    if dims[1] != 3 && dims[3] != 3 {
        return Err(format!(
            "Rust TSR ONNX image input must be NCHW or NHWC with 3 channels, got {dims:?}"
        ));
    }
    Ok(dims)
}

#[cfg(feature = "tsr-onnx")]
struct ImageTensorData {
    data: Vec<f32>,
    geometry: ImagePreprocessGeometry,
}

#[cfg(feature = "tsr-onnx")]
fn image_to_tensor_data(image_path: &Path, shape: [usize; 4]) -> Result<ImageTensorData, String> {
    use image::{imageops::FilterType, ImageReader};

    let nchw = shape[1] == 3;
    let (height, width) = if nchw {
        (shape[2], shape[3])
    } else {
        (shape[1], shape[2])
    };
    let image = ImageReader::open(image_path)
        .map_err(|err| {
            format!(
                "Failed to open Rust TSR image {}: {err}",
                image_path.display()
            )
        })?
        .decode()
        .map_err(|err| {
            format!(
                "Failed to decode Rust TSR image {}: {err}",
                image_path.display()
            )
        })?
        .to_rgb8();
    let source_width = image.width() as usize;
    let source_height = image.height() as usize;
    let ratio = (width as f32 / source_width.max(1) as f32)
        .min(height as f32 / source_height.max(1) as f32);
    let resized_width = ((source_width as f32 * ratio).round() as usize).clamp(1, width);
    let resized_height = ((source_height as f32 * ratio).round() as usize).clamp(1, height);
    let image = image::imageops::resize(
        &image,
        resized_width as u32,
        resized_height as u32,
        FilterType::Triangle,
    );
    let mut data = vec![0.0f32; shape.iter().product()];
    let mean = [0.485f32, 0.456, 0.406];
    let std = [0.229f32, 0.224, 0.225];
    for y in 0..resized_height {
        for x in 0..resized_width {
            let pixel = image.get_pixel(x as u32, y as u32).0;
            for channel in 0..3 {
                let bgr_channel = [2usize, 1, 0][channel];
                let value = (f32::from(pixel[bgr_channel]) / 255.0 - mean[channel]) / std[channel];
                let index = if nchw {
                    ((channel * height + y) * width) + x
                } else {
                    ((y * width + x) * 3) + channel
                };
                data[index] = value;
            }
        }
    }
    Ok(ImageTensorData {
        data,
        geometry: ImagePreprocessGeometry {
            resized_width,
            resized_height,
        },
    })
}

#[cfg(feature = "tsr-onnx")]
fn decode_slanet_outputs(
    outputs: Vec<RawOnnxOutput>,
    geometry: ImagePreprocessGeometry,
    image_path: &Path,
    _asset: &VisualTableAsset,
) -> Result<TableRecognitionResult, String> {
    let struct_output = outputs
        .iter()
        .find(|output| output.shape.last() == Some(&30))
        .ok_or_else(|| "Rust TSR ONNX outputs did not include structure logits".to_string())?;
    let bbox_output = outputs
        .iter()
        .find(|output| output.shape.last() == Some(&4))
        .ok_or_else(|| "Rust TSR ONNX outputs did not include cell boxes".to_string())?;
    let (tokens, confidence) = decode_structure_tokens(struct_output)?;
    let html = tokens.join("");
    let cells = decode_cells_from_tokens(&tokens, bbox_output, geometry, confidence);
    let markdown = structure_cells_to_markdown(&cells);
    Ok(TableRecognitionResult {
        cells,
        html,
        markdown,
        ocr_text: String::new(),
        crop_path: image_path.display().to_string(),
        confidence,
        source: format!("slanet_onnx:{}+{}", bbox_output.name, struct_output.name),
    })
}

#[cfg(feature = "tsr-onnx")]
fn decode_structure_tokens(output: &RawOnnxOutput) -> Result<(Vec<String>, f64), String> {
    if output.shape.len() != 3 || output.shape[0] == 0 {
        return Err(format!(
            "Rust TSR structure output must have shape [N,T,C], got {:?}",
            output.shape
        ));
    }
    let steps = output.shape[1];
    let classes = output.shape[2];
    if classes != SLANET_STRUCTURE_TOKENS.len() + 2 {
        return Err(format!(
            "Rust TSR structure output has {classes} classes, expected {}",
            SLANET_STRUCTURE_TOKENS.len() + 2
        ));
    }
    let eos_index = classes - 1;
    let mut tokens = Vec::new();
    let mut confidence_sum = 0.0f64;
    let mut confidence_count = 0usize;
    for step in 0..steps {
        let offset = step * classes;
        let logits = &output.data[offset..offset + classes];
        let (token_index, confidence) = argmax_with_confidence(logits);
        if token_index == 0 {
            continue;
        }
        if token_index == eos_index {
            break;
        }
        if let Some(token) = SLANET_STRUCTURE_TOKENS.get(token_index - 1) {
            tokens.push((*token).to_string());
            confidence_sum += confidence as f64;
            confidence_count += 1;
        }
    }
    let confidence = if confidence_count == 0 {
        0.0
    } else {
        confidence_sum / confidence_count as f64
    };
    Ok((tokens, confidence))
}

#[cfg(feature = "tsr-onnx")]
fn argmax_with_confidence(logits: &[f32]) -> (usize, f32) {
    let mut best_index = 0usize;
    let mut best_value = f32::NEG_INFINITY;
    for (index, value) in logits.iter().enumerate() {
        if *value > best_value {
            best_index = index;
            best_value = *value;
        }
    }
    let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let denominator = logits
        .iter()
        .map(|value| (*value - max_logit).exp())
        .sum::<f32>();
    let confidence = if denominator.is_finite() && denominator > 0.0 {
        (best_value - max_logit).exp() / denominator
    } else {
        best_value.clamp(0.0, 1.0)
    };
    (best_index, confidence)
}

#[cfg(feature = "tsr-onnx")]
fn decode_cells_from_tokens(
    tokens: &[String],
    bbox_output: &RawOnnxOutput,
    geometry: ImagePreprocessGeometry,
    default_confidence: f64,
) -> Vec<TableRecognitionCell> {
    let mut row_index = 0u32;
    let mut col_index = 0u32;
    let mut in_header = false;
    let mut active_cell: Option<TableRecognitionCell> = None;
    let mut cells = Vec::new();
    for (step, token) in tokens.iter().enumerate() {
        match token.as_str() {
            "<thead>" => in_header = true,
            "</thead>" => in_header = false,
            "<tr>" => col_index = 0,
            "</tr>" => {
                if let Some(cell) = active_cell.take() {
                    col_index += cell.col_span.max(1);
                    cells.push(cell);
                }
                row_index += 1;
            }
            "<td>" | "<td" => {
                if let Some(cell) = active_cell.take() {
                    col_index += cell.col_span.max(1);
                    cells.push(cell);
                }
                active_cell = Some(TableRecognitionCell {
                    row_index,
                    col_index,
                    row_span: 1,
                    col_span: 1,
                    text: String::new(),
                    bbox: bbox_for_step(bbox_output, step, geometry),
                    is_header: in_header,
                    confidence: default_confidence,
                });
            }
            "</td>" => {
                if let Some(cell) = active_cell.take() {
                    col_index += cell.col_span.max(1);
                    cells.push(cell);
                }
            }
            value if value.contains("colspan=\"") => {
                if let Some(cell) = &mut active_cell {
                    cell.col_span = span_value(value).unwrap_or(1);
                }
            }
            value if value.contains("rowspan=\"") => {
                if let Some(cell) = &mut active_cell {
                    cell.row_span = span_value(value).unwrap_or(1);
                }
            }
            _ => {}
        }
    }
    if let Some(cell) = active_cell {
        cells.push(cell);
    }
    cells
}

#[cfg(feature = "tsr-onnx")]
fn bbox_for_step(output: &RawOnnxOutput, step: usize, geometry: ImagePreprocessGeometry) -> Value {
    if output.shape.len() != 3 || output.shape[2] != 4 || step >= output.shape[1] {
        return serde_json::json!([]);
    }
    let offset = step * 4;
    model_bbox_to_crop_bbox_json(
        [
            output.data.get(offset).copied().unwrap_or_default() as f64,
            output.data.get(offset + 1).copied().unwrap_or_default() as f64,
            output.data.get(offset + 2).copied().unwrap_or_default() as f64,
            output.data.get(offset + 3).copied().unwrap_or_default() as f64,
        ],
        geometry,
    )
}

#[cfg(feature = "tsr-onnx")]
fn model_bbox_to_crop_bbox_json(raw: [f64; 4], geometry: ImagePreprocessGeometry) -> Value {
    if !raw.iter().all(|value| value.is_finite()) {
        return serde_json::json!([]);
    }
    let max_coord = raw.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (x_scale, y_scale) = if max_coord <= 1.2 {
        (1.0, 1.0)
    } else if max_coord <= TSR_MODEL_INPUT_SIZE_FOR_BBOX * 1.2 {
        (
            geometry.resized_width.max(1) as f64,
            geometry.resized_height.max(1) as f64,
        )
    } else {
        return serde_json::json!([]);
    };
    let rect = NormalizedRect {
        x1: (raw[0].min(raw[2]) / x_scale).clamp(0.0, 1.0),
        y1: (raw[1].min(raw[3]) / y_scale).clamp(0.0, 1.0),
        x2: (raw[0].max(raw[2]) / x_scale).clamp(0.0, 1.0),
        y2: (raw[1].max(raw[3]) / y_scale).clamp(0.0, 1.0),
    };
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return serde_json::json!([]);
    }
    normalized_rect_to_bbox_json(rect)
}

#[cfg(feature = "tsr-onnx")]
fn span_value(token: &str) -> Option<u32> {
    token
        .split('"')
        .nth(1)
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
}

fn structure_cells_to_markdown(cells: &[TableRecognitionCell]) -> String {
    if cells.is_empty() {
        return String::new();
    }
    let has_text = cells.iter().any(|cell| !cell.text.trim().is_empty());
    if has_text {
        return cells
            .iter()
            .filter(|cell| !cell.text.trim().is_empty())
            .map(|cell| {
                format!(
                    "r{}c{}: {}",
                    cell.row_index + 1,
                    cell.col_index + 1,
                    cell.text.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    format!("Detected {} table cells without OCR text.", cells.len())
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct NormalizedRect {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

impl NormalizedRect {
    fn width(self) -> f64 {
        (self.x2 - self.x1).max(0.0)
    }

    fn height(self) -> f64 {
        (self.y2 - self.y1).max(0.0)
    }

    fn area(self) -> f64 {
        self.width() * self.height()
    }

    fn center_x(self) -> f64 {
        (self.x1 + self.x2) / 2.0
    }

    fn center_y(self) -> f64 {
        (self.y1 + self.y2) / 2.0
    }

    fn contains_point(self, x: f64, y: f64) -> bool {
        x >= self.x1 && x <= self.x2 && y >= self.y1 && y <= self.y2
    }
}

fn normalized_rect_from_bbox_list(bbox: &Value) -> Option<NormalizedRect> {
    let rects = bbox.as_array()?;
    let mut union: Option<NormalizedRect> = None;
    for item in rects {
        let values = item.as_array()?;
        if values.len() < 4 {
            continue;
        }
        let x1 = values[0].as_f64()?;
        let y1 = values[1].as_f64()?;
        let x2 = values[2].as_f64()?;
        let y2 = values[3].as_f64()?;
        let rect = NormalizedRect {
            x1: x1.min(x2).clamp(0.0, 1.0),
            y1: y1.min(y2).clamp(0.0, 1.0),
            x2: x1.max(x2).clamp(0.0, 1.0),
            y2: y1.max(y2).clamp(0.0, 1.0),
        };
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            continue;
        }
        union = Some(match union {
            Some(current) => NormalizedRect {
                x1: current.x1.min(rect.x1),
                y1: current.y1.min(rect.y1),
                x2: current.x2.max(rect.x2),
                y2: current.y2.max(rect.y2),
            },
            None => rect,
        });
    }
    union
}

fn cell_rect_to_page_rect(cell_bbox: &Value, asset_rect: NormalizedRect) -> Option<NormalizedRect> {
    let raw = raw_rect_from_bbox_list(cell_bbox)?;
    let max_coord = raw.x1.max(raw.y1).max(raw.x2).max(raw.y2).abs();
    let scale = if max_coord <= 1.2 {
        1.0
    } else if max_coord <= TSR_MODEL_INPUT_SIZE_FOR_BBOX * 1.2 {
        TSR_MODEL_INPUT_SIZE_FOR_BBOX
    } else {
        return None;
    };
    let crop_rect = NormalizedRect {
        x1: (raw.x1 / scale).clamp(0.0, 1.0),
        y1: (raw.y1 / scale).clamp(0.0, 1.0),
        x2: (raw.x2 / scale).clamp(0.0, 1.0),
        y2: (raw.y2 / scale).clamp(0.0, 1.0),
    };
    if crop_rect.width() <= 0.0 || crop_rect.height() <= 0.0 {
        return None;
    }
    Some(NormalizedRect {
        x1: asset_rect.x1 + asset_rect.width() * crop_rect.x1,
        y1: asset_rect.y1 + asset_rect.height() * crop_rect.y1,
        x2: asset_rect.x1 + asset_rect.width() * crop_rect.x2,
        y2: asset_rect.y1 + asset_rect.height() * crop_rect.y2,
    })
}

fn raw_rect_from_bbox_list(bbox: &Value) -> Option<NormalizedRect> {
    let rects = bbox.as_array()?;
    let mut union: Option<NormalizedRect> = None;
    for item in rects {
        let values = item.as_array()?;
        if values.len() < 4 {
            continue;
        }
        let x1 = values[0].as_f64()?;
        let y1 = values[1].as_f64()?;
        let x2 = values[2].as_f64()?;
        let y2 = values[3].as_f64()?;
        let rect = NormalizedRect {
            x1: x1.min(x2),
            y1: y1.min(y2),
            x2: x1.max(x2),
            y2: y1.max(y2),
        };
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            continue;
        }
        union = Some(match union {
            Some(current) => NormalizedRect {
                x1: current.x1.min(rect.x1),
                y1: current.y1.min(rect.y1),
                x2: current.x2.max(rect.x2),
                y2: current.y2.max(rect.y2),
            },
            None => rect,
        });
    }
    union
}

fn normalized_rect_intersection_area(left: NormalizedRect, right: NormalizedRect) -> f64 {
    let x1 = left.x1.max(right.x1);
    let y1 = left.y1.max(right.y1);
    let x2 = left.x2.min(right.x2);
    let y2 = left.y2.min(right.y2);
    if x2 <= x1 || y2 <= y1 {
        return 0.0;
    }
    (x2 - x1) * (y2 - y1)
}

fn normalized_rect_to_bbox_json(rect: NormalizedRect) -> Value {
    serde_json::json!([[rect.x1, rect.y1, rect.x2, rect.y2]])
}

#[derive(Debug)]
struct TsrConfig {
    model_path: PathBuf,
    #[cfg_attr(not(feature = "tsr-onnx"), allow(dead_code))]
    image_path: Option<PathBuf>,
    #[cfg_attr(not(feature = "tsr-onnx"), allow(dead_code))]
    pdfium_dir: Option<PathBuf>,
}

impl TsrConfig {
    fn from_env() -> Result<Option<Self>, String> {
        let raw_path = env::var(TSR_MODEL_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        #[cfg(feature = "tsr-onnx")]
        let model_path = raw_path.map(PathBuf::from).or_else(find_default_tsr_model);
        #[cfg(not(feature = "tsr-onnx"))]
        let model_path = raw_path.map(PathBuf::from);
        let Some(raw_path) = model_path else {
            return Ok(None);
        };
        let image_path = env::var(TSR_IMAGE_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let pdfium_dir = pdfium_dir_from_env_or_default();
        Self::from_model_path(raw_path, image_path, pdfium_dir).map(Some)
    }

    fn from_model_path(
        path: impl Into<PathBuf>,
        image_path: Option<PathBuf>,
        pdfium_dir: Option<PathBuf>,
    ) -> Result<Self, String> {
        let model_path = path.into();
        if !model_path.is_file() {
            return Err(format!(
                "Rust TSR model is configured but not found: {}",
                model_path.display()
            ));
        }
        if let Some(image_path) = &image_path {
            if !image_path.is_file() {
                return Err(format!(
                    "Rust TSR smoke image is configured but not found: {}",
                    image_path.display()
                ));
            }
        }
        if let Some(pdfium_dir) = &pdfium_dir {
            if !pdfium_dir.is_dir() {
                return Err(format!(
                    "Rust TSR Pdfium directory is configured but not found: {}",
                    pdfium_dir.display()
                ));
            }
        }
        Ok(Self {
            model_path,
            image_path,
            pdfium_dir,
        })
    }
}

#[cfg(feature = "tsr-onnx")]
fn find_default_tsr_model() -> Option<PathBuf> {
    find_project_relative_file(DEFAULT_TSR_MODEL_PATH)
}

fn find_default_pdfium_dir() -> Option<PathBuf> {
    find_project_relative_dir(&format!("{DEFAULT_PDFIUM_DIR}/lib"))
        .or_else(|| find_project_relative_dir(DEFAULT_PDFIUM_DIR))
}

pub(crate) fn pdfium_dir_from_env_or_default() -> Option<PathBuf> {
    env::var(PDFIUM_DIR_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(find_default_pdfium_dir)
}

#[cfg(feature = "tsr-onnx")]
fn find_project_relative_file(relative_path: &str) -> Option<PathBuf> {
    project_relative_candidates(relative_path)
        .into_iter()
        .find(|path| path.is_file())
}

fn find_project_relative_dir(relative_path: &str) -> Option<PathBuf> {
    project_relative_candidates(relative_path)
        .into_iter()
        .find(|path| path.is_dir())
}

fn project_relative_candidates(relative_path: &str) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    // Packaged app: the bundled resource dir holds `.models/` — check it first.
    if let Some(resource_root) = RESOURCE_MODELS_ROOT.get() {
        roots.push(resource_root.clone());
    }
    if let Ok(current_dir) = env::current_dir() {
        roots.push(current_dir.clone());
        roots.push(current_dir.join("markdownnote-desktop"));
        for ancestor in current_dir.ancestors().take(4) {
            roots.push(ancestor.to_path_buf());
            roots.push(ancestor.join("markdownnote-desktop"));
        }
    }
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    if let Some(parent) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
        roots.push(parent.to_path_buf());
    }
    roots
        .into_iter()
        .map(|root| root.join(relative_path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_image_output_path_separates_same_stem_pdfs() {
        let first =
            page_image_output_path(Path::new("/tmp/markdownnote/a/paper.pdf"), 3).expect("path");
        let second =
            page_image_output_path(Path::new("/tmp/markdownnote/b/paper.pdf"), 3).expect("path");

        assert_ne!(first, second);
        assert_eq!(
            first.file_name().and_then(|value| value.to_str()),
            Some("p3.png")
        );
        assert_eq!(
            second.file_name().and_then(|value| value.to_str()),
            Some("p3.png")
        );
    }

    #[test]
    fn missing_tsr_model_is_reported_clearly() {
        let err = TsrConfig::from_model_path("/definitely/missing/slanet.onnx", None, None)
            .expect_err("missing model should fail");

        assert!(err.contains("Rust TSR model is configured but not found"));
    }

    #[test]
    fn attaches_pdf_line_text_to_tsr_cells() {
        let asset = VisualTableAsset {
            id: "asset-1".to_string(),
            page_no: 1,
            asset_type: "table".to_string(),
            caption: "Table 7".to_string(),
            bbox: serde_json::json!([[0.10, 0.10, 0.90, 0.50]]),
            image_path: String::new(),
        };
        let mut result = TableRecognitionResult {
            cells: vec![
                TableRecognitionCell {
                    row_index: 0,
                    col_index: 0,
                    row_span: 1,
                    col_span: 1,
                    text: String::new(),
                    bbox: serde_json::json!([[0.0, 0.0, 0.45, 0.30]]),
                    is_header: true,
                    confidence: 0.9,
                },
                TableRecognitionCell {
                    row_index: 1,
                    col_index: 1,
                    row_span: 1,
                    col_span: 1,
                    text: String::new(),
                    bbox: serde_json::json!([[0.50, 0.40, 1.0, 1.0]]),
                    is_header: false,
                    confidence: 0.9,
                },
            ],
            html: "<table></table>".to_string(),
            markdown: String::new(),
            ocr_text: String::new(),
            crop_path: String::new(),
            confidence: 0.9,
            source: "test".to_string(),
        };
        let lines = vec![
            PdfTextLine {
                text: "Benchmark".to_string(),
                bbox: serde_json::json!([[0.12, 0.12, 0.40, 0.18]]),
            },
            PdfTextLine {
                text: "77.8".to_string(),
                bbox: serde_json::json!([[0.62, 0.32, 0.72, 0.38]]),
            },
        ];

        attach_pdf_text_to_table_cells(&mut result, &asset, &lines);

        assert_eq!(result.cells[0].text, "Benchmark");
        assert_eq!(result.cells[1].text, "77.8");
        assert!(result.ocr_text.contains("77.8"));
        assert!(result.markdown.contains("r2c2: 77.8"));
    }

    #[test]
    fn configured_tsr_model_requires_feature_gate() {
        let temp_dir = env::temp_dir();
        let model_path = temp_dir.join("markdownnote-empty-test-model.onnx");
        std::fs::write(&model_path, b"not-a-real-model").expect("write temp model");
        let config =
            TsrConfig::from_model_path(&model_path, None, None).expect("model path config");
        let asset = VisualTableAsset {
            id: "asset-1".to_string(),
            page_no: 1,
            asset_type: "table".to_string(),
            caption: "Table 1".to_string(),
            bbox: serde_json::json!([[0.1, 0.1, 0.9, 0.5]]),
            image_path: String::new(),
        };

        let result = recognize_table_with_config(Path::new("/tmp/paper.pdf"), &asset, &config);

        let err = result.expect_err("test model is intentionally invalid");
        if cfg!(feature = "tsr-onnx") {
            assert!(err.contains("Failed to load Rust TSR ONNX model"));
        } else {
            assert!(err.contains("without the `tsr-onnx` feature"));
        }
        let _ = std::fs::remove_file(model_path);
    }

    #[cfg(feature = "tsr-onnx")]
    #[test]
    fn normalized_bbox_converts_to_padded_pixel_rect() {
        let rect = bbox_to_pixel_rect(
            &serde_json::json!([[0.25, 0.2, 0.75, 0.6]]),
            1000,
            2000,
            0.02,
        )
        .expect("pixel rect");

        assert_eq!(
            rect,
            PixelRect {
                x: 240,
                y: 384,
                width: 520,
                height: 832,
            }
        );
    }

    #[cfg(feature = "tsr-onnx")]
    #[test]
    fn absolute_model_bbox_removes_letterbox_padding() {
        let bbox = model_bbox_to_crop_bbox_json(
            [122.0, 50.0, 244.0, 100.0],
            ImagePreprocessGeometry {
                resized_width: 244,
                resized_height: 488,
            },
        );

        let rect = normalized_rect_from_bbox_list(&bbox).expect("normalized bbox");
        assert!((rect.x1 - 0.5).abs() < 0.001);
        assert!((rect.x2 - 1.0).abs() < 0.001);
        assert!((rect.y1 - 0.102).abs() < 0.001);
        assert!((rect.y2 - 0.205).abs() < 0.001);
    }

    #[cfg(feature = "tsr-onnx")]
    #[test]
    fn image_tensor_data_supports_nchw_and_nhwc() {
        let image_path = env::temp_dir().join("markdownnote-tsr-test-image.png");
        let mut image = image::RgbImage::new(2, 2);
        image.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        image.put_pixel(1, 0, image::Rgb([0, 255, 0]));
        image.put_pixel(0, 1, image::Rgb([0, 0, 255]));
        image.put_pixel(1, 1, image::Rgb([255, 255, 255]));
        image.save(&image_path).expect("save test image");

        let nchw = image_to_tensor_data(&image_path, [1, 3, 2, 2]).expect("nchw tensor");
        let nhwc = image_to_tensor_data(&image_path, [1, 2, 2, 3]).expect("nhwc tensor");

        assert_eq!(nchw.data.len(), 12);
        assert_eq!(nhwc.data.len(), 12);
        assert_eq!(nchw.geometry.resized_width, 2);
        assert_eq!(nchw.geometry.resized_height, 2);
        assert!((nchw.data[0] - (-0.485 / 0.229)).abs() < 0.001);
        assert!((nhwc.data[0] - (-0.485 / 0.229)).abs() < 0.001);
        let _ = std::fs::remove_file(image_path);
    }

    #[cfg(feature = "tsr-onnx")]
    #[test]
    fn configured_real_model_and_image_smoke_runs_when_env_is_set() {
        let Some(config) = TsrConfig::from_env().expect("read TSR env") else {
            return;
        };
        if config.image_path.is_none() {
            return;
        }
        let asset = VisualTableAsset {
            id: "asset-smoke".to_string(),
            page_no: 1,
            asset_type: "table".to_string(),
            caption: "Smoke table".to_string(),
            bbox: serde_json::json!([[0.1, 0.1, 0.9, 0.5]]),
            image_path: String::new(),
        };

        let result = recognize_table_with_config(Path::new("/tmp/paper.pdf"), &asset, &config)
            .expect("real model image smoke should run");

        let result = result.expect("real model image smoke should return decoded structure");
        assert!(result.source.starts_with("slanet_onnx:"));
        assert!(!result.html.is_empty());
    }

    #[cfg(feature = "tsr-onnx")]
    #[test]
    fn configured_real_pdf_crop_smoke_runs_when_env_is_set() {
        let Some(config) = TsrConfig::from_env().expect("read TSR env") else {
            return;
        };
        let Some(pdf_path) = env::var("MARKDOWNNOTE_TSR_PDF")
            .ok()
            .map(PathBuf::from)
            .filter(|path| path.is_file())
        else {
            return;
        };
        let asset = VisualTableAsset {
            id: "asset-pdf-smoke".to_string(),
            page_no: 1,
            asset_type: "table".to_string(),
            caption: "PDF smoke table".to_string(),
            bbox: serde_json::json!([[0.10, 0.18, 0.90, 0.72]]),
            image_path: String::new(),
        };

        let result = recognize_table_with_config(&pdf_path, &asset, &config)
            .expect("real model PDF crop smoke should run");

        let result = result.expect("real model PDF crop smoke should return decoded structure");
        assert!(result.source.starts_with("slanet_onnx:"));
        assert!(Path::new(&result.crop_path).is_file());
    }
}
