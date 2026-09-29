use merman_core::{Engine, MermaidConfig};
use serde_json::json;

#[test]
fn scoped_appearance_resolves_layers_without_leaking_between_diagrams() {
    for (namespace, header) in [
        ("flowchart", "flowchart LR"),
        ("sequence", "sequenceDiagram"),
        ("state", "stateDiagram-v2"),
        ("gantt", "gantt"),
        ("journey", "journey"),
        ("kanban", "kanban"),
    ] {
        let engine = Engine::new().with_site_config(MermaidConfig::from_value(json!({
            "theme":"forest", "look":"classic", "layout":"dagre",
            namespace: {"theme":"dark", "look":"neo", "layout":"elk"}
        })));
        for (source_config, expected) in [
            (json!({}), ["dark", "neo", "elk"]),
            (
                json!({"theme":"neutral", "look":"classic", "layout":"dagre"}),
                ["neutral", "classic", "dagre"],
            ),
            (
                json!({"theme":"forest", "look":"classic", "layout":"dagre", namespace: {"theme":"base", "look":"handDrawn", "layout":"elk"}}),
                ["base", "handDrawn", "elk"],
            ),
            (
                json!({"theme":"neutral", "look":"classic", namespace: {"theme":"constructor", "look":"invalid", "layout":null}}),
                ["neutral", "classic", "elk"],
            ),
            (
                json!({namespace: {"theme":"null", "layout":"custom-layout"}}),
                ["null", "neo", "custom-layout"],
            ),
        ] {
            for source in [
                format!("---\nconfig: {source_config}\n---\n{header}"),
                format!("%%{{init: {source_config}}}%%\n{header}"),
            ] {
                let cfg = engine
                    .parse_metadata_sync(&source)
                    .unwrap()
                    .effective_config;
                for (key, value) in ["theme", "look", "layout"].into_iter().zip(expected) {
                    assert_eq!(
                        cfg.get_str(key),
                        Some(value),
                        "{namespace}/{source_config}/{key}"
                    );
                    assert_eq!(cfg.get_str(&format!("{namespace}.{key}")), Some(value));
                }
            }
        }
        let again = engine.parse_metadata_sync(header).unwrap().effective_config;
        assert_eq!(again.get_str("theme"), Some("dark"));
        let other_header = if namespace == "flowchart" {
            "sequenceDiagram"
        } else {
            "flowchart LR"
        };
        assert_eq!(
            engine
                .parse_metadata_sync(other_header)
                .unwrap()
                .effective_config
                .get_str("theme"),
            Some("forest")
        );
    }
}

#[test]
fn scoped_appearance_preserves_secure_keys_and_regenerates_theme_variables() {
    let site = json!({"theme":"forest", "flowchart":{"theme":"dark", "look":"classic", "layout":"dagre"}, "secure":["theme", "look", "layout"], "themeVariables":{"primaryColor":"#123456"}});
    let engine = Engine::new().with_site_config(MermaidConfig::from_value(site));
    let cfg = engine.parse_metadata_sync("%%{init: {theme:'neutral', look:'neo', layout:'elk', flowchart:{theme:'base',look:'handDrawn',layout:'elk'}}}%%\nflowchart LR\nA-->B").unwrap().effective_config;
    assert_eq!(cfg.get_str("theme"), Some("dark"));
    assert_eq!(cfg.get_str("look"), Some("classic"));
    assert_eq!(cfg.get_str("layout"), Some("dagre"));
    assert_eq!(cfg.get_str("themeVariables.primaryColor"), Some("#123456"));
    let dark = Engine::new()
        .parse_metadata_sync("---\nconfig: {theme: dark}\n---\nflowchart LR")
        .unwrap()
        .effective_config;
    let scoped = Engine::new()
        .parse_metadata_sync("---\nconfig: {flowchart: {theme: dark}}\n---\nflowchart LR")
        .unwrap()
        .effective_config;
    assert_eq!(
        scoped.as_value()["themeVariables"],
        dark.as_value()["themeVariables"]
    );
}
