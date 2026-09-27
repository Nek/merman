use merman::{
    Engine, MermaidConfig, OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest,
};
use serde_json::{Value, json};

#[test]
fn native_flowchart_provenance_keeps_occurrences_and_renderer_identities() {
    let source = "flowchart LR\r\n%% 😀\r\nA[\"same\"] -->|same| B[\"same\"]\r\nA --> B\r\n";
    let renderer = Renderer::new().with_engine(Engine::new().with_site_config(
        MermaidConfig::from_value(json!({"traceSource":true,"htmlLabels":false})),
    ));
    let RenderOutput::Svg(Some(output)) = renderer
        .render(RenderRequest::svg(
            source,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .unwrap()
    else {
        panic!("missing SVG")
    };
    let svg = roxmltree::Document::parse(output.svg()).unwrap();
    let map: Vec<Value> = serde_json::from_str(
        svg.descendants()
            .find_map(|node| node.attribute("data-mt-native"))
            .unwrap(),
    )
    .unwrap();
    let slice = |span: &Value| {
        &source[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize]
    };
    assert_eq!(
        map.iter()
            .filter(|p| p["kind"] == "node")
            .map(|p| slice(&p["span"]))
            .collect::<Vec<_>>(),
        ["A[\"same\"]", "B[\"same\"]", "A", "B"]
    );
    let edges: Vec<_> = map.iter().filter(|p| p["kind"] == "edge").collect();
    assert_eq!(
        edges.iter().map(|p| slice(&p["span"])).collect::<Vec<_>>(),
        ["-->|same|", "-->"]
    );
    assert_eq!(slice(&edges[0]["labelSpan"]), "same");
    assert_ne!(edges[0]["domId"], edges[1]["domId"]);
    for edge in edges {
        let key = edge["domId"].as_str().unwrap();
        assert!(
            svg.descendants()
                .any(|n| n.attribute("data-mt-key") == Some(key) && n.has_tag_name("path"))
        );
    }
    let RenderOutput::Svg(Some(plain)) = Renderer::new()
        .render(RenderRequest::svg(
            source,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .unwrap()
    else {
        panic!("missing SVG")
    };
    assert!(
        !plain.svg().contains("data-mt-"),
        "provenance must remain opt-in"
    );
}

#[test]
fn native_shape_data_provenance_retains_yaml_bytes_and_authored_label_order() {
    let source = "flowchart LR\r\n%% 😀\r\nA@{label: \"Earlier 😀\"}\r\nA[\"Last 😀\"]\r\nM@{shape: rounded, label: \"First\r\n   second 😀\", unused: ignored}\r\n";
    let renderer = Renderer::new().with_engine(Engine::new().with_site_config(
        MermaidConfig::from_value(json!({"traceSource":true,"htmlLabels":false})),
    ));
    let RenderOutput::Svg(Some(output)) = renderer
        .render(RenderRequest::svg(
            source,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .unwrap()
    else {
        panic!("missing SVG")
    };
    let svg = roxmltree::Document::parse(output.svg()).unwrap();
    let map: Vec<Value> = serde_json::from_str(
        svg.descendants()
            .find_map(|node| node.attribute("data-mt-native"))
            .unwrap(),
    )
    .unwrap();
    let slice = |span: &Value| {
        &source[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize]
    };
    let effective_a = map
        .iter()
        .find(|p| p["domId"] == "node:A" && p["effective"] == true)
        .unwrap();
    assert_eq!(slice(&effective_a["labelSpan"]), "Last 😀");
    let effective_m = map
        .iter()
        .find(|p| p["domId"] == "node:M" && p["effective"] == true)
        .unwrap();
    assert_eq!(
        slice(&effective_m["span"]),
        "M@{shape: rounded, label: \"First\r\n   second 😀\", unused: ignored}"
    );
    assert_eq!(slice(&effective_m["labelSpan"]), "First\r\n   second 😀");
    assert!(
        map.iter()
            .any(|p| p["property"] == "shape" && slice(&p["span"]) == "rounded")
    );
    assert!(map.iter().any(|p| p["kind"] == "nonvisual"
        && p["classification"] == "ignored-shape-data-property"
        && slice(&p["span"]) == "ignored"));
    let a = svg
        .descendants()
        .find(|n| n.attribute("data-mt-key") == Some("node:A"))
        .unwrap();
    let displayed: String = a
        .descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect();
    assert!(displayed.contains("Last 😀"));
    assert!(!displayed.contains("Earlier 😀"));
}

#[test]
fn native_directive_relationships_and_edge_ids_keep_original_utf8_ranges() {
    let source = "flowchart LR\r\n%% 😀\r\nclassDef hot fill:#eee\r\nA[\"Actor 😀\"]:::hot e1@--> B\r\nclass A,e1 hot\r\nclick A href \"https://example.com\"\r\nlinkStyle default stroke-width:2px\r\nclass Missing hot\r\n";
    let renderer = Renderer::new().with_engine(Engine::new().with_site_config(
        MermaidConfig::from_value(json!({"traceSource":true,"htmlLabels":false})),
    ));
    let RenderOutput::Svg(Some(output)) = renderer
        .render(RenderRequest::svg(
            source,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .unwrap()
    else {
        panic!("missing SVG")
    };
    let svg = roxmltree::Document::parse(output.svg()).unwrap();
    let map: Vec<Value> = serde_json::from_str(
        svg.descendants()
            .find_map(|node| node.attribute("data-mt-native"))
            .unwrap(),
    )
    .unwrap();
    let slice = |span: &Value| {
        &source[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize]
    };
    for (key, relation, text) in [
        ("node:A", "inline-class", ":::hot"),
        ("node:A", "class", "class A,e1 hot"),
        ("edge:e1", "class", "class A,e1 hot"),
        ("node:A", "classDef", "classDef hot fill:#eee"),
        ("edge:e1", "classDef", "classDef hot fill:#eee"),
        ("node:A", "click", "click A href \"https://example.com\""),
        ("edge:e1", "linkStyle", "linkStyle default stroke-width:2px"),
    ] {
        assert!(
            map.iter().any(|p| p["domId"] == key
                && p["relation"] == relation
                && slice(&p["span"]) == text),
            "{key} {relation}"
        );
    }
    assert!(map.iter().any(|p| p["domId"] == "edge:e1"
        && p.get("relation").is_none()
        && slice(&p["span"]) == "e1@-->"));
    assert!(map.iter().any(|p| p["kind"] == "nonvisual"
        && p["classification"] == "unresolved-directive-target"
        && slice(&p["span"]) == "class Missing hot"));
}

#[test]
fn native_scoped_directions_retain_group_identity_without_changing_direction_resolution() {
    let source = "flowchart LR\r\n%% 😀\r\ndirection BT\r\nsubgraph G[Group]\r\ndirection TB\r\nA --> B\r\ndirection RL\r\nend\r\n";
    let renderer = Renderer::new().with_engine(Engine::new().with_site_config(
        MermaidConfig::from_value(json!({"traceSource":true,"htmlLabels":false})),
    ));
    let RenderOutput::Svg(Some(output)) = renderer
        .render(RenderRequest::svg(
            source,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .unwrap()
    else {
        panic!("missing SVG")
    };
    let svg = roxmltree::Document::parse(output.svg()).unwrap();
    let map: Vec<Value> = serde_json::from_str(
        svg.descendants()
            .find_map(|node| node.attribute("data-mt-native"))
            .unwrap(),
    )
    .unwrap();
    let slice = |span: &Value| {
        &source[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize]
    };
    let directions: Vec<_> = map
        .iter()
        .filter(|p| p["domId"] == "flowchart:subgraph:G" && p["relation"] == "direction")
        .map(|p| slice(&p["span"]))
        .collect();
    assert_eq!(directions, ["direction TB", "direction RL"]);
    assert!(
        map.iter()
            .any(|p| p["classification"] == "ignored-root-direction"
                && slice(&p["span"]) == "direction BT")
    );
}

#[test]
fn native_accessibility_metadata_retains_bytes_and_complete_occurrence_precedence() {
    let source = "flowchart LR\r\n%% 😀\r\n accTitle : Earlier\r\naccTitle: Last 😀\r\naccDescr {\r\n First 😀\r\n second\r\n}\r\nA --> B\r\naccDescr {Unfinished";
    let renderer = Renderer::new().with_engine(Engine::new().with_site_config(
        MermaidConfig::from_value(json!({"traceSource":true,"htmlLabels":false})),
    ));
    let RenderOutput::Svg(Some(output)) = renderer
        .render(RenderRequest::svg(
            source,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .unwrap()
    else {
        panic!("missing SVG")
    };
    let svg = roxmltree::Document::parse(output.svg()).unwrap();
    let map: Vec<Value> = serde_json::from_str(
        svg.descendants()
            .find_map(|node| node.attribute("data-mt-native"))
            .unwrap(),
    )
    .unwrap();
    let slice = |span: &Value| {
        &source[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize]
    };
    for (statement, payload, effective) in [
        ("accTitle : Earlier", "Earlier", false),
        ("accTitle: Last 😀", "Last 😀", true),
        (
            "accDescr {\r\n First 😀\r\n second\r\n}",
            "First 😀\r\n second",
            true,
        ),
    ] {
        let p = map
            .iter()
            .find(|p| p["classification"] == "accessibility" && slice(&p["span"]) == statement)
            .unwrap();
        assert_eq!(p["kind"], "nonvisual");
        assert_eq!(p["effective"], effective);
        assert_eq!(slice(&p["labelSpan"]), payload);
    }
    assert!(
        map.iter()
            .any(|p| p["classification"] == "incomplete-accessibility" && p["effective"] == false)
    );
}

#[test]
fn native_configuration_evidence_preserves_original_frontmatter_and_directive_ranges() {
    let frontmatter =
        "---\r\ntitle: Config 😀\r\nconfig:\r\n  flowchart:\r\n    nodeSpacing: 60\r\n---\r\n";
    let directive = r#"%%{init: { flowchart: { nodeSpacing: 70, "html\u004cabels": false }, values: [{ nested: 'Value 😀' }] }}%%"#;
    let source = format!("\u{feff}{frontmatter}{directive}\r\nflowchart LR\r\nA --> B\r\n");
    let renderer = Renderer::new().with_engine(Engine::new().with_site_config(
        MermaidConfig::from_value(json!({"traceSource":true,"htmlLabels":false})),
    ));
    let RenderOutput::Svg(Some(output)) = renderer
        .render(RenderRequest::svg(
            &source,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .unwrap()
    else {
        panic!("missing SVG")
    };
    let svg = roxmltree::Document::parse(output.svg()).unwrap();
    let map: Vec<Value> = serde_json::from_str(
        svg.descendants()
            .find_map(|node| node.attribute("data-mt-native"))
            .unwrap(),
    )
    .unwrap();
    let slice = |span: &Value| {
        &source[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize]
    };
    assert!(
        map.iter()
            .any(|p| p["classification"] == "frontmatter" && slice(&p["span"]) == frontmatter)
    );
    assert!(
        map.iter()
            .any(|p| p["classification"] == "source-directive" && slice(&p["span"]) == directive)
    );
    assert!(
        map.iter()
            .any(|p| p["classification"] == "configuration-key"
                && p["origin"]["kind"] == "directive"
                && p["path"] == json!(["flowchart", "nodeSpacing"])
                && slice(&p["span"]) == "nodeSpacing: 70"
                && slice(&p["labelSpan"]) == "70")
    );
    assert!(
        map.iter()
            .any(|p| p["classification"] == "configuration-key"
                && p["path"] == json!(["flowchart", "htmlLabels"])
                && slice(&p["span"]) == r#"html\u004cabels": false"#
                && slice(&p["labelSpan"]) == "false")
    );
    assert!(
        map.iter()
            .any(|p| p["classification"] == "configuration-key"
                && p["path"] == json!(["values", 0, "nested"])
                && slice(&p["labelSpan"]) == "Value 😀")
    );
}

#[test]
fn native_asset_labels_keep_identity_and_generated_helpers_are_explicit() {
    let source = "flowchart LR\r\nA@{ icon: 'missing:icon', label: 'Asset 😀' }\r\nB@{ icon: 'missing:icon', form: circle, label: 'Asset 😀' }\r\nC@{ icon: 'missing:icon', form: rounded, label: 'Asset 😀' }\r\nD@{ icon: 'missing:icon', form: square, label: 'Asset 😀' }\r\nE@{ img: 'data:image/svg+xml;base64,PHN2Zy8+', label: 'Asset 😀', w: 48, h: 48 }\r\nA --> B --> C --> D --> E\r\n";
    let renderer = Renderer::new().with_engine(Engine::new().with_site_config(
        MermaidConfig::from_value(json!({"traceSource":true,"htmlLabels":false})),
    ));
    let RenderOutput::Svg(Some(output)) = renderer
        .render(RenderRequest::svg(
            source,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .unwrap()
    else {
        panic!("missing SVG")
    };
    let svg = roxmltree::Document::parse(output.svg()).unwrap();
    for id in ["A", "B", "C", "D", "E"] {
        let key = format!("node:{id}");
        assert!(
            svg.descendants()
                .any(|n| n.attribute("data-mt-key") == Some(key.as_str())
                    && n.attribute("data-mt-label") == Some("true")),
            "missing native label {id}"
        );
    }
    assert_eq!(
        svg.descendants()
            .filter(|n| n.attribute("data-mt-generated") == Some("asset"))
            .count(),
        4
    );
    assert_eq!(
        svg.descendants()
            .filter(|n| n.attribute("data-mt-generated") == Some("bounds"))
            .count(),
        4
    );
}

#[test]
fn native_svg_labels_have_one_identity_and_console_glyphs_are_generated() {
    for header in ["flowchart LR", "flowchart-elk LR"] {
        for look in ["classic", "handDrawn"] {
            let source = format!(
                "---\r\nconfig:\r\n  htmlLabels: false\r\n  look: {look}\r\n  handDrawnSeed: 42\r\n---\r\n{header}\r\nA@{{ shape: console, label: 'Console 😀' }}\r\nB[\"First 😀<br/>second\"]\r\nC[\"`First **bold** 😀\r\nsecond`\"]\r\nA --> B --> C\r\nE[\"\"]\r\n"
            );
            let renderer = Renderer::new().with_engine(Engine::new().with_site_config(
                MermaidConfig::from_value(json!({"traceSource":true,"htmlLabels":false})),
            ));
            let RenderOutput::Svg(Some(output)) = renderer
                .render(RenderRequest::svg(
                    &source,
                    OperationControl::new(),
                    SvgRequest::default(),
                ))
                .unwrap()
            else {
                panic!("missing SVG")
            };
            let svg = roxmltree::Document::parse(output.svg()).unwrap();
            for id in ["A", "B", "C"] {
                let key = format!("node:{id}");
                let labels: Vec<_> = svg
                    .descendants()
                    .filter(|n| {
                        n.attribute("data-mt-key") == Some(key.as_str())
                            && n.attribute("data-mt-label") == Some("true")
                    })
                    .collect();
                assert_eq!(
                    labels.len(),
                    1,
                    "one label identity for {id}, {header}, {look}"
                );
                assert!(labels[0].has_tag_name("g"));
                assert!(
                    labels[0]
                        .descendants()
                        .filter(|n| n.has_tag_name("text"))
                        .all(|n| !n.has_attribute("data-mt-label"))
                );
            }
            assert!(
                !svg.descendants()
                    .any(|n| n.attribute("data-mt-key") == Some("node:E")
                        && n.has_attribute("data-mt-label"))
            );
            let glyph = svg
                .descendants()
                .find(|n| n.attribute("class") == Some("console-glyph"))
                .unwrap();
            assert_eq!(glyph.attribute("data-mt-generated"), Some("glyph"));
            assert!(!glyph.has_attribute("data-mt-label"));
            assert!(
                glyph
                    .ancestors()
                    .any(|n| n.attribute("data-mt-key") == Some("node:A"))
            );
        }
    }
}
