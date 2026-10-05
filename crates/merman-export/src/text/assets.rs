use super::NativeFontContext;
use merman_render::svg::ResvgCompatibleSvg;
use skera::{Plan, SubsetFlags, subset_font};
use std::collections::{BTreeMap, BTreeSet};
use write_fonts::read::{
    FontRef,
    collections::IntSet,
    types::{GlyphId, Tag},
};

/// A resolved OpenType face prepared for embedding, without source file paths.
#[derive(Debug)]
pub struct FontAsset {
    pub post_script_name: String,
    pub weight: u16,
    pub style: usvg::fontdb::Style,
    pub stretch: usvg::fontdb::Stretch,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct FontAssets {
    /// IDs belong to this NativeFontContext; they are not persistent artifact identifiers.
    pub fonts: BTreeMap<usvg::fontdb::ID, FontAsset>,
    pub missing_characters: Vec<char>,
}

impl NativeFontContext {
    /// Subset fonts for current text, including zero-size or hidden text a later CSS edit reveals.
    /// This does not rewrite or validate font CSS.
    /// The caller supplies the total output byte limit from its artifact resource policy.
    pub fn font_assets(
        &self,
        svg: &ResvgCompatibleSvg,
        max_bytes: usize,
    ) -> Result<FontAssets, String> {
        let (tree, _, missing_characters) = self.parse_with_replacements(svg.as_str())?;
        let mut missing_characters: BTreeSet<_> = missing_characters.into_iter().collect();
        type Coverage = BTreeMap<usvg::fontdb::ID, (BTreeSet<u32>, BTreeSet<u16>)>;
        fn collect(group: &usvg::Group, coverage: &mut Coverage) {
            for node in group.children() {
                match node {
                    usvg::Node::Group(group) => collect(group, coverage),
                    usvg::Node::Text(text) => {
                        for glyph in text
                            .layouted()
                            .iter()
                            .flat_map(|span| &span.positioned_glyphs)
                        {
                            let (characters, glyphs) = coverage.entry(glyph.font).or_default();
                            characters.extend(glyph.text.chars().map(u32::from));
                            glyphs.insert(glyph.id.0);
                        }
                    }
                    _ => {}
                }
                node.subroots(|subroot| collect(subroot, coverage));
            }
        }
        let mut coverage = BTreeMap::new();
        collect(tree.root(), &mut coverage);
        // A static artifact must also carry fonts for text that a later CSS edit reveals.
        // This copy discovers assets only; its geometry never participates in diagram layout.
        let (visible, _, missing) = self.parse_with_replacements_and_stylesheet(svg.as_str(), Some(
            "*{display:inline!important;visibility:visible!important;opacity:1!important;font-size:1px!important;fill:#000!important;fill-opacity:1!important;stroke:none!important}",
        ))?;
        collect(visible.root(), &mut coverage);
        missing_characters.extend(missing);
        let mut fonts = BTreeMap::new();
        let mut remaining = max_bytes;
        for (id, (characters, glyphs)) in coverage {
            let face = tree.fontdb().face(id).ok_or("Resolved font unavailable")?;
            let data = tree
                .fontdb()
                .with_face_data(id, |data, index| {
                    let font = FontRef::from_index(data, index).map_err(|e| e.to_string())?;
                    // Keep original glyph IDs for legacy tables such as kern. Preserve shaping,
                    // variation, hinting, color and legacy name records (e.g. Apple Color Emoji).
                    // A PDF-only subset is not a browser font.
                    let plan = Plan::new(
                        &glyphs
                            .into_iter()
                            .map(|id| GlyphId::new(u32::from(id)))
                            .collect(),
                        &characters.into_iter().collect(),
                        &font,
                        SubsetFlags::SUBSET_FLAGS_RETAIN_GIDS
                            | SubsetFlags::SUBSET_FLAGS_PASSTHROUGH_UNRECOGNIZED
                            | SubsetFlags::SUBSET_FLAGS_NAME_LEGACY,
                        &[Tag::new(b"DSIG")].into_iter().collect(),
                        &IntSet::all(),
                        &IntSet::all(),
                        &IntSet::all(),
                        &IntSet::all(),
                    );
                    subset_font(&font, &plan).map_err(|e| e.to_string())
                })
                .ok_or("Resolved font data unavailable")??;
            remaining = remaining
                .checked_sub(data.len())
                .ok_or("Font assets exceed the total byte budget")?;
            fonts.insert(
                id,
                FontAsset {
                    post_script_name: face.post_script_name.clone(),
                    weight: face.weight.0,
                    style: face.style,
                    stretch: face.stretch,
                    data,
                },
            );
        }
        Ok(FontAssets {
            fonts,
            missing_characters: missing_characters.into_iter().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use merman_render::{environment::RenderEnvironment, svg::SvgPipeline, text::TextStyle};

    fn seal(source: &str) -> ResvgCompatibleSvg {
        let session = RenderEnvironment::deterministic().begin_session().unwrap();
        SvgPipeline::resvg_safe()
            .process_resvg_compatible(source, &session)
            .unwrap()
    }

    #[test]
    fn embedded_subsets_keep_resolved_shaping_and_color_glyph_geometry() {
        let fonts = NativeFontContext::system().unwrap();
        for (text, weight, style) in [
            ("AV office e\u{301}", "normal", "normal"),
            ("AV office e\u{301}", "bold", "italic"),
            ("مرحبا لا", "normal", "normal"),
            ("Alpha 中文", "normal", "normal"),
            ("Alpha 😀", "normal", "normal"),
            ("A\u{10ffff}B", "normal", "normal"),
        ] {
            let mut style = TextStyle {
                font_family: Some("Arial, sans-serif".into()),
                font_size: 16.0,
                font_weight: Some(weight.into()),
                font_style: Some(style.into()),
                ..Default::default()
            };
            let source = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100"><text x="20" y="50" font-family="Arial, sans-serif" font-size="16" font-weight="{}" font-style="{}">{}</text></svg>"#,
                style.font_weight.as_deref().unwrap(),
                style.font_style.as_deref().unwrap(),
                quick_xml::escape::escape(text)
            );
            let sealed = seal(&source);
            let assets = fonts
                .font_assets(&sealed, 4_000_000)
                .expect("prepare browser font assets");
            assert!(!assets.fonts.is_empty());
            let mut database = usvg::fontdb::Database::new();
            for asset in assets.fonts.values() {
                database.load_font_data(asset.data.clone());
            }
            let reopened = NativeFontContext::from_database(database).unwrap();
            for size in [11.0, 16.0, 22.0] {
                style.font_size = size;
                let original = fonts.shape(text, &style).unwrap();
                let subset = reopened
                    .shape(text, &style)
                    .unwrap_or_else(|e| panic!("{text:?} at {size}px: {e}"));
                assert_eq!(
                    subset.bounds, original.bounds,
                    "logical bounds: {text} at {size}px"
                );
                assert_eq!(
                    subset.ink_bounds, original.ink_bounds,
                    "painted bounds: {text} at {size}px"
                );
                assert_eq!(
                    subset.font_faces, original.font_faces,
                    "resolved faces: {text}"
                );
                assert_eq!(assets.missing_characters, original.missing_characters);
            }
        }
    }

    #[test]
    fn nonpainting_text_keeps_fonts_for_later_css_reveal() {
        let fonts = NativeFontContext::system().unwrap();
        let style = TextStyle {
            font_family: Some("Arial, sans-serif".into()),
            font_size: 16.0,
            ..Default::default()
        };
        let expected = fonts.shape("Alpha 😀", &style).unwrap();
        for body in [
            r#"<text font-size="0">Alpha 😀</text>"#,
            r#"<style>*{font-size:0!important;display:none!important}</style><text style="font-size:0!important;fill:none!important">Alpha 😀</text>"#,
            r#"<g style="display:none"><text>Alpha 😀</text></g>"#,
            r#"<g style="visibility:hidden;opacity:0"><text>Alpha 😀</text></g>"#,
            r#"<text style="fill:none;stroke:none">Alpha 😀</text>"#,
            r#"<text>Alpha <tspan style="font-size:0;display:none">😀</tspan></text>"#,
        ] {
            let source = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="500" height="100" font-family="Arial, sans-serif">{body}</svg>"#
            );
            let assets = fonts.font_assets(&seal(&source), 4_000_000).unwrap();
            assert!(
                !assets.fonts.is_empty(),
                "hidden text must retain its font assets: {body}"
            );
            let mut database = usvg::fontdb::Database::new();
            for font in assets.fonts.values() {
                database.load_font_data(font.data.clone());
            }
            let reopened = NativeFontContext::from_database(database).unwrap();
            let actual = reopened.shape("Alpha 😀", &style).unwrap();
            assert_eq!(
                actual.bounds, expected.bounds,
                "revealed logical bounds: {body}"
            );
            assert_eq!(
                actual.ink_bounds, expected.ink_bounds,
                "revealed painted bounds: {body}"
            );
            assert!(assets.missing_characters.is_empty());
        }
        let missing = seal(&format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><text style="display:none;font-size:0">A{}B</text></svg>"#,
            '\u{10ffff}'
        ));
        let assets = fonts.font_assets(&missing, 4_000_000).unwrap();
        assert_eq!(assets.missing_characters, ['\u{10ffff}']);
        assert!(
            !assets.fonts.is_empty(),
            "hidden missing characters retain replacement assets"
        );
    }

    #[test]
    fn font_assets_enforce_total_budget_and_reuse_repeated_faces() {
        let fonts = NativeFontContext::system().unwrap();
        let svg = seal(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><text x="0" y="20" font-family="Arial">Alpha</text><text x="0" y="50" font-family="Arial">Beta</text><text x="0" y="80" font-family="Arial" font-weight="bold">Bold</text></svg>"#,
        );
        let assets = fonts.font_assets(&svg, 1_000_000).unwrap();
        assert_eq!(
            assets.fonts.len(),
            2,
            "repeated regular labels share a face; bold retains its own"
        );
        let size: usize = assets.fonts.values().map(|font| font.data.len()).sum();
        assert!(size > 0);
        assert!(fonts.font_assets(&svg, size).is_ok());
        assert!(
            fonts
                .font_assets(&svg, size - 1)
                .unwrap_err()
                .contains("byte budget")
        );
        assert!(
            fonts
                .font_assets(&svg, 0)
                .unwrap_err()
                .contains("byte budget")
        );
        let blank = seal(r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#);
        assert!(fonts.font_assets(&blank, 0).unwrap().fonts.is_empty());
    }
}
