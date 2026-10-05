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

#[test]
fn template_safe_labels_match_pinned_text_attribute_and_serialization_rules() {
    for (input, expected) in [
        ("Alpha {{secret}} tail", "  tail"),
        ("Alpha {{open", "Alpha  "),
        ("Alpha <%open", "Alpha  "),
        ("head %> tail", "  tail"),
        ("Alpha ${secret} tail", "Alpha  "),
        ("Alpha <%secret%> tail", "  tail"),
        ("head }} Alpha", "  Alpha"),
        ("{{head}} middle }} tail {{last}}", " "),
        ("<p>Alpha {{secret}}</p><b>Beta</b>", "<p> </p><b>Beta</b>"),
        ("<p>Alpha &#123;&#123;secret}}</p>", "<p> </p>"),
        ("<p>Alpha &lt;%secret%&gt;</p>", "<p> </p>"),
        (
            r#"<span title="Alpha {{secret}}" data-note="allowed">Label</span>"#,
            r#"<span title=" ">Label</span>"#,
        ),
        (
            "<p>Alpha {<unknown></unknown>{secret}}</p>",
            "<p>Alpha { </p>",
        ),
        ("<p>Alpha ${secret}\nrest</p>", "<p>Alpha  </p>"),
        (
            "<p>Plain &amp; text &gt; 2</p>",
            "<p>Plain &amp; text &gt; 2</p>",
        ),
        ("<p>ASCII 123 Label</p>", "<p>ASCII 123 Label</p>"),
    ] {
        for html in [false, true] {
            for level in ["loose", "strict"] {
                let config = MermaidConfig::from_value(
                    json!({"htmlLabels":html,"securityLevel":level,"dompurifyConfig":{"SAFE_FOR_TEMPLATES":true}}),
                );
                // Strict HTML mode unwraps the unknown element in its preceding default pass.
                let expected = if html && level == "strict" && input.contains("<unknown>") {
                    "<p> </p>"
                } else {
                    expected
                };
                assert_eq!(
                    sanitize_text(input, &config),
                    expected,
                    "{input} {html} {level}"
                );
            }
        }
    }
}

#[test]
fn template_filtering_respects_opt_out_explicit_data_attributes_and_long_text_nodes() {
    let input = "Alpha ${secret}";
    for policy in [json!({}), json!({"SAFE_FOR_TEMPLATES":false})] {
        let config =
            MermaidConfig::from_value(json!({"htmlLabels":false,"dompurifyConfig":policy}));
        assert_eq!(sanitize_text(input, &config), input);
    }
    let config = MermaidConfig::from_value(
        json!({"htmlLabels":false,"dompurifyConfig":{"SAFE_FOR_TEMPLATES":true,"ALLOW_DATA_ATTR":true,"ADD_ATTR":["data-note"]}}),
    );
    assert_eq!(
        sanitize_text(
            r#"<b data-note="safe ${secret}" data-other="value">Keep</b>"#,
            &config
        ),
        r#"<b data-note="safe  ">Keep</b>"#
    );
    let prefix = "😀 keep ".repeat(5000);
    assert_eq!(
        sanitize_text(&format!("<p>{prefix}&#36;{{secret}} tail</p>"), &config),
        format!("<p>{prefix} </p>")
    );
}

