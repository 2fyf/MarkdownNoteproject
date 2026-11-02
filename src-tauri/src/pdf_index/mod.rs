use std::{cmp::Ordering, path::Path};

mod layout;
mod lines;

pub(crate) use layout::{debug_pdfium_layout_from_lines, PdfLayoutDebugLine};
use layout::{
    GeometricPdfLayoutAnalyzer, PageLayoutInput, PdfLayoutAnalysis, PdfLayoutAnalyzer,
    PdfLayoutSummary,
};
use lines::{build_pdfium_lines, sort_pdfium_text_rects};

use crate::{
    vision, BlockIndexInput, LayoutRegionIndexInput, LineIndexInput, OutlineIndexInput,
    PageIndexInput, TextUnitIndexInput, UpsertDocumentIndexInput,
};

/// Stable sentinel returned when a PDF has pages but no extractable text layer
/// (scanned / image-only). The frontend matches on this exact string to show a
/// localized "scanned PDF, OCR not supported yet" message. Keep in sync with
/// the matcher in src/App.vue.
pub(crate) const SCANNED_PDF_NO_TEXT_ERROR: &str = "SCANNED_PDF_NO_TEXT";

#[derive(Clone)]
struct PdfTextRect {
    text: String,
    source_order: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    font_size: f64,
    font_name: String,
    font_flags: i64,
    baseline: Option<f64>,
}

struct PdfLineDraft {
    line_no: u32,
    text: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    height: f64,
    rects: Vec<PdfTextRect>,
}

struct PdfParagraphDraft {
    text: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    height: f64,
    role: PdfBlockRole,
    region_index: u32,
    lines: Vec<PdfLineDraft>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PdfBlockRole {
    Body,
    Heading,
    Caption,
    Code,
    Formula,
    Footnote,
    Reference,
    Header,
    Footer,
}

impl PdfBlockRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Heading => "heading",
            Self::Caption => "caption",
            Self::Code => "code",
            Self::Formula => "formula",
            Self::Footnote => "footnote",
            Self::Reference => "reference",
            Self::Header => "header",
            Self::Footer => "footer",
        }
    }
}

pub(super) struct PdfIndexProgress {
    pub page_current: u32,
    pub page_done: u32,
    pub page_total: u32,
    pub block_count: usize,
    pub line_count: usize,
    pub page_finished: bool,
    /// True for the tick emitted just before OCR runs on a scanned page, so the
    /// UI can show "running OCR on page X" instead of a frozen bar (OCR is the
    /// slowest single step and produces no sub-page progress).
    pub ocr_running: bool,
    pub layout_confidence: Option<f64>,
    pub region_count: Option<usize>,
    pub two_column_detected: Option<bool>,
}

struct BuiltPdfiumPageIndex {
    page: PageIndexInput,
    layout_summary: PdfLayoutSummary,
}

