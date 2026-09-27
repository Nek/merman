#![cfg(feature = "svg")]
use merman::{
    Engine, MermaidConfig, OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest,
};
use serde_json::{Value, json};

#[test]
fn journey_actor_property_slots_own_their_circles_and_only_first_origin_owns_legend() {
    let source = "journey\r\n%% 😀\r\nFirst : 5 : Alice, Alice, , Bob\r\nSecond : 2 : Alice, Bob : Ignored\r\n";
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
    let pieces: Vec<Value> = serde_json::from_str(
        svg.descendants()
            .find_map(|n| n.attribute("data-mt-native"))
            .unwrap(),
    )
    .unwrap();
    let actors: Vec<_> = pieces
        .iter()
        .filter(|p| p["relation"] == "actor-reference")
        .collect();
    assert_eq!(actors.len(), 5);
    let mut starts = Vec::new();
    for piece in actors {
        let start = piece["span"]["start"].as_u64().unwrap() as usize;
        let end = piece["span"]["end"].as_u64().unwrap() as usize;
        assert_eq!(&source[start..end], piece["target"].as_str().unwrap());
        starts.push(start);
        let key = piece["domId"].as_str().unwrap();
        assert_eq!(
            svg.descendants()
                .filter(|n| n.attribute("data-mt-key") == Some(key))
                .count(),
            1
        );
    }
    starts.sort_unstable();
    starts.dedup();
    assert_eq!(starts.len(), 5);
    for actor in ["Alice", "Bob"] {
        let legend: Vec<_> = pieces
            .iter()
            .filter(|p| p["domId"] == format!("journey:actor:{actor}"))
            .collect();
        assert_eq!(legend.len(), 1);
        assert_eq!(legend[0]["span"]["start"], source.find(actor).unwrap());
    }
    assert!(!pieces.iter().any(|p| p["target"] == "Ignored"));
    assert!(
        !svg.descendants()
            .any(|n| n.attribute("data-mt-key") == Some("journey:actor:0:2"))
    );
}
