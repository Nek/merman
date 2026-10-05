//! Optional native font shaping for labels measured and drawn with the same font assets.
mod assets;
mod measurement;
mod outline;
pub use assets::{FontAsset, FontAssets};

use merman_render::text::TextStyle;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Debug)]
pub struct ShapedText {
    pub svg: String,
    pub bounds: Option<Bounds>,
    pub ink_bounds: Option<Bounds>,
    /// PostScript face names actually used, including fallback faces.
    pub font_faces: Vec<String>,
    /// Authored characters replaced by U+FFFD after installed font fallback failed.
    pub missing_characters: Vec<char>,
}

#[derive(Debug)]
pub struct OutlinedSvg {
    /// Source-preserving draft; the caller must terminally validate it.
    pub svg: String,
    pub missing_characters: Vec<char>,
}

/// Font assets are resolved once and shared by measurement and glyph production.
/// This is an opt-in host facility; it does not change Merman's default measurer.
pub struct NativeFontContext {
    fontdb: Arc<usvg::fontdb::Database>,
}

impl NativeFontContext {
    pub fn system() -> Result<Self, String> {
        let mut fontdb = super::shared_system_fontdb().as_ref().clone();
        // Apple's LastResort draws Unicode-category boxes, not the requested characters.
        // It must not prevent real character fallback to an installed content font.
        let last_resort: Vec<_> = fontdb
            .faces()
            .filter(|face| face.post_script_name == "LastResort")
            .map(|face| face.id)
            .collect();
        for id in last_resort {
            fontdb.remove_face(id);
        }
        Self::from_database(fontdb)
    }

    fn from_database(mut fontdb: usvg::fontdb::Database) -> Result<Self, String> {
        if fontdb.is_empty() {
            return Err("No fonts are available for native label shaping".into());
        }
        super::configure_fontdb_generic_families(&mut fontdb);
        Ok(Self {
            fontdb: Arc::new(fontdb),
        })
    }

    // Both measurement and final output must shape the same replacement, never estimated tofu.
    fn parse_with_replacements(
        &self,
        source: &str,
    ) -> Result<(usvg::Tree, Option<usvg::fontdb::ID>, Vec<char>), String> {
        self.parse_with_replacements_and_stylesheet(source, None)
    }

