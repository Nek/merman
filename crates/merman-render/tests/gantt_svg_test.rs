use merman_core::{Engine, MermaidConfig, ParseOptions};
use merman_render::LayoutOptions;
use merman_render::environment::{
    MeasurementProfileId, RenderEnvironment, TextMeasurementPolicy, TextMeasurementProfile,
    TextMeasurementProfileIdentity,
};
use merman_render::family;
use merman_render::model::GanttDiagramLayout;
use merman_render::svg::{SvgDebugOptions, SvgRenderOptions};
use merman_render::text::{TextMeasurer, TextMetrics, TextStyle};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn layout_gantt_from_text(text: &str) -> GanttDiagramLayout {
    layout_gantt_from_text_at_container_width(text, LayoutOptions::default().container_width)
}

fn layout_gantt_from_text_at_container_width(
    text: &str,
    container_width: f64,
) -> GanttDiagramLayout {
    let environment = RenderEnvironment::deterministic();
    layout_gantt_from_text_with_environment(text, container_width, &environment)
}

fn layout_gantt_from_text_with_environment(
    text: &str,
    container_width: f64,
    environment: &RenderEnvironment,
) -> GanttDiagramLayout {
    let parsed = Engine::new()
        .parse_diagram_for_render_model_sync(text, ParseOptions::default())
        .expect("parse ok")
        .expect("diagram detected");
    let mut options = LayoutOptions::default();
    options.container_width = container_width;
    let session = environment.begin_session().expect("render session");
    let artifact = family::prepare(parsed, &options, session).expect("layout ok");
    let projection = artifact.layout_json().expect("Gantt layout projection");
    serde_json::from_value(projection["layout"]["GanttDiagram"].clone()).expect("Gantt layout")
}

#[test]
fn gantt_layout_uses_the_operation_container_width_unless_config_overrides_it() {
    let source = "gantt\ndateFormat YYYY-MM-DD\nsection Delivery\nTask: 2024-01-01, 1d";
    let narrow = layout_gantt_from_text_at_container_width(source, 640.0);
    let wide = layout_gantt_from_text_at_container_width(source, 960.0);

    assert_eq!(narrow.width, 640.0);
    assert_eq!(wide.width, 960.0);

    let configured = layout_gantt_from_text_at_container_width(
        "---\nconfig:\n  gantt:\n    useWidth: 420\n---\ngantt\ndateFormat YYYY-MM-DD\nsection Delivery\nTask: 2024-01-01, 1d",
        960.0,
    );
    assert_eq!(configured.width, 420.0);
}

struct RawBBoxProbeMeasurer {
    calls: Arc<AtomicUsize>,
    width: f64,
}

impl TextMeasurer for RawBBoxProbeMeasurer {
    fn measure(&self, _text: &str, _style: &TextStyle) -> TextMetrics {
        panic!("Gantt task labels must use the raw SVG text bbox operation")
    }

    fn measure_svg_raw_text_bbox_width_px(&self, _text: &str, _style: &TextStyle) -> f64 {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.width
    }
}

