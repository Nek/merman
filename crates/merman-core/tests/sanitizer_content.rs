use merman_core::{MermaidConfig, sanitize::sanitize_text};
use serde_json::json;

#[test]
fn removed_subtrees_obey_default_replace_add_and_keep_content_policies() {
    let input = "<div>Hidden <b>nested</b></div><audio>Audio</audio><b>Kept</b>";
    for (policy, expected) in [
        (
            json!({"FORBID_TAGS":["div"],"FORBID_CONTENTS":["div"]}),
            "<audio>Audio</audio><b>Kept</b>",
        ),
        (
            json!({"FORBID_TAGS":["div","audio"],"ADD_FORBID_CONTENTS":["DIV"]}),
            "<b>Kept</b>",
        ),
        (
            json!({"FORBID_TAGS":["div","audio"],"FORBID_CONTENTS":[]}),
            "Hidden <b>nested</b>Audio<b>Kept</b>",
        ),
        (json!({"FORBID_CONTENTS":["div"]}), input),
        (
            json!({"FORBID_TAGS":["div"],"FORBID_CONTENTS":[],"KEEP_CONTENT":false}),
            "<audio>Audio</audio><b>Kept</b>",
        ),
        (
            json!({"FORBID_TAGS":["div","audio"]}),
            "Hidden <b>nested</b><b>Kept</b>",
        ),
    ] {
        for html in [false, true] {
            let config = MermaidConfig::from_value(
                json!({"securityLevel":"loose","htmlLabels":html,"dompurifyConfig":policy}),
            );
            assert_eq!(sanitize_text(input, &config), expected, "{config:?}");
        }
    }
}

#[test]
fn removed_content_policy_preserves_allowed_paragraphs_but_discards_nested_forbidden_wrappers() {
    for level in ["strict", "antiscript", "sandbox", "loose"] {
        let config = MermaidConfig::from_value(
            json!({"securityLevel":level,"htmlLabels":true,"dompurifyConfig":{"FORBID_TAGS":["div"],"ADD_FORBID_CONTENTS":["div","p"]}}),
        );
        assert_eq!(
            sanitize_text("<p>ASCII 123 Label</p>", &config),
            "<p>ASCII 123 Label</p>"
        );
        assert_eq!(
            sanitize_text(
                "<p>Keep</p><div>Discard<div>Nested</div>Tail</div><p>After</p>",
                &config
            ),
            "<p>Keep</p><p>After</p>"
        );
        assert_eq!(
            sanitize_text(
                "<script>bad()</script><iframe>bad</iframe><style>bad</style><b>Keep</b>",
                &config
            ),
            "<b>Keep</b>"
        );
    }
}
