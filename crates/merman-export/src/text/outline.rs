use super::{NativeFontContext, OutlinedSvg};
use merman_render::svg::ResvgCompatibleSvg;
use std::collections::{BTreeMap, BTreeSet};

impl NativeFontContext {
    /// Return a draft with text replaced by portable glyphs and source ownership preserved.
    /// The input must have a diagram root ID. The caller must revalidate this draft before publication.
    pub fn outline_svg(&self, svg: &ResvgCompatibleSvg) -> Result<String, String> {
        let output = self.outline_svg_with_diagnostics(svg)?;
        if !output.missing_characters.is_empty() {
            return Err("The resolved fonts lack a requested label glyph; use outline_svg_with_diagnostics to accept measured replacements".into());
        }
        Ok(output.svg)
    }

    /// Preserve a diagram with measured U+FFFD replacements and explicit character diagnostics.
    /// Like `outline_svg`, the returned SVG is a draft requiring terminal validation.
    pub fn outline_svg_with_diagnostics(
        &self,
        svg: &ResvgCompatibleSvg,
    ) -> Result<OutlinedSvg, String> {
        let source = svg.as_str();
        let document = roxmltree::Document::parse(source).map_err(|e| e.to_string())?;
        let labels: Vec<_> = document
            .descendants()
            .filter(|n| n.has_tag_name("text"))
            .collect();
        if labels.is_empty() {
            return Ok(OutlinedSvg {
                svg: source.to_owned(),
                missing_characters: Vec::new(),
            });
        }
        let diagram_id = document
            .root_element()
            .attribute("id")
            .filter(|id| !id.is_empty())
            .ok_or("Glyph conversion requires a diagram root ID")?;
        let mut prefix = format!(
            "mt-glyph-{}-",
            diagram_id
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        while source.contains(&prefix) {
            prefix.push('x');
        }
        let mut shaping_source = source.to_owned();
        let mut ids = Vec::new();
        for (index, label) in labels.iter().enumerate() {
            if label
                .descendants()
                .skip(1)
                .any(|n| n.attributes().any(|a| a.name().starts_with("data-mt-")))
            {
                return Err("Independently mapped text spans require separate text objects before glyph conversion".into());
            }
            ids.push(
                label
                    .attribute("id")
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{prefix}text-{index}")),
            );
        }
        // Temporary IDs identify occurrences, never label text or approximate source positions.
        for (label, id) in labels.iter().zip(&ids).rev() {
            if label.attribute("id").is_none() {
                let start = label.range().start;
                let name_end = source[start..]
                    .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                    .ok_or("Missing text tag boundary")?;
                shaping_source.insert_str(start + name_end, &format!(" id=\"{id}\""));
            }
        }
        let shaping_source = omit_negative_font_sizes(&shaping_source)?;
        let (tree, _, missing_characters) = self.parse_with_replacements(&shaping_source)?;
        fn check_glyphs(group: &usvg::Group) -> Result<(), String> {
            for node in group.children() {
                match node {
                    usvg::Node::Text(text)
                        if text
                            .layouted()
                            .iter()
                            .flat_map(|s| &s.positioned_glyphs)
                            .any(|g| g.id.0 == 0) =>
                    {
                        return Err("The resolved fonts lack a requested label glyph".into());
                    }
                    usvg::Node::Group(group) => check_glyphs(group)?,
                    _ => {}
                }
            }
            Ok(())
        }
        check_glyphs(tree.root())?;
        let normalized = tree.to_string(&usvg::WriteOptions {
            id_prefix: Some(prefix.clone()),
            ..Default::default()
        });
        let glyph_document = roxmltree::Document::parse(&normalized).map_err(|e| e.to_string())?;
        let by_id: BTreeMap<_, _> = glyph_document
            .descendants()
            .filter_map(|n| n.attribute("id").map(|id| (id, n)))
            .collect();
        let css = document
            .descendants()
            .filter(|node| node.has_tag_name("style"))
            .flat_map(|node| node.children())
            .filter_map(|node| node.text())
            .collect::<Vec<_>>()
            .join("\n");
        let styles = simplecss::StyleSheet::parse(&css);
        let mut replacements = Vec::new();
        let mut needed = BTreeSet::new();
        let mut definitions = BTreeMap::new();
        for (label, id) in labels.iter().zip(&ids) {
            let text: String = label
                .descendants()
                .filter(|n| n.is_text())
                .filter_map(|n| n.text())
                .collect();
            let key = format!("{prefix}{id}");
            let glyphs = match by_id.get(key.as_str()) {
                Some(node) => {
                    collect_refs(*node, &mut needed);
                    definitions.insert(node.range().start, protect_paint(*node, &normalized));
                    format!(r##"<use href="#{}"/>"##, quick_xml::escape::escape(&key))
                }
                // Font resolution is checked independently: zero-size, hidden and nonprinting
                // text may legitimately have no painted node in the normalized document.
                None => String::new(),
            };
            let attributes = label
                .attributes()
                .filter(|a| a.name() != "style")
                .map(|a| &source[a.range()])
                .collect::<Vec<_>>()
                .join(" ");
            let inline = label.attribute("style").unwrap_or_default();
            let other_styles = remove_declarations(
                inline,
                simplecss::DeclarationTokenizer::from(inline)
                    .filter(|declaration| declaration.name == "opacity"),
            )
            .unwrap_or_else(|| inline.to_owned());
            let style = [
                other_styles,
                format!("opacity:{}!important", text_opacity(*label, &styles)),
            ]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(";");
            let title = if text.is_empty() {
                String::new()
            } else {
                format!("<title>{}</title>", quick_xml::escape::escape(&text))
            };
            replacements.push((
                label.range(),
                format!(r#"<g xmlns:xlink="http://www.w3.org/1999/xlink" {attributes} style="{}">{title}{glyphs}</g>"#, quick_xml::escape::escape(&style)),
            ));
        }
        // Copy only resources referenced by the glyphs, with IDs scoped away from original SVG.
        while let Some(id) = needed.pop_first() {
            let node = by_id
                .get(id.as_str())
                .ok_or_else(|| format!("Missing glyph resource {id}"))?;
            let definition = node
                .ancestors()
                .find(|n| n.parent().is_some_and(|p| p.has_tag_name("defs")))
                .ok_or_else(|| format!("Glyph reference is not a definition: {id}"))?;
            let key = definition.range().start;
            if definitions.contains_key(&key) {
                continue;
            }
            collect_refs(definition, &mut needed);
            definitions.insert(key, protect_paint(definition, &normalized));
        }
        let mut output = source.to_owned();
        for (range, replacement) in replacements.into_iter().rev() {
            output.replace_range(range, &replacement);
        }
        if !definitions.is_empty() {
            let end = output.rfind("</").ok_or("Missing SVG closing tag")?;
            output.insert_str(
                end,
                &format!(
                    r#"<defs xmlns:xlink="http://www.w3.org/1999/xlink">{}</defs>"#,
                    definitions.into_values().collect::<String>()
                ),
            );
        }
        Ok(OutlinedSvg {
            svg: output,
            missing_characters,
        })
    }
}

// Match the original element, before temporary IDs or the text-to-group replacement.
// The adapter and declaration order follow usvg 0.47's static SVG CSS resolution.
struct CssElement<'a, 'input>(roxmltree::Node<'a, 'input>);
impl simplecss::Element for CssElement<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        self.0.parent_element().map(Self)
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.0.prev_sibling_element().map(Self)
    }
    fn has_local_name(&self, name: &str) -> bool {
        self.0.tag_name().name() == name
    }
    fn attribute_matches(&self, name: &str, operator: simplecss::AttributeOperator) -> bool {
        self.0
            .attribute(name)
            .is_some_and(|value| operator.matches(value))
    }
    fn pseudo_class_matches(&self, class: simplecss::PseudoClass) -> bool {
        matches!(class, simplecss::PseudoClass::FirstChild)
            && self.0.prev_sibling_element().is_none()
    }
}

fn text_opacity<'a>(
    node: roxmltree::Node<'a, 'a>,
    styles: &'a simplecss::StyleSheet<'a>,
) -> &'a str {
    let mut value = node.attribute("opacity").unwrap_or("1");
    let mut important = false;
    for declaration in styles
        .rules
        .iter()
        .filter(|rule| rule.selector.matches(&CssElement(node)))
        .flat_map(|rule| rule.declarations.iter().copied())
        .chain(simplecss::DeclarationTokenizer::from(
            node.attribute("style").unwrap_or_default(),
        ))
    {
        // usvg retains the first important declaration after its specificity-sorted rules.
        if declaration.name == "opacity" && !important {
            value = declaration.value;
            important = declaration.important;
        }
    }
    value
}

