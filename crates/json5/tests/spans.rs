use json5::{Spanned, from_str};
use serde_json::{Value, json};

#[test]
fn token_spans_use_original_utf8_and_exclude_surrounding_comments() {
    for token in [
        "'😀\\u0061'",
        "true",
        "null",
        "-0xF",
        "[1, /*x*/ 'two',]",
        "{a: [1,2,]}",
    ] {
        let input = format!(" /*prefix 😀*/ {token} /*tail*/ ");
        let parsed: Spanned<Value> = from_str(&input).unwrap();
        assert_eq!(&input[parsed.span], token);
        assert_eq!(parsed.value, from_str::<Value>(token).unwrap());
    }
}

#[test]
fn escaped_keys_have_token_spans_without_changing_decoded_names() {
    let input = r#"{ "html\u004cabels": false, \u0061: {a: '😀'}, '0': 1 }"#;
    let keys: std::collections::HashMap<Spanned<String>, Spanned<Value>> = from_str(input).unwrap();
    let mut names = keys.keys().map(|k| k.value.as_str()).collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["0", "a", "htmlLabels"]);
    let (key, value) = keys.iter().find(|(k, _)| k.value == "htmlLabels").unwrap();
    assert_eq!(&input[key.span.clone()], r#""html\u004cabels""#);
    assert_eq!(&input[value.span.clone()], "false");
    assert_eq!(value.value, json!(false));
}

#[test]
fn malformed_values_keep_original_diagnostics() {
    for input in ["[1,", "{a:}", "'bad", "1 junk", "/*bad", "{a: 1 b: 2}"] {
        assert_eq!(
            from_str::<Spanned<Value>>(input).unwrap_err(),
            from_str::<Value>(input).unwrap_err()
        );
    }
}

#[test]
fn spanned_strings_keep_native_borrowing() {
    let input = " /*prefix*/ 'borrowed' ";
    let value: Spanned<&str> = from_str(input).unwrap();
    assert_eq!(&input[value.span.clone()], "'borrowed'");
    assert_eq!(value.value.as_ptr(), input[13..].as_ptr());
}
