use super::cascade::{CascadeIndex, Namespace, SourceElement};
use crate::svg::pipeline::{
    SvgPostprocessExecution, SvgTagScanner, end_tag_name,
    extract_exact_double_quoted_attr_with_checkpoints, start_tag_name,
};
use crate::text::{ResolvedTextRun, TextStyle};
use std::collections::HashMap;

/// Resolve font properties without rewriting SVG or changing its XML source positions.
/// The input must be well-formed SVG; the session supplies cancellation and resource limits.
pub fn svg_font_styles(
    svg: &str,
    session: &crate::environment::RenderSession,
) -> crate::Result<HashMap<usize, TextStyle>> {
    let execution = SvgPostprocessExecution::new(session);
    execution.preflight_svg_byte_count(svg.len())?;
    let mut checkpoint = || execution.checkpoint();
    let mut limit = |actual, maximum| Err(execution.selector_index_limit(actual, maximum));
    let mut cascade = CascadeIndex::new(svg, &mut checkpoint, &mut limit)?;
    let mut scanner = SvgTagScanner::new(svg);
    let mut path = Vec::new();
    let mut computed: Vec<super::cascade::ResolvedStyle> = Vec::new();
    let mut fonts = HashMap::new();
    let mut elements = 0;
    while let Some(tag) = scanner.next_with_checkpoints(&mut checkpoint)? {
        if end_tag_name(tag.raw()).is_some() {
            path.pop();
            computed.pop();
            continue;
        }
        let Some(name) = start_tag_name(tag.raw()) else {
            continue;
        };
        elements += 1;
        execution.preflight_svg_structure(elements, path.len() + 1)?;
        if name == "style" && !tag.is_self_closing() {
            while let Some(close) = scanner.next_with_checkpoints(&mut checkpoint)? {
                if end_tag_name(close.raw()) == Some("style") {
                    break;
                }
            }
            continue;
        }
        path.push(CascadeIndex::source_element(
            tag.raw(),
            Namespace::Svg,
            &mut checkpoint,
        )?);
        let style = cascade.resolve_path(
            &path,
            computed.last(),
            computed.first().map_or(16.0, |s| s.font_size),
            &mut checkpoint,
            &mut limit,
        )?;
        fonts.insert(
            tag.start(),
            TextStyle {
                font_size: style.font_size,
                font_family: Some(style.font_family.clone()),
                font_weight: style.font_weight.clone(),
                font_style: style.font_style.clone(),
            },
        );
        if tag.is_self_closing() {
            path.pop();
        } else {
            computed.push(style);
        }
    }
    checkpoint()?;
    Ok(fonts)
}

#[derive(Default)]
pub(crate) struct MappedLabelStyles {
    pub(crate) math: HashMap<String, super::inline_svg::MathLabel>,
    pub(crate) base: HashMap<String, TextStyle>,
    pub(crate) rich: HashMap<String, Vec<Vec<ResolvedTextRun>>>,
}

