//! Flowchart style compilation helpers.

use crate::mermaid_style::{
    is_label_style_key as is_text_style_key, parse_safe_style_decl as parse_style_decl,
};
use indexmap::IndexMap;
use rustc_hash::FxHashMap;
use std::fmt::Write as _;

#[derive(Debug, Clone)]
pub(crate) struct FlowchartCompiledStyles {
    pub(crate) node_style: String,
    pub(crate) label_style: String,
    pub(crate) label_div_decls: Vec<(String, String)>,
    pub(crate) fill: Option<String>,
    pub(crate) stroke: Option<String>,
    pub(crate) stroke_width: Option<String>,
    pub(crate) stroke_dasharray: Option<String>,
}

pub(crate) fn flowchart_compile_styles(
    class_defs: &IndexMap<String, Vec<String>>,
    classes: &[String],
    inline_styles_a: &[String],
    inline_styles_b: &[String],
    inherited_font_size: f64,
) -> FlowchartCompiledStyles {
    // Ported from Mermaid `handDrawnShapeStyles.compileStyles()` / `styles2String()`:
    // - preserve insertion order of the first occurrence of a key
    // - later occurrences override values, without changing order
    #[derive(Default)]
    struct OrderedMap<'a> {
        order: Vec<(&'a str, &'a str)>,
        idx: FxHashMap<&'a str, usize>,
    }
    impl<'a> OrderedMap<'a> {
        fn set(&mut self, k: &'a str, v: &'a str) {
            if let Some(&i) = self.idx.get(k) {
                self.order[i].1 = v;
                return;
            }
            self.idx.insert(k, self.order.len());
            self.order.push((k, v));
        }
    }

    let mut m: OrderedMap<'_> = OrderedMap::default();

    for c in classes {
        let Some(decls) = class_defs.get(c) else {
            continue;
        };
        for d in decls {
            for d in crate::flowchart::flowchart_split_mermaid_style_decls(d) {
                let Some((k, v)) = parse_style_decl(d) else {
                    continue;
                };
                m.set(k, v);
            }
        }
    }

    for d in inline_styles_a.iter().chain(inline_styles_b.iter()) {
        for d in crate::flowchart::flowchart_split_mermaid_style_decls(d) {
            let Some((k, v)) = parse_style_decl(d) else {
                continue;
            };
            m.set(k, v);
        }
    }

    let mut node_style = String::new();
    let mut label_style = String::new();

    let mut label_div_decls: Vec<(String, String)> = Vec::new();

    let mut fill: Option<String> = None;
    let mut stroke: Option<String> = None;
    let mut stroke_width: Option<String> = None;
    let mut stroke_dasharray: Option<String> = None;

    for (k, v) in &m.order {
        let k = *k;
        let v = *v;
        // Generated HTML wrappers must not repeatedly apply one authored relative size.
        let pixel_size = if k.eq_ignore_ascii_case("font-size")
            && matches!(v.to_ascii_lowercase().as_str(), "revert" | "revert-layer")
        {
            let base = crate::text::TextStyle {
                font_size: inherited_font_size,
                ..Default::default()
            };
            Some(
                crate::flowchart::flowchart_effective_edge_label_text_style(
                    &base,
                    class_defs,
                    classes,
                    inline_styles_a,
                    inline_styles_b,
                )
                .font_size,
            )
        } else {
            (k.eq_ignore_ascii_case("font-size")
                && (v.to_ascii_lowercase().ends_with("em")
                    || v.ends_with('%')
                    || crate::mermaid_style::font_size_keyword_px(v, inherited_font_size)
                        .is_some()))
            .then(|| crate::mermaid_style::parse_css_font_size_px(v, inherited_font_size))
            .flatten()
        }
        .map(|size| format!("{size}px"));
        let v = pixel_size.as_deref().unwrap_or(v);
        if is_text_style_key(k) {
            if !label_style.is_empty() {
                label_style.push(';');
            }
            let _ = write!(&mut label_style, "{k}:{v} !important");
            label_div_decls.push((k.to_string(), v.to_string()));
        } else {
            if !node_style.is_empty() {
                node_style.push(';');
            }
            let _ = write!(&mut node_style, "{k}:{v} !important");
        }
        match k {
            "fill" => fill = Some(v.to_string()),
            "stroke" => stroke = Some(v.to_string()),
            "stroke-width" => stroke_width = Some(v.to_string()),
            "stroke-dasharray" => stroke_dasharray = Some(v.to_string()),
            _ => {}
        }
    }

    FlowchartCompiledStyles {
        node_style,
        label_style,
        label_div_decls,
        fill,
        stroke,
        stroke_width,
        stroke_dasharray,
    }
}

pub(crate) fn flowchart_compile_node_styles(
    class_defs: &IndexMap<String, Vec<String>>,
    classes: &[String],
    inline_styles_a: &[String],
    inline_styles_b: &[String],
    inherited_font_size: f64,
) -> FlowchartCompiledStyles {
    let effective_classes =
        crate::flowchart::flowchart_effective_node_class_names(class_defs, classes)
            .into_iter()
            .map(|class| class.to_string())
            .collect::<Vec<_>>();
    flowchart_compile_styles(
        class_defs,
        &effective_classes,
        inline_styles_a,
        inline_styles_b,
        inherited_font_size,
    )
}
