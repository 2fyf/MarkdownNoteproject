use serde::Serialize;

use super::{total_f64_cmp, PdfBlockRole, PdfLineDraft, PdfParagraphDraft, PdfTextRect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PdfRegionKind {
    FullWidth,
    BodyColumn,
    Code,
    Footnote,
    Header,
    Footer,
}

impl PdfRegionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::FullWidth => "full_width",
            Self::BodyColumn => "body_column",
            Self::Code => "code",
            Self::Footnote => "footnote",
            Self::Header => "header",
            Self::Footer => "footer",
        }
    }
}

struct PdfLayoutRegion {
    kind: PdfRegionKind,
    lines: Vec<PdfLineDraft>,
}

struct PdfLayoutRegions {
    two_column_candidate: bool,
    regions: Vec<PdfLayoutRegion>,
}

#[derive(Clone)]
pub(crate) struct PdfLayoutDebugLine {
    pub line_no: u32,
    pub block_index: u32,
    pub text: String,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

#[derive(Serialize)]
pub(crate) struct PdfLayoutDebug {
    pub confidence: f64,
    pub two_column_detected: bool,
    pub regions: Vec<PdfLayoutDebugRegion>,
    pub warnings: Vec<PdfLayoutDebugWarning>,
}

#[derive(Serialize)]
pub(crate) struct PdfLayoutDebugRegion {
    pub index: u32,
    pub kind: &'static str,
    pub line_count: usize,
    pub line_numbers: Vec<u32>,
    pub block_indexes: Vec<u32>,
    pub bbox: [f64; 4],
    pub absolute_bbox: [f64; 4],
    pub text_preview: String,
}

#[derive(Serialize)]
pub(crate) struct PdfLayoutDebugWarning {
    pub kind: &'static str,
    pub message: String,
}

pub(super) struct PdfLayoutAnalysis {
    pub paragraphs: Vec<PdfParagraphDraft>,
    pub summary: PdfLayoutSummary,
    pub regions: Vec<PdfLayoutRegionSummary>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PdfLayoutSummary {
    pub confidence: f64,
    pub region_count: usize,
    pub two_column_detected: bool,
}

pub(super) struct PdfLayoutRegionSummary {
    pub region_index: u32,
    pub kind: &'static str,
    pub bbox: [f64; 4],
    pub line_numbers: Vec<u32>,
    pub confidence: f64,
}

pub(super) struct PageLayoutInput {
    pub lines: Vec<PdfLineDraft>,
    pub page_width: f64,
    pub page_height: f64,
}

pub(super) trait PdfLayoutAnalyzer {
    fn analyze_page(&self, input: PageLayoutInput) -> PdfLayoutAnalysis;
}

pub(super) struct GeometricPdfLayoutAnalyzer;

impl PdfLayoutAnalyzer for GeometricPdfLayoutAnalyzer {
    fn analyze_page(&self, input: PageLayoutInput) -> PdfLayoutAnalysis {
        analyze_pdfium_lines(input.lines, input.page_width, input.page_height)
    }
}

pub(super) fn analyze_pdfium_lines(
    lines: Vec<PdfLineDraft>,
    page_width: f64,
    page_height: f64,
) -> PdfLayoutAnalysis {
    let layout_regions = build_pdfium_layout_regions(lines, page_width, page_height);
    let summary = summarize_pdfium_regions(
        &layout_regions.regions,
        page_width,
        page_height,
        layout_regions.two_column_candidate,
    );
    let region_summaries =
        summarize_pdfium_region_details(&layout_regions.regions, page_width, page_height);
    let median_height = median_f64(
        layout_regions
            .regions
            .iter()
            .flat_map(|region| region.lines.iter().map(|line| line.height))
            .filter(|value| value.is_finite()),
    )
    .unwrap_or(12.0);
    let mut paragraphs = Vec::new();

    for (index, region) in layout_regions.regions.into_iter().enumerate() {
        paragraphs.extend(group_pdfium_region_lines_into_paragraphs(
            region,
            index as u32 + 1,
            median_height,
            page_width,
        ));
    }

    let paragraphs = repair_split_section_heading_paragraphs(paragraphs)
        .into_iter()
        .filter(|paragraph| !paragraph.text.trim().is_empty() && !paragraph.lines.is_empty())
        .collect::<Vec<_>>();

    PdfLayoutAnalysis {
        paragraphs,
        summary,
        regions: region_summaries,
    }
}

fn summarize_pdfium_regions(
    regions: &[PdfLayoutRegion],
    page_width: f64,
    page_height: f64,
    two_column_candidate: bool,
) -> PdfLayoutSummary {
    let body_column_count = regions
        .iter()
        .filter(|region| region.kind == PdfRegionKind::BodyColumn)
        .count();
    let confidence =
        layout_summary_confidence(page_width, page_height, two_column_candidate, regions);
    PdfLayoutSummary {
        confidence,
        region_count: regions.len(),
        two_column_detected: two_column_candidate && body_column_count >= 2,
    }
}

fn summarize_pdfium_region_details(
    regions: &[PdfLayoutRegion],
    page_width: f64,
    page_height: f64,
) -> Vec<PdfLayoutRegionSummary> {
    regions
        .iter()
        .enumerate()
        .filter_map(|(index, region)| {
            if region.lines.is_empty() {
                return None;
            }
            let absolute_bbox = region_absolute_bbox(&region.lines);
            let normalized_bbox = normalized_debug_bbox(absolute_bbox, page_width, page_height);
            let line_numbers = region
                .lines
                .iter()
                .map(|line| line.line_no)
                .filter(|line_no| *line_no > 0)
                .collect::<Vec<_>>();
            Some(PdfLayoutRegionSummary {
                region_index: index as u32 + 1,
                kind: region.kind.as_str(),
                bbox: normalized_bbox,
                line_numbers,
                confidence: region_confidence(region, page_width, page_height),
            })
        })
        .collect()
}

pub(crate) fn debug_pdfium_layout_from_lines(
    lines: Vec<PdfLayoutDebugLine>,
    page_width: f64,
    page_height: f64,
) -> PdfLayoutDebug {
    let warnings = layout_debug_input_warnings(&lines, page_width, page_height);
    let draft_lines = lines
        .into_iter()
        .map(|line| PdfLineDraft {
            line_no: line.line_no,
            text: line.text,
            x1: line.x1,
            y1: line.y1,
            x2: line.x2,
            y2: line.y2,
            height: (line.y2 - line.y1).max(1.0),
            rects: vec![PdfTextRect {
                text: format!(
                    "__line_no:{}:block_index:{}",
                    line.line_no, line.block_index
                ),
                source_order: line.line_no,
                x: line.x1,
                y: line.y1,
                width: (line.x2 - line.x1).max(1.0),
                height: (line.y2 - line.y1).max(1.0),
                font_size: 0.0,
                font_name: String::new(),
                font_flags: 0,
                baseline: Some(line.y2),
            }],
        })
        .collect::<Vec<_>>();
    let layout_regions = build_pdfium_layout_regions(draft_lines, page_width, page_height);
    let region_count = layout_regions.regions.len();
    let debug_regions = layout_regions
        .regions
        .into_iter()
        .enumerate()
        .map(|(index, region)| {
            debug_region_from_layout_region(index as u32 + 1, region, page_width, page_height)
        })
        .collect::<Vec<_>>();
    let confidence = layout_debug_confidence(
        page_width,
        page_height,
        layout_regions.two_column_candidate,
        region_count,
        &debug_regions,
        &warnings,
    );
    let two_column_detected = layout_regions.two_column_candidate
        && debug_regions
            .iter()
            .filter(|region| region.kind == "body_column")
            .count()
            >= 2;

    PdfLayoutDebug {
        confidence,
        two_column_detected,
        regions: debug_regions,
        warnings,
    }
}

fn debug_region_from_layout_region(
    index: u32,
    region: PdfLayoutRegion,
    page_width: f64,
    page_height: f64,
) -> PdfLayoutDebugRegion {
    let absolute_bbox = region_absolute_bbox(&region.lines);
    let line_numbers = region
        .lines
        .iter()
        .filter_map(debug_line_no_from_line)
        .collect::<Vec<_>>();
    let mut block_indexes = region
        .lines
        .iter()
        .filter_map(debug_block_index_from_line)
        .collect::<Vec<_>>();
    block_indexes.sort_unstable();
    block_indexes.dedup();
    let text_preview = region
        .lines
        .iter()
        .map(|line| line.text.trim())
        .filter(|text| !text.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(360)
        .collect::<String>();

    PdfLayoutDebugRegion {
        index,
        kind: region.kind.as_str(),
        line_count: region.lines.len(),
        line_numbers,
        block_indexes,
        bbox: normalized_debug_bbox(absolute_bbox, page_width, page_height),
        absolute_bbox,
        text_preview,
    }
}

fn region_absolute_bbox(lines: &[PdfLineDraft]) -> [f64; 4] {
    let (x1, y1, x2, y2) = lines.iter().fold(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        |(x1, y1, x2, y2), line| {
            (
                x1.min(line.x1),
                y1.min(line.y1),
                x2.max(line.x2),
                y2.max(line.y2),
            )
        },
    );
    [
        finite_or_zero(x1),
        finite_or_zero(y1),
        finite_or_zero(x2),
        finite_or_zero(y2),
    ]
}

fn region_confidence(region: &PdfLayoutRegion, page_width: f64, page_height: f64) -> f64 {
    if region.lines.is_empty()
        || !page_width.is_finite()
        || !page_height.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
    {
        return 0.3;
    }
    let bbox = region_absolute_bbox(&region.lines);
    if bbox[2] <= bbox[0] || bbox[3] <= bbox[1] {
        return 0.4;
    }
    0.95
}

fn layout_debug_input_warnings(
    lines: &[PdfLayoutDebugLine],
    page_width: f64,
    page_height: f64,
) -> Vec<PdfLayoutDebugWarning> {
    let mut warnings = Vec::new();
    if !page_width.is_finite()
        || !page_height.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
    {
        warnings.push(PdfLayoutDebugWarning {
            kind: "invalid_page_size",
            message: "Indexed page size is invalid; layout regions may be unreliable.".to_string(),
        });
    }
    if lines.is_empty() {
        warnings.push(PdfLayoutDebugWarning {
            kind: "no_indexed_lines",
            message: "This page has no indexed lines to analyze.".to_string(),
        });
    }
    let invalid_lines = lines
        .iter()
        .filter(|line| {
            ![line.x1, line.y1, line.x2, line.y2]
                .into_iter()
                .all(f64::is_finite)
                || line.x2 <= line.x1
                || line.y2 <= line.y1
        })
        .count();
    if invalid_lines > 0 {
        warnings.push(PdfLayoutDebugWarning {
            kind: "invalid_line_bbox",
            message: format!("{invalid_lines} indexed lines have invalid bounding boxes."),
        });
    }
    warnings
}

fn layout_debug_confidence(
    page_width: f64,
    page_height: f64,
    two_column_detected: bool,
    region_count: usize,
    regions: &[PdfLayoutDebugRegion],
    warnings: &[PdfLayoutDebugWarning],
) -> f64 {
    if !page_width.is_finite()
        || !page_height.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
    {
        return 0.2;
    }
    if region_count == 0 {
        return 0.3;
    }
    let mut confidence = 0.95_f64;
    if two_column_detected
        && regions
            .iter()
            .filter(|region| region.kind == "body_column")
            .count()
            < 2
    {
        confidence -= 0.25;
    }
    if regions.iter().any(|region| region.line_count == 0) {
        confidence -= 0.2;
    }
    confidence -= (warnings.len() as f64 * 0.08).min(0.32);
    confidence.clamp(0.0, 1.0)
}

fn layout_summary_confidence(
    page_width: f64,
    page_height: f64,
    two_column_candidate: bool,
    regions: &[PdfLayoutRegion],
) -> f64 {
    if !page_width.is_finite()
        || !page_height.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
    {
        return 0.2;
    }
    if regions.is_empty() {
        return 0.3;
    }
    let mut confidence = 0.95_f64;
    if two_column_candidate
        && regions
            .iter()
            .filter(|region| region.kind == PdfRegionKind::BodyColumn)
            .count()
            < 2
    {
        confidence -= 0.25;
    }
    if regions.iter().any(|region| region.lines.is_empty()) {
        confidence -= 0.2;
    }
    confidence.clamp(0.0, 1.0)
}

fn debug_line_no_from_line(line: &PdfLineDraft) -> Option<u32> {
    (line.line_no > 0).then_some(line.line_no)
}

fn debug_block_index_from_line(line: &PdfLineDraft) -> Option<u32> {
    line.rects
        .first()
        .and_then(|rect| rect.text.split_once(":block_index:"))
        .and_then(|(_, value)| value.parse::<u32>().ok())
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn normalized_debug_bbox(bbox: [f64; 4], page_width: f64, page_height: f64) -> [f64; 4] {
    if !page_width.is_finite()
        || !page_height.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
    {
        return [0.0, 0.0, 0.0, 0.0];
    }
    [
        clamp_unit(bbox[0] / page_width),
        clamp_unit(bbox[1] / page_height),
        clamp_unit(bbox[2] / page_width),
        clamp_unit(bbox[3] / page_height),
    ]
}

fn clamp_unit(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn build_pdfium_layout_regions(
    lines: Vec<PdfLineDraft>,
    page_width: f64,
    page_height: f64,
) -> PdfLayoutRegions {
    if lines.is_empty() {
        return PdfLayoutRegions {
            two_column_candidate: false,
            regions: Vec::new(),
        };
    }
    let two_columns = detect_two_column_layout(&lines, page_width);
    let mut full_width = Vec::new();
    let mut column_lines = Vec::new();
    let mut header = Vec::new();
    let mut footnote = Vec::new();
    let mut footer = Vec::new();

    for line in lines {
        let kind = classify_line_region_kind(&line, page_width, page_height, two_columns);
        match kind {
            PdfRegionKind::Header => header.push(line),
            PdfRegionKind::Footnote => footnote.push(line),
            PdfRegionKind::Footer => footer.push(line),
            PdfRegionKind::FullWidth | PdfRegionKind::Code if !two_columns => full_width.push(line),
            PdfRegionKind::FullWidth => full_width.push(line),
            PdfRegionKind::Code | PdfRegionKind::BodyColumn => column_lines.push(line),
        }
    }

    let mut regions = Vec::new();
    if !header.is_empty() {
        regions.push(PdfLayoutRegion {
            kind: PdfRegionKind::Header,
            lines: sorted_region_lines(header),
        });
    }
    regions.extend(build_pdfium_content_regions(
        sorted_region_lines(full_width),
        sorted_region_lines(column_lines),
        page_width,
    ));
    if !footnote.is_empty() {
        regions.push(PdfLayoutRegion {
            kind: PdfRegionKind::Footnote,
            lines: sorted_region_lines(footnote),
        });
    }
    if !footer.is_empty() {
        regions.push(PdfLayoutRegion {
            kind: PdfRegionKind::Footer,
            lines: sorted_region_lines(footer),
        });
    }
    PdfLayoutRegions {
        two_column_candidate: two_columns,
        regions,
    }
}

fn build_pdfium_content_regions(
    full_width: Vec<PdfLineDraft>,
    mut column_lines: Vec<PdfLineDraft>,
    page_width: f64,
) -> Vec<PdfLayoutRegion> {
    if full_width.is_empty() {
        return column_regions_for_band(column_lines, page_width);
    }
    if column_lines.is_empty() {
        return vec![PdfLayoutRegion {
            kind: PdfRegionKind::FullWidth,
            lines: sorted_region_lines(full_width),
        }];
    }

    let mut regions = Vec::new();
    let mut band_start = f64::NEG_INFINITY;
    for full_line in full_width {
        let mut before = Vec::new();
        let mut after = Vec::new();
        for line in column_lines {
            if line.y1 >= band_start && line.y2 < full_line.y1 - 2.0 {
                before.push(line);
            } else {
                after.push(line);
            }
        }
        regions.extend(column_regions_for_band(before, page_width));
        regions.push(PdfLayoutRegion {
            kind: PdfRegionKind::FullWidth,
            lines: vec![full_line],
        });
        band_start = regions
            .last()
            .and_then(|region| region.lines.first())
            .map(|line| line.y2 + 2.0)
            .unwrap_or(band_start);
        column_lines = after;
    }
    regions.extend(column_regions_for_band(column_lines, page_width));
    regions
}

fn column_regions_for_band(lines: Vec<PdfLineDraft>, page_width: f64) -> Vec<PdfLayoutRegion> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut left = Vec::new();
    let mut right = Vec::new();
    for line in lines {
        if line_center(&line) < page_width * 0.5 {
            left.push(line);
        } else {
            right.push(line);
        }
    }
    let mut regions = Vec::new();
    if !left.is_empty() {
        regions.push(PdfLayoutRegion {
            kind: PdfRegionKind::BodyColumn,
            lines: sorted_region_lines(left),
        });
    }
    if !right.is_empty() {
        regions.push(PdfLayoutRegion {
            kind: PdfRegionKind::BodyColumn,
            lines: sorted_region_lines(right),
        });
    }
    regions
}

fn sorted_region_lines(mut lines: Vec<PdfLineDraft>) -> Vec<PdfLineDraft> {
    lines.sort_by(|left, right| {
        pdfium_line_baseline_bucket(left)
            .cmp(&pdfium_line_baseline_bucket(right))
            .then_with(|| total_f64_cmp(left.x1, right.x1))
    });
    lines
}

fn detect_two_column_layout(lines: &[PdfLineDraft], page_width: f64) -> bool {
    if !page_width.is_finite() || page_width <= 0.0 {
        return false;
    }
    let mut left = 0usize;
    let mut right = 0usize;
    let mut candidates = 0usize;
    for line in lines {
        let width_ratio = (line.x2 - line.x1) / page_width;
        if width_ratio > 0.56 {
            continue;
        }
        if line.y1 < 28.0 {
            continue;
        }
        candidates += 1;
        if line_center(line) < page_width * 0.48 {
            left += 1;
        } else if line_center(line) > page_width * 0.52 {
            right += 1;
        }
    }
    (candidates >= 8 && left >= 3 && right >= 3)
        || (candidates >= 5 && left >= 3 && right >= 1)
        || (candidates >= 4 && left >= 2 && right >= 2)
}

fn classify_line_region_kind(
    line: &PdfLineDraft,
    page_width: f64,
    page_height: f64,
    two_columns: bool,
) -> PdfRegionKind {
    if page_height.is_finite() && page_height > 0.0 {
        if line.y1 <= page_height * 0.035 && is_repeated_page_noise_like(&line.text) {
            return PdfRegionKind::Header;
        }
        if line.y2 >= page_height * 0.955 && is_repeated_page_noise_like(&line.text) {
            return PdfRegionKind::Footer;
        }
        if line.y1 >= page_height * 0.875 && is_footnote_like_text(&line.text) {
            return PdfRegionKind::Footnote;
        }
    }
    if is_formula_like_text(&line.text) {
        if !two_columns {
            return PdfRegionKind::FullWidth;
        }
        let width_ratio = (line.x2 - line.x1) / page_width.max(1.0);
        let crosses_center = line.x1 < page_width * 0.40 && line.x2 > page_width * 0.60;
        if width_ratio >= 0.62 || crosses_center {
            return PdfRegionKind::FullWidth;
        }
        return PdfRegionKind::BodyColumn;
    }
    if is_code_like_text(&line.text) {
        return PdfRegionKind::Code;
    }
    if !two_columns {
        return PdfRegionKind::FullWidth;
    }
    let width_ratio = (line.x2 - line.x1) / page_width.max(1.0);
    let crosses_center = line.x1 < page_width * 0.40 && line.x2 > page_width * 0.60;
    if width_ratio >= 0.62 || crosses_center {
        PdfRegionKind::FullWidth
    } else {
        PdfRegionKind::BodyColumn
    }
}

fn group_pdfium_region_lines_into_paragraphs(
    region: PdfLayoutRegion,
    region_index: u32,
    median_height: f64,
    page_width: f64,
) -> Vec<PdfParagraphDraft> {
    let line_spacing = estimate_pdfium_line_spacing(&region.lines, median_height, page_width);
    let mut paragraphs: Vec<PdfParagraphDraft> = Vec::new();
    for line in region.lines {
        let line_role = classify_line_block_role(&line, region.kind);
        if let Some(last) = paragraphs.last_mut() {
            if last.role == line_role
                && line_role != PdfBlockRole::Reference
                && should_merge_pdfium_line(&line, last, median_height, line_spacing, page_width)
            {
                append_pdfium_line_to_paragraph(last, line);
                continue;
            }
        }
        paragraphs.push(PdfParagraphDraft {
            text: line.text.clone(),
            x1: line.x1,
            y1: line.y1,
            x2: line.x2,
            y2: line.y2,
            height: line.height,
            role: line_role,
            region_index,
            lines: vec![line],
        });
    }
    paragraphs
}

fn classify_line_block_role(line: &PdfLineDraft, region_kind: PdfRegionKind) -> PdfBlockRole {
    match region_kind {
        PdfRegionKind::Header => return PdfBlockRole::Header,
        PdfRegionKind::Footer => return PdfBlockRole::Footer,
        PdfRegionKind::Footnote => return PdfBlockRole::Footnote,
        PdfRegionKind::Code => return PdfBlockRole::Code,
        PdfRegionKind::FullWidth | PdfRegionKind::BodyColumn => {}
    }
    let text = line.text.trim();
    let lower = text.to_lowercase();
    if lower.starts_with("figure ")
        || lower.starts_with("fig. ")
        || lower.starts_with("table ")
        || lower.starts_with("algorithm ")
    {
        return PdfBlockRole::Caption;
    }
    if is_formula_like_text(text) {
        return PdfBlockRole::Formula;
    }
    if is_code_like_text(text) {
        return PdfBlockRole::Code;
    }
    if is_reference_like_text(text) {
        return PdfBlockRole::Reference;
    }
    if is_pdf_heading_like_text(text) {
        return PdfBlockRole::Heading;
    }
    PdfBlockRole::Body
}

fn repair_split_section_heading_paragraphs(
    paragraphs: Vec<PdfParagraphDraft>,
) -> Vec<PdfParagraphDraft> {
    let mut output: Vec<PdfParagraphDraft> = Vec::new();
    let mut iter = paragraphs.into_iter().peekable();

    while let Some(mut paragraph) = iter.next() {
        if paragraph
            .lines
            .first()
            .is_some_and(|line| is_standalone_section_title_fragment(&line.text))
        {
            if let Some(previous) = output.last_mut() {
                if compatible_region_indexes(previous.region_index, paragraph.region_index) {
                    if let Some(number_line) = strip_trailing_section_number_fragment(previous) {
                        let region_index =
                            merged_region_index(previous.region_index, paragraph.region_index);
                        let title_line = strip_leading_section_heading_fragment(&mut paragraph)
                            .expect("checked leading title line");
                        let heading =
                            build_section_heading_from_lines(number_line, title_line, region_index);
                        if previous.lines.is_empty() {
                            output.pop();
                        }
                        output.push(heading);
                        if !paragraph.lines.is_empty() {
                            output.push(paragraph);
                        }
                        continue;
                    }
                }
            }
        }

        if is_standalone_section_number(&paragraph.text) {
            if let Some(previous) = output.last_mut() {
                if compatible_region_indexes(previous.region_index, paragraph.region_index) {
                    if let Some(title_line) = strip_trailing_section_heading_fragment(previous) {
                        let region_index =
                            merged_region_index(previous.region_index, paragraph.region_index);
                        let heading =
                            build_section_heading_paragraph(paragraph, title_line, region_index);
                        if previous.lines.is_empty() {
                            output.pop();
                        }
                        output.push(heading);
                        continue;
                    }
                }
            }

            if let Some(next) = iter.peek() {
                if is_standalone_section_title_fragment(&next.text)
                    && compatible_region_indexes(paragraph.region_index, next.region_index)
                {
                    let title = iter.next().expect("peeked title paragraph");
                    output.push(build_section_heading_from_paragraphs(paragraph, title));
                    continue;
                }
            }
        }

        output.push(paragraph);
    }

    output
}

fn strip_trailing_section_heading_fragment(
    paragraph: &mut PdfParagraphDraft,
) -> Option<PdfLineDraft> {
    let title_line = paragraph.lines.last()?;
    if !is_standalone_section_title_fragment(&title_line.text) {
        return None;
    }

    let title_line = paragraph.lines.pop()?;
    if paragraph.lines.is_empty() {
        paragraph.text.clear();
        return Some(title_line);
    }
    rebuild_paragraph_geometry(paragraph);
    Some(title_line)
}

fn strip_trailing_section_number_fragment(
    paragraph: &mut PdfParagraphDraft,
) -> Option<PdfLineDraft> {
    let number_line = paragraph.lines.last()?;
    if !is_standalone_section_number(&number_line.text) {
        return None;
    }

    let number_line = paragraph.lines.pop()?;
    if paragraph.lines.is_empty() {
        paragraph.text.clear();
        return Some(number_line);
    }
    rebuild_paragraph_geometry(paragraph);
    Some(number_line)
}

fn strip_leading_section_heading_fragment(
    paragraph: &mut PdfParagraphDraft,
) -> Option<PdfLineDraft> {
    let title_line = paragraph.lines.first()?;
    if !is_standalone_section_title_fragment(&title_line.text) {
        return None;
    }

    let title_line = paragraph.lines.remove(0);
    if paragraph.lines.is_empty() {
        paragraph.text.clear();
        return Some(title_line);
    }
    rebuild_paragraph_geometry(paragraph);
    Some(title_line)
}

fn build_section_heading_paragraph(
    number_paragraph: PdfParagraphDraft,
    title_line: PdfLineDraft,
    region_index: u32,
) -> PdfParagraphDraft {
    let text = format!(
        "{} {}",
        number_paragraph.text.trim(),
        title_line.text.trim()
    );
    let mut lines = number_paragraph.lines;
    lines.push(title_line);
    build_paragraph_from_lines(text, PdfBlockRole::Heading, region_index, lines)
}

fn build_section_heading_from_lines(
    number_line: PdfLineDraft,
    title_line: PdfLineDraft,
    region_index: u32,
) -> PdfParagraphDraft {
    let text = format!("{} {}", number_line.text.trim(), title_line.text.trim());
    build_paragraph_from_lines(
        text,
        PdfBlockRole::Heading,
        region_index,
        vec![number_line, title_line],
    )
}

fn build_section_heading_from_paragraphs(
    number_paragraph: PdfParagraphDraft,
    title_paragraph: PdfParagraphDraft,
) -> PdfParagraphDraft {
    let text = format!(
        "{} {}",
        number_paragraph.text.trim(),
        title_paragraph.text.trim()
    );
    let region_index =
        merged_region_index(number_paragraph.region_index, title_paragraph.region_index);
    let mut lines = number_paragraph.lines;
    lines.extend(title_paragraph.lines);
    build_paragraph_from_lines(text, PdfBlockRole::Heading, region_index, lines)
}

fn compatible_region_indexes(left: u32, right: u32) -> bool {
    left == 0 || right == 0 || left == right
}

fn merged_region_index(left: u32, right: u32) -> u32 {
    if left == right {
        left
    } else if left == 0 {
        right
    } else if right == 0 {
        left
    } else {
        0
    }
}

fn build_paragraph_from_lines(
    text: String,
    role: PdfBlockRole,
    region_index: u32,
    lines: Vec<PdfLineDraft>,
) -> PdfParagraphDraft {
    let (x1, y1, x2, y2, height) = paragraph_geometry_from_lines(&lines);
    PdfParagraphDraft {
        text,
        x1,
        y1,
        x2,
        y2,
        height,
        role,
        region_index,
        lines,
    }
}

fn rebuild_paragraph_geometry(paragraph: &mut PdfParagraphDraft) {
    paragraph.text = paragraph
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let (x1, y1, x2, y2, height) = paragraph_geometry_from_lines(&paragraph.lines);
    paragraph.x1 = x1;
    paragraph.y1 = y1;
    paragraph.x2 = x2;
    paragraph.y2 = y2;
    paragraph.height = height;
}

fn paragraph_geometry_from_lines(lines: &[PdfLineDraft]) -> (f64, f64, f64, f64, f64) {
    lines.iter().fold(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
            0.0_f64,
        ),
        |(x1, y1, x2, y2, height), line| {
            (
                x1.min(line.x1),
                y1.min(line.y1),
                x2.max(line.x2),
                y2.max(line.y2),
                height.max(line.height),
            )
        },
    )
}

fn line_center(line: &PdfLineDraft) -> f64 {
    (line.x1 + line.x2) / 2.0
}

fn pdfium_line_baseline_bucket(line: &PdfLineDraft) -> i64 {
    (line.y1 / 6.0).round() as i64
}

fn is_repeated_page_noise_like(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed
        .chars()
        .all(|ch| ch.is_ascii_digit() || ch.is_whitespace())
    {
        return true;
    }
    trimmed.chars().count() <= 24 && trimmed.chars().filter(|ch| ch.is_alphabetic()).count() <= 16
}

fn is_pdf_heading_like_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() > 96 || trimmed.contains('.') && trimmed.split_whitespace().count() > 8 {
        return false;
    }
    if trimmed.chars().all(|ch| {
        ch.is_uppercase() || ch.is_ascii_digit() || ch.is_whitespace() || ".:-".contains(ch)
    }) && trimmed.chars().any(|ch| ch.is_alphabetic())
    {
        return true;
    }
    let lower = trimmed.to_lowercase();
    matches!(
        lower.as_str(),
        "abstract"
            | "introduction"
            | "related work"
            | "background"
            | "method"
            | "methods"
            | "approach"
            | "experiments"
            | "results"
            | "conclusion"
            | "references"
    ) || starts_with_section_number(trimmed)
}

fn is_standalone_section_number(text: &str) -> bool {
    let trimmed = text.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        return false;
    }
    let mut dot_count = 0usize;
    let mut digit_count = 0usize;
    for ch in trimmed.chars() {
        if ch.is_ascii_digit() {
            digit_count += 1;
        } else if ch == '.' {
            dot_count += 1;
        } else {
            return false;
        }
    }
    digit_count >= 1 && dot_count >= 1
}

fn is_standalone_section_title_fragment(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 4 || trimmed.len() > 80 {
        return false;
    }
    if trimmed.ends_with('.') || trimmed.ends_with(':') || trimmed.ends_with(';') {
        return false;
    }
    let lower = trimmed.to_lowercase();
    if lower.starts_with("figure ")
        || lower.starts_with("fig. ")
        || lower.starts_with("table ")
        || lower.starts_with("algorithm ")
    {
        return false;
    }
    let words = trimmed
        .split_whitespace()
        .filter(|word| word.chars().any(|ch| ch.is_alphabetic()))
        .collect::<Vec<_>>();
    if !(2..=8).contains(&words.len()) {
        return false;
    }
    let title_like = words
        .iter()
        .filter(|word| {
            word.chars()
                .find(|ch| ch.is_alphabetic())
                .map(|ch| ch.is_uppercase())
                .unwrap_or(false)
        })
        .count();
    title_like * 2 >= words.len()
}

fn starts_with_section_number(text: &str) -> bool {
    let mut chars = text.chars().peekable();
    let mut saw_digit = false;
    while matches!(chars.peek(), Some(ch) if ch.is_ascii_digit()) {
        saw_digit = true;
        chars.next();
    }
    if !saw_digit {
        return false;
    }
    while matches!(chars.peek(), Some('.')) {
        chars.next();
        let mut part_digit = false;
        while matches!(chars.peek(), Some(ch) if ch.is_ascii_digit()) {
            part_digit = true;
            chars.next();
        }
        if !part_digit {
            return false;
        }
    }
    matches!(chars.peek(), Some(ch) if ch.is_whitespace())
}

fn is_formula_like_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() > 160 {
        return false;
    }
    let math_symbols = trimmed
        .chars()
        .filter(|ch| "=∑∏∫√≤≥≠≈±×÷λθαβγδμσ∞→←".contains(*ch))
        .count();
    math_symbols >= 2 && trimmed.chars().filter(|ch| ch.is_alphabetic()).count() <= 32
}