    fn parse_with_replacements_and_stylesheet(
        &self,
        source: &str,
        style_sheet: Option<&str>,
    ) -> Result<(usvg::Tree, Option<usvg::fontdb::ID>, Vec<char>), String> {
        let selected = Arc::new(Mutex::new(None));
        let primary = Arc::clone(&selected);
        let unresolved = Arc::new(Mutex::new(BTreeSet::new()));
        let missing = Arc::clone(&unresolved);
        let resolver = super::browser_like_font_resolver();
        let options = usvg::Options {
            style_sheet: style_sheet.map(str::to_owned),
            fontdb: Arc::clone(&self.fontdb),
            font_family: super::raster_default_font_family(&self.fontdb)
                .ok_or("No default font")?,
            font_resolver: usvg::FontResolver {
                select_font: Box::new(move |font, database| {
                    let id = (resolver.select_font)(font, database);
                    *primary.lock().expect("font recorder") = id;
                    id
                }),
                select_fallback: Box::new(move |character, used, database| {
                    let id = (resolver.select_fallback)(character, used, database);
                    if id.is_none() {
                        missing
                            .lock()
                            .expect("missing character recorder")
                            .insert(character);
                    }
                    id
                }),
            },
            image_href_resolver: super::data_url_only_image_href_resolver(),
            ..Default::default()
        };
        let document = roxmltree::Document::parse(source).map_err(|e| e.to_string())?;
        let text_nodes: Vec<_> = document
            .descendants()
            .filter(|n| is_display_text(*n))
            .collect();
        // The bidi layer can discard control characters before fallback runs. Check their
        // actual drawable coverage first (some fonts map controls to empty glyphs);
        // XML whitespace still follows SVG normalization.
        let mut replaced: BTreeSet<char> = text_nodes
            .iter()
            .flat_map(|n| n.text().unwrap_or_default().chars())
            .filter(|c| c.is_control() && !matches!(c, '\t' | '\r' | '\n'))
            .filter(|&c| {
                !self.fontdb.faces().any(|info| {
                    self.fontdb
                        .with_face_data(info.id, |data, index| {
                            rustybuzz::ttf_parser::Face::parse(data, index)
                                .ok()
                                .is_some_and(|face| {
                                    face.glyph_index(c).is_some_and(|id| {
                                        face.glyph_bounding_box(id).is_some()
                                            || face.is_color_glyph(id)
                                            || face.glyph_raster_image(id, u16::MAX).is_some()
                                            || face.glyph_svg_image(id).is_some()
                                    })
                                })
                        })
                        .unwrap_or(false)
                })
            })
            .collect();
        let replace = |characters: &BTreeSet<char>| {
            let mut output = source.to_owned();
            for node in text_nodes.iter().rev() {
                let text = node.text().unwrap_or_default();
                if text.chars().any(|c| characters.contains(&c)) {
                    let replacement: String = text
                        .chars()
                        .map(|c| {
                            if characters.contains(&c) {
                                '\u{fffd}'
                            } else {
                                c
                            }
                        })
                        .collect();
                    output.replace_range(node.range(), &quick_xml::escape::escape(&replacement));
                }
            }
            output
        };
        let mut shaping_source = replace(&replaced);
        loop {
            let tree =
                usvg::Tree::from_str(&shaping_source, &options).map_err(|e| e.to_string())?;
            let pending = std::mem::take(
                &mut *unresolved
                    .lock()
                    .map_err(|_| "Missing character recorder poisoned")?,
            );
            if pending.is_empty() {
                return Ok((
                    tree,
                    *selected.lock().map_err(|_| "Font recorder poisoned")?,
                    replaced.into_iter().collect(),
                ));
            }
            if pending.contains(&'\u{fffd}') || pending.iter().any(|c| replaced.contains(c)) {
                return Err("Installed fonts cannot draw the U+FFFD replacement symbol".into());
            }
            replaced.extend(pending);
            // usvg stops fallback at the first unavailable character in each run. Retry only
            // after replacing newly observed failures, retaining original source and metadata.
            shaping_source = replace(&replaced);
        }
    }

