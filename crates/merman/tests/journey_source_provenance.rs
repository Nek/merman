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

#[test]
fn journey_section_runs_keep_parser_occurrence_ownership_and_contiguous_aliases() {
    let source = "journey\r\n%% 😀\r\nsection Day\r\nFirst : 5 : Alice\r\nsection Day\r\nSecond : 2 : Bob\r\nsection Night\r\nThird : 3 : Carol\r\nsection Unused\r\nsection Day\r\nFourth : 4 : Alice\r\nsection \r\nFifth : 3 : Bob\r\n";
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
    let sections: Vec<_> = pieces
        .iter()
        .filter(|p| p.get("sectionIndex").is_some())
        .collect();
    assert_eq!(sections.len(), 6);
    for (index, owner) in [Some(0), Some(0), Some(2), None, Some(4), Some(5)]
        .iter()
        .enumerate()
    {
        let piece = sections[index];
        assert_eq!(piece["sectionIndex"], index);
        let start = piece["span"]["start"].as_u64().unwrap() as usize;
        let end = piece["span"]["end"].as_u64().unwrap() as usize;
        assert!(source[start..end].starts_with("section"));
        if let Some(owner) = owner {
            assert_eq!(piece["semanticId"], format!("section:{owner}"));
            assert_eq!(piece["domId"], format!("journey:section:{owner}"));
            assert_eq!(piece["effective"], index == *owner);
            let key = piece["domId"].as_str().unwrap();
            assert_eq!(
                svg.descendants()
                    .filter(|n| n.attribute("data-mt-key") == Some(key)
                        && !n.has_attribute("data-mt-label"))
                    .count(),
                1
            );
        } else {
            assert_eq!(piece["kind"], "nonvisual");
            assert_eq!(piece["classification"], "unused-section");
        }
    }
    assert!(
        sections[5].get("labelSpan").is_none(),
        "a real empty section frame has no invented label"
    );
}

#[test]
fn journey_frontmatter_title_uses_original_scalar_provenance_without_overriding_body() {
    let renderer = Renderer::new().with_engine(
        Engine::new().with_site_config(MermaidConfig::from_value(json!({"traceSource":true}))),
    );
    for body in ["", "title Body 😀\r\n"] {
        let source = format!(
            "---\r\ntitle: 'Configured 😀'\r\n---\r\njourney\r\n{body}Task : 5 : Alice\r\n"
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
                .find_map(|n| n.attribute("data-mt-native"))
                .unwrap(),
        )
        .unwrap();
        let titles: Vec<_> = pieces
            .iter()
            .filter(|p| p["domId"] == "journey:title")
            .collect();
        assert_eq!(titles.len(), 1);
        let span = &titles[0]["labelSpan"];
        assert_eq!(
            &source
                [span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize],
            if body.is_empty() {
                "Configured 😀"
            } else {
                "Body 😀"
            }
        );
        let title = svg
            .descendants()
            .find(|n| n.attribute("data-mt-key") == Some("journey:title"))
            .unwrap();
        assert_eq!(
            title.attribute("data-mt-label"),
            if body.is_empty() { Some("true") } else { None }
        );
    }
}