// usvg accepts negative font sizes and drops their text; CSS ignores those declarations.
// Normalize only the temporary shaping copy, using the same CSS parser as usvg.
fn omit_negative_font_sizes(source: &str) -> Result<String, String> {
    let document = roxmltree::Document::parse(source).map_err(|e| e.to_string())?;
    let invalid = |name: &str, value: &str| {
        name.eq_ignore_ascii_case("font-size")
            && value
                .trim()
                .parse::<svgtypes::Length>()
                .is_ok_and(|length| length.number < 0.0)
    };
    let mut replacements = Vec::new();
    for node in document.descendants().filter(|n| n.is_element()) {
        if node.has_tag_name("style") {
            let css: String = node
                .children()
                .filter(|n| n.is_text())
                .filter_map(|n| n.text())
                .collect();
            let sheet = simplecss::StyleSheet::parse(&css);
            let declarations: Vec<_> = sheet
                .rules
                .iter()
                .flat_map(|r| r.declarations.iter().copied())
                .collect();
            if let Some(css) = without_negative_declarations(&css, &declarations) {
                let attributes = node
                    .attributes()
                    .map(|a| &source[a.range()])
                    .collect::<Vec<_>>()
                    .join(" ");
                replacements.push((
                    node.range(),
                    format!(
                        "<style {attributes}>{}</style>",
                        quick_xml::escape::escape(&css)
                    ),
                ));
            }
            continue;
        }
        for attribute in node.attributes() {
            if invalid(attribute.name(), attribute.value()) {
                replacements.push((attribute.range(), String::new()));
            } else if attribute.name() == "style" {
                let declarations: Vec<_> =
                    simplecss::DeclarationTokenizer::from(attribute.value()).collect();
                if let Some(css) = without_negative_declarations(attribute.value(), &declarations) {
                    replacements.push((
                        attribute.range_value(),
                        quick_xml::escape::escape(&css).into_owned(),
                    ));
                }
            }
        }
    }
    let mut result = source.to_owned();
    replacements.sort_by_key(|(range, _)| range.start);
    for (range, replacement) in replacements.into_iter().rev() {
        result.replace_range(range, &replacement);
    }
    Ok(result)
}

