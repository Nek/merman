//! HTML wrapper styles used by SVG emission.

use super::*;

pub(in crate::svg::parity) fn flowchart_label_div_style_prefix(
    styles: &FlowchartCompiledStyles,
    color_as_rgb: bool,
) -> String {
    fn div_style_survives_mermaid_overrides(key: &str) -> bool {
        !matches!(key, "line-height" | "text-align" | "white-space")
    }

    let mut out = String::new();
    for (key, value) in &styles.label_div_decls {
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() || !div_style_survives_mermaid_overrides(key) {
            continue;
        }
        if key == "color" {
            if color_as_rgb {
                let color = super::super::util::cssom_color_value(value);
                let _ = write!(&mut out, "color: {color} !important; ");
            } else {
                let _ = write!(
                    &mut out,
                    "color: {} !important; ",
                    value.to_ascii_lowercase()
                );
            }
        } else {
            let _ = write!(&mut out, "{key}: {value} !important; ");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color_style(value: &str) -> FlowchartCompiledStyles {
        FlowchartCompiledStyles {
            node_style: String::new(),
            label_style: String::new(),
            label_div_decls: vec![("color".to_string(), value.to_string())],
            fill: None,
            stroke: None,
            stroke_width: None,
            stroke_dasharray: None,
        }
    }

    #[test]
    fn flowchart_html_label_color_uses_the_shared_cssom_boundary() {
        assert_eq!(
            flowchart_label_div_style_prefix(&color_style("#12345680"), true),
            "color: rgba(18, 52, 86, 0.502) !important; "
        );
        assert_eq!(
            flowchart_label_div_style_prefix(&color_style("hsl(210 50% 40%)"), true),
            "color: rgb(51, 102, 153) !important; "
        );
        assert_eq!(
            flowchart_label_div_style_prefix(&color_style("var(--LabelColor)"), true),
            "color: var(--LabelColor) !important; "
        );
    }
}
