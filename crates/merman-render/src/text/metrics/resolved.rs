//! One font-aware line plan for measuring and emitting already decoded styled text.
use super::{InlineHtmlPlanningStats, InlineRunFragment, InlineTextRun, index_inline_run_breaks};
use crate::text::{TextMeasurer, TextStyle, WrapMode};

#[derive(Debug, Clone)]
pub(crate) struct ResolvedTextRun {
    pub(crate) text: String,
    pub(crate) style: TextStyle,
    pub(crate) line_height: f64,
}

impl ResolvedTextRun {
    pub(crate) fn same_typography(&self, other: &Self) -> bool {
        same_font(&self.style, &other.style) && self.line_height == other.line_height
    }
}

#[derive(Debug)]
pub(crate) struct ResolvedTextLine {
    pub(crate) fragments: Vec<InlineRunFragment>,
    pub(crate) width: f64,
    pub(crate) left: f64,
    pub(crate) baseline: f64,
}

#[derive(Debug)]
pub(crate) struct ResolvedTextLayout {
    pub(crate) text: String,
    pub(crate) lines: Vec<ResolvedTextLine>,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

fn same_font(a: &TextStyle, b: &TextStyle) -> bool {
    a.font_size == b.font_size
        && a.font_family == b.font_family
        && a.font_weight == b.font_weight
        && a.font_style == b.font_style
}

// Adjacent paint-only boundaries must not split shaping or introduce extra letter spacing.
fn line_metrics<E>(
    text: &str,
    fragments: &[InlineRunFragment],
    runs: &[ResolvedTextRun],
    measurer: &dyn TextMeasurer,
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<(f64, f64, f64, f64), E> {
    let mut advance: f64 = 0.0;
    let mut left: f64 = 0.0;
    let mut right: f64 = 0.0;
    let mut ascent: f64 = 0.0;
    let mut descent: f64 = 0.0;
    let mut i = 0;
    while i < fragments.len() {
        checkpoint()?;
        let first = fragments[i];
        let run = &runs[first.run_index];
        let mut end = i + 1;
        let mut line_height = run.line_height;
        while end < fragments.len() && same_font(&run.style, &runs[fragments[end].run_index].style)
        {
            checkpoint()?;
            line_height = line_height.max(runs[fragments[end].run_index].line_height);
            end += 1;
        }
        let value = &text[first.start..fragments[end - 1].end];
        let width = measurer.measure_svg_text_computed_length_px(value, &run.style);
        let (before, after) = measurer.measure_svg_text_bbox_x(value, &run.style);
        left = left.min(advance + width / 2.0 - before);
        right = right.max(advance + width / 2.0 + after);
        let height = measurer
            .measure_wrapped(value, &run.style, None, WrapMode::SvgLike)
            .height;
        let top = measurer.measure_svg_create_text_bbox_y_offset_px(value, &run.style)
            - run.style.font_size;
        let leading = (line_height - height) / 2.0;
        ascent = ascent.max(-top + leading);
        descent = descent.max(top + height + leading);
        advance += width;
        i = end;
    }
    checkpoint()?;
    Ok((right.max(advance) - left, left, ascent, descent))
}

pub(crate) fn layout_resolved_text<E>(
    runs: &[ResolvedTextRun],
    max_width: Option<f64>,
    measurer: &dyn TextMeasurer,
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<ResolvedTextLayout, E> {
    checkpoint()?;
    let plain_runs = runs
        .iter()
        .map(|run| InlineTextRun {
            text: run.text.clone(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let breaks = index_inline_run_breaks(&plain_runs, &mut InlineHtmlPlanningStats::default());
    checkpoint()?;
    let natural = line_metrics(
        &breaks.source,
        &breaks.fragments,
        runs,
        measurer,
        checkpoint,
    )?;
    let active_width =
        max_width.filter(|width| width.is_finite() && *width > 0.0 && natural.0 > *width);
    let mut ranges = Vec::new();
    if let Some(width) = active_width {
        let mut first = 0;
        let mut last = 0;
        for segment in &breaks.segments {
            let candidate = line_metrics(
                &breaks.source,
                &breaks.fragments[first..segment.fragment_end],
                runs,
                measurer,
                checkpoint,
            )?;
            if last > first && candidate.0 > width {
                ranges.push(first..last);
                first = segment.fragment_start;
            }
            last = segment.fragment_end;
        }
        if last > first {
            ranges.push(first..last);
        }
    } else {
        ranges.push(0..breaks.fragments.len());
    }
    let mut lines = Vec::new();
    let mut total_height = 0.0;
    let mut width: f64 = active_width.unwrap_or(0.0);
    for range in ranges {
        let (line_width, left, ascent, descent) = line_metrics(
            &breaks.source,
            &breaks.fragments[range.clone()],
            runs,
            measurer,
            checkpoint,
        )?;
        width = width.max(line_width);
        let height = ascent + descent;
        lines.push(ResolvedTextLine {
            fragments: breaks.fragments[range].to_vec(),
            width: line_width,
            left,
            baseline: total_height + ascent,
        });
        total_height += height;
    }
    Ok(ResolvedTextLayout {
        text: breaks.source,
        lines,
        width,
        height: total_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{DeterministicTextMeasurer, TextMetrics};
    use std::convert::Infallible;
    struct FontSized;
    impl TextMeasurer for FontSized {
        fn measure(&self, text: &str, style: &TextStyle) -> TextMetrics {
            TextMetrics {
                width: text.chars().count() as f64 * style.font_size,
                height: style.font_size,
                line_count: 1,
            }
        }
        fn measure_svg_text_computed_length_px(&self, text: &str, style: &TextStyle) -> f64 {
            self.measure(text, style).width
        }
        fn measure_svg_text_bbox_x(&self, text: &str, style: &TextStyle) -> (f64, f64) {
            let half = self.measure(text, style).width / 2.0;
            (half, half)
        }
    }
    #[test]
    fn resolved_fonts_share_one_wrapping_plan_and_keep_unicode_boundaries() {
        let runs = [
            ResolvedTextRun {
                text: "Wide ".into(),
                style: TextStyle {
                    font_size: 32.0,
                    ..Default::default()
                },
                line_height: 48.0,
            },
            ResolvedTextRun {
                text: "小字".into(),
                style: TextStyle {
                    font_size: 16.0,
                    ..Default::default()
                },
                line_height: 24.0,
            },
        ];
        let plan =
            layout_resolved_text(&runs, None, &FontSized, &mut || Ok::<_, Infallible>(())).unwrap();
        assert_eq!(plan.width, 192.0);
        assert_eq!(plan.lines.len(), 1);
        let plan = layout_resolved_text(&runs, Some(170.0), &FontSized, &mut || {
            Ok::<_, Infallible>(())
        })
        .unwrap();
        assert!(plan.lines.len() > 1);
        let emitted = plan
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter())
            .map(|f| &plan.text[f.start..f.end])
            .collect::<String>();
        assert_eq!(emitted, "Wide 小字");
        assert!(plan.lines.iter().all(|line| line.baseline.is_finite()));
        assert!(
            layout_resolved_text(
                &runs,
                None,
                &DeterministicTextMeasurer::default(),
                &mut || Err::<(), _>("cancelled")
            )
            .is_err()
        );
    }
}