fn without_negative_declarations(
    css: &str,
    declarations: &[simplecss::Declaration<'_>],
) -> Option<String> {
    remove_declarations(
        css,
        declarations.iter().copied().filter(|d| {
            d.name.eq_ignore_ascii_case("font-size")
                && d.value
                    .trim()
                    .parse::<svgtypes::Length>()
                    .is_ok_and(|n| n.number < 0.0)
        }),
    )
}

fn remove_declarations<'a>(
    css: &'a str,
    declarations: impl Iterator<Item = simplecss::Declaration<'a>>,
) -> Option<String> {
    let mut ranges = Vec::new();
    for declaration in declarations {
        // Declaration names borrow their original CSS bytes; preserve selectors and unrelated
        // declarations rather than serializing the selector AST (which can lose quoting).
        let start = declaration.name.as_ptr() as usize - css.as_ptr() as usize;
        let mut input = cssparser::ParserInput::new(&css[start..]);
        let mut parser = cssparser::Parser::new(&mut input);
        let end = loop {
            let position = parser.position().byte_index();
            match parser.next_including_whitespace_and_comments() {
                Ok(cssparser::Token::CloseCurlyBracket) | Err(_) => break start + position,
                Ok(cssparser::Token::Semicolon) => break start + parser.position().byte_index(),
                _ => {}
            }
        };
        ranges.push(start..end);
    }
    if ranges.is_empty() {
        return None;
    }
    ranges.sort_by_key(|range| range.start);
    ranges.dedup();
    let mut result = css.to_owned();
    for range in ranges.into_iter().rev() {
        result.replace_range(range, "");
    }
    Some(result)
}

fn collect_refs(node: roxmltree::Node<'_, '_>, needed: &mut BTreeSet<String>) {
    for attribute in node.descendants().flat_map(|n| n.attributes()) {
        if attribute.name() == "href" {
            if let Some(id) = attribute.value().strip_prefix('#') {
                needed.insert(id.to_owned());
            }
        }
        for part in attribute.value().split("url(#").skip(1) {
            if let Some((id, _)) = part.split_once(')') {
                needed.insert(id.to_owned());
            }
        }
    }
}

