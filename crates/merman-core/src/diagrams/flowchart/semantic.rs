use crate::diagram::{DiagramWarningFact, FLOWCHART_UNKNOWN_STYLE_TARGET_WARNING_RULE_ID};
use crate::sanitize::sanitize_text;
use crate::utils::format_url;
use crate::{Error, MermaidConfig, OperationControl, OperationControlResult, Result};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use super::{
    ClickAction, Edge, EdgeDefaults, FlowNodeProvenance, FlowNodeSyntax, FlowSubGraph,
    FlowSubgraphVertexStyle, FlowchartRenderStyleSources, LinkStylePos, Node, Stmt, TitleKind,
    apply_shape_data_value_to_node, value_to_bool, value_to_string,
};

pub(super) struct FlowchartSemanticContext<'a> {
    pub(super) source_occurrences: &'a mut Vec<serde_json::Value>,
    pub(super) nodes: &'a mut Vec<Node>,
    pub(super) node_index: &'a mut HashMap<String, usize>,
    pub(super) edges: &'a mut Vec<Edge>,
    pub(super) subgraphs: &'a mut Vec<FlowSubGraph>,
    pub(super) subgraph_vertex_styles: &'a mut FlowchartRenderStyleSources,
    pub(super) collapsed_subgraphs: &'a mut rustc_hash::FxHashSet<String>,
    pub(super) vertex_calls: &'a mut Vec<String>,
    pub(super) warning_facts: &'a mut Vec<DiagramWarningFact>,
    pub(super) class_defs: &'a mut IndexMap<String, Vec<String>>,
    pub(super) tooltips: &'a mut HashMap<String, String>,
    pub(super) edge_defaults: &'a mut EdgeDefaults,
    pub(super) security_level_loose: bool,
    pub(super) diagram_type: &'a str,
    pub(super) config: &'a MermaidConfig,
    pub(super) shape_data_documents: &'a HashMap<String, crate::yaml_config::YamlValueCapture>,
    pub(super) control: &'a OperationControl,
}

pub(super) fn apply_semantic_statements(
    statements: &[Stmt],
    ctx: &mut FlowchartSemanticContext<'_>,
) -> OperationControlResult<Result<()>> {
    ctx.apply_statements(statements)
}

