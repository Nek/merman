use crate::XtaskError;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn is_canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn read_text(path: &Path) -> Result<String, XtaskError> {
    fs::read_to_string(path).map_err(|source| XtaskError::ReadFile {
        path: path.display().to_string(),
        source,
    })
}

pub(crate) fn read_text_normalized(path: &Path) -> Result<String, XtaskError> {
    let text = read_text(path)?;
    let normalized_line_endings = text.replace("\r\n", "\n");
    Ok(normalized_line_endings.trim_end().to_string())
}

pub(crate) fn extract_add_to_set_string_array(
    src: &str,
    ident: &str,
) -> Result<Vec<String>, XtaskError> {
    let needle = format!("const {ident} = addToSet({{}}, [");
    let start = src
        .find(&needle)
        .ok_or_else(|| XtaskError::ParseDompurify(format!("missing {ident} definition")))?;
    let bracket_start = start + needle.len() - 1; // points at '['
    extract_string_array_at(src, bracket_start)
}

pub(crate) fn extract_frozen_string_array(
    src: &str,
    ident: &str,
) -> Result<Vec<String>, XtaskError> {
    let needle = format!("const {ident} = freeze([");
    let start = src
        .find(&needle)
        .ok_or_else(|| XtaskError::ParseDompurify(format!("missing {ident} definition")))?;
    let bracket_start = start + needle.len() - 1; // points at '['
    extract_string_array_at(src, bracket_start)
}

pub(crate) fn extract_string_array_at(
    src: &str,
    bracket_start: usize,
) -> Result<Vec<String>, XtaskError> {
    let bytes = src.as_bytes();
    if *bytes.get(bracket_start).unwrap_or(&0) != b'[' {
        return Err(XtaskError::ParseDompurify("expected array '['".to_string()));
    }

    let mut out: Vec<String> = Vec::new();
    let mut i = bracket_start + 1;
    let mut in_string = false;
    let mut cur = String::new();

    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            match b {
                b'\\' => {
                    // Minimal escape handling: keep the escaped character verbatim.
                    if i + 1 >= bytes.len() {
                        return Err(XtaskError::ParseDompurify(
                            "unterminated escape".to_string(),
                        ));
                    }
                    let next = bytes[i + 1] as char;
                    cur.push(next);
                    i += 2;
                    continue;
                }
                b'\'' => {
                    out.push(cur.clone());
                    cur.clear();
                    in_string = false;
                    i += 1;
                    continue;
                }
                _ => {
                    cur.push(b as char);
                    i += 1;
                    continue;
                }
            }
        }

        if bytes[i..].starts_with(b"//") {
            i += bytes[i..]
                .iter()
                .position(|b| *b == b'\n')
                .unwrap_or(bytes.len() - i);
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            let end = bytes[i + 2..]
                .windows(2)
                .position(|pair| pair == b"*/")
                .ok_or_else(|| XtaskError::ParseDompurify("unterminated comment".to_string()))?;
            i += end + 4;
            continue;
        }

        match b {
            b'\'' => {
                in_string = true;
                i += 1;
            }
            b']' => return Ok(out),
            _ => i += 1,
        }
    }

    Err(XtaskError::ParseDompurify("unterminated array".to_string()))
}

#[cfg(test)]
mod tests {
    use super::extract_string_array_at;

    #[test]
    fn generated_sanitizer_lists_ignore_quoted_comment_text_and_brackets() {
        let source = "['script', // the option's subtree ] is not a tag\n'selectedcontent', /* 'ignored' ] */ 'style']";
        assert_eq!(
            extract_string_array_at(source, 0).unwrap(),
            ["script", "selectedcontent", "style"]
        );
        assert!(extract_string_array_at("['script', /* unfinished", 0).is_err());
    }
}