fn is_footnote_like_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 8 || trimmed.len() > 220 {
        return false;
    }
    let mut chars = trimmed.chars();
    let starts_with_marker =
        matches!(chars.next(), Some(ch) if ch.is_ascii_digit() || ch == '*' || ch == '†');
    starts_with_marker
        && trimmed.chars().filter(|ch| ch.is_alphabetic()).count() >= 8
        && !is_formula_like_text(trimmed)
}

fn is_reference_like_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 12 {
        return false;
    }
    if starts_with_section_number(trimmed) {
        return false;
    }
    if trimmed.starts_with('[') {
        let mut chars = trimmed.chars();
        chars.next();
        let mut saw_digit = false;
        while matches!(chars.clone().next(), Some(ch) if ch.is_ascii_digit()) {
            saw_digit = true;
            chars.next();
        }
        if saw_digit && matches!(chars.next(), Some(']')) {
            return true;
        }
    }
    trimmed.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        && trimmed.contains('.')
        && trimmed.chars().filter(|ch| ch.is_alphabetic()).count() >= 12
}

fn is_code_like_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let lower = trimmed.to_lowercase();
    if lower.starts_with("def ")
        || lower.starts_with("class ")
        || lower.starts_with("return ")
        || lower.starts_with("import ")
        || lower.starts_with("from ")
        || lower.starts_with("if ")
        || lower.starts_with("for ")
        || lower.starts_with("while ")
        || lower.starts_with("fn ")
        || lower.starts_with("let ")
        || lower.starts_with("const ")
    {
        return true;
    }
    let symbol_count = trimmed
        .chars()
        .filter(|ch| "{}[]();=<>_#".contains(*ch))
        .count();
    let slash_count = trimmed
        .chars()
        .filter(|ch| matches!(ch, '/' | '\\'))
        .count();
    let alpha_count = trimmed.chars().filter(|ch| ch.is_alphabetic()).count();
    (symbol_count >= 4 && alpha_count >= 2)
        || (trimmed.contains("()") && symbol_count >= 2)
        || (slash_count >= 2 && symbol_count >= 2)
}

