use super::{NativeFontContext, is_display_text};
use base64::Engine as _;
use merman_render::svg::ResvgCompatibleSvg;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct EmbeddedSvg {
    /// Source-preserving draft; the caller must terminally validate it.
    pub svg: String,
    pub missing_characters: Vec<char>,
    pub font_substitutions: BTreeMap<String, String>,
}

impl NativeFontContext {
    /// Prepare portable SVG text with exact resolved fonts and editable CSS sizes.
    pub fn embed_fonts(
        &self,
        svg: &ResvgCompatibleSvg,
        max_bytes: usize,
    ) -> Result<EmbeddedSvg, String> {
        let assets = self.font_assets(svg, max_bytes)?;
        let mut families = BTreeMap::new();
        let mut css = String::new();
        for (id, font) in &assets.fonts {
            let family = format!("merman-font-{:x}", Sha256::digest(&font.data));
            let style = match font.style {
                usvg::fontdb::Style::Normal => "normal",
                usvg::fontdb::Style::Italic => "italic",
                usvg::fontdb::Style::Oblique => "oblique",
            };
            let stretch = [
                "ultra-condensed",
                "extra-condensed",
                "condensed",
                "semi-condensed",
                "normal",
                "semi-expanded",
                "expanded",
                "extra-expanded",
                "ultra-expanded",
            ][usize::from(font.stretch.to_number()) - 1];
            let descriptors = format!(
                "font-weight:{};font-style:{style};font-stretch:{stretch}",
                font.weight
            );
            let mime = if font.data.starts_with(b"OTTO") {
                "otf"
            } else {
                "ttf"
            };
            css.push_str(&format!("@font-face{{font-family:{family};{descriptors};src:url(data:font/{mime};base64,{})}}", base64::engine::general_purpose::STANDARD.encode(&font.data)));
            families.insert(*id, family);
        }
        let document = roxmltree::Document::parse(&assets.svg).map_err(|e| e.to_string())?;
        let text_nodes: BTreeMap<_, _> = document
            .descendants()
            .filter(|node| is_display_text(*node))
            .map(|node| (node.range().start, node))
            .collect();
        let mut edits = Vec::new();
        // One stack per existing owner keeps source structure, selectors and inherited sizing.
        let mut owners = BTreeMap::new();
        for binding in &assets.bindings {
            let node = text_nodes
                .get(&binding.xml_range.start)
                .filter(|node| node.range() == binding.xml_range)
                .ok_or("Font binding has no original text node")?;
            let parent = node.parent().ok_or("Font binding has no text owner")?;
            let (_, primary, used) = owners.entry(parent.range().start).or_insert_with(|| {
                (
                    parent,
                    binding.primary_font,
                    std::collections::BTreeSet::new(),
                )
            });
            if *primary != binding.primary_font {
                return Err("One text owner has conflicting primary fonts".into());
            }
            used.extend(binding.runs.iter().map(|run| run.font));
            edits.push((
                binding.xml_range.clone(),
                quick_xml::escape::escape(&binding.text).into_owned(),
            ));
        }
        for (_, (parent, primary, used)) in owners {
            let mut order: Vec<_> = self
                .fontdb
                .faces()
                .map(|face| face.id)
                .filter(|id| used.contains(id))
                .collect();
            order.sort_by_key(|id| *id != primary);
            let stack: Vec<_> = order
                .iter()
                .map(|id| families.get(id).ok_or("Missing prepared font asset"))
                .collect::<Result<_, _>>()?;
            let style = format!(
                "font-family:{}!important;font-synthesis:none!important",
                stack
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if let Some(original) = parent.attribute_node("style") {
                edits.push((
                    original.range_value(),
                    quick_xml::escape::escape(&format!("{};{style}", original.value()))
                        .into_owned(),
                ));
            } else {
                let start = parent.range().start;
                let name_end = assets.svg[start..]
                    .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                    .ok_or("Missing text owner tag boundary")?;
                edits.push((
                    start + name_end..start + name_end,
                    format!(r#" style="{style}""#),
                ));
            }
        }
        if !assets.missing_characters.is_empty() {
            let original = roxmltree::Document::parse(svg.as_str()).map_err(|e| e.to_string())?;
            let draft = roxmltree::Document::parse(&assets.svg).map_err(|e| e.to_string())?;
            for (before, after) in original
                .descendants()
                .filter(|n| n.has_tag_name("text"))
                .zip(draft.descendants().filter(|n| n.has_tag_name("text")))
            {
                if before.attribute("aria-label").is_some()
                    || before.attribute("aria-labelledby").is_some()
                {
                    continue;
                }
                let text: String = before
                    .descendants()
                    .filter(|n| is_display_text(*n))
                    .filter_map(|n| n.text())
                    .collect();
                if text.chars().any(|c| assets.missing_characters.contains(&c)) {
                    let start = after.range().start;
                    let name_end = assets.svg[start..]
                        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                        .ok_or("Missing text tag boundary")?;
                    edits.push((
                        start + name_end..start + name_end,
                        format!(r#" aria-label="{}""#, quick_xml::escape::escape(&text)),
                    ));
                }
            }
        }
        let mut output = assets.svg;
        edits.sort_by_key(|(range, _)| range.start);
        for (range, value) in edits.into_iter().rev() {
            output.replace_range(range, &value);
        }
        if !css.is_empty() {
            let end = output.rfind("</").ok_or("Missing SVG closing tag")?;
            output.insert_str(end, &format!("<style>{css}</style>"));
        }
        Ok(EmbeddedSvg {
            svg: output,
            missing_characters: assets.missing_characters,
            font_substitutions: assets.font_substitutions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use merman_render::{environment::RenderEnvironment, svg::SvgPipeline};

    #[test]
    fn single_face_text_preserves_existing_element_structure() {
        let source = r#"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="100"><text font-family="Arial" x="10" y="30"><tspan class="row"><tspan class="label" style="fill:#333;font-size:16px">Alpha</tspan></tspan></text></svg>"#;
        let session = RenderEnvironment::deterministic().begin_session().unwrap();
        let pipeline = SvgPipeline::resvg_safe();
        let sealed = pipeline.process_resvg_compatible(source, &session).unwrap();
        let embedded = NativeFontContext::system()
            .unwrap()
            .embed_fonts(&sealed, 4_000_000)
            .unwrap();
        let final_svg = pipeline
            .process_resvg_compatible(&embedded.svg, &session)
            .unwrap();
        let doc = roxmltree::Document::parse(final_svg.as_str()).unwrap();
        let label = doc
            .descendants()
            .find(|n| n.attribute("class") == Some("label"))
            .unwrap();
        assert_eq!(
            label.children().filter(|n| n.is_element()).count(),
            0,
            "assign a single resolved face to the existing element without changing child selectors"
        );
        let style = label.attribute("style").unwrap();
        assert!(
            style.starts_with("fill:#333;font-size:16px;"),
            "preserve authored declarations: {style}"
        );
        assert!(style.contains("font-family:merman-font-"));
        assert_eq!(label.text(), Some("Alpha"));
    }

    #[test]
    fn mixed_face_text_preserves_existing_element_structure() {
        let source = r#"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="100"><text font-family="Arial" x="10" y="30"><tspan class="row"><tspan class="label" style="fill:#333;font-size:16px">Alpha 😀 中文</tspan></tspan></text></svg>"#;
        let session = RenderEnvironment::deterministic().begin_session().unwrap();
        let pipeline = SvgPipeline::resvg_safe();
        let sealed = pipeline.process_resvg_compatible(source, &session).unwrap();
        let embedded = NativeFontContext::system()
            .unwrap()
            .embed_fonts(&sealed, 4_000_000)
            .unwrap();
        let final_svg = pipeline
            .process_resvg_compatible(&embedded.svg, &session)
            .unwrap();
        let doc = roxmltree::Document::parse(final_svg.as_str()).unwrap();
        let label = doc
            .descendants()
            .find(|n| n.attribute("class") == Some("label"))
            .unwrap();
        assert_eq!(
            label.children().filter(|n| n.is_element()).count(),
            0,
            "font transport must not change structural CSS matches for mixed-font labels"
        );
        assert_eq!(label.text(), Some("Alpha 😀 中文"));
    }

    #[test]
    fn embedded_replacements_preserve_accessible_original_text() {
        let source = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100"><text id="label" x="20" y="30"><title>Original {missing}</title><desc>Description {missing}</desc>Before {missing} after</text></svg>"#,
            missing = '\u{10ffff}'
        );
        let session = RenderEnvironment::deterministic().begin_session().unwrap();
        let sealed = SvgPipeline::resvg_safe()
            .process_resvg_compatible(&source, &session)
            .unwrap();
        let output = NativeFontContext::system()
            .unwrap()
            .embed_fonts(&sealed, 4_000_000)
            .unwrap();
        assert_eq!(output.missing_characters, ['\u{10ffff}']);
        let doc = roxmltree::Document::parse(&output.svg).unwrap();
        assert_eq!(
            doc.descendants()
                .find(|n| n.has_tag_name("title"))
                .unwrap()
                .text(),
            Some("Original \u{10ffff}")
        );
        assert_eq!(
            doc.descendants()
                .find(|n| n.has_tag_name("desc"))
                .unwrap()
                .text(),
            Some("Description \u{10ffff}")
        );
        assert_eq!(
            doc.descendants()
                .find(|n| n.has_tag_name("text"))
                .unwrap()
                .attribute("aria-label"),
            Some("Before \u{10ffff} after")
        );
    }

    #[test]
    fn embedded_fonts_preserve_text_source_elements_and_authored_sizes() {
        let source = r#"<svg xmlns="http://www.w3.org/2000/svg" id="diagram" width="600" height="150" font-family="Arial">
<style>text > tspan {font-size:99px}</style>
<text id="label" x="20" y="30" font-size="0" data-mt-start="1" data-mt-end="20">Alpha 😀 <tspan id="child" font-size="10" style="font-size:10px">child</tspan></text>
</svg>"#;
        let session = RenderEnvironment::deterministic().begin_session().unwrap();
        let sealed = SvgPipeline::resvg_safe()
            .process_resvg_compatible(source, &session)
            .unwrap();
        let fonts = NativeFontContext::system().unwrap();
        let output = fonts
            .embed_fonts(&sealed, 4_000_000)
            .expect("embed resolved font assets");
        assert!(output.missing_characters.is_empty());
        let final_svg = SvgPipeline::resvg_safe()
            .process_resvg_compatible(&output.svg, &session)
            .unwrap();
        let doc = roxmltree::Document::parse(final_svg.as_str()).unwrap();
        let label = doc
            .descendants()
            .find(|n| n.attribute("id") == Some("label"))
            .unwrap();
        assert!(label.has_tag_name("text"));
        assert_eq!(label.attribute("font-size"), Some("0"));
        assert_eq!(label.attribute("data-mt-start"), Some("1"));
        assert_eq!(label.attribute("data-mt-end"), Some("20"));
        assert_eq!(
            label
                .descendants()
                .filter(|n| n.is_text())
                .filter_map(|n| n.text())
                .collect::<String>(),
            "Alpha 😀 child"
        );
        let child = doc
            .descendants()
            .find(|n| n.attribute("id") == Some("child"))
            .unwrap();
        assert_eq!(child.attribute("font-size"), Some("10"));
        assert!(
            child
                .attribute("style")
                .unwrap()
                .starts_with("font-size:10px;")
        );
        assert_eq!(child.children().filter(|n| n.is_element()).count(), 0);
        assert!(final_svg.as_str().contains("data:font/ttf;base64,"));
        assert!(
            fonts.embed_fonts(&sealed, 1).is_err(),
            "font byte budget stays mandatory"
        );
    }
}
