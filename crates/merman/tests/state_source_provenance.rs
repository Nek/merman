use merman::{
    Engine, MermaidConfig, OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest,
};
use serde_json::{Value, json};

#[test]
fn concurrency_regions_keep_native_child_ranges_and_separator_relationships() {
    let source = "stateDiagram-v2\r\n%% 😀\r\nstate Parallel {\r\n  state \"Left 😀\" as A\r\n  --\r\n  [*] --> B\r\n  B --> [*] : done\r\n}\r\n";
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
    let pieces: Vec<Value> = serde_json::from_str(
        svg.descendants()
            .find_map(|n| n.attribute("data-mt-native"))
            .unwrap(),
    )
    .unwrap();
    let slice = |span: &Value| {
        &source[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize]
    };
    for expected in ["state \"Left 😀\" as A", "[*] --> B\r\n  B --> [*] : done"] {
        let piece = pieces
            .iter()
            .find(|p| p["effective"] == true && slice(&p["span"]) == expected)
            .expect("native region range");
        assert!(
            svg.descendants()
                .any(|n| n.attribute("data-mt-key") == piece["domId"].as_str())
        );
    }
    let separator = pieces
        .iter()
        .find(|p| p.get("span").is_some() && slice(&p["span"]) == "--")
        .expect("original separator relationship");
    assert!(
        pieces
            .iter()
            .any(|p| p["domId"] == separator["domId"] && p["effective"] == true)
    );
}

#[test]
fn attached_note_connectors_keep_original_note_statement_ranges() {
    let source = "stateDiagram-v2\r\n%% 😀\r\nstate A\r\nnote left of A : Same 😀\r\nnote right of A\r\n Same 😀\r\n second row\r\nend note\r\n";
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
    let pieces: Vec<Value> = serde_json::from_str(
        svg.descendants()
            .find_map(|n| n.attribute("data-mt-native"))
            .unwrap(),
    )
    .unwrap();
    let slice = |span: &Value| {
        &source[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize]
    };
    let connectors: Vec<_> = pieces
        .iter()
        .filter(|p| p["relationship"] == "note")
        .collect();
    assert_eq!(connectors.len(), 2);
    for (piece, expected) in connectors.iter().zip([
        "note left of A : Same 😀",
        "note right of A\r\n Same 😀\r\n second row\r\nend note",
    ]) {
        assert_eq!(piece["kind"], "edge");
        assert_eq!(slice(&piece["span"]), expected);
        assert!(piece.get("labelSpan").is_none());
        assert!(svg.descendants().any(
            |n| n.has_tag_name("path") && n.attribute("data-mt-key") == piece["domId"].as_str()
        ));
    }
}

#[test]
fn note_properties_belong_to_the_note_and_keep_their_reference_as_data() {
    for header in ["stateDiagram", "stateDiagram-v2"] {
        for (before, after) in [
            ("Published --> [*]\r\n", ""),
            ("", "Published\r\n"),
            ("", ""),
        ] {
            let note = "note right of Published : Available 😀";
            let source = format!("{header}\r\n%% 😀\r\n{before}{note}\r\n{after}");
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
            let pieces: Vec<Value> = serde_json::from_str(
                svg.descendants()
                    .find_map(|n| n.attribute("data-mt-native"))
                    .unwrap(),
            )
            .unwrap();
            let start = source.find(note).unwrap();
            let end = start + note.len();
            assert!(!pieces.iter().any(|p| p["kind"] == "node"
                && p["span"]["start"].as_u64().unwrap() < end as u64
                && p["span"]["end"].as_u64().unwrap() > start as u64));
            let owner = pieces
                .iter()
                .find(|p| p["kind"] == "control" && p["span"] == json!({"start":start,"end":end}))
                .unwrap();
            assert_eq!(owner["target"], "Published");
            assert_eq!(owner["position"], "right of");
            assert!(
                svg.descendants()
                    .any(|n| n.attribute("data-mt-key") == owner["domId"].as_str())
            );
        }
    }
}

#[test]
fn transition_endpoint_references_keep_their_owner_and_first_creation() {
    for prefix in [
        "A\nB\n",
        "A --> B : create\n",
        "note right of A : first\nA --> B : create\n",
    ] {
        let statement = "A --> B : again";
        let source = format!("stateDiagram-v2\n%% 😀\n{prefix}{statement}\n");
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
        let pieces: Vec<Value> = serde_json::from_str(
            svg.descendants()
                .find_map(|n| n.attribute("data-mt-native"))
                .unwrap(),
        )
        .unwrap();
        let start = source.find(statement).unwrap();
        for offset in [0, 6] {
            assert!(!pieces.iter().any(|p| p["kind"] == "node"
                && p["span"] == json!({"start":start+offset,"end":start+offset+1})));
        }
        let edge = pieces
            .iter()
            .find(|p| {
                p["kind"] == "edge"
                    && p["span"] == json!({"start":start,"end":start+statement.len()})
            })
            .unwrap();
        assert_eq!(edge["from"], "A");
        assert_eq!(edge["to"], "B");
        assert!(
            svg.descendants()
                .any(|n| n.attribute("data-mt-key") == edge["domId"].as_str())
        );
        for id in ["A", "B"] {
            assert!(
                pieces
                    .iter()
                    .any(|p| p["kind"] == "node" && p["semanticId"] == id)
            );
        }
    }
}