    /// Shape one plain text run at baseline (0, 0), preserving spaces.
    /// Formatted labels and line breaking remain the caller's responsibility.
    /// Both bounds and self-contained SVG derive from the same resolved font assets.
    pub fn shape(&self, text: &str, style: &TextStyle) -> Result<ShapedText, String> {
        if !style.font_size.is_finite()
            || style.font_size <= 0.0
            || style.font_size > f32::MAX as f64
        {
            return Err("Font size must be finite and positive".into());
        }
        if text.contains(['\r', '\n', '\t']) {
            return Err(
                "Shape one text run at a time; split line breaks and tabs before shaping".into(),
            );
        }
        if text.is_empty() {
            return Ok(ShapedText {
                svg: empty_svg(text),
                bounds: None,
                ink_bounds: None,
                font_faces: Vec::new(),
                missing_characters: Vec::new(),
            });
        }
        let default_family =
            super::raster_default_font_family(&self.fontdb).ok_or("No default font")?;
        let escape = |value: &str| quick_xml::escape::escape(value).into_owned();
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1" overflow="visible"><text xml:space="preserve" font-family="{}" font-size="{}" font-weight="{}" font-style="{}">{}</text></svg>"#,
            escape(style.font_family.as_deref().unwrap_or(&default_family)),
            style.font_size,
            escape(style.font_weight.as_deref().unwrap_or("normal")),
            escape(style.font_style.as_deref().unwrap_or("normal")),
            escape(text),
        );
        let (tree, selected, missing_characters) = self.parse_with_replacements(&svg)?;
        fn find_text(group: &usvg::Group) -> Option<&usvg::Text> {
            group.children().iter().find_map(|node| match node {
                usvg::Node::Text(text) => Some(text.as_ref()),
                usvg::Node::Group(group) => find_text(group),
                _ => None,
            })
        }
        let Some(shaped) = find_text(tree.root()) else {
            // usvg discards runs that paint nothing. Retain their real shaping advance,
            // not an estimated space width or an artificial visible sentinel glyph.
            let id = selected.ok_or("No font resolved for the text run")?;
            let bounds = self
                .fontdb
                .with_face_data(id, |data, index| {
                    let face =
                        rustybuzz::Face::from_slice(data, index).ok_or("Invalid font face")?;
                    let mut buffer = rustybuzz::UnicodeBuffer::new();
                    buffer.push_str(text);
                    buffer.guess_segment_properties();
                    let glyphs = rustybuzz::shape(&face, &[], buffer);
                    if glyphs.glyph_infos().iter().any(|glyph| glyph.glyph_id == 0) {
                        return Err("Font lacks a requested glyph");
                    }
                    if !text.chars().all(char::is_whitespace)
                        && !(glyphs
                            .glyph_positions()
                            .iter()
                            .all(|p| p.x_advance == 0 && p.y_advance == 0)
                            && glyphs.glyph_infos().iter().all(|g| {
                                face.glyph_bounding_box(rustybuzz::ttf_parser::GlyphId(
                                    g.glyph_id as u16,
                                ))
                                .is_none()
                            }))
                    {
                        return Err("The resolved fonts produced no drawable label glyphs");
                    }
                    let scale = style.font_size / f64::from(face.units_per_em());
                    Ok(Bounds {
                        left: 0.0,
                        top: -f64::from(face.ascender()) * scale,
                        right: glyphs
                            .glyph_positions()
                            .iter()
                            .map(|p| f64::from(p.x_advance) * scale)
                            .sum(),
                        bottom: -f64::from(face.descender()) * scale,
                    })
                })
                .ok_or("Font data unavailable")??;
            return Ok(ShapedText {
                missing_characters,
                svg: empty_svg(text),
                bounds: Some(bounds),
                ink_bounds: None,
                font_faces: vec![
                    self.fontdb
                        .face(id)
                        .ok_or("Font face unavailable")?
                        .post_script_name
                        .clone(),
                ],
            });
        };
        if shaped
            .layouted()
            .iter()
            .flat_map(|span| &span.positioned_glyphs)
            .any(|glyph| glyph.id.0 == 0)
        {
            return Err("The resolved fonts lack a requested label glyph".into());
        }
        let font_faces = shaped
            .layouted()
            .iter()
            .flat_map(|span| &span.positioned_glyphs)
            .filter_map(|glyph| tree.fontdb().face(glyph.font))
            .map(|face| face.post_script_name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let bounds = Bounds::from(shaped.bounding_box());
        let ink = Bounds::from(shaped.flattened().bounding_box());
        let viewport = Bounds {
            left: bounds.left.min(ink.left),
            top: bounds.top.min(ink.top),
            right: bounds.right.max(ink.right),
            bottom: bounds.bottom.max(ink.bottom),
        };
        let output = fit_glyph_svg(
            &tree.to_string(&usvg::WriteOptions::default()),
            text,
            viewport,
        )?;
        Ok(ShapedText {
            svg: output,
            bounds: Some(bounds),
            ink_bounds: Some(ink),
            font_faces,
            missing_characters,
        })
    }
}

// Only the SVG text content model paints characters; titles/descriptions keep authored text.
fn is_display_text(node: roxmltree::Node<'_, '_>) -> bool {
    node.is_text()
        && node.ancestors().any(|parent| parent.has_tag_name("text"))
        && node
            .ancestors()
            .skip(1)
            .take_while(|parent| !parent.has_tag_name("text"))
            .all(|parent| matches!(parent.tag_name().name(), "tspan" | "textPath" | "a"))
}