// Use emitted semantic identities and ancestry, never label text, to resolve layout typography.
pub(crate) fn mapped_label_styles(
    svg: &str,
    execution: SvgPostprocessExecution<'_>,
) -> crate::Result<MappedLabelStyles> {
    let mut checkpoint = || execution.checkpoint();
    let mut limit = |actual, maximum| Err(execution.selector_index_limit(actual, maximum));
    let mut cascade = CascadeIndex::new(svg, &mut checkpoint, &mut limit)?;
    let mut scanner = SvgTagScanner::new(svg);
    let mut path: Vec<SourceElement> = Vec::new();
    let mut owners: Vec<Option<&str>> = Vec::new();
    let mut rich = HashMap::new();
    let mut math = HashMap::new();
    let mut styles: HashMap<String, (TextStyle, Vec<SourceElement>, bool)> = HashMap::new();
    while let Some(tag) = scanner.next_with_checkpoints(&mut checkpoint)? {
        if end_tag_name(tag.raw()).is_some() {
            path.pop();
            owners.pop();
            continue;
        }
        let Some(name) = start_tag_name(tag.raw()) else {
            continue;
        };
        if name == "style" {
            while let Some(close) = scanner.next_with_checkpoints(&mut checkpoint)? {
                if end_tag_name(close.raw()) == Some("style") {
                    break;
                }
            }
            continue;
        }
        let label = extract_exact_double_quoted_attr_with_checkpoints(
            tag.raw(),
            "data-mt-label",
            &mut checkpoint,
        )?;
        let owner = if label == Some("true") {
            extract_exact_double_quoted_attr_with_checkpoints(
                tag.raw(),
                "data-mt-key",
                &mut checkpoint,
            )?
        } else {
            owners.last().copied().flatten()
        };
        if name == "foreignObject" {
            let start = scanner.cursor();
            while let Some(close) = scanner.next_with_checkpoints(&mut checkpoint)? {
                if end_tag_name(close.raw()) == Some("foreignObject") {
                    if let Some(owner) = owner {
                        let style = cascade.resolve_foreign_object(
                            &path,
                            tag.raw(),
                            &svg[start..close.start()],
                            &mut checkpoint,
                            &mut limit,
                        )?;
                        record(&mut styles, owner, style.text_style(), &path);
                        if svg[start..close.start()].contains("<svg") {
                            math.insert(
                                merman_core::entities::decode_html_entities_to_unicode(owner)
                                    .into_owned(),
                                super::inline_svg::MathLabel {
                                    html: svg[start..close.start()].to_owned(),
                                    typography: style.clone(),
                                },
                            );
                        }
                        if let Some(first) = style.lines.iter().flatten().next()
                            && style
                                .lines
                                .iter()
                                .flatten()
                                .any(|run| !run.text.same_typography(&first.text))
                            && !svg[start..close.start()].contains("<svg")
                        {
                            let key = merman_core::entities::decode_html_entities_to_unicode(owner)
                                .into_owned();
                            rich.insert(
                                key,
                                style
                                    .lines
                                    .into_iter()
                                    .map(|line| line.into_iter().map(|run| run.text).collect())
                                    .collect(),
                            );
                        }
                    }
                    break;
                }
            }
            continue;
        }
        path.push(CascadeIndex::source_element(
            tag.raw(),
            Namespace::Svg,
            &mut checkpoint,
        )?);
        owners.push(owner);
        if name == "text"
            && let Some(owner) = owner
        {
            let style = cascade.resolve_full_path(&path, &mut checkpoint, &mut limit)?;
            record(
                &mut styles,
                owner,
                TextStyle {
                    font_size: style.font_size,
                    font_family: Some(style.font_family),
                    font_weight: style.font_weight,
                    font_style: style.font_style,
                },
                &path,
            );
        }
        if tag.is_self_closing() {
            path.pop();
            owners.pop();
        }
    }
    let base = styles
        .into_iter()
        .map(|(owner, (style, common_path, same))| {
            if same {
                return Ok((owner, style));
            }
            let style = cascade.resolve_full_path(&common_path, &mut checkpoint, &mut limit)?;
            Ok((
                owner,
                TextStyle {
                    font_size: style.font_size,
                    font_family: Some(style.font_family),
                    font_weight: style.font_weight,
                    font_style: style.font_style,
                },
            ))
        })
        .collect::<crate::Result<_>>()?;
    Ok(MappedLabelStyles { base, rich, math })
}