fn protect_paint(node: roxmltree::Node<'_, '_>, xml: &str) -> String {
    let range = node.range();
    let mut fragment = xml[range.clone()].to_owned();
    for element in node
        .descendants()
        .filter(|n| matches!(n.tag_name().name(), "path" | "g" | "image" | "use"))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        // Resolved glyph paint must win against selectors for diagram shapes and groups.
        let mut style = [
            element.attribute("style").unwrap_or_default().to_owned(),
            format!(
                "opacity:{}!important",
                element.attribute("opacity").unwrap_or("1")
            ),
        ]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(";");
        if element.has_tag_name("path") {
            for (key, default) in [
                ("fill", "black"),
                ("fill-opacity", "1"),
                ("fill-rule", "nonzero"),
                ("stroke", "none"),
                ("stroke-opacity", "1"),
                ("stroke-width", "1"),
                ("stroke-linecap", "butt"),
                ("stroke-linejoin", "miter"),
                ("stroke-miterlimit", "4"),
                ("stroke-dasharray", "none"),
                ("stroke-dashoffset", "0"),
            ] {
                style.push_str(&format!(
                    ";{key}:{}!important",
                    element.attribute(key).unwrap_or(default)
                ));
            }
        }
        if let Some(attribute) = element.attributes().find(|a| a.name() == "style") {
            let start = attribute.range().start - range.start;
            let end = attribute.range().end - range.start;
            fragment.replace_range(start..end, "");
        }
        let markers = if element.has_tag_name("path") {
            r#" marker-start="none" marker-mid="none" marker-end="none""#
        } else {
            ""
        };
        fragment.insert_str(
            element.range().start - range.start + 1 + element.tag_name().name().len(),
            &format!(r#"{markers} style="{}""#, quick_xml::escape::escape(&style)),
        );
    }
    fragment
}

#[cfg(test)]
mod tests {
    use super::*;
    use merman_render::{environment::RenderEnvironment, svg::SvgPipeline};
    use std::sync::Arc;

    fn seal(source: &str) -> ResvgCompatibleSvg {
        let session = RenderEnvironment::deterministic().begin_session().unwrap();
        SvgPipeline::resvg_safe()
            .process_resvg_compatible(source, &session)
            .unwrap()
    }