impl From<usvg::Rect> for Bounds {
    fn from(rect: usvg::Rect) -> Self {
        Self {
            left: rect.left().into(),
            top: rect.top().into(),
            right: rect.right().into(),
            bottom: rect.bottom().into(),
        }
    }
}

fn fit_glyph_svg(svg: &str, text: &str, bounds: Bounds) -> Result<String, String> {
    // Only the usvg-owned root is replaced. Its serialized children retain their definitions,
    // transforms and embedded color glyphs; this never rewrites a user's mapped SVG.
    let (_, children) = svg.split_once('>').ok_or("Missing glyph SVG root")?;
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    Ok(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{width}" height="{height}" viewBox="{} {} {width} {height}"><title>{}</title>{children}"#,
        bounds.left,
        bounds.top,
        quick_xml::escape::escape(text),
    ))
}

fn empty_svg(text: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><title>{}</title></svg>"#,
        quick_xml::escape::escape(text)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonpainting_text_metadata_does_not_trigger_missing_character_replacement() {
        let source = "<svg xmlns=\"http://www.w3.org/2000/svg\"><text y=\"20\"><title>Title \u{85}</title><desc>Description \u{85}</desc>Visible<tspan> label</tspan></text></svg>";
        let (_, _, missing) = NativeFontContext::system()
            .unwrap()
            .parse_with_replacements(source)
            .unwrap();
        assert!(
            missing.is_empty(),
            "nonpainting metadata cannot require a glyph: {missing:?}"
        );
    }

    #[test]
    fn unavailable_characters_use_measured_replacement_geometry() {
        let fonts = NativeFontContext::system().unwrap();
        let style = TextStyle {
            font_family: Some("Arial, sans-serif".into()),
            ..Default::default()
        };
        for (authored, painted) in [
            ("\u{10ffff}", "\u{fffd}"),
            (
                "A\u{10ffff}B\u{10fffe}C\u{10ffff}",
                "A\u{fffd}B\u{fffd}C\u{fffd}",
            ),
            ("\u{85}A\u{85}", "\u{fffd}A\u{fffd}"),
        ] {
            let actual = fonts
                .shape(authored, &style)
                .unwrap_or_else(|e| panic!("{authored:?}: {e}"));
            let expected = fonts.shape(painted, &style).unwrap();
            assert_eq!(actual.bounds, expected.bounds);
            assert_eq!(actual.ink_bounds, expected.ink_bounds);
            assert_eq!(actual.font_faces, expected.font_faces);
            assert_eq!(
                actual.missing_characters,
                authored
                    .chars()
                    .filter(|c| matches!(c, '\u{85}' | '\u{10ffff}' | '\u{10fffe}'))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            );
            assert!(expected.missing_characters.is_empty());
            let document = roxmltree::Document::parse(&actual.svg).unwrap();
            assert_eq!(
                document
                    .descendants()
                    .find(|n| n.has_tag_name("title"))
                    .unwrap()
                    .text(),
                Some(authored)
            );
        }
    }

    #[test]
    fn xml_valid_next_line_controls_keep_their_resolved_font_geometry() {
        let fonts = NativeFontContext::system().unwrap();
        let style = TextStyle {
            font_family: Some("Arial, sans-serif".into()),
            ..Default::default()
        };
        for text in ["\u{85}", "\u{85}A\u{85}"] {
            let shaped = fonts
                .shape(text, &style)
                .expect("NEL is valid SVG text, not an authored line break");
            assert!(shaped.bounds.is_some());
        }
    }

    #[test]
    fn font_policy_uses_real_bounds_for_layout_wrapping_and_baselines() {
        use merman_render::environment::{
            RenderEnvironment, TextMeasurementPhase, TextMeasurementSource,
        };
        use merman_render::text::{TextMeasurer, WrapMode};
        let fonts = Arc::new(NativeFontContext::system().unwrap());
        let session = RenderEnvironment::deterministic()
            .with_text_measurement_policy(Arc::clone(&fonts).measurement_policy())
            .begin_session()
            .unwrap();
        let measured = session.text_measurer(TextMeasurementPhase::Layout);
        let style = TextStyle {
            font_family: Some("Arial, sans-serif".into()),
            font_style: Some("italic".into()),
            ..Default::default()
        };
        let single = fonts.shape("AV fj", &style).unwrap();
        let bounds = single.bounds.unwrap();
        let ink = single.ink_bounds.unwrap();
        let left = bounds.left.min(ink.left);
        let right = bounds.right.max(ink.right);
        let top = bounds.top.min(ink.top);
        let bottom = bounds.bottom.max(ink.bottom);
        let actual = measured.measure("AV fj", &style);
        assert!(
            (actual.width - (right - left)).abs() < 0.001,
            "font width must reach layout: {actual:?}"
        );
        assert!(
            (actual.height - (bottom - top)).abs() < 0.001,
            "font height must reach layout: {actual:?}"
        );
        let rows = measured.measure_wrapped("AV fj<br/>AV fj", &style, None, WrapMode::SvgLike);
        assert_eq!(rows.line_count, 2);
        assert!((rows.height - (bottom - top + style.font_size * 1.1)).abs() < 0.001);
        let wrapped = measured.measure_wrapped(
            "AV fj AV fj",
            &style,
            Some(actual.width + 0.01),
            WrapMode::SvgLike,
        );
        assert_eq!(wrapped.line_count, 2);
        assert!((wrapped.height - rows.height).abs() < 0.001);
        assert!(
            (measured.measure_svg_create_text_bbox_y_offset_px("AV fj", &style)
                - (top + style.font_size))
                .abs()
                < 0.001
        );
        let report = session.text_measurement_report();
        assert!(
            report
                .entries()
                .iter()
                .all(
                    |entry| entry.provenance().source == TextMeasurementSource::Host
                        && entry.provenance().fallback_reason.is_none()
                ),
            "{report:?}"
        );
        measured.measure("\u{10ffff}", &style);
        assert!(
            session
                .text_measurement_report()
                .entries()
                .iter()
                .all(|entry| entry.provenance().fallback_reason.is_none()),
            "a measured replacement must not use approximate measurement fallback"
        );
    }

    #[test]
    fn native_labels_keep_font_bounds_and_portable_glyphs_together() {
        let fonts = NativeFontContext::system().expect("native tests require installed fonts");
        let base = TextStyle {
            font_family: Some("Arial, sans-serif".into()),
            ..Default::default()
        };
        for text in [
            "AV fj",
            "A & <B>",
            "e\u{301}",
            "مرحبا",
            "Alpha 中文",
            "Alpha 😀",
        ] {
            for (weight, italic) in [
                ("normal", "normal"),
                ("bold", "normal"),
                ("normal", "italic"),
            ] {
                let style = TextStyle {
                    font_weight: Some(weight.into()),
                    font_style: Some(italic.into()),
                    ..base.clone()
                };
                let shaped = fonts.shape(text, &style).unwrap();
                let logical = shaped.bounds.expect("actual font metrics must survive");
                let ink = shaped
                    .ink_bounds
                    .expect("visible text must have glyph bounds");
                assert!(logical.right > logical.left && logical.bottom > logical.top);
                assert!(ink.right > ink.left && ink.bottom > ink.top);
                assert!(
                    !shaped.svg.contains("<text"),
                    "portable output must not resolve fonts again"
                );
                assert!(
                    shaped.svg.contains("<title>"),
                    "authored text must remain accessible"
                );
                let reopened =
                    usvg::Tree::from_str(&shaped.svg, &usvg::Options::default()).unwrap();
                assert!(
                    reopened.size().width() as f64 >= ink.right - ink.left,
                    "glyph viewport must fit the actual label"
                );
                assert!(
                    reopened.size().height() as f64 >= ink.bottom - ink.top,
                    "glyph viewport must fit the actual label"
                );
                let actual = reopened.root().bounding_box();
                let left = logical.left.min(ink.left);
                let top = logical.top.min(ink.top);
                for (got, want) in [
                    (actual.left() as f64, ink.left - left),
                    (actual.top() as f64, ink.top - top),
                    (actual.right() as f64, ink.right - left),
                    (actual.bottom() as f64, ink.bottom - top),
                ] {
                    assert!(
                        (got - want).abs() < 0.01,
                        "saved glyph ink differs: {text:?} {got} != {want}"
                    );
                }
                let larger = fonts
                    .shape(
                        text,
                        &TextStyle {
                            font_size: 32.0,
                            ..style
                        },
                    )
                    .unwrap()
                    .bounds
                    .unwrap();
                assert!(
                    ((larger.right - larger.left) / (logical.right - logical.left) - 2.0).abs()
                        < 0.02
                );
            }
        }
    }

    #[test]
    fn native_labels_report_resolved_faces_and_missing_glyphs() {
        assert!(NativeFontContext::from_database(usvg::fontdb::Database::new()).is_err());
        let fonts = NativeFontContext::system().unwrap();
        let style = TextStyle {
            font_family: Some("Arial, sans-serif".into()),
            ..Default::default()
        };
        let plain = fonts.shape("A", &style).unwrap();
        assert!(
            !plain.font_faces.is_empty(),
            "resolved fonts must be explicit"
        );
        let fallback = fonts
            .shape(
                "A",
                &TextStyle {
                    font_family: Some("__missing_trace_font__, aRiAl, sans-serif".into()),
                    ..style.clone()
                },
            )
            .unwrap();
        assert_eq!(fallback.font_faces, plain.font_faces);
        assert_eq!(fallback.bounds, plain.bounds);
        assert!(
            !fonts
                .shape("中文 😀", &style)
                .unwrap()
                .font_faces
                .iter()
                .any(|face| face.contains("LastResort"))
        );
        assert_eq!(
            fonts
                .shape("\u{10ffff}", &style)
                .unwrap()
                .missing_characters,
            vec!['\u{10ffff}']
        );
        assert!(
            fonts
                .shape("中文 😀", &style)
                .unwrap()
                .missing_characters
                .is_empty()
        );
        assert!(fonts.shape("line\nbreak", &style).is_err());
        assert!(
            fonts
                .shape(
                    "text",
                    &TextStyle {
                        font_size: 0.0,
                        ..style
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn native_labels_preserve_whitespace_advances_and_empty_input() {
        let fonts = NativeFontContext::system().unwrap();
        let style = TextStyle {
            font_family: Some("monospace".into()),
            ..Default::default()
        };
        let empty = fonts.shape("", &style).unwrap();
        assert!(empty.bounds.is_none() && empty.ink_bounds.is_none());
        let invisible = fonts.shape("\u{200b}", &style).unwrap();
        assert!(invisible.ink_bounds.is_none());
        assert_eq!(invisible.bounds.unwrap().right, 0.0);
        let space = fonts.shape(" ", &style).unwrap();
        assert!(space.bounds.is_some(), "whitespace has real font advance");
        assert!(space.ink_bounds.is_none(), "whitespace paints nothing");
        let advance = space.bounds.unwrap().right;
        let two = fonts.shape("  ", &style).unwrap().bounds.unwrap();
        assert!((two.right - 2.0 * advance).abs() < 0.01);
        let plain = fonts.shape("A", &style).unwrap().bounds.unwrap();
        let padded = fonts.shape(" A ", &style).unwrap().bounds.unwrap();
        assert!(
            ((padded.right - padded.left) - (plain.right - plain.left) - 2.0 * advance).abs()
                < 0.01,
            "plain={plain:?} padded={padded:?} advance={advance}"
        );
        assert!(
            fonts
                .shape(
                    "text",
                    &TextStyle {
                        font_size: f64::NAN,
                        ..style
                    }
                )
                .is_err()
        );
    }
}