fn record(
    styles: &mut HashMap<String, (TextStyle, Vec<SourceElement>, bool)>,
    owner: &str,
    style: TextStyle,
    path: &[SourceElement],
) {
    match styles.entry(merman_core::entities::decode_html_entities_to_unicode(owner).into_owned()) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert((style, path.to_vec(), true));
        }
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            let (first, common, same) = entry.get_mut();
            *same &= first.font_size == style.font_size
                && first.font_family == style.font_family
                && first.font_weight == style.font_weight
                && first.font_style == style.font_style;
            let shared = common.iter().zip(path).take_while(|(a, b)| a == b).count();
            common.truncate(shared);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::RenderEnvironment;
    use merman_core::OperationControl;

    #[test]
    fn export_typography_uses_original_element_offsets_and_shared_cascade() {
        let session = RenderEnvironment::deterministic().begin_session().unwrap();
        let svg = r#"<svg style="--Size:32px;--Face:monospace"><style>text{font-size:var(--Size);font-family:var(--Face);font-weight:700}</style><text>repeat<tspan style="font-size:50%;font-weight:normal">repeat</tspan></text><text style="--Size:0px">repeat</text></svg>"#;
        let styles = svg_font_styles(svg, &session).unwrap();
        let first = &styles[&svg.find("<text>").unwrap()];
        assert_eq!(first.font_size, 32.0);
        assert_eq!(first.font_family.as_deref(), Some("monospace"));
        assert_eq!(first.font_weight.as_deref(), Some("700"));
        let child = &styles[&svg.find("<tspan").unwrap()];
        assert_eq!(child.font_size, 16.0);
        assert_eq!(child.font_weight.as_deref(), Some("normal"));
        assert_eq!(styles[&svg.find("<text style").unwrap()].font_size, 0.0);
        for css in [
            "g &gt; text[data-x=&quot;a:b&quot;]{font-size:20px}",
            "<![CDATA[g > text[data-x=\"a:b\"]{font-size:20px}]]>",
        ] {
            let svg =
                format!("<svg><style>{css}</style><g><text data-x=\"a:b\">repeat</text></g></svg>");
            let styles = svg_font_styles(&svg, &session).unwrap();
            assert_eq!(
                styles[&svg.find("<text ").unwrap()].font_size,
                20.0,
                "CSS follows XML text semantics"
            );
        }
        let control = OperationControl::new();
        let session = RenderEnvironment::deterministic()
            .begin_session_with_control(control.clone())
            .unwrap();
        control.cancel();
        assert!(matches!(
            svg_font_styles(svg, &session),
            Err(crate::Error::Cancelled(_))
        ));
    }

    #[test]
    fn label_typography_uses_owned_ancestry_and_preserves_mixed_run_base() {
        let session = RenderEnvironment::deterministic().begin_session().unwrap();
        let svg = r#"<svg id="diagram" style="font-size:16px"><style>@layer theme {#diagram .large{font-size:32px!important}} .mixed{font-size:24px}</style>
            <g class="large" data-mt-key="node:A&amp;B" data-mt-label="true"><text>Same</text></g>
            <g data-mt-key="node:B" data-mt-label="true"><text>Same</text></g>
            <g class="mixed" data-mt-key="node:C" data-mt-label="true"><text>Normal</text><text style="font-weight:bold">Bold</text></g>
            <g class="large" data-mt-key="node:D" data-mt-label="true"><foreignObject width="100" height="50"><div><span>Same</span></div></foreignObject></g>
            <text>Unowned</text></svg>"#;
        let styles = mapped_label_styles(svg, SvgPostprocessExecution::new(&session))
            .unwrap()
            .base;
        assert_eq!(styles.len(), 4);
        assert_eq!(styles["node:A&B"].font_size, 32.0);
        assert_eq!(styles["node:B"].font_size, 16.0);
        assert_eq!(styles["node:C"].font_size, 24.0);
        assert_eq!(styles["node:C"].font_weight, None);
        assert_eq!(styles["node:D"].font_size, 32.0);
    }

    #[test]
    fn label_typography_keeps_operation_cancellation_and_selector_limits() {
        let control = OperationControl::new();
        let session = RenderEnvironment::deterministic()
            .begin_session_with_control(control.clone())
            .unwrap();
        control.cancel();
        assert!(matches!(
            mapped_label_styles("<svg/>", SvgPostprocessExecution::new(&session)),
            Err(crate::Error::Cancelled(_))
        ));
        let session = RenderEnvironment::deterministic().begin_session().unwrap();
        let svg = format!("<svg><style>{}</style></svg>", "@layer a;".repeat(8193));
        assert!(matches!(
            mapped_label_styles(&svg, SvgPostprocessExecution::new(&session)),
            Err(crate::Error::ResourceLimitExceeded(_))
        ));
    }
}
