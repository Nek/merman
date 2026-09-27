//! Preserve native inline SVG formulas while converting their HTML label shell.
use super::cascade::ResolvedFallbackTypography;
use crate::svg::pipeline::{
    checkpoint_loop, escape_xml_attr_with_checkpoints, escape_xml_text_with_checkpoints,
};
use crate::text::TextMeasurer;
use std::fmt::Write;

pub(super) fn render<E>(
    inner: &str,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    typography: &ResolvedFallbackTypography,
    measurer: &dyn TextMeasurer,
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<Option<(String, usize)>, E> {
    if !inner.contains("<svg") {
        return Ok(None);
    }
    checkpoint()?;
    let wrapped = format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{inner}</svg>");
    let Ok(document) = roxmltree::Document::parse(&wrapped) else {
        return Ok(None);
    };
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
        let mut line_width = 0.0;
        let mut line_height: f64 = 0.0;
        for node in row.descendants().skip(1).filter(|n| {
            !n.ancestors()
                .take_while(|a| *a != row)
                .skip(1)
                .any(|a| a.has_tag_name("svg"))
        }) {
            let dimensions = if node.has_tag_name("svg") {
                let dimension = |name| {
                    let raw = node.attribute(name)?;
                    let (value, scale) = if let Some(value) = raw.strip_suffix("em") {
                        (value, typography.font_size)
                    } else {
                        (raw.strip_suffix("px").unwrap_or(raw), 1.0)
                    };
                    let value = value.parse::<f64>().ok()? * scale;
                    (value.is_finite() && value >= 0.0).then_some(value)
                };
                let (Some(w), Some(h)) = (dimension("width"), dimension("height")) else {
                    return Ok(None);
                };
                Some((w, h))
            } else if node.is_text() && node.text().is_some_and(|text| !text.is_empty()) {
                let measured = measurer.measure(node.text().unwrap(), &typography.text_style());
                Some((measured.width, typography.line_height.max(measured.height)))
            } else {
                None
            };
            if let Some((w, h)) = dimensions {
                line_width += w;
                line_height = line_height.max(h);
                parts.push((node, w, h));
            }
        }
        total_height += line_height;
        lines.push((parts, line_width, line_height));
    }
    let mut output = String::new();
    let mut count = 0;
    let mut top = y + (height - total_height) / 2.0;
    for (parts, line_width, line_height) in lines {
        let mut left = x + (width - line_width) / 2.0;
        for (node, w, h) in parts {
            checkpoint()?;
            if node.has_tag_name("svg") {
                let _ = write!(
                    output,
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" x=\"{left}\" y=\"{}\" width=\"{w}\" height=\"{h}\"",
                    top + (line_height - h) / 2.0
                );
                for attribute in node
                    .attributes()
                    .filter(|a| !["x", "y", "width", "height"].contains(&a.name()))
                {
                    let value = escape_xml_attr_with_checkpoints(attribute.value(), checkpoint)?;
                    let _ = write!(output, " {}=\"{value}\"", attribute.name());
                }
                output.push('>');
                for child in node.children() {
                    output.push_str(&wrapped[child.range()]);
                }
                output.push_str("</svg>");
                count += node.descendants().filter(|n| n.is_element()).count();
            } else {
                let text = escape_xml_text_with_checkpoints(node.text().unwrap(), checkpoint)?;
                let font = escape_xml_attr_with_checkpoints(&typography.font_family, checkpoint)?;
                let fill = escape_xml_attr_with_checkpoints(&typography.fill, checkpoint)?;
                let _ = write!(
                    output,
                    "<text x=\"{left}\" y=\"{}\" dominant-baseline=\"central\" fill=\"{fill}\" font-size=\"{}\" font-family=\"{font}\"",
                    top + line_height / 2.0,
                    typography.font_size
                );
                for (name, value) in [
                    ("font-weight", typography.font_weight.as_deref()),
                    ("font-style", typography.font_style.as_deref()),
                ] {
                    if let Some(value) = value {
                        let value = escape_xml_attr_with_checkpoints(value, checkpoint)?;
                        let _ = write!(output, " {name}=\"{value}\"");
                    }
                }
                let _ = write!(output, ">{text}</text>");
                count += 1;
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
            ("width", "20"),
            ("height", "10"),
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