    #[test]
    fn glyph_conversion_preserves_original_mapping_and_nontext_svg() {
        let fonts = NativeFontContext::system().unwrap();
        let svg = seal(
            r##"<svg xmlns="http://www.w3.org/2000/svg" id="diagram" width="400" height="200" viewBox="0 0 400 200"><style>.node path {fill:red!important;stroke:blue!important}.label{fill:#123456;font-family:Arial;font-size:20px}</style><metadata data-mt-native="[]"/><g class="node" data-mt-key="node:A" transform="translate(20 30)"><rect id="body" x="-10" y="-20" width="200" height="80" fill="yellow"/><text id="title" class="label" data-mt-key="label:A" data-mt-label="true" transform="rotate(10)" x="4" y="24">Alpha <tspan font-weight="bold">bold</tspan> 😀</text><text class="label" data-mt-key="label:B" data-mt-label="true" x="4" y="48">Repeated</text></g></svg>"##,
        );
        let result = fonts.outline_svg(&svg).unwrap();
        let document = roxmltree::Document::parse(&result).unwrap();
        assert!(
            !document.descendants().any(|n| n.has_tag_name("text")),
            "font-dependent text remains"
        );
        for key in ["label:A", "label:B"] {
            let label = document
                .descendants()
                .find(|n| n.attribute("data-mt-key") == Some(key))
                .unwrap();
            assert!(label.has_tag_name("g"));
            let glyph = label.children().find(|n| n.has_tag_name("use")).unwrap();
            let id = glyph.attribute("href").unwrap().strip_prefix('#').unwrap();
            let definition = document
                .descendants()
                .find(|n| n.attribute("id") == Some(id))
                .unwrap();
            assert!(definition.parent().unwrap().has_tag_name("defs"));
            assert!(definition.descendants().any(|n| n.has_tag_name("path")));
            assert_eq!(label.attribute("data-mt-label"), Some("true"));
        }
        assert!(result.contains(r#"<metadata data-mt-native="[]"/>"#));
        assert!(result.contains(
            r#"<rect id="body" x="-10" y="-20" width="200" height="80" fill="yellow"/>"#
        ));
        let label = document
            .descendants()
            .find(|n| n.attribute("id") == Some("title"))
            .unwrap();
        assert_eq!(label.attribute("transform"), Some("rotate(10)"));
        assert!(
            label
                .descendants()
                .any(|n| n.has_tag_name("title") && n.text() == Some("Alpha bold 😀"))
        );
        let output = seal(&result);
        assert_eq!(output.as_str(), result);
    }
    #[test]
    fn invalid_negative_font_sizes_preserve_the_valid_cascade_and_glyphs() {
        let fonts = NativeFontContext::system().unwrap();
        for (invalid, valid) in [
            (r#"<text font-size="-1">Label</text>"#, "<text>Label</text>"),
            (
                r#"<text style="font-size:18px;font-size:-1px;fill:red">Label</text>"#,
                r#"<text style="font-size:18px;fill:red">Label</text>"#,
            ),
            (
                r#"<g style="font-size:-0.5%"><text>Label</text></g>"#,
                "<g><text>Label</text></g>",
            ),
            (
                r#"<style>.label{font-size:20px}.label{font-size:-1em!important}</style><text class="label">Label</text>"#,
                r#"<style>.label{font-size:20px}</style><text class="label">Label</text>"#,
            ),
            (
                r#"<style>.label{font-size:20px}.label,.other{font-size:-1px /* ; } */ !important;fill:red}</style><text class="label">Label</text>"#,
                r#"<style>.label{font-size:20px}.label,.other{fill:red}</style><text class="label">Label</text>"#,
            ),
            (
                r#"<style>.label[data-x="a:b"]{font-size:20px}.label{font-size:-1px}</style><text class="label" data-x="a:b">Label</text>"#,
                r#"<style>.label[data-x="a:b"]{font-size:20px}</style><text class="label" data-x="a:b">Label</text>"#,
            ),
        ] {
            let outline = |body: &str| {
                let svg = seal(&format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" id="invalid-font" width="100" height="100" font-size="24">{body}</svg>"#
                ));
                fonts.outline_svg(&svg).unwrap()
            };
            let glyphs = |svg: &str| {
                roxmltree::Document::parse(svg)
                    .unwrap()
                    .descendants()
                    .filter(|n| n.has_tag_name("path"))
                    .map(|n| n.attribute("d").unwrap().to_owned())
                    .collect::<Vec<_>>()
            };
            let expected = glyphs(&outline(valid));
            assert!(!expected.is_empty());
            assert_eq!(
                glyphs(&outline(invalid)),
                expected,
                "invalid font size must not hide text or replace earlier valid declarations: {invalid}"
            );
        }
    }

    #[test]
    fn nonpainting_labels_keep_ownership_without_inventing_glyphs() {
        let fonts = NativeFontContext::system().unwrap();
        for body in [
            "<text font-size=\"0\">Invisible</text>",
            "<style>.zero {font-size:0}</style><text class=\"zero\">Invisible</text>",
            "<g display=\"none\"><text>Hidden</text></g>",
            "<text visibility=\"hidden\">Hidden</text>",
            "<text>\u{200b}</text>",
        ] {
            let body = body.replacen(
                "<text",
                "<text data-mt-key=\"label:1\" data-mt-start=\"4\" data-mt-end=\"13\"",
                1,
            );
            let input = seal(&format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" id="invisible" width="100" height="100">{body}</svg>"#
            ));
            let result = seal(
                &fonts
                    .outline_svg(&input)
                    .unwrap_or_else(|error| panic!("{body}: {error}")),
            );
            let document = roxmltree::Document::parse(result.as_str()).unwrap();
            let label = document
                .descendants()
                .find(|n| n.attribute("data-mt-key") == Some("label:1"))
                .unwrap();
            assert_eq!(label.attribute("data-mt-start"), Some("4"));
            assert_eq!(label.attribute("data-mt-end"), Some("13"));
            assert!(!document.descendants().any(|n| n.has_tag_name("text")));
            #[cfg(feature = "png")]
            {
                let tree =
                    usvg::Tree::from_str(result.as_str(), &usvg::Options::default()).unwrap();
                let mut pixels = tiny_skia::Pixmap::new(100, 100).unwrap();
                resvg::render(&tree, usvg::Transform::identity(), &mut pixels.as_mut());
                assert!(
                    pixels.data().iter().all(|channel| *channel == 0),
                    "nonpainting label became visible: {body}"
                );
            }
        }
    }

    #[test]
    fn glyph_conversion_rejects_independent_nested_ownership() {
        let fonts = NativeFontContext::system().unwrap();
        let svg = seal(
            r#"<svg xmlns="http://www.w3.org/2000/svg" id="errors" width="100" height="100"><text><tspan data-mt-key="independent">Row</tspan></text></svg>"#,
        );
        assert!(
            fonts
                .outline_svg(&svg)
                .unwrap_err()
                .contains("separate text objects")
        );
    }

    #[test]
    fn glyph_conversion_reports_replacements_without_changing_source_ownership() {
        let fonts = NativeFontContext::system().unwrap();
        for body in [
            "Missing \u{10ffff}",
            "\u{10ffff}",
            "A<tspan>\u{10ffff}</tspan> &amp; B",
            "\u{85}A\u{85}",
            "A\u{10ffff}B\u{10fffe}",
        ] {
            let input = seal(&format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" id="missing" width="400" height="100"><metadata>original 􏿿</metadata><text data-mt-key="label:A" data-mt-start="3" data-mt-end="15" x="10" y="30" font-family="Arial" font-size="20">{body}</text></svg>"#
            ));
            let output = fonts.outline_svg_with_diagnostics(&input).unwrap();
            assert!(
                fonts
                    .outline_svg(&input)
                    .unwrap_err()
                    .contains("use outline_svg_with_diagnostics")
            );
            let expected_chars: BTreeSet<_> = body
                .chars()
                .filter(|c| matches!(c, '\u{85}' | '\u{10ffff}' | '\u{10fffe}'))
                .collect();
            assert_eq!(
                output.missing_characters,
                expected_chars.iter().copied().collect::<Vec<_>>()
            );
            let expected = fonts
                .outline_svg_with_diagnostics(&seal(
                    &input
                        .as_str()
                        .replace(['\u{85}', '\u{10ffff}', '\u{10fffe}'], "\u{fffd}"),
                ))
                .unwrap();
            let actual_doc = roxmltree::Document::parse(&output.svg).unwrap();
            let expected_doc = roxmltree::Document::parse(&expected.svg).unwrap();
            let paths = |doc: &roxmltree::Document<'_>| {
                doc.descendants()
                    .filter(|n| n.has_tag_name("path"))
                    .map(|n| n.attribute("d").unwrap().to_owned())
                    .collect::<Vec<_>>()
            };
            assert!(!paths(&actual_doc).is_empty());
            assert_eq!(paths(&actual_doc), paths(&expected_doc));
            let label = actual_doc
                .descendants()
                .find(|n| n.attribute("data-mt-key") == Some("label:A"))
                .unwrap();
            assert_eq!(label.attribute("data-mt-start"), Some("3"));
            assert_eq!(label.attribute("data-mt-end"), Some("15"));
            assert!(label.children().any(|n| n.has_tag_name("use")));
            let original_doc = roxmltree::Document::parse(input.as_str()).unwrap();
            let authored: String = original_doc
                .descendants()
                .filter(|n| n.is_text() && n.ancestors().any(|a| a.has_tag_name("text")))
                .filter_map(|n| n.text())
                .collect();
            assert_eq!(
                label
                    .children()
                    .find(|n| n.has_tag_name("title"))
                    .unwrap()
                    .text(),
                Some(authored.as_str())
            );
            assert!(output.svg.contains("<metadata>original 􏿿</metadata>"));
            seal(&output.svg);
        }
    }

    #[cfg(feature = "png")]
    #[test]
    fn glyph_conversion_preserves_native_pixels_without_fonts() {
        let fonts = NativeFontContext::system().unwrap();
        let svg = seal(
            r##"<svg xmlns="http://www.w3.org/2000/svg" id="pixels" width="450" height="220" viewBox="-10 -20 450 220"><defs><linearGradient id="paint"><stop stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient><clipPath id="clip"><rect width="420" height="190"/></clipPath></defs><style>.node path{fill:yellow!important;stroke:red!important}text{fill:#126789;font-family:Arial;font-size:22px}</style><g class="node" transform="translate(20 25)"><text id="mt-glyph-existing" x="8" y="30" opacity="0.7" transform="rotate(5)">AV fj <tspan font-weight="bold">bold</tspan><tspan font-style="italic">italic</tspan> 😀</text><text x="8" y="65" fill="url(#paint)" style="fill:url(#paint)">Gradient</text><text x="8" y="95">汉字 é العربية</text><text x="8" y="125" clip-path="url(#clip)">Repeated</text></g></svg>"##,
        );
        let result = seal(&fonts.outline_svg(&svg).unwrap());
        let options = usvg::Options {
            fontdb: Arc::clone(&fonts.fontdb),
            font_resolver: super::super::super::browser_like_font_resolver(),
            ..Default::default()
        };
        let original = usvg::Tree::from_str(svg.as_str(), &options).unwrap();
        // Reopened output has no font assets at all.
        let outlined = usvg::Tree::from_str(result.as_str(), &usvg::Options::default()).unwrap();
        let paint = |tree: &usvg::Tree| {
            let mut pixels = tiny_skia::Pixmap::new(450, 220).unwrap();
            resvg::render(tree, usvg::Transform::identity(), &mut pixels.as_mut());
            pixels
        };
        let before = paint(&original);
        let after = paint(&outlined);
        let changed = before
            .data()
            .iter()
            .zip(after.data())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(changed, 0, "glyph conversion changed rendered channels");
    }
    #[cfg(feature = "png")]
    #[test]
    fn glyph_conversion_preserves_text_selector_opacity() {
        let fonts = NativeFontContext::system().unwrap();
        for (css, parent, attributes) in [
            ("text{opacity:0.4}", "", ""),
            ("text.label{opacity:0.4!important}", "", ""),
            ("g{opacity:0.6} text{opacity:0.4}", "", ""),
            ("text{opacity:40%}", "opacity=\"0.5\"", ""),
            ("text{opacity:inherit}", "opacity=\"0.5\"", ""),
            ("text{opacity:0.4}", "", "style=\"opacity:0.8;fill:red\""),
            (
                "text{opacity:0.4!important}",
                "",
                "style=\"fill:red;opacity:0.8!important\"",
            ),
            ("text.label{opacity:0.4} text{opacity:0.8}", "", ""),
            ("text{opacity:0.4} text{opacity:0.8}", "", ""),
        ] {
            let input = seal(&format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" id="opacity" width="180" height="60"><style>{css}</style><g {parent}><text class="label" {attributes} x="8" y="35" font-family="Arial" font-size="24">Visible</text></g></svg>"#
            ));
            let output = seal(&fonts.outline_svg(&input).unwrap());
            let options = usvg::Options {
                fontdb: Arc::clone(&fonts.fontdb),
                font_resolver: super::super::super::browser_like_font_resolver(),
                ..Default::default()
            };
            let paint = |source: &str, options: &usvg::Options| {
                let tree = usvg::Tree::from_str(source, options).unwrap();
                let mut pixels = tiny_skia::Pixmap::new(180, 60).unwrap();
                resvg::render(&tree, usvg::Transform::identity(), &mut pixels.as_mut());
                pixels
            };
            let expected = paint(input.as_str(), &options);
            let actual = paint(output.as_str(), &usvg::Options::default());
            assert!(expected.data().iter().any(|value| *value > 0));
            assert_eq!(
                expected
                    .data()
                    .iter()
                    .zip(actual.data())
                    .filter(|(a, b)| a != b)
                    .count(),
                0,
                "{css}"
            );
        }
    }

    #[test]
    fn glyph_definition_ids_are_scoped_to_the_diagram() {
        let fonts = NativeFontContext::system().unwrap();
        let ids = ["one", "two"].map(|id| {
            let input = seal(&format!(r#"<svg xmlns="http://www.w3.org/2000/svg" id="{id}" width="100" height="100"><text y="30">Label</text></svg>"#));
            let output = fonts.outline_svg(&input).unwrap();
            let document = roxmltree::Document::parse(&output).unwrap();
            document.descendants().filter_map(|n| n.attribute("id").map(str::to_owned)).collect::<BTreeSet<_>>()
        });
        assert!(
            ids[0].is_disjoint(&ids[1]),
            "diagram glyph definitions must not cross-bind"
        );
    }
}
