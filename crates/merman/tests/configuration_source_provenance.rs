#![cfg(feature = "svg")]
use merman::{
    Engine, MermaidConfig, OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest,
};
use serde_json::{Value, json};

#[test]
fn all_mapped_families_retain_nonvisual_configuration_origins() {
    let renderer = Renderer::new().with_engine(
        Engine::new().with_site_config(MermaidConfig::from_value(json!({"traceSource":true}))),
    );
    for body in [
        "flowchart LR\nA --> B\n",
        "stateDiagram-v2\nA --> B\n",
        "sequenceDiagram\nA->>B: Message\n",
        "gantt\ndateFormat YYYY-MM-DD\ntodayMarker off\nTask :a, 2026-01-01, 2d\n",
        "journey\nTask :5: Alice\n",
        "kanban\na[Column]\n  b[Task]\n",
    ] {
        let source = format!(
            "\u{feff}---\r\nconfig:\r\n  unknown: 'Value 😀'\r\n---\r\n%%{{init: {{ htmlLabels: false }} }}%%\r\n{body}"
        );
        let RenderOutput::Svg(Some(output)) = renderer
            .render(RenderRequest::svg(
                &source,
                OperationControl::new(),
                SvgRequest::default(),
            ))
            .unwrap()
        else {
            panic!("no SVG")
        };
        let svg = roxmltree::Document::parse(output.svg()).unwrap();
        let pieces: Vec<Value> = serde_json::from_str(
            svg.descendants()
                .find_map(|node| node.attribute("data-mt-native"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            pieces
                .iter()
                .filter(|piece| piece["classification"] == "frontmatter")
                .count(),
            1,
            "{body}"
        );
        assert_eq!(
            pieces
                .iter()
                .filter(|piece| piece["classification"] == "source-directive")
                .count(),
            1
        );
        let value = pieces
            .iter()
            .find(|piece| piece["path"] == json!(["config", "unknown"]))
            .unwrap();
        assert_eq!(value["kind"], "nonvisual");
        assert!(value.get("domId").is_none());
        let span = &value["labelSpan"];
        assert_eq!(
            &source
                [span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize],
            "Value 😀"
        );
    }
}
