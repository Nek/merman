use super::cascade::ResolvedFallbackTypography;
use crate::svg::pipeline::{escape_xml_attr_with_checkpoints, escape_xml_text_with_checkpoints};
use crate::text::{TextMeasurer, layout_resolved_text};
use std::fmt::Write;

// Keep the same line plan for font measurement and editable SVG text emission.
pub(super) fn render<E>(
    typography: &ResolvedFallbackTypography,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    wrap_width: Option<f64>,
    measurer: &dyn TextMeasurer,
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<Option<(String, usize)>, E> {
    let Some(first) = typography.lines.iter().flatten().next() else {
        return Ok(None);
    };
    if typography
        .lines
        .iter()
        .flatten()
        .all(|run| run.text.same_typography(&first.text) && run.fill == first.fill)
    {
        return Ok(None);
    }
    let mut plans = Vec::new();
    let mut total_height = 0.0;
    for line in &typography.lines {
        checkpoint()?;
        let runs = line.iter().map(|run| run.text.clone()).collect::<Vec<_>>();
        let plan = layout_resolved_text(&runs, wrap_width, measurer, checkpoint)?;
        total_height += plan.height;
        plans.push(plan);
    }
    let mut out = String::new();
    let mut count = 0;
    let mut top = y + (height - total_height) / 2.0;
    for (runs, plan) in typography.lines.iter().zip(plans) {
        let (markup, elements) = render_plan(runs, &plan, x, top, width, checkpoint)?;
        out.push_str(&markup);
        count += elements;
        top += plan.height;
    }
    Ok(Some((out, count)))
}

pub(super) fn render_plan<E>(
    runs: &[super::cascade::ResolvedHtmlRun],
    plan: &crate::text::ResolvedTextLayout,
    x: f64,
    top: f64,
    width: f64,
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<(String, usize), E> {
    let mut out = String::new();
    let mut count = 0;
    for line in &plan.lines {
        checkpoint()?;
        let left = x + (width - line.width) / 2.0 - line.left;
        let _ = write!(
            out,
            r#"<text x="{left}" y="{}" text-anchor="start" xml:space="preserve" class="merman-foreignobject-fallback-text">"#,
            top + line.baseline
        );
        count += 1;
        // A word boundary does not create a typography boundary.
        let mut i = 0;
        while i < line.fragments.len() {
            checkpoint()?;
            let fragment = line.fragments[i];
            let run = &runs[fragment.run_index];
            let mut end = i + 1;
            while end < line.fragments.len() && line.fragments[end].run_index == fragment.run_index
            {
                end += 1;
            }
            let text = escape_xml_text_with_checkpoints(
                &plan.text[fragment.start..line.fragments[end - 1].end],
                checkpoint,
            )?;
            let style = &run.text.style;
            let mut css = format!(
                "font-size:{}px;font-family:{};",
                style.font_size,
                style.font_family.as_deref().unwrap_or("sans-serif")
            );
            if let Some(weight) = &style.font_weight {
                let _ = write!(css, "font-weight:{weight};");
            }
            if let Some(style) = &style.font_style {
                let _ = write!(css, "font-style:{style};");
            }
            let css = escape_xml_attr_with_checkpoints(&css, checkpoint)?;
            let fill = escape_xml_attr_with_checkpoints(&run.fill, checkpoint)?;
            let _ = write!(out, r#"<tspan style="{css}" fill="{fill}">{text}</tspan>"#);
            count += 1;
            i = end;
        }
        out.push_str("</text>");
    }
    Ok((out, count))
}
