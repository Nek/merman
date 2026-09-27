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
