use merman_core::{Engine, MermaidConfig, ParseOptions};
use serde_json::{Value, json};

fn check(config: Value, source: &str, allowed: bool) {
    let engine = Engine::new().with_site_config(MermaidConfig::from_value(config));
    for result in [
        engine
            .parse_diagram_sync(source, ParseOptions::default())
            .map(|v| v.is_some()),
        engine
            .parse_diagram_for_render_model_sync(source, ParseOptions::default())
            .map(|v| v.is_some()),
    ] {
        if allowed {
            assert!(result.unwrap(), "{source}");
        } else {
            let error = result
                .expect_err("maxEdges must reject the diagram")
                .to_string();
            assert!(error.contains("maxEdges"), "{error}");
        }
    }
}

#[test]
fn expanded_edges_obey_the_exact_host_limit_in_both_semantic_paths() {
    for (body, count) in [
        ("A --> B", 1),
        ("A --> B --> C", 2),
        ("A & B --> C & D", 4),
        ("A --> A\nA --> A", 2),
        ("A e@--> B\nA e@--> B", 2),
        (
            "subgraph G\nA --> B\nsubgraph H\nB --> C\nend\nend\nG --> D",
            3,
        ),
    ] {
        for trace in [false, true] {
            let source = format!("flowchart LR\n{body}");
            check(
                json!({"maxEdges": count, "traceSource":trace}),
                &source,
                true,
            );
            check(
                json!({"maxEdges": count - 1, "traceSource":trace}),
                &source,
                false,
            );
        }
    }
}

#[test]
fn zero_default_null_and_larger_host_limits_are_respected() {
    check(json!({"maxEdges":0}), "flowchart LR\nA[only node]", true);
    check(json!({"maxEdges":0}), "flowchart LR\nA --> B", false);
    for limit in [json!({}), json!({"maxEdges":null})] {
        check(
            limit.clone(),
            &format!("flowchart LR\n{}", "A --> B\n".repeat(500)),
            true,
        );
        check(
            limit,
            &format!("flowchart LR\n{}", "A --> B\n".repeat(501)),
            false,
        );
    }
    check(
        json!({"maxEdges":501}),
        &format!("flowchart LR\n{}", "A --> B\n".repeat(501)),
        true,
    );
    check(
        json!({"maxEdges":4.0}),
        "flowchart LR\nA & B --> C & D",
        true,
    );
    check(
        json!({"maxEdges":4.0}),
        "flowchart LR\nA & B --> C & D\nD --> A",
        false,
    );
}

#[test]
fn diagram_config_cannot_replace_a_protected_host_edge_limit() {
    for config in [json!({"maxEdges":100}), json!({"maxEdges":0})] {
        for prefix in [
            format!("%%{{init: {config}}}%%\n"),
            format!("---\nconfig: {config}\n---\n"),
        ] {
            check(
                json!({"maxEdges":1}),
                &format!("{prefix}flowchart LR\nA --> B"),
                true,
            );
            check(
                json!({"maxEdges":1}),
                &format!("{prefix}flowchart LR\nA --> B --> C"),
                false,
            );
        }
    }
    check(
        json!({"maxEdges":1,"secure":["secure"]}),
        "---\nconfig: {maxEdges: 2}\n---\nflowchart LR\nA --> B --> C",
        true,
    );
}

#[test]
fn invalid_edge_limits_report_configuration_errors() {
    for value in [json!(-1), json!(1.5), json!("2"), json!(true)] {
        check(json!({"maxEdges":value}), "flowchart LR\nA --> B", false);
    }
}

#[test]
fn flowchart_edge_limit_does_not_limit_sequence_messages_or_state_transitions() {
    for source in ["sequenceDiagram\nA->>B: hello", "stateDiagram-v2\nA --> B"] {
        check(json!({"maxEdges":0}), source, true);
    }
}

#[test]
fn object_or_array_overrides_keep_the_existing_numeric_limit() {
    for value in [json!({}), json!([1000])] {
        let engine =
            Engine::new().with_site_config(MermaidConfig::from_value(json!({"maxEdges":value})));
        assert_eq!(
            engine
                .parse_metadata_sync("flowchart LR\nA --> B")
                .unwrap()
                .effective_config
                .as_value()["maxEdges"],
            500
        );
        check(
            json!({"maxEdges":value}),
            &format!("flowchart LR\n{}", "A --> B\n".repeat(501)),
            false,
        );
    }
}
