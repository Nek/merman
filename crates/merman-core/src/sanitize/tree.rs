use super::*;
use html5ever::serialize::{HtmlSerializer, Serialize, SerializeOpts, TraversalScope};
use html5ever::tendril::TendrilSink;
use html5ever::{QualName, local_name, ns};
use scraper::{ElementRef, Html, HtmlTreeSink, Node};
use std::io::{self, Write};

struct Output<'a, S: SanitizeOutputSink> {
    sink: &'a S,
    bytes: Vec<u8>,
    error: Option<S::Error>,
}

impl<S: SanitizeOutputSink> Write for Output<'_, S> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Err(error) = self.sink.push_output_chunk(&mut self.bytes, bytes) {
            self.error = Some(error);
            return Err(io::Error::other("sanitizer output rejected"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialize_children<S: SanitizeOutputSink>(
    element: ElementRef<'_>,
    sink: &S,
) -> Result<String, SanitizeFailure<S::Error>> {
    let mut output = Output {
        sink,
        bytes: sink.output_buffer(0).map_err(SanitizeFailure::Output)?,
        error: None,
    };
    let options = SerializeOpts {
        scripting_enabled: false,
        traversal_scope: TraversalScope::ChildrenOnly(Some(element.value().name.clone())),
        ..Default::default()
    };
    // Give the serializer the parent context without emitting the parent element.
    let result = element.serialize(
        &mut HtmlSerializer::new(&mut output, options),
        TraversalScope::ChildrenOnly(None),
    );
    if let Some(error) = output.error {
        return Err(SanitizeFailure::Output(error));
    }
    result.map_err(|_| SanitizeFailure::RejectedInput)?;
    String::from_utf8(output.bytes).map_err(|_| SanitizeFailure::InvalidUtf8Output)
}

fn markup_probe(text: &str, include_bang: bool) -> bool {
    text.as_bytes().windows(2).any(|pair| {
        pair[0] == b'<'
            && (pair[1] == b'/'
                || is_js_regex_word_byte(pair[1])
                || (include_bang && pair[1] == b'!'))
    })
}

fn valid_namespace(element: ElementRef<'_>, cfg: &DompurifyEffectiveConfig) -> bool {
    let name = &element.value().name;
    let tag = name.local.as_ref().to_ascii_lowercase();
    let parent = element.ancestors().find_map(ElementRef::wrap);
    let Some(parent) = parent else {
        return name.ns == ns!(html);
    };
    let parent_name = &parent.value().name;
    let parent_tag = parent_name.local.as_ref().to_ascii_lowercase();
    let math_text = cfg.mathml_text_integration_points.contains(&parent_tag);
    let html_point = cfg.html_integration_points.contains(&parent_tag);
    match name.ns {
        ns!(svg) => match parent_name.ns {
            ns!(html) => tag == "svg",
            ns!(mathml) => tag == "svg" && (parent_tag == "annotation-xml" || math_text),
            _ => dompurify_defaults::ALL_SVG_TAGS
                .binary_search(&tag.as_str())
                .is_ok(),
        },
        ns!(mathml) => match parent_name.ns {
            ns!(html) => tag == "math",
            ns!(svg) => tag == "math" && html_point,
            _ => dompurify_defaults::ALL_MATHML_TAGS
                .binary_search(&tag.as_str())
                .is_ok(),
        },
        ns!(html) => {
            if (parent_name.ns == ns!(svg) && !html_point)
                || (parent_name.ns == ns!(mathml) && !math_text)
            {
                return false;
            }
            dompurify_defaults::ALL_MATHML_TAGS
                .binary_search(&tag.as_str())
                .is_err()
                && (matches!(tag.as_str(), "title" | "style" | "font" | "a" | "script")
                    || dompurify_defaults::ALL_SVG_TAGS
                        .binary_search(&tag.as_str())
                        .is_err())
        }
        _ => false,
    }
}

fn filter_attributes<S: SanitizeOutputSink>(
    element: &mut scraper::node::Element,
    cfg: &DompurifyEffectiveConfig,
    sink: &S,
) -> Result<(), SanitizeFailure<S::Error>> {
    let tag = element.name.local.as_ref().to_ascii_lowercase();
    let temporary = QualName::new(None, ns!(), "data-temp-href-target".into());
    let target_name = QualName::new(None, ns!(), "target".into());
    if tag == "a"
        && let Some(target) = element.attrs.get(&target_name).cloned()
    {
        element.attrs.insert(temporary.clone(), target);
    }
    let names: Vec<_> = element.attrs.keys().cloned().collect();
    for name in names {
        let full_name = if name.ns == ns!(xmlns) && name.local == local_name!("xmlns") {
            "xmlns".to_owned()
        } else {
            match &name.prefix {
                Some(prefix) => format!("{prefix}:{}", name.local),
                None => name.local.to_string(),
            }
        };
        let lc_name = full_name.to_ascii_lowercase();
        let value = element.attrs.get(&name).unwrap();
        let normalized = dompurify_normalize_dom_attribute_value(&lc_name, value);
        if (cfg.safe_for_xml && has_xml_unsafe_attribute_value(normalized))
            || (!cfg.allow_self_close_in_attr && normalized.contains("/>"))
        {
            element.attrs.shift_remove(&name);
            continue;
        }
        let prefix = if cfg.sanitize_named_props
            && matches!(lc_name.as_str(), "id" | "name")
            && !normalized.starts_with("user-content-")
        {
            "user-content-"
        } else {
            ""
        };
        let length = sink
            .checked_output_len(prefix.len(), normalized.len())
            .map_err(SanitizeFailure::Output)?;
        let mut filtered = sink
            .string_with_capacity(length)
            .map_err(SanitizeFailure::Output)?;
        filtered.push_str(prefix);
        filtered.push_str(normalized);
        if cfg.safe_for_templates {
            strip_template_expressions(&mut filtered);
        }
        if dompurify_is_valid_attribute(cfg, &tag, &lc_name, &filtered) {
            element.attrs.insert(name, filtered.into());
        } else {
            element.attrs.shift_remove(&name);
        }
    }
    if tag == "a"
        && let Some(target) = element.attrs.shift_remove(&temporary)
    {
        let blank = target.as_ref() == "_blank";
        element.attrs.insert(target_name, target);
        if blank {
            element
                .attrs
                .insert(QualName::new(None, ns!(), "rel".into()), "noopener".into());
        }
    }
    Ok(())
}

pub(super) fn sanitize<S: SanitizeOutputSink>(
    text: &str,
    cfg: &DompurifyEffectiveConfig,
    sink: &S,
) -> Result<String, SanitizeFailure<S::Error>> {
    if text.is_empty() {
        return Ok(String::new());
    }
    // Preserve the existing fail-closed admission boundary before constructing the full tree.
    let mut admission = lol_html::HtmlRewriter::new(
        lol_html::Settings::new()
            .append_element_content_handler(lol_html::element!("*", |_| Ok(()))),
        |_: &[u8]| {},
    );
    admission
        .write(text.as_bytes())
        .map_err(|_| SanitizeFailure::RejectedInput)?;
    admission
        .end()
        .map_err(|_| SanitizeFailure::RejectedInput)?;

    let mut options = html5ever::ParseOpts::default();
    options.tree_builder.scripting_enabled = false;
    let mut document =
        html5ever::parse_document(HtmlTreeSink::new(Html::new_document()), options).one(text);
    let body = document
        .root_element()
        .children()
        .find_map(|node| {
            ElementRef::wrap(node).filter(|e| e.value().name.local == local_name!("body"))
        })
        .ok_or(SanitizeFailure::RejectedInput)?
        .id();
    let leading = text.trim_start_matches(['\r', '\n', '\t', ' ']);
    let prefix = &text[..text.len() - leading.len()];
    if !prefix.is_empty() {
        document
            .tree
            .get_mut(body)
            .unwrap()
            .prepend(Node::Text(scraper::node::Text {
                text: prefix.into(),
            }));
    }
    let mut pending: Vec<_> = document
        .tree
        .get(body)
        .unwrap()
        .children()
        .rev()
        .map(|node| node.id())
        .collect();
    while let Some(id) = pending.pop() {
        let node = document.tree.get(id).unwrap();
        match node.value() {
            Node::Comment(comment) => {
                if !cfg.allowed_tags.contains("#comment")
                    || cfg.forbid_tags.contains("#comment")
                    || (cfg.safe_for_xml && markup_probe(comment, false))
                {
                    document.tree.get_mut(id).unwrap().detach();
                }
            }
            Node::ProcessingInstruction(_) | Node::Doctype(_) => {
                document.tree.get_mut(id).unwrap().detach();
            }
            Node::Text(text) => {
                if cfg.forbid_tags.contains("#text") || !cfg.allowed_tags.contains("#text") {
                    document.tree.get_mut(id).unwrap().detach();
                    continue;
                }
                if cfg.safe_for_templates {
                    let mut filtered = owned_output(text, sink)?;
                    strip_template_expressions(&mut filtered);
                    *document.tree.get_mut(id).unwrap().value() = Node::Text(scraper::node::Text {
                        text: filtered.into(),
                    });
                }
            }
            Node::Element(element) => {
                let tag = element.name.local.as_ref().to_ascii_lowercase();
                let reference = ElementRef::wrap(node).unwrap();
                let has_element_children = node.children().any(|child| child.value().is_element());
                let mut unsafe_node = matches!(tag.as_str(), "script" | "iframe" | "style");
                if !unsafe_node && cfg.safe_for_xml && !has_element_children {
                    let mut text = sink.output_buffer(0).map_err(SanitizeFailure::Output)?;
                    for child in node.children().filter_map(|child| child.value().as_text()) {
                        sink.push_output_chunk(&mut text, child.as_bytes())
                            .map_err(SanitizeFailure::Output)?;
                    }
                    let text = std::str::from_utf8(&text)
                        .map_err(|_| SanitizeFailure::InvalidUtf8Output)?;
                    unsafe_node = markup_probe(text, true)
                        && markup_probe(&serialize_children(reference, sink)?, true);
                }
                if unsafe_node {
                    document.tree.get_mut(id).unwrap().detach();
                    continue;
                }
                let children: Vec<_> = node.children().map(|child| child.id()).collect();
                if !cfg.allowed_tags.contains(&tag) || cfg.forbid_tags.contains(&tag) {
                    if cfg.keep_content && !cfg.forbid_contents.contains(&tag) {
                        for child in &children {
                            document.tree.get_mut(id).unwrap().insert_id_before(*child);
                        }
                        pending.extend(children.into_iter().rev());
                    }
                    document.tree.get_mut(id).unwrap().detach();
                    continue;
                }
                if !valid_namespace(reference, cfg) {
                    document.tree.get_mut(id).unwrap().detach();
                    continue;
                }
                if matches!(tag.as_str(), "noscript" | "noembed" | "noframes") {
                    let inner = serialize_children(reference, sink)?.to_ascii_lowercase();
                    if ["</noscript", "</noembed", "</noframes"]
                        .iter()
                        .any(|closing| inner.contains(closing))
                    {
                        document.tree.get_mut(id).unwrap().detach();
                        continue;
                    }
                }
                if let Node::Element(element) = document.tree.get_mut(id).unwrap().value() {
                    filter_attributes(element, cfg, sink)?;
                }
                pending.extend(children.into_iter().rev());
            }
            _ => {
                pending.extend(node.children().rev().map(|child| child.id()));
            }
        }
    }
    let mut output = serialize_children(
        ElementRef::wrap(document.tree.get(body).unwrap()).unwrap(),
        sink,
    )?;
    if cfg.safe_for_templates {
        strip_template_expressions(&mut output);
    }
    Ok(output)
}
