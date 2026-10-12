use super::NativeFontContext;
use merman_render::svg::ResvgCompatibleSvg;
use skera::{Plan, SubsetFlags, subset_font};
use std::collections::{BTreeMap, BTreeSet};
use write_fonts::read::{
    FontRef, TableProvider,
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
pub struct TextFontRun {
    /// UTF-8 range in the binding's normalized text.
    pub range: std::ops::Range<usize>,
    pub font: usvg::fontdb::ID,
}

#[derive(Debug)]
pub struct TextFontBinding {
    /// Original XML text-node range in FontAssets::svg, not a Mermaid source range.
    pub xml_range: std::ops::Range<usize>,
    pub text: String,
    /// Authored primary face before character fallback; may have no painted glyphs.
    pub primary_font: usvg::fontdb::ID,
    pub runs: Vec<TextFontRun>,
}

#[derive(Debug)]
pub struct FontAssets {
    /// Source-preserving draft with measured replacements in display text.
    pub svg: String,
    pub bindings: Vec<TextFontBinding>,
    /// IDs belong to this NativeFontContext; they are not persistent artifact identifiers.
    pub fonts: BTreeMap<usvg::fontdb::ID, FontAsset>,
    pub missing_characters: Vec<char>,
    /// Original PostScript face to portable primary face; glyph fallback uses the same database.
    pub font_substitutions: BTreeMap<String, String>,
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
        let (mut tree, _, missing, visible_source) =
            self.parse_with_replacements_and_stylesheet(svg.as_str(), None)?;
        let mut missing_characters: BTreeSet<_> = missing.into_iter().collect();
        // Discovery affects font collection only, never published text sizes or layout.
        let (discovery, _, missing, source) = self.parse_with_replacements_and_stylesheet(&visible_source, Some(
            "*{display:inline!important;visibility:visible!important;opacity:1!important;font-size:1px!important;fill:#000!important;fill-opacity:1!important;stroke:none!important}",
        ))?;
        missing_characters.extend(missing);
        if source != visible_source {
            let (updated, _, missing, updated_source) =
                self.parse_with_replacements_and_stylesheet(&source, None)?;
            if updated_source != source || !missing.is_empty() {
                return Err("Font replacements differ between visible and discovery runs".into());
            }
            tree = updated;
        }
        let mut coverage = BTreeMap::new();
        let mut bindings = BTreeMap::new();
        let mut primary_fonts = BTreeMap::new();
        let mut font_substitutions = BTreeMap::new();
        let mut database = tree.fontdb().clone();
        collect(
            tree.root(),
            &mut coverage,
            &mut bindings,
            &mut primary_fonts,
            &mut database,
            self.original_fontdb.as_deref(),
            &mut font_substitutions,
        )?;
        let mut hidden = BTreeMap::new();
        collect(
            discovery.root(),
            &mut coverage,
            &mut hidden,
            &mut primary_fonts,
            &mut database,
            self.original_fontdb.as_deref(),
            &mut font_substitutions,
        )?;
        for (range, (text, characters)) in hidden {
            let (existing_text, existing) = bindings
                .entry(range)
                .or_insert_with(|| (text.clone(), BTreeMap::new()));
            if *existing_text != text {
                return Err("Font discovery changed normalized text".into());
            }
            for (start, font) in characters {
                existing.entry(start).or_insert(font);
            }
        }
        let mut fonts = BTreeMap::new();
        let mut remaining = max_bytes;
        for (id, (characters, glyphs)) in coverage {
            let face = tree.fontdb().face(id).ok_or("Resolved font unavailable")?;
            let data = tree
                .fontdb()
                .with_face_data(id, |data, index| -> Result<Vec<u8>, String> {
                    let font = FontRef::from_index(data, index).map_err(|e| e.to_string())?;
                    // Keep original glyph IDs for legacy tables such as kern. Preserve shaping,
                    // variation, hinting, color and legacy name records (e.g. Apple Color Emoji).
                    // A PDF-only subset is not a browser font.
                    let first_char = characters.first().copied().unwrap_or(0).min(0xffff) as u16;
                    let last_char = characters.last().copied().unwrap_or(0).min(0xffff) as u16;
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
                    let subset = subset_font(&font, &plan).map_err(|e| e.to_string())?;
                    if font.data_for_tag(Tag::new(b"OS/2")).is_some() {
                        return Ok(subset);
                    }
                    // Some macOS TrueType faces omit OS/2, which web-font sanitizers require.
                    // Add only the missing table, deriving metrics from the native font.
                    let native = rustybuzz::ttf_parser::Face::parse(data, index)
                        .map_err(|e| e.to_string())?;
                    let mut selection = write_fonts::tables::os2::SelectionFlags::empty();
                    if native.is_italic() {
                        selection |= write_fonts::tables::os2::SelectionFlags::ITALIC;
                    }
                    if native.is_bold() {
                        selection |= write_fonts::tables::os2::SelectionFlags::BOLD;
                    }
                    if selection.is_empty() {
                        selection = write_fonts::tables::os2::SelectionFlags::REGULAR;
                    }
                    let (sum, count) = (0..native.number_of_glyphs())
                        .filter_map(|id| {
                            native.glyph_hor_advance(rustybuzz::ttf_parser::GlyphId(id))
                        })
                        .filter(|advance| *advance > 0)
                        .fold((0u64, 0u64), |(sum, count), advance| {
                            (sum + u64::from(advance), count + 1)
                        });
                    let average = sum / count.max(1);
                    let metrics = write_fonts::tables::os2::Os2 {
                        x_avg_char_width: i16::try_from(average)
                            .map_err(|_| "Font average advance exceeds OS/2 range")?,
                        us_weight_class: face.weight.0,
                        us_width_class: face.stretch.to_number(),
                        fs_selection: selection,
                        us_first_char_index: first_char,
                        us_last_char_index: last_char,
                        s_typo_ascender: native.ascender(),
                        s_typo_descender: native.descender(),
                        s_typo_line_gap: native.line_gap(),
                        us_win_ascent: native.ascender().max(0) as u16,
                        us_win_descent: (-i32::from(native.descender())).max(0) as u16,
                        ..Default::default()
                    };
                    let mut builder = write_fonts::FontBuilder::new();
                    builder.copy_missing_tables(FontRef::new(&subset).map_err(|e| e.to_string())?);
                    builder.add_table(&metrics).map_err(|e| e.to_string())?;
                    Ok(builder.build())
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
            svg: source,
            bindings: bindings
                .into_iter()
                .map(|((start, end), (text, characters))| {
                    let mut runs: Vec<TextFontRun> = Vec::new();
                    for (start, font) in characters {
                        let end = start
                            + text[start..]
                                .chars()
                                .next()
                                .expect("character boundary")
                                .len_utf8();
                        if let Some(last) = runs
                            .last_mut()
                            .filter(|run| run.font == font && run.range.end == start)
                        {
                            last.range.end = end;
                        } else {
                            runs.push(TextFontRun {
                                range: start..end,
                                font,
                            });
                        }
                    }
                    TextFontBinding {
                        xml_range: start..end,
                        text,
                        primary_font: primary_fonts[&(start, end)],
                        runs,
                    }
                })
                .collect(),
            fonts,
            missing_characters: missing_characters.into_iter().collect(),
            font_substitutions,
        })
    }
}

type Coverage = BTreeMap<usvg::fontdb::ID, (BTreeSet<u32>, BTreeSet<u16>)>;
type Bindings = BTreeMap<(usize, usize), (String, BTreeMap<usize, usvg::fontdb::ID>)>;

fn collect(
    group: &usvg::Group,
    coverage: &mut Coverage,
    bindings: &mut Bindings,
    primary_fonts: &mut BTreeMap<(usize, usize), usvg::fontdb::ID>,
    database: &mut std::sync::Arc<usvg::fontdb::Database>,
    original: Option<&usvg::fontdb::Database>,
    substitutions: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for node in group.children() {
        match node {
            usvg::Node::Group(group) => collect(
                group,
                coverage,
                bindings,
                primary_fonts,
                database,
                original,
                substitutions,
            )?,
            usvg::Node::Text(text) => {
                let resolver = super::super::browser_like_font_resolver();
                for span in text.chunks().iter().flat_map(|chunk| chunk.spans()) {
                    let Some(source) = span.source() else {
                        continue;
                    };
                    let primary = (resolver.select_font)(span.font(), database)
                        .ok_or("Primary font unavailable during asset preparation")?;
                    if let Some(original) = original {
                        if let Some(requested) =
                            super::super::select_font_case_insensitively(span.font(), original)
                        {
                            if database.face(requested).is_none() {
                                substitutions.insert(
                                    original
                                        .face(requested)
                                        .ok_or("Original font unavailable")?
                                        .post_script_name
                                        .clone(),
                                    database
                                        .face(primary)
                                        .ok_or("Portable font unavailable")?
                                        .post_script_name
                                        .clone(),
                                );
                            }
                        }
                    }
                    let key = (source.xml_range.start, source.xml_range.end);
                    if primary_fonts
                        .insert(key, primary)
                        .is_some_and(|prior| prior != primary)
                    {
                        return Err("One text source has conflicting primary fonts".into());
                    }
                }
                for glyph in text
                    .layouted()
                    .iter()
                    .flat_map(|span| &span.positioned_glyphs)
                {
                    if glyph.id.0 == 0 {
                        return Err("The resolved fonts lack a requested label glyph".into());
                    }
                    let (characters, glyphs) = coverage.entry(glyph.font).or_default();
                    characters.extend(glyph.text.chars().map(u32::from));
                    glyphs.insert(glyph.id.0);
                    let chunk = &text.chunks()[glyph.chunk_index];
                    let spans = chunk.spans();
                    let first =
                        spans.partition_point(|span| span.end() <= glyph.source_range.start);
                    for span in spans[first..]
                        .iter()
                        .take_while(|span| span.start() < glyph.source_range.end)
                    {
                        let start = span.start().max(glyph.source_range.start);
                        let end = span.end().min(glyph.source_range.end);
                        if start >= end {
                            continue;
                        }
                        let source = span
                            .source()
                            .ok_or("Resolved text has no direct XML source origin")?;
                        let (normalized, fonts) = bindings
                            .entry((source.xml_range.start, source.xml_range.end))
                            .or_insert_with(|| (source.text.to_string(), BTreeMap::new()));
                        if normalized != source.text.as_ref() {
                            return Err("Inconsistent normalized text origin".into());
                        }
                        for (offset, _) in chunk.text()[start..end].char_indices() {
                            let index = source.range.start + start - span.start() + offset;
                            if fonts
                                .insert(index, glyph.font)
                                .is_some_and(|prior| prior != glyph.font)
                            {
                                return Err(
                                    "One text cluster has conflicting resolved fonts".into()
                                );
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        let mut result = Ok(());
        node.subroots(|root| {
            if result.is_ok() {
                result = collect(
                    root,
                    coverage,
                    bindings,
                    primary_fonts,
                    database,
                    original,
                    substitutions,
                );
            }
        });
        result?;
    }
    Ok(())
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
    fn font_assets_bind_repeated_nested_and_hidden_text_to_exact_occurrences() {
        let fonts = NativeFontContext::system().unwrap();
        let source = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="150" font-family="Arial" font-size="16">
<title>Original {missing}</title><metadata>Original {missing}</metadata>
<text x="20" y="30" data-mt-start="1" data-mt-end="8">repeat <tspan font-weight="bold" data-mt-start="2">repeat</tspan> repeat</text>
<text x="20 40 60" y="60">ABC</text>
<text x="20" y="90" style="font-size:0;display:none">Alpha 😀 {missing}</text>
</svg>"#,
            missing = '\u{10ffff}'
        );
        let assets = fonts.font_assets(&seal(&source), 4_000_000).unwrap();
        assert_eq!(assets.missing_characters, ['\u{10ffff}']);
        assert!(assets.svg.contains("<title>Original \u{10ffff}</title>"));
        assert!(
            assets
                .svg
                .contains("<metadata>Original \u{10ffff}</metadata>")
        );
        assert!(assets.svg.contains(r#"data-mt-start="1" data-mt-end="8""#));
        let doc = roxmltree::Document::parse(&assets.svg).unwrap();
        let text_nodes: Vec<_> = doc
            .descendants()
            .filter(|n| n.is_text() && n.ancestors().any(|p| p.has_tag_name("text")))
            .collect();
        assert_eq!(assets.bindings.len(), text_nodes.len());
        for (binding, node) in assets.bindings.iter().zip(text_nodes) {
            assert_eq!(binding.xml_range, node.range());
            assert_eq!(binding.text, node.text().unwrap());
            let mut end = 0;
            for run in &binding.runs {
                assert_eq!(
                    run.range.start, end,
                    "font runs must cover the normalized text"
                );
                assert!(run.range.end > run.range.start);
                assert!(binding.text.is_char_boundary(run.range.start));
                assert!(binding.text.is_char_boundary(run.range.end));
                assert!(assets.fonts.contains_key(&run.font));
                end = run.range.end;
            }
            assert_eq!(end, binding.text.len());
        }
        assert!(assets.bindings.last().unwrap().text.contains('\u{fffd}'));
        assert_ne!(
            assets.bindings[0].runs[0].font, assets.bindings[1].runs[0].font,
            "nested bold font remains distinct"
        );
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
            ("A\u{20dd} A 😀", "normal", "normal"),
            ("🧑\u{200d}💻 123 ABC", "normal", "normal"),
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
    fn portable_courier_has_web_metrics_without_changing_native_geometry() {
        let fonts = NativeFontContext::system().unwrap();
        let text = "Task Line 123";
        let style = TextStyle {
            font_family: Some("Courier".into()),
            font_size: 16.0,
            ..Default::default()
        };
        let source = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="100"><text font-family="Courier" font-size="16">{text}</text></svg>"#
        );
        let assets = fonts.font_assets(&seal(&source), 4_000_000).unwrap();
        let mut database = usvg::fontdb::Database::new();
        for font in assets.fonts.values() {
            assert!(
                FontRef::new(&font.data).unwrap().os2().is_ok(),
                "browser fonts require OS/2 metrics: {}",
                font.post_script_name
            );
            database.load_font_data(font.data.clone());
        }
        let original = fonts.shape(text, &style).unwrap();
        let reopened = NativeFontContext::from_database(database)
            .unwrap()
            .shape(text, &style)
            .unwrap();
        assert_eq!(original.font_faces, reopened.font_faces);
        assert_eq!(original.bounds, reopened.bounds);
        assert_eq!(original.ink_bounds, reopened.ink_bounds);
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
