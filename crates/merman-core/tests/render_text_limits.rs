use merman_core::{Engine, MermaidConfig, OperationControl, ParseOptions};
use serde_json::{Value, json};

const SOURCES: [&str; 7] = [
    "flowchart LR\nA[Alpha 😀] --> B[Beta]",
    "flowchart-elk LR\nA[Alpha 😀] --> B[Beta]",
    "sequenceDiagram\nA->>B: Alpha 😀",
    "gantt\ndateFormat YYYY-MM-DD\nAlpha 😀 :a, 2026-01-01, 1d",
    "journey\nsection Work\nAlpha 😀: 5: A",
    "kanban\n  todo[Todo]\n    a[Alpha 😀]",
    "stateDiagram-v2\nstate \"Alpha 😀\" as A",
];

fn check(config: Value, source: &str, allowed: bool) {
    let engine = Engine::new().with_site_config(MermaidConfig::from_value(config));
    let diagram_type = engine.parse_metadata_sync(source).unwrap().diagram_type;
    for suppress_errors in [false, true] {
        let options = ParseOptions { suppress_errors };
        let control = OperationControl::new();
        for result in [
            engine.parse_diagram_for_render_model_sync(source, options),
            engine
                .parse_diagram_for_render_model_controlled_sync(source, options, &control)
                .unwrap(),
            engine.parse_diagram_for_render_model_with_type_sync(&diagram_type, source, options),
            engine
                .parse_diagram_for_render_model_with_type_controlled_sync(
                    &diagram_type,
                    source,
                    options,
                    &control,
                )
                .unwrap(),
        ] {
            if allowed {
                assert!(result.unwrap().is_some(), "{source}");
            } else {
                let error = result
                    .err()
                    .expect("over-limit render must fail even with suppression")
                    .to_string();
                assert!(
                    error.contains("maxTextSize") && error.contains("UTF-16"),
                    "{error}"
                );
            }
        }
    }
}

#[test]
fn original_utf16_source_defines_the_exact_render_boundary_for_every_family() {
    for body in SOURCES {
        for source in [
            body.to_owned(),
            format!("%% 😀 comment\n{body}\n"),
            format!("---\nconfig: {{maxTextSize: 999999}}\n---\n{body}"),
            format!("%%{{init: {{maxTextSize: 999999}}}}%%\n{body}"),
        ]
        .map(|s| s.replace('\n', "\r\n"))
        {
            let units = source.encode_utf16().count();
            assert!(source.len() > units, "astral UTF-16 fixture");
            for trace in [false, true] {
                check(
                    json!({"maxTextSize":units,"traceSource":trace}),
                    &source,
                    true,
                );
                check(
                    json!({"maxTextSize":units-1,"traceSource":trace}),
                    &source,
                    false,
                );
                check(
                    json!({"maxTextSize":units as f64 - 0.5,"traceSource":trace}),
                    &source,
                    false,
                );
            }
        }
    }
}

#[test]
fn default_zero_fractional_and_higher_limits_use_utf16_not_bytes_or_cleaned_text() {
    let body = "flowchart LR\nA[Alpha]\n%% ";
    let exact = format!("{body}{}", "x".repeat(50_000 - body.len()));
    check(json!({}), &exact, true);
    check(json!({}), &format!("{exact}x"), false);
    check(json!({"maxTextSize":50_001}), &format!("{exact}x"), true);
    check(json!({"maxTextSize":0}), SOURCES[0], false);
    let units = SOURCES[0].encode_utf16().count();
    check(json!({"maxTextSize":units as f64 + 0.5}), SOURCES[0], true);
    for value in [json!(null), json!({}), json!([100000])] {
        check(json!({"maxTextSize":value}), &exact, true);
        check(json!({"maxTextSize":value}), &format!("{exact}x"), false);
    }
}

#[test]
fn only_the_host_can_unlock_source_text_limits() {
    for prefix in [
        "---\nconfig: {maxTextSize: 0}\n---\n",
        "%%{init: {maxTextSize: 0}}%%\n",
    ] {
        let source = format!("{prefix}{}", SOURCES[0]);
        check(json!({}), &source, true);
        check(json!({"secure":["secure"]}), &source, false);
    }
}

#[test]
fn invalid_effective_text_limits_return_configuration_errors() {
    for value in [json!(-1), json!("100"), json!(true)] {
        check(json!({"maxTextSize":value}), SOURCES[0], false);
    }
}

#[test]
fn cancelled_render_remains_cancelled_and_metadata_and_json_are_not_rendered() {
    let engine =
        Engine::new().with_site_config(MermaidConfig::from_value(json!({"maxTextSize":0})));
    let control = OperationControl::new();
    control.cancel();
    assert!(
        engine
            .parse_diagram_for_render_model_controlled_sync(
                SOURCES[0],
                ParseOptions::default(),
                &control
            )
            .is_err()
    );
    assert!(engine.parse_metadata_sync(SOURCES[0]).is_ok());
    assert!(
        engine
            .parse_diagram_sync(SOURCES[0], ParseOptions::default())
            .unwrap()
            .is_some()
    );
    assert!(
        engine
            .parse_diagram_snapshot_sync(SOURCES[0])
            .unwrap()
            .is_some()
    );
}

#[test]
fn over_limit_source_is_rejected_before_invalid_family_syntax_can_be_suppressed() {
    check(json!({"maxTextSize":1}), "flowchart LR\nA -->", false);
}