fn should_merge_pdfium_line(
    line: &PdfLineDraft,
    paragraph: &PdfParagraphDraft,
    median_height: f64,
    line_spacing: (f64, f64),
    page_width: f64,
) -> bool {
    let Some(previous) = paragraph.lines.last() else {
        return false;
    };
    let vertical_gap = line.y1 - previous.y2;
    let top_delta = line.y1 - previous.y1;
    let line_height = median_height.max(line.height).max(previous.height);
    if vertical_gap > (line_spacing.0 * 2.2).max(line_height * 0.65) {
        return false;
    }
    if top_delta > (line_spacing.1 * 1.38).max(line_height * 1.62) {
        return false;
    }
    let height_ratio =
        line.height.max(previous.height) / 1.0_f64.max(line.height.min(previous.height));
    if height_ratio > 1.35 {
        return false;
    }
    is_likely_same_pdfium_column_line(line, previous, line_height, page_width)
}

fn estimate_pdfium_line_spacing(
    lines: &[PdfLineDraft],
    median_height: f64,
    page_width: f64,
) -> (f64, f64) {
    let mut gaps = Vec::new();
    let mut pitches = Vec::new();
    for index in 1..lines.len() {
        let line = &lines[index];
        for previous in lines[..index].iter().rev() {
            let vertical_gap = line.y1 - previous.y2;
            if vertical_gap < -4.0_f64.max(median_height * 0.35) {
                continue;
            }
            let pitch = line.y1 - previous.y1;
            if pitch > 32.0_f64.max(median_height * 2.2) {
                break;
            }
            if !is_likely_same_pdfium_column_line(line, previous, median_height, page_width) {
                continue;
            }
            gaps.push(vertical_gap.max(0.0));
            pitches.push(pitch.max(0.0));
            break;
        }
    }
    (
        median_f64(gaps.into_iter()).unwrap_or_else(|| 2.0_f64.max(median_height * 0.18)),
        median_f64(pitches.into_iter()).unwrap_or_else(|| 8.0_f64.max(median_height * 1.18)),
    )
}

