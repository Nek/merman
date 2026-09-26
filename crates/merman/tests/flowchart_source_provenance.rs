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