pub(super) fn build_document_index_from_pdfium_with_progress(
    document_id: &str,
    pdf_path: &Path,
    mut on_progress: impl FnMut(PdfIndexProgress),
) -> Result<UpsertDocumentIndexInput, String> {
    use pdfium_render::prelude::Pdfium;

    // pdfium is not thread-safe — hold the global lock for the whole extraction so
    // a concurrent visual-index render can't collide with it (the intermittent
    // indexing error). See vision::pdfium_lock.
    let _pdfium_guard = vision::pdfium_lock();

    let pdfium_dir = vision::pdfium_dir_from_env_or_default();
    let bindings = match pdfium_dir.as_deref() {
        Some(dir) => Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(dir))
            .or_else(|_| Pdfium::bind_to_system_library()),
        None => Pdfium::bind_to_system_library(),
    }
    .map_err(|err| {
        let hint = pdfium_dir
            .as_ref()
            .map(|dir| format!(" from {}", dir.display()))
            .unwrap_or_else(|| " from the system library path".to_string());
        format!("Failed to bind Pdfium{hint} for document reindex: {err}")
    })?;
    let pdfium = Pdfium::new(bindings);
    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|err| format!("Failed to load PDF for document reindex: {err}"))?;
    let page_count = u32::from(document.pages().len());
    let mut pages = Vec::with_capacity(page_count as usize);
    let mut extracted_blocks = 0usize;
    let mut extracted_lines = 0usize;

    for page_index in document.pages().as_range() {
        let page_no = u32::from(page_index) + 1;
        on_progress(PdfIndexProgress {
            page_current: page_no,
            page_done: page_no.saturating_sub(1),
            page_total: page_count,
            block_count: extracted_blocks,
            line_count: extracted_lines,
            page_finished: false,
            ocr_running: false,
            layout_confidence: None,
            region_count: None,
            two_column_detected: None,
        });
        let page = document
            .pages()
            .get(page_index)
            .map_err(|err| format!("Failed to open PDF page {}: {err}", page_index + 1))?;
        let width = f64::from(page.width().value);
        let height = f64::from(page.height().value);
        let text = page.text().map_err(|err| {
            format!(
                "Failed to extract PDF text on page {}: {err}",
                page_index + 1
            )
        })?;
        let rects = text
            .segments()
            .iter()
            .enumerate()
            .filter_map(|(source_order, segment)| {
                let raw_text = normalize_inline_text(&segment.text());
                if raw_text.is_empty() {
                    return None;
                }
                let bounds = segment.bounds();
                let x = f64::from(bounds.left().value);
                let top = f64::from(bounds.top().value);
                let font = pdfium_segment_font_metadata(&segment, height);
                let rect = PdfTextRect {
                    text: raw_text,
                    source_order: source_order as u32,
                    x,
                    y: (height - top).max(0.0),
                    width: f64::from(bounds.width().value).max(1.0),
                    height: f64::from((bounds.top() - bounds.bottom()).value).max(1.0),
                    font_size: font.size,
                    font_name: font.name,
                    font_flags: font.flags,
                    baseline: font.baseline,
                };
                is_usable_pdf_text_rect(&rect, width, height).then_some(rect)
            })
            .collect::<Vec<_>>();
        // Scanned / image-only page: pdfium found no text layer. Try OCR to
        // recover text + boxes so the page becomes a first-class indexed page
        // (searchable, quotable, annotatable). We render the page bitmap from
        // THIS already-open `page` handle and hand the pixels to OCR — OCR must
        // not re-bind pdfium while this document is live (that deadlocks under
        // pdfium's thread_safe global lock). Without `ocr-onnx` or when models
        // are absent this yields no rects and we fall through to zero-text.
        let rects = if rects.is_empty() {
            // Signal the slow OCR step so the UI shows "running OCR on page X"
            // rather than a frozen bar while the model runs.
            on_progress(PdfIndexProgress {
                page_current: page_no,
                page_done: page_no.saturating_sub(1),
                page_total: page_count,
                block_count: extracted_blocks,
                line_count: extracted_lines,
                page_finished: false,
                ocr_running: true,
                layout_confidence: None,
                region_count: None,
                two_column_detected: None,
            });
            ocr_rects_for_page(&page, page_no, width, height)
        } else {
            rects
        };
        let page_input =
            build_pdfium_page_index_with_layout(document_id, page_no, width, height, rects);
        extracted_blocks += page_input.page.blocks.len();
        extracted_lines += page_input.page.lines.as_ref().map_or(0, Vec::len);
        on_progress(PdfIndexProgress {
            page_current: page_no,
            page_done: page_no,
            page_total: page_count,
            block_count: extracted_blocks,
            line_count: extracted_lines,
            page_finished: true,
            ocr_running: false,
            layout_confidence: Some(page_input.layout_summary.confidence),
            region_count: Some(page_input.layout_summary.region_count),
            two_column_detected: Some(page_input.layout_summary.two_column_detected),
        });
        pages.push(page_input.page);
    }

    if extracted_blocks == 0 {
        // No pdfium text layer AND OCR (per-page fallback above) recovered
        // nothing either — either a scanned doc whose OCR found no text, or a
        // build without the `ocr-onnx` feature / missing models. Surface the
        // stable sentinel the frontend maps to a friendly, localized "scanned
        // PDF, OCR unavailable" message (rather than a cryptic Pdfium error).
        if page_count > 0 {
            return Err(SCANNED_PDF_NO_TEXT_ERROR.to_string());
        }
        return Err("Pdfium did not extract any usable text blocks from this PDF".to_string());
    }

    Ok(UpsertDocumentIndexInput {
        document_id: document_id.to_string(),
        page_count,
        pages,
        outlines: Some(build_pdfium_outlines(&document)),
    })
}

fn build_pdfium_outlines(
    document: &pdfium_render::prelude::PdfDocument<'_>,
) -> Vec<OutlineIndexInput> {
    document
        .bookmarks()
        .iter()
        .filter_map(|bookmark| {
            let title = bookmark
                .title()
                .map(|value| normalize_inline_text(&value))
                .unwrap_or_default();
            if title.is_empty() {
                return None;
            }
            let page_start = bookmark
                .destination()
                .and_then(|destination| destination.page_index().ok())
                .map(|page_index| u32::from(page_index) + 1)
                .unwrap_or(0);
            (page_start > 0).then_some((bookmark, title, page_start))
        })
        .enumerate()
        .map(|(index, (bookmark, title, page_start))| OutlineIndexInput {
            title,
            level: pdfium_bookmark_level(bookmark),
            page_start,
            order_index: index as u32,
        })
        .collect()
}

fn pdfium_bookmark_level(mut bookmark: pdfium_render::prelude::PdfBookmark<'_>) -> u32 {
    let mut level = 1u32;
    while let Some(parent) = bookmark.parent() {
        level = level.saturating_add(1);
        bookmark = parent;
    }
    level
}

#[cfg(test)]
fn build_pdfium_page_index(
    document_id: &str,
    page_no: u32,
    width: f64,
    height: f64,
    rects: Vec<PdfTextRect>,
) -> PageIndexInput {
    build_pdfium_page_index_with_layout(document_id, page_no, width, height, rects).page
}

