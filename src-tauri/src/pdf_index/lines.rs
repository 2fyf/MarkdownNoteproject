use super::{normalize_inline_text, total_f64_cmp, PdfLineDraft, PdfTextRect};

pub(super) fn sort_pdfium_text_rects(mut rects: Vec<PdfTextRect>) -> Vec<PdfTextRect> {
    rects.sort_by(|left, right| {
        pdfium_baseline_bucket(left)
            .cmp(&pdfium_baseline_bucket(right))
            .then_with(|| total_f64_cmp(left.x, right.x))
            .then_with(|| total_f64_cmp(left.y, right.y))
            .then_with(|| left.source_order.cmp(&right.source_order))
    });
    rects
}

fn pdfium_baseline_bucket(rect: &PdfTextRect) -> i64 {
    (rect.y / 6.0).round() as i64
}

pub(super) fn build_pdfium_lines(rects: Vec<PdfTextRect>, page_width: f64) -> Vec<PdfLineDraft> {
    let mut lines: Vec<PdfLineDraft> = Vec::new();
    for rect in rects {
        if let Some(last) = lines.last_mut() {
            let same_baseline = (last.y1 - rect.y).abs() <= 4.0_f64.max(rect.height * 0.45);
            let inline_gap = if same_baseline { rect.x - last.x2 } else { 0.0 };
            let same_column_run = same_baseline
                && is_likely_same_pdfium_text_run(last, &rect, inline_gap, page_width);
            if same_column_run {
                last.text = normalize_inline_text(&format!("{} {}", last.text, rect.text));
                last.x1 = last.x1.min(rect.x);
                last.y1 = last.y1.min(rect.y);
                last.x2 = last.x2.max(rect.x + rect.width);
                last.y2 = last.y2.max(rect.y + rect.height);
                last.height = last.height.max(rect.height);
                last.rects.push(rect);
                continue;
            }
        }
        lines.push(PdfLineDraft {
            line_no: lines.len() as u32 + 1,
            text: rect.text.clone(),
            x1: rect.x,
            y1: rect.y,
            x2: rect.x + rect.width,
            y2: rect.y + rect.height,
            height: rect.height,
            rects: vec![rect],
        });
    }
    lines
}

fn is_likely_same_pdfium_text_run(
    line: &PdfLineDraft,
    rect: &PdfTextRect,
    inline_gap: f64,
    page_width: f64,
) -> bool {
    if inline_gap < -1.0 {
        return false;
    }
    if is_likely_pdfium_column_jump(line, rect, page_width) {
        return false;
    }
    inline_gap <= 24.0_f64.max(rect.height * 1.8)
}

fn is_likely_pdfium_column_jump(line: &PdfLineDraft, rect: &PdfTextRect, page_width: f64) -> bool {
    if !page_width.is_finite() || page_width <= 0.0 {
        return false;
    }
    let line_width = line.x2 - line.x1;
    let rect_width = rect.width;
    let line_crosses_left = line.x1 < page_width * 0.40 && line.x2 <= page_width * 0.56;
    let rect_crosses_right = rect.x >= page_width * 0.44 && rect.x + rect.width > page_width * 0.58;
    line_crosses_left
        && rect_crosses_right
        && line_width >= page_width * 0.18
        && rect_width >= page_width * 0.18
}