impl<'a> FlowchartSemanticContext<'a> {
    fn apply_statements(&mut self, statements: &[Stmt]) -> OperationControlResult<Result<()>> {
        enum ReplayItem<'a> {
            Statement(&'a Stmt),
            FinishSubgraph,
        }

        let mut stack = statements
            .iter()
            .rev()
            .map(ReplayItem::Statement)
            .collect::<Vec<_>>();
        let mut visited = 0usize;
        let mut active_subgraphs = HashMap::new();
        let mut seen_vertex_ids = HashSet::new();
        let mut vertex_css = HashMap::new();
        let mut seen_edge_indices: HashMap<String, Vec<usize>> = HashMap::new();
        let mut next_built_edge_index = 0usize;
        let mut next_built_subgraph_index = 0usize;
        let mut class_definition_sources = Vec::new();
        let mut link_style_sources = Vec::new();
        while let Some(item) = stack.pop() {
            if visited.is_multiple_of(128) {
                self.control.checkpoint()?;
            }
            visited = visited.saturating_add(1);

            let ReplayItem::Statement(stmt) = item else {
                let Some(subgraph) = self.subgraphs.get(next_built_subgraph_index) else {
                    return Ok(Err(Error::diagram_parse_fallback(
                        self.diagram_type.to_string(),
                        "flowchart subgraph replay diverged from the built model",
                    )));
                };
                active_subgraphs.insert(subgraph.id.clone(), next_built_subgraph_index);
                next_built_subgraph_index = next_built_subgraph_index.saturating_add(1);
                continue;
            };

            match stmt {
                Stmt::Subgraph(sg) => {
                    stack.push(ReplayItem::FinishSubgraph);
                    stack.extend(sg.statements.iter().rev().map(ReplayItem::Statement));
                }
                Stmt::Style(s) => {
                    let trace_span = (self
                        .config
                        .as_value()
                        .get("traceSource")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true))
                    .then(|| {
                        s.editor_evidence
                            .iter()
                            .find(|e| e.kind == crate::EditorExpectedSyntaxKind::Directive)
                            .map(|e| e.span)
                    })
                    .flatten();
                    if seen_edge_indices.contains_key(&s.target) {
                        if let Some(span) = trace_span {
                            self.source_occurrences.push(serde_json::json!({"kind":"nonvisual","relation":"style","classification":"ignored-edge-style","semanticId":s.target,"span":span}));
                        }
                        continue;
                    }
                    self.vertex_calls.push(s.target.clone());
                    let is_new_vertex = seen_vertex_ids.insert(s.target.clone());
                    if let Some(span) = trace_span {
                        let mut piece = serde_json::json!({"kind":"node","semanticId":s.target,"domId":format!("node:{}",s.target),"relation":"style","span":span});
                        if is_new_vertex && !active_subgraphs.contains_key(&s.target) {
                            piece["declaration"] = serde_json::json!(true);
                            if let Some(target) = s.target_span {
                                piece["defaultLabelOrigin"] = serde_json::json!(target);
                            }
                        }
                        self.source_occurrences.push(piece);
                    }
                    vertex_css
                        .entry(s.target.clone())
                        .or_insert_with(FlowSubgraphVertexStyle::default)
                        .styles
                        .extend(s.styles.iter().cloned());
                    if !active_subgraphs.contains_key(&s.target) {
                        if is_new_vertex {
                            let mut warning = DiagramWarningFact::new(
                                FLOWCHART_UNKNOWN_STYLE_TARGET_WARNING_RULE_ID,
                                format!(
                                    "Style applied to unknown node \"{}\". This may indicate a typo. The node will be created automatically.",
                                    s.target
                                ),
                            );
                            if let Some(span) = s.target_span {
                                warning = warning.with_span(span);
                            }
                            self.warning_facts.push(warning);
                        }
                        let idx = self.ensure_authored_node(&s.target);
                        if is_new_vertex {
                            self.nodes[idx].id_span = s.target_span;
                        }
                        self.nodes[idx].styles.extend(s.styles.iter().cloned());
                    }
                }
                Stmt::ClassDef(c) => {
                    if self.tracing()
                        && let Some(span) = c.editor_evidence.statement_span()
                    {
                        class_definition_sources.push((span, c.ids.clone()));
                    }
                    for (index, id) in c.ids.iter().enumerate() {
                        if index % 128 == 0 {
                            self.control.checkpoint()?;
                        }
                        self.class_defs.insert(id.clone(), c.styles.clone());
                    }
                }
                Stmt::ClassAssign(c) => {
                    for (index, target) in c.targets.iter().enumerate() {
                        if index % 128 == 0 {
                            self.control.checkpoint()?;
                        }
                        if let Some(style) = vertex_css.get_mut(target) {
                            style.classes.push(c.class_name.clone());
                        }
                        self.add_class_to_target(
                            target,
                            &c.class_name,
                            &active_subgraphs,
                            &seen_vertex_ids,
                            &seen_edge_indices,
                        )?;
                        self.trace_directive_target(
                            c.editor_evidence.statement_span(),
                            "class",
                            target,
                            &active_subgraphs,
                            &seen_vertex_ids,
                            &seen_edge_indices,
                        );
                    }
                }
                Stmt::Click(c) => {
                    for (index, id) in c.ids.iter().enumerate() {
                        if index % 128 == 0 {
                            self.control.checkpoint()?;
                        }
                        if let Some(tt) = &c.tooltip {
                            self.tooltips
                                .insert(id.clone(), sanitize_text(tt, self.config));
                        }
                        if let Some(style) = vertex_css.get_mut(id) {
                            style.classes.push("clickable".to_string());
                        }
                        self.add_class_to_target(
                            id,
                            "clickable",
                            &active_subgraphs,
                            &seen_vertex_ids,
                            &seen_edge_indices,
                        )?;

                        self.trace_directive_target(
                            c.editor_evidence.statement_span(),
                            "click",
                            id,
                            &active_subgraphs,
                            &seen_vertex_ids,
                            &seen_edge_indices,
                        );
                        match &c.action {
                            ClickAction::Link { href, target } => {
                                if seen_vertex_ids.contains(id)
                                    && let Some(&idx) = self.node_index.get(id)
                                {
                                    self.nodes[idx].link = format_url(href, self.config);
                                    self.nodes[idx].link_target = target.clone();
                                }
                            }
                            ClickAction::Callback => {
                                if self.security_level_loose
                                    && seen_vertex_ids.contains(id)
                                    && let Some(&idx) = self.node_index.get(id)
                                {
                                    self.nodes[idx].have_callback = true;
                                }
                            }
                        }
                    }
                }
                Stmt::LinkStyle(ls) => {
                    if self.tracing()
                        && let Some(span) = ls.span
                    {
                        if ls.interpolate.is_some() || !ls.styles.is_empty() {
                            link_style_sources.push((span, ls.positions.clone()));
                        } else {
                            self.trace_nonvisual(span, "linkStyle", "empty-link-style");
                        }
                    }
                    if let Some(algo) = &ls.interpolate {
                        for (index, pos) in ls.positions.iter().enumerate() {
                            if index % 128 == 0 {
                                self.control.checkpoint()?;
                            }
                            match pos {
                                LinkStylePos::Default => {
                                    self.edge_defaults.interpolate = Some(algo.clone())
                                }
                                LinkStylePos::Index(i) => {
                                    if *i >= next_built_edge_index {
                                        return Ok(Err(Error::diagram_parse_fallback(
                                            self.diagram_type.to_string(),
                                            format!(
                                                "The index {i} for linkStyle is out of bounds. Valid indices for linkStyle are between 0 and {}. (Help: Ensure that the index is within the range of existing edges.)",
                                                next_built_edge_index.saturating_sub(1)
                                            ),
                                        )));
                                    }
                                    self.edges[*i].interpolate = Some(algo.clone());
                                }
                            }
                        }
                    }

                    if !ls.styles.is_empty() {
                        for (index, pos) in ls.positions.iter().enumerate() {
                            if index % 128 == 0 {
                                self.control.checkpoint()?;
                            }
                            match pos {
                                LinkStylePos::Default => {
                                    self.edge_defaults.style = ls.styles.clone()
                                }
                                LinkStylePos::Index(i) => {
                                    if *i >= next_built_edge_index {
                                        return Ok(Err(Error::diagram_parse_fallback(
                                            self.diagram_type.to_string(),
                                            format!(
                                                "The index {i} for linkStyle is out of bounds. Valid indices for linkStyle are between 0 and {}. (Help: Ensure that the index is within the range of existing edges.)",
                                                next_built_edge_index.saturating_sub(1)
                                            ),
                                        )));
                                    }
                                    self.edges[*i].style = ls.styles.clone();
                                    if !self.edges[*i].style.is_empty()
                                        && !self.edges[*i]
                                            .style
                                            .iter()
                                            .any(|s| s.trim_start().starts_with("fill"))
                                    {
                                        self.edges[*i].style.push("fill:none".to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                Stmt::ShapeData {
                    target,
                    target_span,
                    yaml,
                } => {
                    let value = match Self::shape_data_value(
                        self.shape_data_documents,
                        self.diagram_type,
                        yaml,
                    ) {
                        Ok(value) => value,
                        Err(error) => return Ok(Err(error)),
                    };
                    // Mermaid checks subgraph metadata before edge metadata. This matters when
                    // an id is shared by a subgraph and a user-defined edge.
                    let routed_to_subgraph =
                        active_subgraphs.get(target).is_some_and(|&subgraph_index| {
                            self.apply_shape_data_to_subgraph(subgraph_index, target, value)
                        });
                    if routed_to_subgraph {
                        self.trace_shape_data(target, *target_span, yaml, "control", None);
                        // The parser first calls addVertex for the bare reference and then
                        // calls it again with shapeData. A subgraph metadata call returns from
                        // the second call, so retain only the preceding vertex call unless the
                        // id already names an edge (where addVertex returns immediately).
                        if !seen_edge_indices.contains_key(target) {
                            self.vertex_calls.push(target.clone());
                            seen_vertex_ids.insert(target.clone());
                            vertex_css.entry(target.clone()).or_default();
                        }
                        continue;
                    }

                    if let Some(indices) = seen_edge_indices.get(target) {
                        Self::apply_shape_data_to_edges(self.edges, self.control, indices, value)?;
                        for &index in indices {
                            self.trace_shape_data(target, *target_span, yaml, "edge", Some(index));
                        }
                        continue;
                    }

                    // The Jison grammar calls addVertex once for the node and once for its
                    // shapeData. Preserve both calls because Mermaid derives DOM ids from this
                    // sequence even though they update the same typed node.
                    self.vertex_calls.push(target.clone());
                    self.vertex_calls.push(target.clone());
                    seen_vertex_ids.insert(target.clone());
                    vertex_css.entry(target.clone()).or_default();
                    let idx = self.ensure_authored_node(target);
                    if let Err(error) = apply_shape_data_value_to_node(&mut self.nodes[idx], value)
                    {
                        return Ok(Err(Error::diagram_parse_fallback(
                            self.diagram_type.to_string(),
                            error,
                        )));
                    }
                    self.trace_shape_data(target, *target_span, yaml, "node", None);
                }
                Stmt::Chain {
                    node_groups,
                    edge_groups,
                } => {
                    if let Some(first_group) = node_groups.first()
                        && let Err(error) = self.observe_node_group(
                            first_group,
                            &active_subgraphs,
                            &mut seen_vertex_ids,
                            &mut vertex_css,
                            &seen_edge_indices,
                        )?
                    {
                        return Ok(Err(error));
                    }
                    for (segment_index, edge_group) in edge_groups.iter().enumerate() {
                        if let Some(next_group) = node_groups.get(segment_index + 1)
                            && let Err(error) = self.observe_node_group(
                                next_group,
                                &active_subgraphs,
                                &mut seen_vertex_ids,
                                &mut vertex_css,
                                &seen_edge_indices,
                            )?
                        {
                            return Ok(Err(error));
                        }
                        if let Err(error) = self.activate_edges(
                            edge_group.len(),
                            &mut next_built_edge_index,
                            &mut seen_edge_indices,
                        )? {
                            return Ok(Err(error));
                        }
                    }
                }
                Stmt::Node(node) => {
                    if let Err(error) = self.observe_node_group(
                        std::slice::from_ref(node.as_ref()),
                        &active_subgraphs,
                        &mut seen_vertex_ids,
                        &mut vertex_css,
                        &seen_edge_indices,
                    )? {
                        return Ok(Err(error));
                    }
                }
                Stmt::Direction(_) => {}
            }
        }

        if next_built_edge_index != self.edges.len()
            || next_built_subgraph_index != self.subgraphs.len()
        {
            return Ok(Err(Error::diagram_parse_fallback(
                self.diagram_type.to_string(),
                "flowchart semantic replay did not consume the built model",
            )));
        }
        for (index, (id, style)) in vertex_css.into_iter().enumerate() {
            if index % 128 == 0 {
                self.control.checkpoint()?;
            }
            if let Some(&declaration_ordinal) = active_subgraphs.get(&id) {
                // FlowDB emits duplicate subgraphs in reverse declaration order, then applies
                // the single vertex record to the first matching node. Preserve that exact
                // declaration owner instead of broadcasting vertex CSS to every duplicate id.
                self.subgraph_vertex_styles
                    .insert(id, declaration_ordinal, style);
            }
        }
        if self.tracing() {
            for (span, names) in class_definition_sources {
                self.control.checkpoint()?;
                for name in names {
                    let mut targets = Vec::new();
                    for node in self
                        .nodes
                        .iter()
                        .filter(|node| !active_subgraphs.contains_key(&node.id))
                    {
                        if super::flowchart_effective_node_class_names(
                            self.class_defs,
                            &node.classes,
                        )
                        .contains(&name.as_str())
                        {
                            targets.push(("node", node.id.clone(), None));
                        }
                    }
                    for (index, group) in self.subgraphs.iter().enumerate() {
                        let (classes, _) =
                            self.subgraph_vertex_styles.effective_subgraph_css_values(
                                index,
                                &group.id,
                                &group.classes,
                                &group.styles,
                            );
                        if classes.contains(&name) {
                            targets.push(("control", group.id.clone(), None));
                        }
                    }
                    for (index, edge) in self.edges.iter().enumerate() {
                        if edge.classes.contains(&name) {
                            targets.push((
                                "edge",
                                edge.id.clone().expect("native edge ID"),
                                Some(index),
                            ));
                        }
                    }
                    if targets.is_empty() {
                        self.trace_nonvisual(span, &name, "unused-class-definition");
                    } else {
                        for (kind, target, index) in targets {
                            self.trace_relationship(span, "classDef", kind, &target, index);
                        }
                    }
                }
            }
            for (span, positions) in link_style_sources {
                self.control.checkpoint()?;
                let indices: Vec<_> = if positions.contains(&LinkStylePos::Default) {
                    (0..self.edges.len()).collect()
                } else {
                    positions
                        .into_iter()
                        .filter_map(|pos| match pos {
                            LinkStylePos::Index(index) => Some(index),
                            LinkStylePos::Default => None,
                        })
                        .collect()
                };
                if indices.is_empty() {
                    self.trace_nonvisual(span, "linkStyle", "no-edge-targets");
                }
                for index in indices {
                    let id = self.edges[index]
                        .id
                        .clone()
                        .expect("validated native edge ID");
                    self.trace_relationship(span, "linkStyle", "edge", &id, Some(index));
                }
            }
        }
        self.control.checkpoint()?;
        Ok(Ok(()))
    }

    fn observe_node_group(
        &mut self,
        nodes: &[Node],
        active_subgraphs: &HashMap<String, usize>,
        seen_vertex_ids: &mut HashSet<String>,
        vertex_css: &mut HashMap<String, FlowSubgraphVertexStyle>,
        seen_edge_indices: &HashMap<String, Vec<usize>>,
    ) -> OperationControlResult<Result<()>> {
        let mut deferred_shape_data_calls = Vec::new();
        for (index, node) in nodes.iter().enumerate() {
            if index % 128 == 0 {
                self.control.checkpoint()?;
            }
            if let Some(span) = node.class_span {
                if active_subgraphs.contains_key(&node.id)
                    || seen_edge_indices.contains_key(&node.id)
                {
                    self.trace_directive_target(
                        Some(span),
                        "inline-class",
                        &node.id,
                        active_subgraphs,
                        seen_vertex_ids,
                        seen_edge_indices,
                    );
                } else {
                    self.trace_relationship(span, "inline-class", "node", &node.id, None);
                }
            }
            if let Some(yaml) = node.shape_data.as_ref()
                && let Some(&subgraph_index) = active_subgraphs.get(&node.id)
            {
                let value = match Self::shape_data_value(
                    self.shape_data_documents,
                    self.diagram_type,
                    yaml,
                ) {
                    Ok(value) => value,
                    Err(error) => return Ok(Err(error)),
                };
                if self.apply_shape_data_to_subgraph(subgraph_index, &node.id, value) {
                    self.trace_shape_data(&node.id, node.id_span, yaml, "control", None);
                    // The node production has already emitted one bare addVertex call before
                    // its shapeData action. Preserve that call for DOM-id sequencing, while
                    // avoiding it when an existing edge makes addVertex return early.
                    if !seen_edge_indices.contains_key(&node.id) {
                        self.vertex_calls.push(node.id.clone());
                        seen_vertex_ids.insert(node.id.clone());
                        let style = vertex_css.entry(node.id.clone()).or_default();
                        style.classes.extend(node.classes.iter().cloned());
                        style.styles.extend(node.styles.iter().cloned());
                    }
                    continue;
                }
            }

            if let Some(indices) = seen_edge_indices.get(&node.id) {
                if let Some(yaml) = node.shape_data.as_ref() {
                    let value = match Self::shape_data_value(
                        self.shape_data_documents,
                        self.diagram_type,
                        yaml,
                    ) {
                        Ok(value) => value,
                        Err(error) => return Ok(Err(error)),
                    };
                    Self::apply_shape_data_to_edges(self.edges, self.control, indices, value)?;
                    for &index in indices {
                        self.trace_shape_data(&node.id, node.id_span, yaml, "edge", Some(index));
                    }
                }
                continue;
            }

            // Replay explicit labels/shapes in authored order alongside shapeData updates.
            // The structural builder reserves nodes ahead of this semantic pass.
            if let Some(&index) = self.node_index.get(&node.id) {
                if node.label.is_some() {
                    self.nodes[index].label = node.label.clone();
                    self.nodes[index].label_type = node.label_type.clone();
                    self.nodes[index].label_span = node.label_span;
                    self.nodes[index].label_selection = node.label_selection;
                }
                if node.shape.is_some() {
                    self.nodes[index].shape = node.shape.clone();
                }
            }
            self.vertex_calls.push(node.id.clone());
            seen_vertex_ids.insert(node.id.clone());
            let style = vertex_css.entry(node.id.clone()).or_default();
            style.classes.extend(node.classes.iter().cloned());
            style.styles.extend(node.styles.iter().cloned());
            if let Some(yaml) = node.shape_data.as_ref() {
                deferred_shape_data_calls.push(node.id.clone());
                let value = match Self::shape_data_value(
                    self.shape_data_documents,
                    self.diagram_type,
                    yaml,
                ) {
                    Ok(value) => value,
                    Err(error) => return Ok(Err(error)),
                };
                let idx = self.ensure_authored_node(&node.id);
                if let Err(error) = apply_shape_data_value_to_node(&mut self.nodes[idx], value) {
                    return Ok(Err(Error::diagram_parse_fallback(
                        self.diagram_type.to_string(),
                        error,
                    )));
                }
                self.trace_shape_data(&node.id, node.id_span, yaml, "node", None);
            }
        }
        for (index, id) in deferred_shape_data_calls.into_iter().enumerate() {
            if index % 128 == 0 {
                self.control.checkpoint()?;
            }
            self.vertex_calls.push(id);
        }
        Ok(Ok(()))
    }

    fn tracing(&self) -> bool {
        self.config
            .as_value()
            .get("traceSource")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    }

    fn trace_nonvisual(&mut self, span: crate::SourceSpan, target: &str, classification: &str) {
        if self.tracing() {
            self.source_occurrences.push(serde_json::json!({"kind":"nonvisual","semanticId":target,"classification":classification,"span":span}));
        }
    }

    fn trace_relationship(
        &mut self,
        span: crate::SourceSpan,
        relation: &str,
        kind: &str,
        target: &str,
        edge_index: Option<usize>,
    ) {
        if !self.tracing() {
            return;
        }
        let dom_id = match kind {
            "control" => format!("flowchart:subgraph:{target}"),
            "edge" => format!("edge:{target}"),
            _ => format!("node:{target}"),
        };
        let mut piece = serde_json::json!({"kind":kind,"semanticId":target,"domId":dom_id,"relation":relation,"span":span});
        if let Some(index) = edge_index {
            piece["from"] = serde_json::json!(self.edges[index].from);
            piece["to"] = serde_json::json!(self.edges[index].to);
        }
        self.source_occurrences.push(piece);
    }

    fn trace_directive_target(
        &mut self,
        span: Option<crate::SourceSpan>,
        relation: &str,
        target: &str,
        groups: &HashMap<String, usize>,
        vertices: &HashSet<String>,
        edges: &HashMap<String, Vec<usize>>,
    ) {
        let Some(span) = span.filter(|_| self.tracing()) else {
            return;
        };
        let mut found = false;
        if groups.contains_key(target) {
            self.trace_relationship(span, relation, "control", target, None);
            found = true;
        }
        if vertices.contains(target) && self.node_index.contains_key(target) {
            self.trace_relationship(span, relation, "node", target, None);
            found = true;
        }
        if let Some(indices) = edges.get(target) {
            for &index in indices {
                let id = self.edges[index].id.clone().expect("native edge ID");
                self.trace_relationship(span, relation, "edge", &id, Some(index));
            }
            found = true;
        }
        if !found {
            self.trace_nonvisual(span, target, "unresolved-directive-target");
        }
    }

    fn trace_shape_data(
        &mut self,
        target: &str,
        target_span: Option<crate::SourceSpan>,
        yaml: &super::ShapeDataToken,
        kind: &str,
        edge_index: Option<usize>,
    ) {
        if self
            .config
            .as_value()
            .get("traceSource")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        {
            return;
        }
        let document = self
            .shape_data_documents
            .get(&**yaml)
            .expect("prepared shape data");
        let (semantic_id, dom_id) = if let Some(index) = edge_index {
            let id = self.edges[index]
                .id
                .as_deref()
                .expect("native assigned edge ID");
            (id.to_string(), format!("edge:{id}"))
        } else if kind == "control" {
            (target.to_string(), format!("flowchart:subgraph:{target}"))
        } else {
            (target.to_string(), format!("node:{target}"))
        };
        let span = crate::SourceSpan::new(
            target_span.map_or(yaml.span.start, |span| span.start),
            yaml.span.end,
        );
        let mut piece = serde_json::json!({"kind":kind,"semanticId":semantic_id,"domId":dom_id,"relation":"shape-data","span":span});
        if let Some(index) = edge_index {
            piece["from"] = serde_json::json!(self.edges[index].from);
            piece["to"] = serde_json::json!(self.edges[index].to);
        }
        if kind == "node" {
            piece["declaration"] = serde_json::json!(true);
            if document
                .value
                .as_ref()
                .ok()
                .and_then(|v| v.get("label"))
                .and_then(value_to_string)
                .is_some()
            {
                if let Some(label) = document
                    .keys
                    .iter()
                    .rev()
                    .find(|key| key.path.matches(&["label"]))
                {
                    let origin = label.value_span.as_ref().and_then(|span| {
                        yaml.map_span(crate::SourceSpan::new(span.start, span.end))
                    });
                    let selection = label.value_selection.as_ref().and_then(|span| {
                        yaml.map_span(crate::SourceSpan::new(span.start, span.end))
                    });
                    if let Some(&index) = self.node_index.get(target) {
                        self.nodes[index].label_span = origin;
                        self.nodes[index].label_selection = selection;
                    }
                    if let Some(origin) = origin {
                        piece["labelOrigin"] = serde_json::json!(origin);
                    }
                    if let Some(selection) = selection.filter(|span| span.start < span.end) {
                        piece["labelSpan"] = serde_json::json!(selection);
                    }
                }
            }
        }
        self.source_occurrences.push(piece.clone());
        for key in &document.keys {
            let Some(property) = key
                .path
                .first()
                .filter(|property| key.path.matches(&[property]))
            else {
                continue;
            };
            let Some(span) = key
                .value_selection
                .as_ref()
                .and_then(|span| yaml.map_span(crate::SourceSpan::new(span.start, span.end)))
                .filter(|span| span.start < span.end)
            else {
                continue;
            };
            let mut property_piece = piece.clone();
            let fields = property_piece.as_object_mut().expect("native piece object");
            for name in ["labelOrigin", "labelSpan", "declaration"] {
                fields.remove(name);
            }
            property_piece["relation"] = serde_json::json!("shape-data-property");
            property_piece["property"] = serde_json::json!(property);
            property_piece["span"] = serde_json::json!(span);
            let applied = match kind {
                "edge" => matches!(property, "animate" | "animation" | "curve"),
                "node" => matches!(
                    property,
                    "shape"
                        | "label"
                        | "labelType"
                        | "icon"
                        | "form"
                        | "pos"
                        | "img"
                        | "constraint"
                        | "w"
                        | "h"
                ),
                _ => true,
            };
            if !applied {
                property_piece["kind"] = serde_json::json!("nonvisual");
                property_piece["classification"] = serde_json::json!("ignored-shape-data-property");
            }
            self.source_occurrences.push(property_piece);
        }
    }

    fn add_class_to_target(
        &mut self,
        target: &str,
        class_name: &str,
        active_subgraphs: &HashMap<String, usize>,
        seen_vertex_ids: &HashSet<String>,
        seen_edge_indices: &HashMap<String, Vec<usize>>,
    ) -> OperationControlResult<()> {
        if let Some(&idx) = active_subgraphs.get(target) {
            self.subgraphs[idx].classes.push(class_name.to_string());
        }
        if seen_vertex_ids.contains(target)
            && let Some(&idx) = self.node_index.get(target)
        {
            self.nodes[idx].classes.push(class_name.to_string());
        }
        if let Some(edge_indices) = seen_edge_indices.get(target) {
            for (index, edge_index) in edge_indices.iter().copied().enumerate() {
                if index % 128 == 0 {
                    self.control.checkpoint()?;
                }
                if let Some(edge) = self.edges.get_mut(edge_index) {
                    edge.classes.push(class_name.to_string());
                }
            }
        } else {
            // Preserve the canonical cancellation cadence even when the requested edge id is
            // absent. The previous implementation scanned every built edge in this case.
            for index in 0..self.edges.len() {
                if index % 128 == 0 {
                    self.control.checkpoint()?;
                }
            }
        }
        Ok(())
    }

    fn activate_edges(
        &self,
        count: usize,
        next_edge_index: &mut usize,
        seen_edge_indices: &mut HashMap<String, Vec<usize>>,
    ) -> OperationControlResult<Result<()>> {
        let Some(edge_end) = next_edge_index.checked_add(count) else {
            return Ok(Err(Error::diagram_parse_fallback(
                self.diagram_type.to_string(),
                "flowchart edge replay index overflow",
            )));
        };
        if edge_end > self.edges.len() {
            return Ok(Err(Error::diagram_parse_fallback(
                self.diagram_type.to_string(),
                "flowchart edge replay diverged from the built model",
            )));
        }
        for (index, edge_index) in (*next_edge_index..edge_end).enumerate() {
            if index % 128 == 0 {
                self.control.checkpoint()?;
            }
            if let Some(id) = self.edges[edge_index].id.as_deref() {
                seen_edge_indices
                    .entry(id.to_string())
                    .or_default()
                    .push(edge_index);
            }
        }
        *next_edge_index = edge_end;
        Ok(Ok(()))
    }

    fn shape_data_value<'b>(
        documents: &'b HashMap<String, crate::yaml_config::YamlValueCapture>,
        diagram_type: &str,
        yaml: &str,
    ) -> Result<&'b serde_json::Value> {
        match documents
            .get(yaml)
            .expect("flowchart shape data must be prepared before semantic construction")
            .value
            .as_ref()
        {
            Ok(document) => Ok(document),
            Err(error) => Err(Error::diagram_parse_fallback(
                diagram_type.to_string(),
                format!("Invalid shapeData: {error}"),
            )),
        }
    }

    fn apply_shape_data_to_subgraph(
        &mut self,
        subgraph_index: usize,
        target: &str,
        value: &serde_json::Value,
    ) -> bool {
        let Some(map) = value.as_object() else {
            return false;
        };
        let Some(subgraph) = self.subgraphs.get_mut(subgraph_index) else {
            return false;
        };
        if subgraph.id != target {
            return false;
        }

        let metadata = subgraph
            .metadata
            .get_or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        let Some(metadata_map) = metadata.as_object_mut() else {
            *metadata = serde_json::Value::Object(serde_json::Map::new());
            unreachable!("subgraph metadata was replaced with an object");
        };
        for (key, item) in map {
            metadata_map.insert(key.clone(), item.clone());
        }

        if map.contains_key("view") {
            if metadata_map.get("view").and_then(serde_json::Value::as_str) == Some("collapsed") {
                self.collapsed_subgraphs.insert(target.to_string());
            } else {
                self.collapsed_subgraphs.remove(target);
            }
        }
        true
    }

    fn apply_shape_data_to_edges(
        edges: &mut [Edge],
        control: &OperationControl,
        indices: &[usize],
        value: &serde_json::Value,
    ) -> OperationControlResult<()> {
        let Some(map) = value.as_object() else {
            return Ok(());
        };
        for (index, edge_index) in indices.iter().copied().enumerate() {
            if index % 128 == 0 {
                control.checkpoint()?;
            }
            let Some(edge) = edges.get_mut(edge_index) else {
                continue;
            };
            for (key, value) in map {
                match key.as_str() {
                    "animate" => {
                        if let Some(value) = value_to_bool(value) {
                            edge.animate = Some(value);
                        }
                    }
                    "animation" => {
                        if let Some(value) = value_to_string(value) {
                            edge.animation = Some(value);
                        }
                    }
                    "curve" => {
                        if let Some(value) = value_to_string(value) {
                            edge.interpolate = Some(value);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn ensure_authored_node(&mut self, id: &str) -> usize {
        if let Some(&idx) = self.node_index.get(id) {
            self.nodes[idx].provenance = FlowNodeProvenance::Authored;
            self.nodes[idx].syntax = FlowNodeSyntax::ExplicitDefinition;
            return idx;
        }
        let idx = self.nodes.len();
        self.nodes.push(Node {
            id: id.to_string(),
            provenance: FlowNodeProvenance::Authored,
            syntax: FlowNodeSyntax::ExplicitDefinition,
            id_span: None,
            class_span: None,
            label: None,
            label_type: TitleKind::Text,
            label_span: None,
            label_selection: None,
            shape: None,
            shape_data: None,
            icon: None,
            form: None,
            pos: None,
            img: None,
            constraint: None,
            asset_width: None,
            asset_height: None,
            styles: Vec::new(),
            classes: Vec::new(),
            link: None,
            link_target: None,
            have_callback: false,
        });
        self.node_index.insert(id.to_string(), idx);
        idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagrams::flowchart::{
        ClassAssignStmt, FlowEdgeMarker, FlowEdgeStroke, FlowEdgeVisibility, LinkToken,
    };

    #[test]
    fn class_assignment_can_cancel_during_edge_scanning() {
        let mut nodes = Vec::new();
        let mut node_index = HashMap::new();
        let mut edges = (0..512)
            .map(|index| Edge {
                source_span: None,
                from: format!("n{index}"),
                to: format!("n{}", index + 1),
                id: Some(format!("edge-{index}")),
                link: LinkToken {
                    end: "arrow_point".to_string(),
                    start_marker: FlowEdgeMarker::None,
                    end_marker: FlowEdgeMarker::Point,
                    stroke_kind: FlowEdgeStroke::Normal,
                    visibility: FlowEdgeVisibility::Visible,
                    length: 1,
                },
                label: None,
                label_type: TitleKind::Text,
                label_span: None,
                label_selection: None,
                style: Vec::new(),
                classes: Vec::new(),
                interpolate: None,
                is_user_defined_id: true,
                animate: None,
                animation: None,
            })
            .collect::<Vec<_>>();
        let mut subgraphs = Vec::new();
        let mut subgraph_vertex_styles = FlowchartRenderStyleSources::default();
        let mut vertex_calls = Vec::new();
        let mut warning_facts = Vec::new();
        let mut class_defs = IndexMap::new();
        let mut tooltips = HashMap::new();
        let mut edge_defaults = EdgeDefaults {
            style: Vec::new(),
            interpolate: None,
        };
        let config = MermaidConfig::empty_object();
        let shape_data_documents = HashMap::new();
        let control = OperationControl::new();
        control.cancel_after_checkpoints(3);
        let mut context = FlowchartSemanticContext {
            source_occurrences: &mut Vec::new(),
            nodes: &mut nodes,
            node_index: &mut node_index,
            edges: &mut edges,
            subgraphs: &mut subgraphs,
            subgraph_vertex_styles: &mut subgraph_vertex_styles,
            collapsed_subgraphs: &mut rustc_hash::FxHashSet::default(),
            vertex_calls: &mut vertex_calls,
            warning_facts: &mut warning_facts,
            class_defs: &mut class_defs,
            tooltips: &mut tooltips,
            edge_defaults: &mut edge_defaults,
            security_level_loose: false,
            diagram_type: "flowchart-v2",
            config: &config,
            shape_data_documents: &shape_data_documents,
            control: &control,
        };
        let statements = [Stmt::ClassAssign(ClassAssignStmt {
            targets: vec!["missing-edge".to_string()],
            target_spans: Vec::new(),
            class_name: "hot".to_string(),
            class_name_span: None,
            editor_evidence: Default::default(),
        })];

        assert!(matches!(
            apply_semantic_statements(&statements, &mut context),
            Err(crate::OperationCancelled { .. })
        ));
    }
}