fn build_pdfium_page_index_with_layout(
    document_id: &str,
    page_no: u32,
    width: f64,
    height: f64,
    rects: Vec<PdfTextRect>,
) -> BuiltPdfiumPageIndex {
    let text_units = rects
        .iter()
        .map(|rect| TextUnitIndexInput {
            source_order: rect.source_order,
            text: rect.text.clone(),
            bbox: normalized_bbox_value(
                rect.x,
                rect.y,
                rect.x + rect.width,
                rect.y + rect.height,
                width,
                height,
            ),
            font_size: rect.font_size,
            font_name: rect.font_name.clone(),
            font_flags: rect.font_flags,
        })
        .collect::<Vec<_>>();
    let lines = build_pdfium_lines(sort_pdfium_text_rects(rects), width);
    let mut index_lines = lines
        .iter()
        .filter(|line| !line.text.is_empty() && !line.rects.is_empty())
        .map(|line| LineIndexInput {
            line_no: line.line_no,
            text: line.text.clone(),
            bbox_list: serde_json::Value::Array(
                line.rects
                    .iter()
                    .map(|rect| {
                        normalized_bbox_value(
                            rect.x,
                            rect.y,
                            rect.x + rect.width,
                            rect.y + rect.height,
                            width,
                            height,
                        )
                    })
                    .collect(),
            ),
            source_order_list: Some(line.rects.iter().map(|rect| rect.source_order).collect()),
            region_index: None,
            role_hint: None,
            baseline: Some(pdfium_line_baseline(line)),
        })
        .collect::<Vec<_>>();
    let layout_analyzer = GeometricPdfLayoutAnalyzer;
    let PdfLayoutAnalysis {
        paragraphs,
        summary: layout_summary,
        regions,
    } = layout_analyzer.analyze_page(PageLayoutInput {
        lines,
        page_width: width,
        page_height: height,
    });
    let layout_regions = regions
        .into_iter()
        .map(|region| LayoutRegionIndexInput {
            region_index: region.region_index,
            kind: region.kind.to_string(),
            bbox: serde_json::json!(region.bbox),
            line_numbers: region.line_numbers,
            confidence: region.confidence,
        })
        .collect::<Vec<_>>();
    let line_region_by_no = layout_regions
        .iter()
        .flat_map(|region| {
            region
                .line_numbers
                .iter()
                .copied()
                .map(move |line_no| (line_no, region.region_index))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let line_role_by_no = paragraphs
        .iter()
        .flat_map(|paragraph| {
            paragraph
                .lines
                .iter()
                .map(move |line| (line.line_no, paragraph.role.as_str()))
        })
        .collect::<std::collections::HashMap<_, _>>();
    for line in &mut index_lines {
        line.region_index = line_region_by_no.get(&line.line_no).copied();
        line.role_hint = line_role_by_no
            .get(&line.line_no)
            .map(|role| (*role).to_string());
    }
    let blocks = paragraphs
        .iter()
        .enumerate()
        .filter(|(_, paragraph)| !paragraph.text.is_empty() && !paragraph.lines.is_empty())
        .map(|(index, paragraph)| {
            let bbox_values = paragraph
                .lines
                .iter()
                .flat_map(|line| line.rects.iter())
                .map(|rect| {
                    normalized_bbox_value(
                        rect.x,
                        rect.y,
                        rect.x + rect.width,
                        rect.y + rect.height,
                        width,
                        height,
                    )
                })
                .collect::<Vec<_>>();
            BlockIndexInput {
                id: format!("{document_id}-p{page_no}-b{}", index + 1),
                block_index: (index + 1) as u32,
                text: paragraph.text.clone(),
                bbox_list: serde_json::Value::Array(bbox_values),
                role: Some(paragraph.role.as_str().to_string()),
                region_index: Some(paragraph.region_index),
            }
        })
        .collect::<Vec<_>>();

    BuiltPdfiumPageIndex {
        layout_summary,
        page: PageIndexInput {
            page_no,
            width,
            height,
            text: blocks
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            blocks,
            lines: Some(index_lines),
            units: Some(text_units),
            regions: Some(layout_regions),
        },
    }
}

fn total_f64_cmp(left: f64, right: f64) -> Ordering {
    left.total_cmp(&right)
}

/// Run OCR on a page that has no pdfium text layer and convert the recognized
/// lines into `PdfTextRect`s in the page's point-coordinate space. OCR returns
/// normalized (0..1) top-down bboxes; `PdfTextRect.y` is also top-down (it is
/// stored as `height - top`), so no axis flip is needed — just scale by the
/// page width/height. Failures (or OCR disabled) yield an empty vec, leaving
/// the existing zero-text handling intact.
fn ocr_rects_for_page(
    page: &pdfium_render::prelude::PdfPage<'_>,
    page_no: u32,
    page_width: f64,
    page_height: f64,
) -> Vec<PdfTextRect> {
    use pdfium_render::prelude::PdfRenderConfig;

    // Render this page to a bitmap using the already-open page handle (no new
    // pdfium binding — that would deadlock). 1600px target width matches the
    // size the OCR models were validated against.
    let rendered = match page.render_with_config(
        &PdfRenderConfig::new()
            .set_target_width(1600)
            .render_annotations(false)
            .render_form_data(false),
    ) {
        Ok(rendered) => rendered.as_image().to_rgba8(),
        Err(err) => {
            log::warn!("OCR page render failed for page {page_no}: {err}");
            return Vec::new();
        }
    };
    let (img_w, img_h) = (rendered.width(), rendered.height());
    let lines = match vision::ocr_recognize_image_rgba(rendered.as_raw(), img_w, img_h) {
        Ok(lines) => lines,
        Err(err) => {
            log::warn!("OCR failed for page {page_no}: {err}");
            return Vec::new();
        }
    };
    let mut rects = Vec::new();
    for (source_order, line) in lines.into_iter().enumerate() {
        let [nx0, ny0, nx1, ny1] = line.bbox;
        let x = nx0 * page_width;
        let y = ny0 * page_height;
        let w = ((nx1 - nx0) * page_width).max(1.0);
        let h = ((ny1 - ny0) * page_height).max(1.0);
        let rect = PdfTextRect {
            text: line.text,
            source_order: source_order as u32,
            x,
            y,
            width: w,
            height: h,
            // OCR has no font metadata; use the line height as a reasonable
            // font-size proxy so downstream layout heuristics behave.
            font_size: h,
            font_name: String::new(),
            font_flags: 0,
            baseline: None,
        };
        if is_usable_pdf_text_rect(&rect, page_width, page_height) {
            rects.push(rect);
        }
    }
    rects
}

fn is_usable_pdf_text_rect(rect: &PdfTextRect, page_width: f64, page_height: f64) -> bool {
    if ![
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        page_width,
        page_height,
    ]
    .into_iter()
    .all(f64::is_finite)
    {
        return false;
    }
    if rect.width < 1.5 || rect.height < 1.5 {
        return false;
    }
    if rect.x < -4.0 || rect.y < -4.0 {
        return false;
    }
    if rect.x > page_width + 4.0 || rect.y > page_height + 4.0 {
        return false;
    }
    if rect.width > page_width * 0.95 {
        return false;
    }
    if rect.height > page_height * 0.08 {
        return false;
    }
    true
}

fn normalize_inline_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalized_bbox_value(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    page_width: f64,
    page_height: f64,
) -> serde_json::Value {
    if ![x1, y1, x2, y2, page_width, page_height]
        .into_iter()
        .all(f64::is_finite)
        || page_width <= 0.0
        || page_height <= 0.0
    {
        return serde_json::json!([0.0, 0.0, 0.0, 0.0]);
    }
    serde_json::json!([
        round_normalized_bbox(x1 / page_width),
        round_normalized_bbox(y1 / page_height),
        round_normalized_bbox(x2 / page_width),
        round_normalized_bbox(y2 / page_height)
    ])
}

fn round_normalized_bbox(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    (value * 10000.0).round() / 10000.0
}

struct PdfSegmentFontMetadata {
    size: f64,
    name: String,
    flags: i64,
    baseline: Option<f64>,
}

fn pdfium_segment_font_metadata(
    segment: &pdfium_render::prelude::PdfPageTextSegment<'_>,
    page_height: f64,
) -> PdfSegmentFontMetadata {
    let Ok(chars) = segment.chars() else {
        return PdfSegmentFontMetadata {
            size: 0.0,
            name: String::new(),
            flags: 0,
            baseline: None,
        };
    };
    let Ok(char) = chars.get(0) else {
        return PdfSegmentFontMetadata {
            size: 0.0,
            name: String::new(),
            flags: 0,
            baseline: None,
        };
    };

    let mut flags = 0i64;
    if char.font_is_fixed_pitch() {
        flags |= 1;
    }
    if char.font_is_serif() {
        flags |= 1 << 1;
    }
    if char.font_is_symbolic() {
        flags |= 1 << 2;
    }
    if char.font_is_cursive() {
        flags |= 1 << 3;
    }
    if char.font_is_non_symbolic() {
        flags |= 1 << 5;
    }
    if char.font_is_italic() {
        flags |= 1 << 6;
    }
    if char.font_is_all_caps() {
        flags |= 1 << 16;
    }
    if char.font_is_small_caps() {
        flags |= 1 << 17;
    }
    if char.font_is_bold_reenforced() {
        flags |= 1 << 18;
    }

    PdfSegmentFontMetadata {
        size: f64::from(char.scaled_font_size().value).max(0.0),
        name: char.font_name(),
        flags,
        baseline: char
            .origin_y()
            .ok()
            .map(|origin_y| (page_height - f64::from(origin_y.value)).clamp(0.0, page_height)),
    }
}

fn pdfium_line_baseline(line: &PdfLineDraft) -> f64 {
    median_f64_for_indexing(line.rects.iter().filter_map(|rect| rect.baseline))
        .unwrap_or(line.y2)
        .max(line.y1)
}

fn median_f64_for_indexing(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut values = values.filter(|value| value.is_finite()).collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    values.sort_by(|left, right| total_f64_cmp(*left, *right));
    Some(values[values.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::path::Path;

    #[test]
    fn scanned_pdf_sentinel_is_stable() {
        // The frontend (src/App.vue formatIndexError / scanned-pdf matcher) keys
        // off this exact value. Changing it requires updating the frontend too.
        assert_eq!(SCANNED_PDF_NO_TEXT_ERROR, "SCANNED_PDF_NO_TEXT");
    }

    fn rect(text: &str, x: f64, y: f64, width: f64) -> PdfTextRect {
        PdfTextRect {
            text: text.to_string(),
            source_order: 0,
            x,
            y,
            width,
            height: 8.0,
            font_size: 0.0,
            font_name: String::new(),
            font_flags: 0,
            baseline: None,
        }
    }

    fn rect_with_source_order(
        text: &str,
        x: f64,
        y: f64,
        width: f64,
        source_order: u32,
    ) -> PdfTextRect {
        PdfTextRect {
            text: text.to_string(),
            source_order,
            x,
            y,
            width,
            height: 8.0,
            font_size: 0.0,
            font_name: String::new(),
            font_flags: 0,
            baseline: None,
        }
    }

    #[derive(Deserialize)]
    struct LayoutGeometryFixture {
        page_width: f64,
        page_height: f64,
        rects: Vec<LayoutRectFixture>,
        expected_blocks: Vec<String>,
        #[serde(default)]
        expected_roles: Vec<String>,
        #[serde(default)]
        must_not_contain_pairs: Vec<[String; 2]>,
    }

    #[derive(Deserialize)]
    struct LayoutRectFixture {
        text: String,
        x: f64,
        y: f64,
        width: f64,
    }

    #[derive(Deserialize)]
    struct ExternalPdfGoldenFixture {
        name: String,
        env_var: String,
        default_hint: Option<String>,
        pages: Vec<ExternalPdfGoldenPage>,
    }

    #[derive(Deserialize)]
    struct ExternalPdfGoldenPage {
        page_no: u32,
        min_blocks: usize,
        #[serde(default)]
        ordered_terms: Vec<String>,
        #[serde(default)]
        must_not_mix: Vec<[String; 2]>,
    }

    fn load_layout_geometry_fixture(name: &str) -> LayoutGeometryFixture {
        let raw = match name {
            "code_formula_boundaries" => {
                include_str!("../../tests/fixtures/pdf_layout/code_formula_boundaries.json")
            }
            "header_footer_noise" => {
                include_str!("../../tests/fixtures/pdf_layout/header_footer_noise.json")
            }
            "reference_footnote_boundaries" => {
                include_str!("../../tests/fixtures/pdf_layout/reference_footnote_boundaries.json")
            }
            "section_reset_between_columns" => {
                include_str!("../../tests/fixtures/pdf_layout/section_reset_between_columns.json")
            }
            "single_column_paragraphs" => {
                include_str!("../../tests/fixtures/pdf_layout/single_column_paragraphs.json")
            }
            "split_section_heading_fragments" => {
                include_str!("../../tests/fixtures/pdf_layout/split_section_heading_fragments.json")
            }
            "table_caption_boundaries" => {
                include_str!("../../tests/fixtures/pdf_layout/table_caption_boundaries.json")
            }
            "two_column_mixed_geometry" => {
                include_str!("../../tests/fixtures/pdf_layout/two_column_mixed_geometry.json")
            }
            _ => panic!("unknown layout geometry fixture: {name}"),
        };
        serde_json::from_str(raw).expect("layout fixture json")
    }

    fn load_external_pdf_golden_fixtures() -> Vec<ExternalPdfGoldenFixture> {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/pdf_layout/external_pdf_golden_cases.json"
        ))
        .expect("external PDF golden fixture json")
    }

    fn assert_layout_geometry_fixture(name: &str) {
        let fixture = load_layout_geometry_fixture(name);
        let page = build_pdfium_page_index(
            "doc",
            1,
            fixture.page_width,
            fixture.page_height,
            fixture
                .rects
                .iter()
                .map(|item| rect(&item.text, item.x, item.y, item.width))
                .collect(),
        );
        let texts = page
            .blocks
            .iter()
            .map(|block| block.text.clone())
            .collect::<Vec<_>>();

        assert_eq!(texts, fixture.expected_blocks, "fixture: {name}");
        if !fixture.expected_roles.is_empty() {
            let roles = page
                .blocks
                .iter()
                .map(|block| block.role.clone().unwrap_or_default())
                .collect::<Vec<_>>();
            assert_eq!(roles, fixture.expected_roles, "fixture roles: {name}");
        }
        for [left, right] in fixture.must_not_contain_pairs {
            assert!(
                texts
                    .iter()
                    .all(|block| !(block.contains(&left) && block.contains(&right))),
                "fixture {name} block should not mix '{left}' and '{right}': {texts:#?}"
            );
        }
    }

    #[test]
    fn pdfium_line_builder_keeps_two_columns_separate() {
        let lines = build_pdfium_lines(
            vec![
                rect("left column text", 50.0, 120.0, 245.0),
                rect("right column text", 310.0, 120.0, 250.0),
            ],
            600.0,
        );

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "left column text");
        assert_eq!(lines[1].text, "right column text");
    }

    #[test]
    fn pdfium_line_builder_merges_nearby_inline_segments() {
        let lines = build_pdfium_lines(
            vec![
                rect("Abstract", 50.0, 120.0, 46.0),
                rect("- Agentic workflows", 99.0, 120.0, 150.0),
            ],
            600.0,
        );

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Abstract - Agentic workflows");
    }

    #[test]
    fn pdfium_line_index_preserves_source_order_for_debugging() {
        let mut second = rect_with_source_order("second", 110.0, 120.0, 40.0, 7);
        second.font_size = 9.5;
        second.font_name = "Times-Roman".to_string();
        second.font_flags = 32;
        second.baseline = Some(128.0);
        let mut first = rect_with_source_order("first", 50.0, 120.0, 40.0, 3);
        first.font_size = 9.5;
        first.font_name = "Times-Roman".to_string();
        first.font_flags = 32;
        first.baseline = Some(128.0);
        let page = build_pdfium_page_index_with_layout("doc", 1, 600.0, 800.0, vec![second, first]);

        let line = page
            .page
            .lines
            .as_ref()
            .and_then(|lines| lines.first())
            .expect("first line");
        assert_eq!(line.text, "first second");
        assert_eq!(line.source_order_list.as_deref(), Some(&[3, 7][..]));
        assert_eq!(line.region_index, Some(1));
        assert_eq!(line.role_hint.as_deref(), Some("body"));
        assert_eq!(line.baseline, Some(128.0));
        let units = page.page.units.as_ref().expect("text units");
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].source_order, 7);
        assert_eq!(units[1].source_order, 3);
        assert_eq!(units[0].font_size, 9.5);
        assert_eq!(units[0].font_name, "Times-Roman");
        assert_eq!(units[0].font_flags, 32);
    }

    #[test]
    fn normalized_bbox_value_rejects_invalid_page_size() {
        assert_eq!(
            normalized_bbox_value(10.0, 20.0, 30.0, 40.0, 0.0, 800.0),
            serde_json::json!([0.0, 0.0, 0.0, 0.0])
        );
        assert_eq!(
            normalized_bbox_value(10.0, f64::NAN, 30.0, 40.0, 600.0, 800.0),
            serde_json::json!([0.0, 0.0, 0.0, 0.0])
        );
    }

    #[test]
    fn pdfium_line_builder_merges_same_line_segments_with_small_baseline_drift() {
        let page = build_pdfium_page_index(
            "doc",
            1,
            600.0,
            800.0,
            vec![
                rect("workflows,", 145.0, 121.5, 72.0),
                rect("on", 50.0, 120.0, 20.0),
                rect("agentic", 78.0, 119.0, 60.0),
                rect("within", 225.0, 120.8, 55.0),
            ],
        );

        assert_eq!(page.lines.as_ref().map(Vec::len), Some(1));
        assert_eq!(page.blocks[0].text, "on agentic workflows, within");
    }

    #[test]
    fn pdfium_line_builder_keeps_same_column_segments_with_wide_spaces() {
        let lines = build_pdfium_lines(
            vec![
                rect("left phrase", 50.0, 120.0, 120.0),
                rect("after a larger gap", 188.0, 120.0, 150.0),
            ],
            600.0,
        );

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "left phrase after a larger gap");
    }

    #[test]
    fn pdfium_paragraph_builder_does_not_let_shorter_right_column_split_left_column() {
        let page = build_pdfium_page_index(
            "doc",
            1,
            600.0,
            800.0,
            vec![
                rect("left line one", 50.0, 100.0, 245.0),
                rect("right line one", 310.0, 101.0, 250.0),
                rect("left line two", 50.0, 112.0, 245.0),
                rect("left line three", 50.0, 124.0, 245.0),
                rect("left line four", 50.0, 136.0, 245.0),
                rect("left line five", 50.0, 148.0, 245.0),
            ],
        );

        assert_eq!(page.blocks.len(), 2);
        assert_eq!(
            page.blocks[0].text,
            "left line one\nleft line two\nleft line three\nleft line four\nleft line five"
        );
        assert_eq!(page.blocks[1].text, "right line one");
    }

    #[test]
    fn pdfium_page_index_marks_code_blocks_without_merging_with_body() {
        let page = build_pdfium_page_index(
            "doc",
            1,
            600.0,
            800.0,
            vec![
                rect(
                    "The wrapper keeps the original workflow stable.",
                    50.0,
                    100.0,
                    300.0,
                ),
                rect(
                    "def grep_with_pruner(file_path, pattern):",
                    50.0,
                    130.0,
                    260.0,
                ),
                rect("return prune(raw_output, pattern)", 70.0, 142.0, 230.0),
                rect(
                    "The agent then receives the pruned context.",
                    50.0,
                    176.0,
                    300.0,
                ),
            ],
        );

        let texts = page
            .blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            vec![
                "The wrapper keeps the original workflow stable.",
                "def grep_with_pruner(file_path, pattern):\nreturn prune(raw_output, pattern)",
                "The agent then receives the pruned context."
            ]
        );
        assert_eq!(page.blocks[1].role.as_deref(), Some("code"));
    }

    #[test]
    fn pdfium_reading_order_keeps_late_full_width_caption_after_columns() {
        let page = build_pdfium_page_index(
            "doc",
            1,
            600.0,
            800.0,
            vec![
                rect("left line one", 50.0, 100.0, 245.0),
                rect("right line one", 310.0, 101.0, 250.0),
                rect("left line two", 50.0, 112.0, 245.0),
                rect("right line two", 310.0, 113.0, 250.0),
                rect(
                    "Figure 1 Full width caption under both columns.",
                    40.0,
                    190.0,
                    520.0,
                ),
            ],
        );

        let texts = page
            .blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            vec![
                "left line one\nleft line two",
                "right line one\nright line two",
                "Figure 1 Full width caption under both columns."
            ]
        );
        assert_eq!(page.blocks[2].role.as_deref(), Some("caption"));
    }

    #[test]
    fn pdfium_reading_order_does_not_drop_column_lines_overlapping_full_width_band() {
        let page = build_pdfium_page_index(
            "doc",
            1,
            600.0,
            800.0,
            vec![
                rect("left before", 50.0, 100.0, 245.0),
                rect("right before", 310.0, 101.0, 245.0),
                rect("Figure 2 Full width caption.", 40.0, 112.0, 520.0),
                rect("left overlap", 50.0, 112.0, 245.0),
                rect("right overlap", 310.0, 113.0, 245.0),
                rect("left after", 50.0, 126.0, 245.0),
                rect("right after", 310.0, 127.0, 245.0),
            ],
        );
        let joined = page
            .blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        for term in [
            "left before",
            "right before",
            "Figure 2 Full width caption.",
            "left overlap",
            "right overlap",
            "left after",
            "right after",
        ] {
            assert!(joined.contains(term), "missing term {term}: {joined}");
        }
    }

    #[test]
    fn pdfium_section_heading_repair_does_not_merge_across_regions() {
        let page = build_pdfium_page_index(
            "doc",
            1,
            600.0,
            800.0,
            vec![
                rect("left intro one", 50.0, 100.0, 245.0),
                rect("right intro one", 310.0, 101.0, 245.0),
                rect("3.2", 50.0, 112.0, 28.0),
                rect("Goal Hint Generation", 310.0, 113.0, 170.0),
                rect("left after", 50.0, 126.0, 245.0),
                rect("right after", 310.0, 127.0, 245.0),
            ],
        );

        assert!(
            page.blocks
                .iter()
                .all(|block| block.text != "3.2 Goal Hint Generation"),
            "cross-region section fragments must not be merged: {:#?}",
            page.blocks
                .iter()
                .map(|block| &block.text)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn pdfium_reading_order_matches_two_column_geometry_fixture() {
        assert_layout_geometry_fixture("two_column_mixed_geometry");
    }

    #[test]
    fn pdfium_reading_order_matches_single_column_fixture() {
        assert_layout_geometry_fixture("single_column_paragraphs");
    }

    #[test]
    fn pdfium_reading_order_matches_section_reset_fixture() {
        assert_layout_geometry_fixture("section_reset_between_columns");
    }

    #[test]
    fn pdfium_repairs_split_section_heading_fragments_fixture() {
        assert_layout_geometry_fixture("split_section_heading_fragments");
    }

    #[test]
    fn pdfium_roles_match_code_formula_fixture() {
        assert_layout_geometry_fixture("code_formula_boundaries");
    }

    #[test]
    fn pdfium_roles_match_table_caption_fixture() {
        assert_layout_geometry_fixture("table_caption_boundaries");
    }

    #[test]
    fn pdfium_roles_match_header_footer_fixture() {
        assert_layout_geometry_fixture("header_footer_noise");
    }

    #[test]
    fn pdfium_roles_match_reference_footnote_fixture() {
        assert_layout_geometry_fixture("reference_footnote_boundaries");
    }

    #[test]
    fn pdfium_page_index_reports_layout_summary() {
        let page = build_pdfium_page_index_with_layout(
            "doc",
            1,
            600.0,
            800.0,
            vec![
                rect("left line one", 50.0, 100.0, 245.0),
                rect("right line one", 310.0, 101.0, 250.0),
                rect("left line two", 50.0, 112.0, 245.0),
                rect("right line two", 310.0, 113.0, 250.0),
            ],
        );

        assert_eq!(page.layout_summary.region_count, 2);
        assert!(page.layout_summary.two_column_detected);
        assert!(page.layout_summary.confidence >= 0.9);
        assert_eq!(page.page.blocks.len(), 2);
        let regions = page.page.regions.as_ref().expect("layout regions");
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].kind, "body_column");
        assert_eq!(regions[0].line_numbers, vec![1, 3]);
        assert_eq!(regions[1].line_numbers, vec![2, 4]);
        let lines = page.page.lines.as_ref().expect("layout lines");
        assert_eq!(lines[0].region_index, Some(1));
        assert_eq!(lines[1].region_index, Some(2));
        assert_eq!(lines[2].region_index, Some(1));
        assert_eq!(lines[3].region_index, Some(2));
    }

    #[test]
    fn pdfium_page_index_reports_low_confidence_for_low_text_page() {
        let page = build_pdfium_page_index_with_layout("doc", 1, 600.0, 800.0, Vec::new());

        assert_eq!(page.layout_summary.region_count, 0);
        assert!(!page.layout_summary.two_column_detected);
        assert!(page.layout_summary.confidence <= 0.35);
        assert!(page.page.blocks.is_empty());
        assert_eq!(page.page.lines.as_ref().map(Vec::len), Some(0));
        assert_eq!(page.page.units.as_ref().map(Vec::len), Some(0));
        assert_eq!(page.page.regions.as_ref().map(Vec::len), Some(0));
    }

    #[test]
    fn configured_real_pdf_layout_smoke_runs_when_env_is_set() {
        let Ok(path) = std::env::var("MARKDOWNNOTE_LAYOUT_PDF_SMOKE") else {
            return;
        };
        if path.trim().is_empty() {
            return;
        }

        let mut page_done_events = Vec::new();
        let index = build_document_index_from_pdfium_with_progress(
            "real-doc",
            Path::new(&path),
            |progress| {
                if progress.page_finished {
                    page_done_events.push((
                        progress.page_done,
                        progress.layout_confidence,
                        progress.region_count,
                    ));
                }
            },
        )
        .expect("real PDF layout smoke");

        assert_eq!(
            usize::try_from(index.page_count).ok(),
            Some(index.pages.len())
        );
        assert_eq!(page_done_events.len(), index.pages.len());
        assert!(index.pages.iter().any(|page| !page.blocks.is_empty()));
        assert!(page_done_events.iter().any(|(_, _, regions)| {
            regions
                .map(|region_count| region_count > 0)
                .unwrap_or(false)
        }));
        for (_, confidence, _) in page_done_events {
            let confidence = confidence.expect("layout confidence on page done");
            assert!((0.0..=1.0).contains(&confidence));
        }
    }

    #[test]
    fn configured_real_pdf_layout_golden_cases_run_when_env_is_set() {
        let mut ran_any = false;
        for fixture in load_external_pdf_golden_fixtures() {
            let Ok(path) = std::env::var(&fixture.env_var) else {
                continue;
            };
            if path.trim().is_empty() {
                continue;
            }
            ran_any = true;
            let index = build_document_index_from_pdfium_with_progress(
                &fixture.name,
                Path::new(&path),
                |_| {},
            )
            .unwrap_or_else(|err| {
                panic!(
                    "external PDF golden fixture '{}' failed to index '{}': {err}",
                    fixture.name, path
                )
            });

            for expected_page in fixture.pages {
                let page = index
                    .pages
                    .iter()
                    .find(|page| page.page_no == expected_page.page_no)
                    .unwrap_or_else(|| {
                        panic!(
                            "external PDF golden fixture '{}' missing page {}; hint path {:?}",
                            fixture.name, expected_page.page_no, fixture.default_hint
                        )
                    });
                assert!(
                    page.blocks.len() >= expected_page.min_blocks,
                    "external PDF golden fixture '{}' page {} expected at least {} blocks, got {}",
                    fixture.name,
                    expected_page.page_no,
                    expected_page.min_blocks,
                    page.blocks.len()
                );
                assert_ordered_terms_in_blocks(
                    &fixture.name,
                    expected_page.page_no,
                    &page.blocks,
                    &expected_page.ordered_terms,
                );
                assert_terms_not_mixed_in_blocks(
                    &fixture.name,
                    expected_page.page_no,
                    &page.blocks,
                    expected_page.must_not_mix,
                );
            }
        }
        if !ran_any {
            eprintln!(
                "skipping external PDF golden cases; set MARKDOWNNOTE_LAYOUT_SWE_PRUNER_PDF or MARKDOWNNOTE_LAYOUT_FORGE_PDF"
            );
        }
    }

    fn assert_ordered_terms_in_blocks(
        fixture_name: &str,
        page_no: u32,
        blocks: &[BlockIndexInput],
        ordered_terms: &[String],
    ) {
        let mut previous_index = None;
        for term in ordered_terms {
            let Some(index) = blocks.iter().position(|block| block.text.contains(term)) else {
                panic!(
                    "external PDF golden fixture '{fixture_name}' page {page_no} missing ordered term '{term}' in blocks: {:#?}",
                    blocks
                        .iter()
                        .map(|block| block.text.as_str())
                        .collect::<Vec<_>>()
                );
            };
            if let Some(previous_index) = previous_index {
                assert!(
                    index >= previous_index,
                    "external PDF golden fixture '{fixture_name}' page {page_no} term '{term}' appears before a prior ordered term"
                );
            }
            previous_index = Some(index);
        }
    }

    fn assert_terms_not_mixed_in_blocks(
        fixture_name: &str,
        page_no: u32,
        blocks: &[BlockIndexInput],
        must_not_mix: Vec<[String; 2]>,
    ) {
        for [left, right] in must_not_mix {
            assert!(
                blocks
                    .iter()
                    .all(|block| !(block.text.contains(&left) && block.text.contains(&right))),
                "external PDF golden fixture '{fixture_name}' page {page_no} block should not mix '{left}' and '{right}'"
            );
        }
    }
}
