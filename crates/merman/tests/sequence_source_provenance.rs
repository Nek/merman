#![cfg(feature = "svg")]
use merman::{
    Engine, MermaidConfig, OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest,
};
use serde_json::{Value, json};

#[test]
fn native_sequence_source_keys_resolve_repeated_message_occurrences() {
    let source = "sequenceDiagram\r\n%% 😀\r\nparticipant A\r\nparticipant B\r\nA->>B: same\r\nB->>A: same\r\n";
    let renderer = Renderer::new().with_engine(
        Engine::new().with_site_config(MermaidConfig::from_value(json!({"traceSource":true}))),
    );
    let RenderOutput::Svg(Some(output)) = renderer
        .render(RenderRequest::svg(
            source,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .unwrap()
    else {
        panic!("no SVG")
    };
    let svg = roxmltree::Document::parse(output.svg()).unwrap();
    let occurrences: Vec<Value> = serde_json::from_str(
        svg.descendants()
            .find_map(|node| node.attribute("data-mt-native"))
            .unwrap(),
    )
    .unwrap();
    let edges: Vec<_> = occurrences.iter().filter(|p| p["kind"] == "edge").collect();
    assert_eq!(edges.len(), 2);
    for (piece, text) in edges.iter().zip(["A->>B: same", "B->>A: same"]) {
        let span = &piece["span"];
        assert_eq!(
            &source
                [span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize],
            text
        );
        let key = piece["domId"].as_str().unwrap();
        assert!(svg.descendants().any(|node| node.attribute("data-mt-key") == Some(key) && node.has_tag_name("line")));
        assert!(
            svg.descendants()
                .any(|node| node.attribute("data-mt-key") == Some(key)
                    && node.attribute("data-mt-label") == Some("true"))
        );
    }
}