fn is_likely_same_pdfium_column_line(
    line: &PdfLineDraft,
    previous: &PdfLineDraft,
    line_height: f64,
    page_width: f64,
) -> bool {
    let overlap = horizontal_overlap_ratio(line.x1, line.x2, previous.x1, previous.x2);
    let left_delta = (line.x1 - previous.x1).abs();
    let center_delta = ((line.x1 + line.x2) - (previous.x1 + previous.x2)).abs() / 2.0;
    let same_column_width =
        (page_width * 0.42).min((line.x2 - line.x1).max(previous.x2 - previous.x1) * 1.25);
    overlap >= 0.35
        || (left_delta <= 10.0_f64.max(line_height * 1.3)
            && center_delta <= same_column_width * 0.35)
}

fn append_pdfium_line_to_paragraph(paragraph: &mut PdfParagraphDraft, line: PdfLineDraft) {
    paragraph.text = format!("{}\n{}", paragraph.text, line.text)
        .trim()
        .to_string();
    paragraph.x1 = paragraph.x1.min(line.x1);
    paragraph.y1 = paragraph.y1.min(line.y1);
    paragraph.x2 = paragraph.x2.max(line.x2);
    paragraph.y2 = paragraph.y2.max(line.y2);
    paragraph.height = paragraph.height.max(line.height);
    paragraph.lines.push(line);
}

