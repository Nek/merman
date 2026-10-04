use super::{Bounds, NativeFontContext};
use merman_render::environment::{
    HostMeasurementResult, HostTextMeasurement, HostTextMeasurementError,
    HostTextMeasurementRequest, HostTextMeasurer, MeasurementProfileId,
    TextMeasurementOperation as Op, TextMeasurementPhase, TextMeasurementPolicy,
    TextMeasurementProfileIdentity,
};
use merman_render::text::{
    DeterministicTextMeasurer, TextMetrics, TextStyle, WrapMode, wrap_text_lines_with_width,
};
use std::sync::{Arc, Mutex};

impl NativeFontContext {
    /// Routes the operation's layout, wrapping and SVG measurements through the resolved fonts.
    /// A host must reject fallback provenance before publishing font-accurate output and use the
    /// same context for final glyphs. Installing measurements alone does not make SVG text portable.
    pub fn measurement_policy(self: Arc<Self>) -> TextMeasurementPolicy {
        TextMeasurementPolicy::host_display(
            TextMeasurementProfileIdentity::new(
                MeasurementProfileId::new("native-glyph").expect("static profile id"),
                "1",
            )
            .expect("static profile identity"),
            self,
            TextMeasurementPhase::ALL,
        )
    }

    fn run_bounds(&self, text: &str, style: &TextStyle) -> Result<(Bounds, Bounds), String> {
        // SVG text whitespace normalization happens before shaping an individual run.
        let text = text.replace(['\r', '\n', '\t'], " ");
        let shaped = self.shape(&text, style)?;
        let zero = Bounds {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        let logical = shaped.bounds.unwrap_or(zero);
        let bounds = if let Some(ink) = shaped.ink_bounds {
            Bounds {
                left: logical.left.min(ink.left),
                top: logical.top.min(ink.top),
                right: logical.right.max(ink.right),
                bottom: logical.bottom.max(ink.bottom),
            }
        } else {
            logical
        };
        Ok((logical, bounds))
    }

    fn wrapped_metrics(
        &self,
        request: HostTextMeasurementRequest<'_>,
    ) -> Result<(TextMetrics, f64), String> {
        let failure = Mutex::new(None);
        let width = |text: &str, style: &TextStyle| match self.run_bounds(text, style) {
            Ok((_, bounds)) => bounds.right - bounds.left,
            Err(error) => {
                // The shared wrapper has an infallible width callback. Discard its entire result
                // on any font error; the host route records the failure, never a fabricated width.
                *failure.lock().expect("local font failure recorder") = Some(error);
                0.0
            }
        };
        let rows = wrap_text_lines_with_width(
            request.text,
            request.style,
            request.max_width,
            request.wrap_mode,
            &width,
        );
        if let Some(error) = failure
            .into_inner()
            .map_err(|_| "Font failure recorder poisoned")?
        {
            return Err(error);
        }
        let mut widest: f64 = 0.0;
        let mut top = f64::INFINITY;
        let mut bottom = f64::NEG_INFINITY;
        for (i, row) in rows.iter().enumerate() {
            let (_, bounds) = self.run_bounds(row, request.style)?;
            widest = widest.max(bounds.right - bounds.left);
            if bounds.bottom > bounds.top {
                let baseline = i as f64 * request.style.font_size * 1.1;
                top = top.min(baseline + bounds.top);
                bottom = bottom.max(baseline + bounds.bottom);
            }
        }
        let mut raw_width: f64 = 0.0;
        for row in DeterministicTextMeasurer::normalized_text_lines(request.text) {
            let (_, bounds) = self.run_bounds(&row, request.style)?;
            raw_width = raw_width.max(bounds.right - bounds.left);
        }
        let height = if request.wrap_mode == WrapMode::HtmlLike {
            if let Some(limit) = request.max_width.filter(|w| w.is_finite() && *w > 0.0) {
                if raw_width > limit {
                    widest = widest.max(limit);
                }
            }
            // Authored HTML line-box spacing is separate from the glyph ink stored by shape().
            request.style.font_size * 1.5 * rows.len() as f64
        } else if top.is_finite() {
            bottom - top
        } else {
            0.0
        };
        Ok((
            TextMetrics {
                width: widest,
                height,
                line_count: rows.len(),
            },
            raw_width,
        ))
    }
}

impl HostTextMeasurer for NativeFontContext {
    fn measure(&self, request: HostTextMeasurementRequest<'_>) -> HostMeasurementResult {
        use HostTextMeasurement::{HorizontalExtents, Length, Metrics, WrappedWithRawWidth};
        if request.operation == Op::CreateTextMiddleBBoxYOffset {
            // Architecture's inherited middle baseline needs its own measured glyph projection.
            // It is not interchangeable with the formatted flowchart baseline.
            return Ok(None);
        }
        if matches!(
            request.operation,
            Op::Measure | Op::Wrapped | Op::WrappedWithRawWidth
        ) {
            let (metrics, raw_width) = self
                .wrapped_metrics(request)
                .map_err(HostTextMeasurementError::new)?;
            return Ok(Some(if request.operation == Op::WrappedWithRawWidth {
                WrappedWithRawWidth {
                    metrics,
                    raw_width: (request.wrap_mode == WrapMode::HtmlLike).then_some(raw_width),
                }
            } else {
                Metrics(metrics)
            }));
        }
        let (logical, bounds) = self
            .run_bounds(request.text, request.style)
            .map_err(HostTextMeasurementError::new)?;
        let advance = logical.right - logical.left;
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        Ok(Some(match request.operation {
            Op::ComputedLength | Op::CanvasMeasureTextWidth => Length(advance),
            Op::BBoxX | Op::BBoxXWithAsciiOverhang | Op::TitleBBoxX => HorizontalExtents {
                left: advance * 0.5 - bounds.left,
                right: bounds.right - advance * 0.5,
            },
            Op::SimpleBBoxWidth
            | Op::RawBBoxWidth
            | Op::TspanBBoxWidth
            | Op::WrapProbeBBoxWidth
            | Op::BoundingClientRectWidth => Length(width),
            Op::TspanBBoxHeight | Op::SimpleBBoxHeight | Op::RawBBoxHeight => Length(height),
            Op::CreateTextBBoxYOffset => Length(request.style.font_size + bounds.top),
            Op::MermaidCalculateTextDimensions => Metrics(TextMetrics {
                width,
                height,
                line_count: 1,
            }),
            Op::Measure
            | Op::Wrapped
            | Op::WrappedWithRawWidth
            | Op::CreateTextMiddleBBoxYOffset => unreachable!("handled above"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatted_baseline_offset_matches_emitted_svg_font_bounds() {
        let fonts = NativeFontContext::system().unwrap();
        for variant in ["normal", "italic"] {
            let style = TextStyle {
                font_family: Some("Arial".into()),
                font_style: Some(variant.into()),
                font_size: 24.0,
                ..Default::default()
            };
            let source = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><text y="-10.1" font-family="Arial" font-size="24" font-style="{variant}"><tspan x="0" y="-0.1em" dy="1.1em">AV fj</tspan></text></svg>"#
            );
            let tree = usvg::Tree::from_str(
                &source,
                &usvg::Options {
                    fontdb: Arc::clone(&fonts.fontdb),
                    ..Default::default()
                },
            )
            .unwrap();
            let usvg::Node::Text(text) = &tree.root().children()[0] else {
                panic!("expected text");
            };
            let expected = text
                .bounding_box()
                .top()
                .min(text.flattened().bounding_box().top()) as f64;
            let result = HostTextMeasurer::measure(
                &fonts,
                HostTextMeasurementRequest {
                    operation: Op::CreateTextBBoxYOffset,
                    phase: TextMeasurementPhase::SvgBBox,
                    text: "AV fj",
                    style: &style,
                    max_width: None,
                    wrap_mode: WrapMode::SvgLike,
                },
            )
            .unwrap()
            .unwrap();
            let HostTextMeasurement::Length(actual) = result else {
                panic!("expected offset");
            };
            assert!(
                (actual - expected).abs() < 0.001,
                "{variant}: {actual} != {expected}"
            );
        }
    }
}
