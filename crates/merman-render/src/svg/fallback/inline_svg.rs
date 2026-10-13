//! Preserve native inline SVG formulas while converting their HTML label shell.
use super::cascade::{ResolvedFallbackTypography, ResolvedHtmlRun};
use crate::svg::pipeline::{checkpoint_loop, escape_xml_attr_with_checkpoints};
use crate::text::{ResolvedTextLayout, TextMeasurer, TextMetrics, layout_resolved_text};
use std::fmt::Write;

enum Part<'a, 'input> {
    Svg(roxmltree::Node<'a, 'input>, ResolvedHtmlRun),
    Text(Vec<ResolvedHtmlRun>, ResolvedTextLayout),
}

const WRAPPER: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\">";
type Line<'a, 'input> = (Vec<(Part<'a, 'input>, f64, f64)>, f64, f64);

#[derive(Clone, Debug)]
pub(crate) struct MathLabel {
    pub(super) html: String,
    pub(super) typography: ResolvedFallbackTypography,
}

impl MathLabel {
    pub(crate) fn measure(
        &self,
        wrap_width: Option<f64>,
        measurer: &dyn TextMeasurer,
    ) -> Option<TextMetrics> {
        let wrapped = format!("{WRAPPER}{}</svg>", self.html);
        let document = roxmltree::Document::parse(&wrapped).ok()?;
        let (lines, height) = plan(
            &document,
            wrap_width,
            &self.typography,
            measurer,
            &mut || Ok::<_, std::convert::Infallible>(()),
        )
        .unwrap()?;
        Some(TextMetrics {
            width: lines.iter().map(|line| line.1).fold(0.0, f64::max),
            height,
            line_count: lines
                .iter()
                .map(|line| {
                    line.0
                        .iter()
                        .map(|(part, _, _)| match part {
                            Part::Text(_, plan) => plan.lines.len(),
                            Part::Svg(..) => 1,
                        })
                        .max()
                        .unwrap_or(1)
                })
                .sum(),
        })
    }
}