fn horizontal_overlap_ratio(left_x1: f64, left_x2: f64, right_x1: f64, right_x2: f64) -> f64 {
    let overlap = left_x2.min(right_x2) - left_x1.max(right_x1);
    if overlap <= 0.0 {
        return 0.0;
    }
    let min_width = (left_x2 - left_x1).min(right_x2 - right_x1);
    if min_width > 0.0 {
        overlap / min_width
    } else {
        0.0
    }
}

fn median_f64(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut values = values.filter(|value| value.is_finite()).collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    values.sort_by(|left, right| total_f64_cmp(*left, *right));
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[middle - 1] + values[middle]) / 2.0)
    } else {
        Some(values[middle])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn debug_line(
        line_no: u32,
        block_index: u32,
        text: &str,
        x: f64,
        y: f64,
    ) -> PdfLayoutDebugLine {
        PdfLayoutDebugLine {
            line_no,
            block_index,
            text: text.to_string(),
            x1: x,
            y1: y,
            x2: x + 220.0,
            y2: y + 8.0,
        }
    }

    #[test]
    fn debug_layout_reports_two_column_regions() {
        let debug = debug_pdfium_layout_from_lines(
            vec![
                debug_line(1, 1, "left line one", 50.0, 100.0),
                debug_line(2, 1, "left line two", 50.0, 112.0),
                debug_line(3, 2, "right line one", 320.0, 100.0),
                debug_line(4, 2, "right line two", 320.0, 112.0),
            ],
            600.0,
            800.0,
        );

        assert!(debug.two_column_detected);
        assert_eq!(debug.regions.len(), 2);
        assert_eq!(debug.regions[0].kind, "body_column");
        assert_eq!(debug.regions[0].line_numbers, vec![1, 2]);
        assert_eq!(debug.regions[1].line_numbers, vec![3, 4]);
        assert_eq!(
            debug.regions[0].bbox,
            [50.0 / 600.0, 100.0 / 800.0, 270.0 / 600.0, 120.0 / 800.0]
        );
        assert_eq!(debug.regions[0].absolute_bbox, [50.0, 100.0, 270.0, 120.0]);
        assert!(debug.confidence >= 0.9);
    }

    #[test]
    fn debug_layout_reports_invalid_input_warning() {
        let debug = debug_pdfium_layout_from_lines(Vec::new(), 0.0, 800.0);

        assert!(debug.confidence < 0.5);
        assert!(debug
            .warnings
            .iter()
            .any(|warning| warning.kind == "invalid_page_size"));
        assert!(debug
            .warnings
            .iter()
            .any(|warning| warning.kind == "no_indexed_lines"));
    }
}
