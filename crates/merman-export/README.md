# merman-export

`merman-export` is the bounded binary-export layer behind Merman's PNG, JPEG, and PDF output. It encodes SVG that has already passed Merman's terminal resvg-compatible finalizer; it does not parse Mermaid source or choose a layout engine.

Most applications should depend on [`merman`](https://crates.io/crates/merman) and select its
typed PNG, JPEG, or PDF target. Use this crate directly only when the application needs to retain a
validated SVG artifact, inspect an allocation plan, or schedule encoding separately from Mermaid
rendering.

## Choose A Feature

The crate has no default features. Enable only the formats the application emits:

| Feature | Output | Main API |
| --- | --- | --- |
| `png` | Bounded PNG bitmap | `svg_to_png`, `prepare_raster`, `RasterOptions`, `RasterPlan` |
| `jpeg` | Bounded JPEG bitmap | `svg_to_jpeg`, `prepare_raster`, `RasterOptions`, `RasterPlan` |
| `pdf` | Vector PDF with bounded localized raster work | `svg_to_pdf`, `svg_to_pdf_with_options`, `prepare_pdf`, `PdfOptions` |
| `text` | Native font metrics and portable glyph labels | `text::NativeFontContext` |

`png` and `jpeg` share private bitmap preparation. `pdf` is a separate vector export capability. Features are additive, but one output does not implicitly expose another output's API. The published `merman-export`, `merman`, and `merman-render` versions must match because the sealed SVG type crosses their crate boundaries.

## First Export

The `merman` facade owns the shortest source-to-output path. This dependency enables PNG and its
required basic SVG path without Cytoscape, ELK, or math engines:

```toml
[dependencies]
merman = { version = "=0.8.0-alpha.6", default-features = false, features = ["png"] }
```

```rust
use merman::svg::export::{RasterFitBox, RasterOptions};
use merman::{OperationControl, PngRequest, RenderOutput, RenderRequest, Renderer, SvgRequest};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = RasterOptions::default()
        .with_fit_to(RasterFitBox::contain(960, 540))
        .with_scale(2.0)
        .with_background("white");

    let output = Renderer::new().render(RenderRequest::png(
        "flowchart LR\n  Source --> PNG",
        OperationControl::new(),
        PngRequest {
            svg: SvgRequest::default(),
            options,
        },
    ))?;
    let RenderOutput::Png(Some(png)) = output else {
        return Err("no Mermaid diagram detected".into());
    };

    std::fs::write("diagram.png", png.bytes)?;
    Ok(())
}
```

Replace `png` with `jpeg` or `pdf` when only that format is required. JPEG uses `RasterOptions`; PDF uses its independent `PdfOptions` page and filter policy.

## Direct Encoding

A host that already owns SVG can run the terminal finalizer and encoder under one caller-owned
operation control:

```toml
[dependencies]
merman-core = { version = "=0.8.0-alpha.6", default-features = false }
merman-render = { version = "=0.8.0-alpha.6", default-features = false }
merman-export = { version = "=0.8.0-alpha.6", default-features = false, features = ["png"] }
```

```rust
use merman_core::OperationControl;
use merman_export::{RasterOptions, svg_to_png_controlled};
use merman_render::{environment::RenderEnvironment, svg::finalize_resvg_svg};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let control = OperationControl::new();
    let session = RenderEnvironment::deterministic()
        .begin_session_with_control(control.clone())?;
    let sealed = finalize_resvg_svg(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40">
              <text x="8" y="24">Render then encode</text>
            </svg>"#,
        &session,
    )?;
    let png = svg_to_png_controlled(&sealed, &RasterOptions::default(), control)?;
    std::fs::write("diagram.png", png)?;
    Ok(())
}
```

The encoder APIs accept `merman_render::svg::ResvgCompatibleSvg`, whose inner string cannot be forged directly. This type attests that the terminal resvg-compatible finalizer ran. It does not attest that the input originated from Mermaid: `merman_render::svg::finalize_resvg_svg` intentionally accepts arbitrary SVG and returns the sealed type after finalization. Applications that accept untrusted SVG still need appropriate source, time, memory, and concurrency policy.

The sealed producer contract intentionally keeps `merman-render` in this crate's resolved dependency closure. `merman-export` is therefore not a standalone arbitrary-SVG converter, and its crate boundary alone is not evidence that parsing or rendering dependencies were removed. Exact PNG, JPEG, and PDF closure claims are verified from the repository's artifact profiles.

All formats keep explicit allocation, embedded-image, and structural conversion limits. See the main project's [SVG, PNG, JPEG, and PDF output guide](https://github.com/Latias94/merman/blob/main/docs/rendering/RASTER_OUTPUT.md) for sizing policy, resource controls, PDF behavior, and known parity gaps.

## License

Licensed under either of Apache License, Version 2.0 or MIT at your option.

## Native font labels

The optional `text` feature resolves installed fonts through the existing case-insensitive font
resolver. `NativeFontContext::system()?.shape(text, &style)` returns logical bounds, glyph ink
bounds, the actual PostScript face names (including fallback) and a standalone glyph SVG with
accessible authored text. Both bounds use the original baseline at (0, 0); the SVG viewport fits
their union. Reopening the artifact needs no installed fonts. Empty and whitespace-only runs
have no ink; whitespace keeps its shaped advance. Missing fonts/glyphs and invalid sizes are
errors. Apple's LastResort category-box font is excluded from this context's fallback database.

The host measurement policy also accepts zero-size labels: all horizontal and vertical bounds
are zero, without approximate fallback. This does not make an invisible label drawable.

This is a plain-run host primitive, not a complete formatted-label renderer or a change to
Merman's default approximate profile. Callers own input admission, cancellation, line breaking,
formatted run composition, source wrappers and final text paint. Control characters (including
tabs/newlines) must be handled before shaping a run. The final artifact never comes from replacing a mapped
Mermaid document with usvg serialization. System discovery remains process-cached and host-dependent;
glyph artifacts retain their resolved appearance rather than promising identical font choices
on different machines. Integrating this primitive into Trace's layout and SVG emission remains
required before claiming font-accurate diagram output.

`Arc<NativeFontContext>::measurement_policy()` connects these fonts to Merman's existing
operation-aware layout/wrapping/SVG measurement routes. Wrapped vertical bounds use the retained
line plan from Merman's existing wrapper; HTML's authored 1.5em line spacing remains distinct from
glyph ink. The formatted SVG baseline offset is checked against the emitted tspan structure.
Font failures are reported through host fallback provenance. A font-accurate producer must reject
that provenance before publishing and pair this policy with final glyph output from the same
context. The Architecture-only middle-baseline operation is explicitly declined; no equivalent
measurement is guessed. This policy is not enabled by Trace's production renderer yet.

Font resolution failures are recorded even when a text object paints nothing. Zero-size, hidden
and nonprinting labels retain empty source wrappers. `shape` measures unavailable characters as
U+FFFD with the same resolved-font geometry used for drawing and lists the original code points
in `missing_characters`. Normal installed-font fallback happens first. If the replacement itself
cannot be drawn, rendering still fails explicitly. Original accessible text and source metadata
are never replaced. SVG whitespace normalization remains distinct from unavailable controls.
Negative font-size lengths are ignored in the temporary shaping copy, matching CSS cascade
behavior across attributes, inline styles and stylesheets. Original SVG styles remain unchanged;
the existing CSS parsers preserve selector bytes and unrelated declarations.

`NativeFontContext::outline_svg(&sealed_svg)` converts final text objects into font-independent
SVG glyph definitions referenced from their original source wrappers. It accepts a terminally
validated `ResvgCompatibleSvg` with a nonempty root diagram ID and returns a **draft**: validate
that draft through the existing SVG pipeline before publication. Source attributes, original
non-text SVG and accessible label text survive. Definition IDs are scoped to the diagram; only
referenced paint resources are copied. Independently mapped descendant text spans are rejected,
not merged. Producers must expose independent labels as separate text objects. The existing
`outline_svg` entry point retains strict rejection of unavailable characters. To accept measured
replacements, use `outline_svg_with_diagnostics`, which returns `OutlinedSvg { svg,
missing_characters }`; the host must surface those diagnostics. Both entry points share shaping
and source-preserving conversion. Measurement does not silently fall back to approximate metrics
when it measures a replacement.

Glyph definitions keep node/connector path selectors away from lettering; resolved paint also
uses inline declarations. Text-local opacity is materialized on its replacement wrapper using
the same static selector/declaration rules as usvg; ancestor opacity remains on its original
group. Generated glyph groups retain their own resolved opacity, so diagram group selectors
do not apply that effect again. Other text-local compositing effects still require verification. Native regression checks retain an adversarial `!important` node-path
rule and compare exact raster pixels with no fonts available on reopening. Measurement and
outlining must share the same context, and portable source bindings must be applied before
outlining if their producer still depends on text elements. The new operation is an opt-in draft
transformation, not a production pipeline preset or a claim of complete font/configuration
coverage. Its usvg call is synchronous; the integrating host still owns admission, operation
controls and terminal resource validation.

`NativeFontContext::font_assets(&sealed_svg, max_bytes)` prepares OpenType subsets of
faces used by the SVG's shaped glyphs. Assets retain Unicode mapping, layout,
variation, hinting, color data and legacy names; IDs refer to the originating
font context, not persistent SVG identities. The operation shares missing-character
replacement with measurement and reports original missing code points. Repeated
faces share one subset, and the total returned font bytes must fit `max_bytes`.
This limit bounds output, not the subsetter's peak working memory.

Asset preparation leaves the input SVG untouched and does not admit `@font-face`
CSS or embed fonts in a terminal artifact. Nonpainting/zero-size text has no
shaped glyphs to collect; a future embedding operation must resolve those fonts
before claiming CSS reveal support. Source-preserving fallback bindings, safe
font CSS admission and saved/browser resize acceptance remain integration work.