fn plan<'a, 'input, E>(
    document: &'a roxmltree::Document<'input>,
    wrap_width: Option<f64>,
    typography: &ResolvedFallbackTypography,
    measurer: &dyn TextMeasurer,
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<Option<(Vec<Line<'a, 'input>>, f64)>, E> {
    checkpoint()?;
    if !document
        .root_element()
        .descendants()
        .skip(1)
        .any(|n| n.has_tag_name("svg"))
    {
        return Ok(None);
    }
    // RaTeX emits one block per line; the surrounding label wrappers do not add rows.
    let rows: Vec<_> = document
        .descendants()
        .filter(|n| {
            n.has_tag_name("div")
                && !n
                    .descendants()
                    .skip(1)
                    .any(|child| child.has_tag_name("div"))
        })
        .collect();
    let rows = if rows.is_empty() {
        vec![document.root_element()]
    } else {
        rows
    };
    let mut lines = Vec::new();
    let mut total_height = 0.0;
    for (index, row) in rows.into_iter().enumerate() {
        checkpoint_loop(index, checkpoint)?;
        let mut parts = Vec::new();
        let mut runs = Vec::new();
        for node in row.descendants().skip(1).filter(|n| {
            !n.ancestors()
                .take_while(|a| *a != row)
                .skip(1)
                .any(|a| a.has_tag_name("svg"))
        }) {
            checkpoint()?;
            if !node.has_tag_name("svg") && !node.is_text() {
                continue;
            }
            let Some(mut style) = typography
                .fragments
                .get(&(node.range().start - WRAPPER.len()))
                .cloned()
            else {
                return Ok(None);
            };
            if node.has_tag_name("svg") {
                push_text(&mut parts, &mut runs, None, measurer, checkpoint)?;
                let dimension = |name| {
                    let raw = node.attribute(name)?;
                    let (value, scale) = if let Some(value) = raw.strip_suffix("em") {
                        (value, style.text.style.font_size)
                    } else {
                        (raw.strip_suffix("px").unwrap_or(raw), 1.0)
                    };
                    let value = value.parse::<f64>().ok()? * scale;
                    (value.is_finite() && value >= 0.0).then_some(value)
                };
                let (Some(w), Some(h)) = (dimension("width"), dimension("height")) else {
                    return Ok(None);
                };
                parts.push((Part::Svg(node, style), w, h));
            } else if let Some(text) = node.text().filter(|text| !text.is_empty()) {
                style.text.text = text.to_owned();
                runs.push(style);
            }
        }
        let wrap_width = wrap_width.filter(|_| !row.descendants().any(|n| n.has_tag_name("svg")));
        push_text(&mut parts, &mut runs, wrap_width, measurer, checkpoint)?;
        let line_width = parts.iter().map(|p| p.1).sum();
        let line_height = parts.iter().map(|p| p.2).fold(0.0, f64::max);
        total_height += line_height;
        lines.push((parts, line_width, line_height));
    }
    Ok(Some((lines, total_height)))
}

fn push_text<'a, 'input, E>(
    parts: &mut Vec<(Part<'a, 'input>, f64, f64)>,
    runs: &mut Vec<ResolvedHtmlRun>,
    wrap_width: Option<f64>,
    measurer: &dyn TextMeasurer,
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<(), E> {
    if runs.is_empty() {
        return Ok(());
    }
    let text_runs = runs.iter().map(|run| run.text.clone()).collect::<Vec<_>>();
    let plan = layout_resolved_text(&text_runs, wrap_width, measurer, checkpoint)?;
    let (width, height) = (plan.width, plan.height);
    parts.push((Part::Text(std::mem::take(runs), plan), width, height));
    Ok(())
}

pub(super) fn render<E>(
    inner: &str,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    wrap_width: Option<f64>,
    typography: &ResolvedFallbackTypography,
    measurer: &dyn TextMeasurer,
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<Option<(String, usize)>, E> {
    if !inner.contains("<svg") {
        return Ok(None);
    }
    checkpoint()?;
    let wrapped = format!("{WRAPPER}{inner}</svg>");
    let Ok(document) = roxmltree::Document::parse(&wrapped) else {
        return Ok(None);
    };
    let Some((lines, total_height)) =
        plan(&document, wrap_width, typography, measurer, checkpoint)?
    else {
        return Ok(None);
    };
    let mut output = String::new();
    let mut count = 0;
    let mut top = y + (height - total_height) / 2.0;
    for (parts, line_width, line_height) in lines {
        let mut left = x + (width - line_width) / 2.0;
        for (part, w, h) in parts {
            checkpoint()?;
            if let Part::Svg(node, style) = part {
                let svg_width =
                    escape_xml_attr_with_checkpoints(node.attribute("width").unwrap(), checkpoint)?;
                let svg_height = escape_xml_attr_with_checkpoints(
                    node.attribute("height").unwrap(),
                    checkpoint,
                )?;
                let svg_style = escape_xml_attr_with_checkpoints(
                    &format!(
                        "{};font-size:{}px",
                        node.attribute("style").unwrap_or(""),
                        style.text.style.font_size
                    ),
                    checkpoint,
                )?;
                let _ = write!(
                    output,
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" x=\"{left}\" y=\"{}\" width=\"{svg_width}\" height=\"{svg_height}\" style=\"{svg_style}\"",
                    top + (line_height - h) / 2.0
                );
                let color = escape_xml_attr_with_checkpoints(&style.fill, checkpoint)?;
                let _ = write!(output, " color=\"{color}\"");
                for attribute in node.attributes().filter(|a| {
                    !["x", "y", "width", "height", "font-size", "style", "color"]
                        .contains(&a.name())
                }) {
                    let value = escape_xml_attr_with_checkpoints(attribute.value(), checkpoint)?;
                    let _ = write!(output, " {}=\"{value}\"", attribute.name());
                }
                output.push('>');
                for child in node.children() {
                    output.push_str(&wrapped[child.range()]);
                }
                output.push_str("</svg>");
                count += node.descendants().filter(|n| n.is_element()).count();
            } else if let Part::Text(runs, plan) = part {
                let (markup, elements) = super::rich_text::render_plan(
                    &runs,
                    &plan,
                    left,
                    top + (line_height - h) / 2.0,
                    w,
                    checkpoint,
                )?;
                output.push_str(&markup);
                count += elements;
            }
            left += w;
        }
        top += line_height;
    }
    Ok(Some((output, count)))
}

#[cfg(test)]
mod tests {
    use crate::svg::fallback::foreign_object_label_fallback_svg_text;
    use crate::text::DeterministicTextMeasurer;

    #[test]
    fn formula_and_adjacent_text_keep_nested_fonts_colors_and_boundary_spaces() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><foreignObject width="200" height="80"><div xmlns="http://www.w3.org/1999/xhtml" style="font-size:10px"><div><strong>bold</strong> <span style="font-size:20px;color:red"><em>large</em> <svg xmlns="http://www.w3.org/2000/svg" width="2em" height="1em"><path d="M0 0L1 1" fill="currentColor"/></svg></span> tail</div></div></foreignObject></svg>"#;
        let output =
            foreign_object_label_fallback_svg_text(svg, &DeterministicTextMeasurer::default());
        let document = roxmltree::Document::parse(&output).unwrap();
        let formula = document
            .descendants()
            .find(|n| n.has_attribute("data-merman-foreignobject"))
            .unwrap()
            .descendants()
            .find(|n| n.has_tag_name("svg"))
            .unwrap();
        assert_eq!(formula.attribute("style"), Some(";font-size:20px"));
        assert_eq!(formula.attribute("color"), Some("red"));
        assert_eq!(formula.attribute("width"), Some("2em"));
        let runs: Vec<_> = document
            .descendants()
            .filter(|n| n.has_tag_name("tspan"))
            .collect();
        assert_eq!(
            runs.iter().filter_map(|n| n.text()).collect::<String>(),
            "bold large  tail"
        );
        assert!(
            runs[0]
                .attribute("style")
                .unwrap()
                .contains("font-weight:700")
        );
        let large = runs.iter().find(|n| n.text() == Some("large")).unwrap();
        assert!(large.attribute("style").unwrap().contains("font-size:20px"));
        assert!(
            large
                .attribute("style")
                .unwrap()
                .contains("font-style:italic")
        );
        assert_eq!(large.attribute("fill"), Some("red"));
    }

    #[test]
    fn native_formula_preserves_geometry_em_size_local_translation_and_identity() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><g transform="translate(20,5)"><foreignObject transform="translate(30,7)" width="40" height="20" data-mt-key="node:A" data-mt-label="true"><div xmlns="http://www.w3.org/1999/xhtml" style="font-size:10px"><div><svg xmlns="http://www.w3.org/2000/svg" width="2em" height="1em" viewBox="0 0 2 1"><path id="formula" d="M0 0L2 0L2 1Z"/></svg></div></div></foreignObject></g></svg>"#;
        let output =
            foreign_object_label_fallback_svg_text(svg, &DeterministicTextMeasurer::default());
        let document = roxmltree::Document::parse(&output).unwrap();
        let converted = document
            .descendants()
            .find(|n| n.has_attribute("data-merman-foreignobject"))
            .unwrap();
        assert_eq!(converted.attribute("data-mt-key"), Some("node:A"));
        let formula = converted
            .descendants()
            .find(|n| n.has_tag_name("svg"))
            .unwrap();
        for (name, value) in [
            ("x", "60"),
            ("y", "17"),
            ("width", "2em"),
            ("height", "1em"),
            ("style", ";font-size:10px"),
            ("viewBox", "0 0 2 1"),
        ] {
            assert_eq!(formula.attribute(name), Some(value), "{name}");
        }
        assert!(
            formula
                .descendants()
                .any(|n| n.attribute("id") == Some("formula")
                    && n.attribute("d") == Some("M0 0L2 0L2 1Z"))
        );
    }

    #[test]
    fn plain_rows_between_formulas_keep_configured_wrapping() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><foreignObject width="40" height="80"><div xmlns="http://www.w3.org/1999/xhtml" style="font-size:10px;white-space:break-spaces;max-width:40px"><span><div style="white-space:nowrap"><svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em"><path d="M0 0L1 1"/></svg></div><div>alpha beta gamma</div></span></div></foreignObject></svg>"#;
        let output =
            foreign_object_label_fallback_svg_text(svg, &DeterministicTextMeasurer::default());
        let document = roxmltree::Document::parse(&output).unwrap();
        let rows: Vec<_> = document
            .descendants()
            .filter(|n| n.has_tag_name("text"))
            .collect();
        assert_eq!(
            rows.iter()
                .map(|n| n
                    .descendants()
                    .filter(|n| n.is_text())
                    .filter_map(|n| n.text())
                    .collect::<String>()
                    .trim_end()
                    .to_owned())
                .collect::<Vec<_>>(),
            ["alpha", "beta", "gamma"]
        );
        for pair in rows.windows(2) {
            let y = |n: roxmltree::Node| n.attribute("y").unwrap().parse::<f64>().unwrap();
            assert!((y(pair[1]) - y(pair[0]) - 15.0).abs() < 1e-9);
        }
    }

    #[test]
    fn mixed_rows_keep_all_formula_fragments_text_and_row_order() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><foreignObject width="100" height="60"><div xmlns="http://www.w3.org/1999/xhtml" style="font-size:10px"><span><div>before <svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 1 1"><path id="one" d="M0 0L1 1"/></svg> between <svg xmlns="http://www.w3.org/2000/svg" width="1em" height="2em" viewBox="0 0 1 2"><path id="two" d="M0 0L1 2"/></svg> after</div><div>plain</div><div><svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 1 1"><path id="three" d="M0 0L1 1"/></svg></div></span></div></foreignObject></svg>"#;
        let output =
            foreign_object_label_fallback_svg_text(svg, &DeterministicTextMeasurer::default());
        let document = roxmltree::Document::parse(&output).unwrap();
        let converted = document
            .descendants()
            .find(|n| n.has_attribute("data-merman-foreignobject"))
            .unwrap();
        let text: String = converted
            .descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect();
        assert_eq!(text, "before  between  afterplain");
        let formulas: Vec<_> = converted
            .descendants()
            .filter(|n| n.has_tag_name("svg"))
            .collect();
        assert_eq!(formulas.len(), 3);
        assert!(
            formulas[0].attribute("x").unwrap().parse::<f64>().unwrap()
                < formulas[1].attribute("x").unwrap().parse::<f64>().unwrap()
        );
        assert!(
            formulas[2].attribute("y").unwrap().parse::<f64>().unwrap()
                > formulas[0].attribute("y").unwrap().parse::<f64>().unwrap()
        );
    }
}