#[test]
fn gantt_task_labels_route_through_raw_svg_bbox_measurement() {
    let calls = Arc::new(AtomicUsize::new(0));
    let profile = TextMeasurementProfile::new(
        TextMeasurementProfileIdentity::new(
            MeasurementProfileId::new("test.gantt-raw-bbox").unwrap(),
            "v1",
        )
        .unwrap(),
        Arc::new(RawBBoxProbeMeasurer {
            calls: Arc::clone(&calls),
            width: 200.0,
        }),
    );
    let environment = RenderEnvironment::deterministic()
        .with_text_measurement_policy(TextMeasurementPolicy::uniform(profile));
    let layout = layout_gantt_from_text_with_environment(
        "gantt\ndateFormat YYYY-MM-DD\nsection Delivery\nTask: task, 2024-01-01, 1d",
        1_184.0,
        &environment,
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(layout.tasks[0].label.width, 200.0);
}

#[test]
fn gantt_label_placement_uses_the_resolved_container_edges() {
    let profile = TextMeasurementProfile::new(
        TextMeasurementProfileIdentity::new(
            MeasurementProfileId::new("test.gantt-raw-bbox-placement").unwrap(),
            "v1",
        )
        .unwrap(),
        Arc::new(RawBBoxProbeMeasurer {
            calls: Arc::new(AtomicUsize::new(0)),
            width: 200.0,
        }),
    );
    let environment = RenderEnvironment::deterministic()
        .with_text_measurement_policy(TextMeasurementPolicy::uniform(profile));
    let layout = layout_gantt_from_text_with_environment(
        "gantt\ndateFormat YYYY-MM-DD\nsection Delivery\nFull range: full, 2024-01-01, 10d\nStart label: start, 2024-01-01, 1d\nEnd label: end, 2024-01-10, 1d",
        1_184.0,
        &environment,
    );
    let start = layout
        .tasks
        .iter()
        .find(|task| task.id == "start")
        .expect("start task");
    let end = layout
        .tasks
        .iter()
        .find(|task| task.id == "end")
        .expect("end task");

    assert!(start.label.class.contains("taskTextOutsideRight"));
    assert!(start.label.x > start.bar.x + start.bar.width);
    assert!(end.label.class.contains("taskTextOutsideLeft"));
    assert_eq!(end.label.x, end.bar.x - 5.0);
}

#[test]
fn gantt_layout_stops_at_the_maximum_utc_date_without_panicking() {
    // The raw model boundary case lives beside the private layout entry point.
}

fn render_gantt_svg_from_text(text: &str) -> String {
    render_gantt_svg_from_text_with_engine(Engine::new(), text)
}

fn render_gantt_svg_from_text_with_engine(engine: Engine, text: &str) -> String {
    let session = RenderEnvironment::deterministic()
        .with_runtime_policy(
            merman_core::runtime::RuntimePolicy::deterministic()
                .with_fixed_unix_millis(1_704_067_200_000),
        )
        .begin_session()
        .expect("begin render session");
    let parsed = futures::executor::block_on(
        engine.parse_diagram_for_render_model(text, ParseOptions::default()),
    )
    .expect("parse ok")
    .expect("diagram detected");
    let artifact = family::prepare(parsed, &LayoutOptions::default(), session).expect("layout ok");
    artifact
        .render_svg(
            &SvgRenderOptions {
                diagram_id: Some("gantt-config".to_string()),
                ..SvgRenderOptions::default()
            },
            &SvgDebugOptions::default(),
        )
        .expect("render svg")
        .svg()
        .to_owned()
}

#[test]
fn gantt_repeated_ids_keep_their_own_task_label_styles() {
    let svg = render_gantt_svg_from_text(
        "gantt\ndateFormat YYYY-MM-DD\nsection Work\nMarker :vert, dup, 2026-01-06, 1d\nFirst :done, dup, 2026-01-05, 1d\nSecond :active, dup, 2026-01-03, 1d\nThird :crit, dup, 2026-01-01, 1d\n",
    );
    let document = roxmltree::Document::parse(&svg).unwrap();
    let styles: Vec<_> = document
        .descendants()
        .filter(|node| node.has_tag_name("text"))
        .filter_map(|node| node.attribute("class"))
        .filter(|class| class.split_whitespace().any(|token| token == "taskText"))
        .collect();
    assert_eq!(styles.len(), 4);
    for (class, expected) in
        styles
            .iter()
            .zip(["critText0", "activeText0", "doneText0", "vertText"])
    {
        assert!(
            class.split_whitespace().any(|token| token == expected),
            "{class}"
        );
    }
}

#[test]
fn gantt_task_text_height_follows_final_svg_security_sanitization() {
    let source = r#"gantt
dateFormat YYYY-MM-DD
section Delivery
Task: task, 2024-01-01, 1d
"#;
    let strict = render_gantt_svg_from_text(source);
    let loose = render_gantt_svg_from_text_with_engine(
        Engine::new().with_site_config(MermaidConfig::from_value(serde_json::json!({
            "securityLevel": "loose"
        }))),
        source,
    );

    let task_text_heights = |svg: &str| {
        let document = roxmltree::Document::parse(svg).expect("valid Gantt SVG XML");
        document
            .descendants()
            .filter(|node| node.has_tag_name("text") && node.attribute("id").is_some())
            .filter_map(|node| node.attribute("text-height").map(str::to_owned))
            .collect::<Vec<_>>()
    };

    assert!(task_text_heights(&strict).is_empty(), "{strict}");
    assert_eq!(task_text_heights(&loose), vec!["20"], "{loose}");
}

#[test]
fn gantt_frontmatter_title_renders_unless_the_body_overrides_it() {
    let frontmatter_svg = render_gantt_svg_from_text(
        r#"---
title: Frontmatter schedule
---
gantt
dateFormat YYYY-MM-DD
section Delivery
Task: 2024-01-01, 1d
"#,
    );
    assert!(
        frontmatter_svg.contains(r#"class="titleText">Frontmatter schedule</text>"#),
        "frontmatter title should render when the Gantt body has none: {frontmatter_svg}"
    );

    let body_svg = render_gantt_svg_from_text(
        r#"---
title: Frontmatter schedule
---
gantt
title Body schedule
dateFormat YYYY-MM-DD
section Delivery
Task: 2024-01-01, 1d
"#,
    );
    assert!(body_svg.contains(r#"class="titleText">Body schedule</text>"#));
    assert!(!body_svg.contains(">Frontmatter schedule</text>"));
}

#[test]
fn gantt_explicit_whitespace_title_overrides_frontmatter_without_trimming() {
    let svg = render_gantt_svg_from_text(concat!(
        "---\n",
        "title: Frontmatter schedule\n",
        "---\n",
        "gantt\n",
        "title  \n",
        "dateFormat YYYY-MM-DD\n",
        "section Delivery\n",
        "Task: 2024-01-01, 1d\n",
    ));

    assert!(
        svg.contains(r#"class="titleText"> </text>"#),
        "the one remaining Jison separator must be rendered exactly: {svg}"
    );
    assert!(!svg.contains(">Frontmatter schedule</text>"));
}

#[test]
fn gantt_svg_frontmatter_config_fields_affect_visible_output() {
    let svg = render_gantt_svg_from_text(
        r#"---
displayMode: compact
config:
  gantt:
    useWidth: 420
    rightPadding: 10
    topAxis: true
    numberSectionStyles: 2
---
gantt
  title Config Frontmatter SVG Fields
  dateFormat YYYY-MM-DD
  axisFormat %Y-%m-%d
  tickInterval 1day
  todayMarker off
  section Alpha
  Task A :a1, 2024-01-01, 1d
  section Beta
  Task B :b1, 2024-01-02, 1d
"#,
    );

    assert!(
        svg.contains(r#"viewBox="0 0 420 "#)
            && svg.contains(r#"style="max-width: 420px; background-color: white;""#),
        "frontmatter gantt.useWidth should set rendered SVG width: {svg}"
    );
    assert_eq!(
        svg.matches(r#"<g class="grid" transform="translate(75, 50)""#)
            .count(),
        1,
        "frontmatter gantt.topAxis should add the top axis grid at top padding: {svg}"
    );
    assert_eq!(
        svg.matches(r#"<g class="grid" transform="translate(75, "#)
            .count(),
        2,
        "frontmatter gantt.topAxis should render both top and bottom axes: {svg}"
    );
    assert!(
        svg.contains(r#"width="415" height="24" class="section section0""#)
            && svg.contains(r#"width="415" height="24" class="section section1""#),
        "frontmatter gantt.rightPadding and numberSectionStyles should affect visible rows: {svg}"
    );
    assert!(
        svg.contains(r#"class="sectionTitle sectionTitle0""#)
            && svg.contains(r#"class="sectionTitle sectionTitle1""#)
            && svg.contains(r#"id="gantt-config-a1""#)
            && svg.contains(r#"id="gantt-config-b1-text""#),
        "configured Gantt SVG should expose section classes and scoped task DOM: {svg}"
    );
}

#[test]
fn gantt_section_font_size_preserves_css_units() {
    for (value, expected) in [("'1.5em'", "1.5em"), ("21", "21"), ("0", "0")] {
        let source = format!(
            "---\nconfig:\n  gantt:\n    sectionFontSize: {value}\n---\ngantt\ndateFormat YYYY-MM-DD\nsection Work\nTask :a, 2026-01-01, 1d\n"
        );
        let svg = render_gantt_svg_from_text(&source);
        let document = roxmltree::Document::parse(&svg).unwrap();
        let title = document
            .descendants()
            .find(|node| {
                node.has_tag_name("text")
                    && node
                        .attribute("class")
                        .is_some_and(|class| class.contains("sectionTitle"))
            })
            .expect("section title");
        assert_eq!(title.attribute("font-size"), Some(expected), "{source}");
    }
}

#[test]
fn gantt_configured_tick_interval_uses_diagram_override_and_both_axes() {
    let body = "gantt\ndateFormat YYYY-MM-DD\naxisFormat %Y-%m-%d\ntodayMarker off\nsection Work\nTask :a, 2026-01-01, 14d\n";
    let config = "---\nconfig:\n  gantt:\n    tickInterval: 2day\n    topAxis: true\n---\n";
    let layout = layout_gantt_from_text(&format!("{config}{body}"));
    let expected = [
        "2026-01-01",
        "2026-01-03",
        "2026-01-05",
        "2026-01-07",
        "2026-01-09",
        "2026-01-11",
        "2026-01-13",
        "2026-01-15",
    ];
    assert_eq!(layout.tick_interval.as_deref(), Some("2day"));
    assert_eq!(
        layout
            .bottom_ticks
            .iter()
            .map(|tick| tick.label.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        layout
            .top_ticks
            .iter()
            .map(|tick| tick.label.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    let svg = render_gantt_svg_from_text(&format!("{config}{body}"));
    assert_eq!(svg.matches("class=\"tick\"").count(), 2 * expected.len());

    let overridden = layout_gantt_from_text(&format!(
        "{config}gantt\ntickInterval 1day\n{}",
        body.strip_prefix("gantt\n").unwrap()
    ));
    assert_eq!(overridden.tick_interval.as_deref(), Some("1day"));
    assert_eq!(overridden.bottom_ticks.len(), 15);

    let weekly = layout_gantt_from_text(&format!(
        "---\nconfig:\n  gantt:\n    tickInterval: 1week\n    weekday: monday\n---\n{body}"
    ));
    assert_eq!(
        weekly
            .bottom_ticks
            .iter()
            .map(|tick| tick.label.as_str())
            .collect::<Vec<_>>(),
        ["2026-01-04", "2026-01-11"],
        "Mermaid's default Sunday database value takes precedence over config-only weekday"
    );
}

#[test]
fn gantt_svg_use_max_width_controls_root_sizing() {
    let body = "gantt\ndateFormat YYYY-MM-DD\nsection Work\nTask :a, 2026-01-01, 1d\n";
    for (use_max_width, width, height) in [(true, "100%", None), (false, "420", Some("124"))] {
        let source = format!(
            "---\nconfig:\n  gantt:\n    useWidth: 420\n    useMaxWidth: {use_max_width}\n---\n{body}"
        );
        let svg = render_gantt_svg_from_text(&source);
        let document = roxmltree::Document::parse(&svg).unwrap();
        let root = document.root_element();
        assert_eq!(root.attribute("width"), Some(width), "{source}");
        assert_eq!(root.attribute("height"), height, "{source}");
        assert_eq!(root.attribute("viewBox"), Some("0 0 420 124"));
        assert_eq!(
            root.attribute("style").unwrap().contains("max-width"),
            use_max_width
        );
    }
}

#[test]
fn gantt_narrow_plot_retains_signed_bar_width_and_label_placement() {
    for (width, bar_width, label_x) in [(149, -1.0, 79.0), (150, 0.0, 80.0), (151, 1.0, 81.0)] {
        let source = format!(
            "---\nconfig:\n  gantt:\n    useWidth: {width}\n---\ngantt\ndateFormat YYYY-MM-DD\nsection Work\nTask :a, 2026-01-01, 1d\n"
        );
        let layout = layout_gantt_from_text(&source);
        assert_eq!(layout.tasks[0].bar.width, bar_width, "{width}");
        assert_eq!(layout.tasks[0].label.x, label_x, "{width}");
        assert!(
            layout.tasks[0]
                .label
                .class
                .starts_with("taskTextOutsideRight"),
            "{width}"
        );
    }
}

#[test]
fn gantt_svg_subpixel_font_keeps_a_narrow_task_label_inside_its_bar() {
    let source = "---\nconfig:\n  gantt:\n    useWidth: 153\n    fontSize: 0.5\n---\ngantt\ndateFormat YYYY-MM-DD\nsection Work\nTask label :a, 2026-01-01, 1d\n";
    let svg = render_gantt_svg_from_text(source);
    let document = roxmltree::Document::parse(&svg).unwrap();
    let label = document
        .descendants()
        .find(|node| {
            node.has_tag_name("text")
                && node
                    .attribute("class")
                    .is_some_and(|class| class.starts_with("taskText"))
        })
        .unwrap();
    assert_eq!(label.attribute("font-size"), Some("0.5"));
    assert_eq!(label.attribute("x"), Some("76.5"));
    assert!(
        label
            .attribute("class")
            .unwrap()
            .starts_with("taskText taskText0"),
        "{svg}"
    );
}

#[test]
fn gantt_vertical_markers_do_not_affect_standard_row_layout() {
    let layout = layout_gantt_from_text(
        r#"
gantt
dateFormat YYYY-MM-DD
section Delivery
Start marker: vert,marker-start,2024-01-01,0d
Task A: task-a,2024-01-02,1d
Middle marker: vert,marker-middle,2024-01-05,0d
Task B: task-b,2024-01-06,1d
Final marker: vert,marker-final,2024-01-10,0d
"#,
    );
    assert_eq!(layout.height, 148.0);
    assert_eq!(
        layout.rows.iter().map(|row| row.index).collect::<Vec<_>>(),
        vec![0, 1]
    );

    let markers = layout
        .tasks
        .iter()
        .filter(|task| task.vert)
        .collect::<Vec<_>>();
    assert_eq!(markers.len(), 3);
    assert!(markers.iter().all(|task| task.order == -1));
    assert!(markers.iter().all(|task| task.bar.height == 88.0));
    assert!(markers.iter().all(|task| task.label.y == 143.0));

    let final_marker = markers
        .iter()
        .find(|task| task.id == "marker-final")
        .expect("final marker");
    assert_eq!(
        final_marker.bar.x,
        layout.width - layout.right_padding,
        "vertical markers must remain part of the time domain"
    );
}

#[test]
fn gantt_vertical_markers_do_not_affect_compact_row_packing() {
    let layout = layout_gantt_from_text(
        r#"---
displayMode: compact
---
gantt
dateFormat YYYY-MM-DD
section Delivery
Long marker: vert,marker-long,2024-01-01,31d
Task A: task-a,2024-01-01,1d
Task B: task-b,2024-01-03,1d
"#,
    );
    assert_eq!(layout.height, 124.0);
    assert_eq!(
        layout.rows.iter().map(|row| row.index).collect::<Vec<_>>(),
        vec![0]
    );
    assert_eq!(
        layout
            .tasks
            .iter()
            .map(|task| (task.id.as_str(), task.order))
            .collect::<Vec<_>>(),
        vec![("marker-long", -1), ("task-a", 0), ("task-b", 0)]
    );
}