#[test]
fn self_closing_attribute_policy_runs_after_decoding_and_before_template_filtering() {
    for html in [false, true] {
        for level in ["loose", "strict"] {
            for input in [
                r#"<b title="x/>y">Keep</b>"#,
                r#"<b title="x/&#62;y">Keep</b>"#,
                r#"<b title="x&#47;>y">Keep</b>"#,
                r#"<b title="${x}/>y">Keep</b>"#,
            ] {
                let config = MermaidConfig::from_value(
                    json!({"htmlLabels":html,"securityLevel":level,"dompurifyConfig":{"ALLOW_SELF_CLOSE_IN_ATTR":false,"SAFE_FOR_TEMPLATES":true}}),
                );
                assert_eq!(sanitize_text(input, &config), "<b>Keep</b>");
            }
        }
    }
    for policy in [json!({}), json!({"ALLOW_SELF_CLOSE_IN_ATTR":true})] {
        let config =
            MermaidConfig::from_value(json!({"htmlLabels":false,"dompurifyConfig":policy}));
        assert_eq!(
            sanitize_text(r#"<b title="x/>y">Keep</b>"#, &config),
            r#"<b title="x/&gt;y">Keep</b>"#
        );
    }
    let config = MermaidConfig::from_value(
        json!({"htmlLabels":false,"dompurifyConfig":{"ALLOW_SELF_CLOSE_IN_ATTR":false}}),
    );
    assert_eq!(
        sanitize_text(r#"<b title="x/ >y">Keep</b>"#, &config),
        r#"<b title="x/ &gt;y">Keep</b>"#
    );
}

#[test]
fn named_attribute_isolation_is_idempotent_and_obeys_existing_attribute_policy() {
    let input = r#"<b id="  item&#49; " name="user-content-other">Keep</b>"#;
    let config = MermaidConfig::from_value(
        json!({"htmlLabels":false,"dompurifyConfig":{"SANITIZE_NAMED_PROPS":true}}),
    );
    let expected = r#"<b id="user-content-item1" name="user-content-other">Keep</b>"#;
    assert_eq!(sanitize_text(input, &config), expected);
    assert_eq!(sanitize_text(expected, &config), expected);
    for policy in [json!({}), json!({"SANITIZE_NAMED_PROPS":false})] {
        let config =
            MermaidConfig::from_value(json!({"htmlLabels":false,"dompurifyConfig":policy}));
        assert_eq!(
            sanitize_text(input, &config),
            r#"<b id="item1" name="user-content-other">Keep</b>"#
        );
    }
    for policy in [
        json!({"SANITIZE_NAMED_PROPS":true,"FORBID_ATTR":["name"]}),
        json!({"SANITIZE_NAMED_PROPS":true,"ALLOWED_ATTR":["id"]}),
    ] {
        let config =
            MermaidConfig::from_value(json!({"htmlLabels":false,"dompurifyConfig":policy}));
        assert_eq!(
            sanitize_text(input, &config),
            r#"<b id="user-content-item1">Keep</b>"#
        );
    }
    let config = MermaidConfig::from_value(
        json!({"htmlLabels":false,"dompurifyConfig":{"SANITIZE_NAMED_PROPS":true,"SAFE_FOR_TEMPLATES":true}}),
    );
    assert_eq!(
        sanitize_text(r#"<b id="item${secret}" name="">Keep</b>"#, &config),
        r#"<b id="user-content-item " name="user-content-">Keep</b>"#
    );
}

#[test]
fn decoded_attribute_serialization_does_not_create_tags_or_decode_a_second_entity_layer() {
    let config = MermaidConfig::from_value(json!({"htmlLabels":false}));
    let input = r#"<b title="left/>right &amp; &amp;copy; &lt;i&gt;">Keep</b>"#;
    let expected = r#"<b title="left/&gt;right &amp; &amp;copy; &lt;i&gt;">Keep</b>"#;
    assert_eq!(sanitize_text(input, &config), expected);
    assert_eq!(sanitize_text(expected, &config), expected);
}

#[test]
fn xml_safe_attribute_values_obey_decoding_opt_out_and_strict_preprocessing() {
    let denied = [
        "x-->y",
        "x--!>y",
        "x]>y",
        "x--&#62;y",
        "x&#93;>y",
        "x&lt;/ScRiPture",
        "</style",
        "</script",
        "</title",
        "</xmp",
        "</textarea",
        "</noscript",
        "</iframe",
        "</noembed",
        "</noframes",
    ];
    for value in denied {
        for html in [false, true] {
            for level in ["loose", "strict", "antiscript", "sandbox"] {
                for safe in [None, Some(false), Some(true)] {
                    let mut policy = json!({});
                    if let Some(safe) = safe {
                        policy["SAFE_FOR_XML"] = json!(safe);
                    }
                    let config = MermaidConfig::from_value(
                        json!({"htmlLabels":html,"securityLevel":level,"dompurifyConfig":policy}),
                    );
                    let output = sanitize_text(
                        &format!(
                            r#"<b title="{value}" data-note="{value}" aria-label="{value}">Keep</b>"#
                        ),
                        &config,
                    );
                    if safe != Some(false) || (html && level != "loose") {
                        assert_eq!(output, "<b>Keep</b>", "{value} {html} {level} {safe:?}");
                    } else {
                        assert!(
                            output.contains("title=")
                                && output.contains("data-note=")
                                && output.contains("aria-label="),
                            "{output}"
                        );
                    }
                }
            }
        }
    }
    let config = MermaidConfig::from_value(json!({"htmlLabels":false}));
    for value in [
        "plain",
        "x-- >y",
        "x] >y",
        "</span",
        "</ script",
        "</styled",
    ] {
        // The pinned regexp intentionally matches closing-tag prefixes, without a tag boundary.
        let output = sanitize_text(&format!(r#"<b title="{value}">Keep</b>"#), &config);
        assert_eq!(output.contains("title="), value != "</styled", "{value}");
    }
}

#[test]
fn xml_safe_patch_linkage_cannot_be_allowlisted_but_form_associations_survive() {
    for safe in [false, true] {
        let config = MermaidConfig::from_value(
            json!({"htmlLabels":false,"dompurifyConfig":{"SAFE_FOR_XML":safe,"ADD_ATTR":["for","patchsrc"],"ADD_URI_SAFE_ATTR":["for","patchsrc"]}}),
        );
        for tag in ["b", "label", "output"] {
            let input = format!(
                r#"<{tag} for="target" patchsrc="https://example.invalid/fragment">Keep</{tag}>"#
            );
            let output = sanitize_text(&input, &config);
            assert_eq!(output.contains("patchsrc="), !safe, "{output}");
            assert_eq!(output.contains("for="), !safe || tag != "b", "{output}");
        }
    }
    let config = MermaidConfig::from_value(
        json!({"htmlLabels":false,"dompurifyConfig":{"SAFE_FOR_XML":false,"FORBID_ATTR":["for"]}}),
    );
    assert_eq!(
        sanitize_text(r#"<label for="target">Keep</label>"#, &config),
        "<label>Keep</label>"
    );
}
