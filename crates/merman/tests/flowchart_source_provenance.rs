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
