use super::NativeFontContext;
use merman_render::svg::ResvgCompatibleSvg;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

impl NativeFontContext {
    /// Return a draft with text replaced by portable glyphs and source ownership preserved.
    /// The input must have a diagram root ID. The caller must revalidate this draft before publication.
    pub fn outline_svg(&self, svg: &ResvgCompatibleSvg) -> Result<String, String> {
        let source = svg.as_str();
        let document = roxmltree::Document::parse(source).map_err(|e| e.to_string())?;
        let labels: Vec<_> = document
            .descendants()
            .filter(|n| n.has_tag_name("text"))
            .collect();
        if labels.is_empty() {
            return Ok(source.to_owned());
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
        let unresolved = Arc::new(Mutex::new(None));
        let missing = Arc::clone(&unresolved);
        let resolver = super::super::browser_like_font_resolver();
        let options = usvg::Options {
            fontdb: Arc::clone(&self.fontdb),
            font_family: super::super::raster_default_font_family(&self.fontdb)
                .ok_or("No default font")?,
            font_resolver: usvg::FontResolver {
                select_font: resolver.select_font,
                select_fallback: Box::new(move |character, used, database| {
                    let selected = (resolver.select_fallback)(character, used, database);
                    if selected.is_none() {
                        *missing.lock().expect("local missing glyph recorder") = Some(character);
                    }
                    selected
                }),
            },
            image_href_resolver: super::super::data_url_only_image_href_resolver(),
            ..Default::default()
        };
        let shaping_source = omit_negative_font_sizes(&shaping_source)?;
        let tree = usvg::Tree::from_str(&shaping_source, &options).map_err(|e| e.to_string())?;
        if let Some(character) = *unresolved
            .lock()
            .map_err(|_| "Missing glyph recorder poisoned")?
        {
            return Err(format!(
                "The resolved fonts lack a requested label glyph: {character:?}"
            ));
        }
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
                .map(|a| &source[a.range()])
                .collect::<Vec<_>>()
                .join(" ");
            let title = if text.is_empty() {
                String::new()
            } else {
                format!("<title>{}</title>", quick_xml::escape::escape(&text))
            };
            replacements.push((
                label.range(),
                format!(r#"<g xmlns:xlink="http://www.w3.org/1999/xlink" {attributes}>{title}{glyphs}</g>"#),
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
        Ok(output)
    }
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
    let mut ranges = Vec::new();
    for declaration in declarations.iter().filter(|d| {
        d.name.eq_ignore_ascii_case("font-size")
            && d.value
                .trim()
                .parse::<svgtypes::Length>()
                .is_ok_and(|n| n.number < 0.0)
    }) {
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
    for path in node
        .descendants()
        .filter(|n| n.has_tag_name("path"))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        // Resolved glyph paint must win against shape selectors such as `.node path`.
        let defaults = [
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
        ];
        let style = defaults
            .iter()
            .map(|(key, default)| {
                format!(
                    "{key}:{}!important",
                    path.attribute(*key).unwrap_or(default)
                )
            })
            .collect::<Vec<_>>()
            .join(";");
        fragment.insert_str(
            path.range().start - range.start + "<path".len(),
            &format!(
                r#" marker-start="none" marker-mid="none" marker-end="none" style="{}""#,
                quick_xml::escape::escape(&style)
            ),
        );
    }
    fragment
}

#[cfg(test)]
mod tests {
    use super::*;
    use merman_render::{environment::RenderEnvironment, svg::SvgPipeline};

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
    fn glyph_conversion_rejects_independent_nested_ownership_and_missing_glyphs() {
        let fonts = NativeFontContext::system().unwrap();
        for (body, message) in [
            (
                r#"<text><tspan data-mt-key="independent">Row</tspan></text>"#,
                "separate text objects",
            ),
            (
                "<text>Missing \u{10ffff}</text>",
                "lack a requested label glyph",
            ),
            ("<text>\u{10ffff}</text>", "lack a requested label glyph"),
            (
                "<text><tspan>\u{10ffff}</tspan></text>",
                "lack a requested label glyph",
            ),
        ] {
            let svg = seal(&format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" id="errors" width="100" height="100">{body}</svg>"#
            ));
            assert!(fonts.outline_svg(&svg).unwrap_err().contains(message));
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
