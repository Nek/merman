//! Phase 2 layering processors.
//!
//! Source references:
//! - https://github.com/eclipse-elk/elk/blob/62d5909f96fad541bc101ad52dabaece6b7eab7e/plugins/org.eclipse.elk.alg.layered/src/org/eclipse/elk/alg/layered/p2layers/NetworkSimplexLayerer.java
//! - https://github.com/eclipse-elk/elk/tree/62d5909f96fad541bc101ad52dabaece6b7eab7e/plugins/org.eclipse.elk.alg.common/src/org/eclipse/elk/alg/common/networksimplex

use std::collections::{HashMap, VecDeque};

use crate::common::networksimplex::{NGraph, NetworkSimplex};
use crate::graph::{LGraph, LayeredEdge};

const ITER_LIMIT_FACTOR: usize = 4;

pub fn layer_network_simplex(graph: &mut LGraph) {
    graph.clear_layers();
    let nodes = graph
        .layerless_nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| (!node.hidden).then_some(index))
        .collect::<Vec<_>>();
    if nodes.is_empty() {
        return;
    }

    let connected_components = connected_components(graph, &nodes);
    let mut previous_layering_node_counts = None;

    for component in connected_components.iter() {
        let iter_limit = graph.options.thoroughness
            * ITER_LIMIT_FACTOR
            * (component.len() as f64).sqrt() as usize;
        let mut ngraph = initialize(graph, component);

        NetworkSimplex::for_graph(&mut ngraph)
            .with_iteration_limit(iter_limit)
            .with_previous_layering(previous_layering_node_counts.clone())
            .with_balancing(true)
            .execute();

        for n_node in ngraph.active_nodes().iter().copied() {
            let Some(l_node) = ngraph.nodes[n_node].origin else {
                continue;
            };
            graph.set_node_layer(l_node, ngraph.nodes[n_node].layer as usize);
        }

        if connected_components.len() > 1 {
            previous_layering_node_counts =
                Some(graph.layers.iter().map(|layer| layer.nodes.len()).collect());
        }
    }
}

// Ports of LongestPathLayerer, LongestPathSourceLayerer and CoffmanGrahamLayerer:
// https://github.com/eclipse-elk/elk/tree/62d5909f96fad541bc101ad52dabaece6b7eab7e/plugins/org.eclipse.elk.alg.layered/src/org/eclipse/elk/alg/layered/p2layers
// Eclipse ELK, Copyright Kiel University and others, EPL-2.0.
use crate::work::{WorkControl, WorkError};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayeringError {
    #[error("ELK layering requires an acyclic graph after cycle breaking")]
    Cycle,
    #[error(transparent)]
    Work(#[from] WorkError),
}

struct Dag {
    outgoing: Vec<Vec<(usize, usize)>>,
    incoming: Vec<Vec<(usize, usize)>>,
    order: Vec<usize>,
}

impl Dag {
    fn new(graph: &LGraph, control: &mut dyn WorkControl) -> Result<Self, LayeringError> {
        let mut outgoing = vec![Vec::new(); graph.layerless_nodes.len()];
        let mut incoming = outgoing.clone();
        let mut degree = vec![0; outgoing.len()];
        let mut count = 0;
        for (node, value) in graph.layerless_nodes.iter().enumerate() {
            control.check(0)?;
            if value.hidden {
                continue;
            }
            count += 1;
            for edge_id in graph.node_outgoing_edges(node) {
                let edge = &graph.edges[edge_id];
                let target = edge.target.node;
                if target == node || graph.layerless_nodes[target].hidden {
                    continue;
                }
                outgoing[node].push((target, edge_id));
                incoming[target].push((node, edge_id));
                degree[target] += 1;
            }
        }
        let mut ready: VecDeque<_> = (0..outgoing.len())
            .filter(|&node| !graph.layerless_nodes[node].hidden && degree[node] == 0)
            .collect();
        let mut order = Vec::with_capacity(count);
        while let Some(node) = ready.pop_front() {
            control.check(0)?;
            order.push(node);
            for &(target, _) in &outgoing[node] {
                degree[target] -= 1;
                if degree[target] == 0 {
                    ready.push_back(target);
                }
            }
        }
        if order.len() != count {
            return Err(LayeringError::Cycle);
        }
        Ok(Self {
            outgoing,
            incoming,
            order,
        })
    }
}

pub fn layer_longest_path(
    graph: &mut LGraph,
    from_source: bool,
    control: &mut dyn WorkControl,
) -> Result<(), LayeringError> {
    let dag = Dag::new(graph, control)?;
    let mut rank = vec![0usize; graph.layerless_nodes.len()];
    for index in 0..dag.order.len() {
        control.check(0)?;
        let node = dag.order[if from_source {
            index
        } else {
            dag.order.len() - 1 - index
        }];
        let adjacent = if from_source {
            &dag.outgoing[node]
        } else {
            &dag.incoming[node]
        };
        for &(next, _) in adjacent {
            rank[next] = rank[next].max(rank[node] + 1);
        }
    }
    let height = rank.iter().copied().max().unwrap_or(0);
    control.check(0)?;
    graph.clear_layers();
    for &node in &dag.order {
        graph.set_node_layer(
            node,
            if from_source {
                rank[node]
            } else {
                height - rank[node]
            },
        );
    }
    Ok(())
}

pub fn layer_coffman_graham(
    graph: &mut LGraph,
    control: &mut dyn WorkControl,
) -> Result<(), LayeringError> {
    let bound = graph.options.coffman_graham_layer_bound.max(1);
    let dag = Dag::new(graph, control)?;
    let n = graph.layerless_nodes.len();
    let mut redundant = vec![false; graph.edges.len()];
    let mut visited = vec![false; n];
    let mut transitive = vec![false; n];
    let mut stack = Vec::new();
    // Mark direct edges reached through a path of at least two edges; retain all graph edges.
    for &start in &dag.order {
        control.check(0)?;
        visited.fill(false);
        transitive.fill(false);
        stack.extend(dag.outgoing[start].iter().map(|&(node, _)| node));
        while let Some(node) = stack.pop() {
            control.check(0)?;
            if visited[node] {
                continue;
            }
            visited[node] = true;
            for &(target, _) in &dag.outgoing[node] {
                transitive[target] = true;
                if !visited[target] {
                    stack.push(target);
                }
            }
        }
        for &(target, edge) in &dag.outgoing[start] {
            redundant[edge] = transitive[target];
        }
    }
    let mut degree = vec![0; n];
    let mut predecessors = vec![Vec::new(); n];
    let mut ready = BinaryHeap::new();
    for &node in &dag.order {
        degree[node] = dag.incoming[node]
            .iter()
            .filter(|&&(_, edge)| !redundant[edge])
            .count();
        if degree[node] == 0 {
            ready.push(Reverse((Vec::<usize>::new(), node)));
        }
    }
    let mut topo = vec![0; n];
    let mut index = 0;
    while let Some(Reverse((_, node))) = ready.pop() {
        control.check(0)?;
        topo[node] = index;
        index += 1;
        for &(target, edge) in &dag.outgoing[node] {
            if redundant[edge] {
                continue;
            }
            degree[target] -= 1;
            predecessors[target].push(topo[node]);
            if degree[target] == 0 {
                // Lexicographic predecessor order, with authored node order as a stable tie-break.
                ready.push(Reverse((
                    predecessors[target].iter().rev().copied().collect(),
                    target,
                )));
            }
        }
    }
    let mut sinks = BinaryHeap::new();
    for &node in &dag.order {
        degree[node] = dag.outgoing[node]
            .iter()
            .filter(|&&(_, edge)| !redundant[edge])
            .count();
        if degree[node] == 0 {
            sinks.push((topo[node], node));
        }
    }
    let mut ranks = vec![None; n];
    let mut layer = 0;
    let mut width = 0;
    let mut assignment = Vec::with_capacity(dag.order.len());
    while let Some((_, node)) = sinks.pop() {
        control.check(0)?;
        if width >= bound
            || dag.outgoing[node]
                .iter()
                .any(|&(target, _)| ranks[target] == Some(layer))
        {
            layer += 1;
            width = 0;
        }
        ranks[node] = Some(layer);
        width += 1;
        assignment.push((node, layer));
        for &(source, edge) in &dag.incoming[node] {
            if redundant[edge] {
                continue;
            }
            degree[source] -= 1;
            if degree[source] == 0 {
                sinks.push((topo[source], source));
            }
        }
    }
    control.check(0)?;
    graph.clear_layers();
    for (node, rank) in assignment {
        graph.set_node_layer(node, layer - rank);
    }
    Ok(())
}

// Source ports of MinWidthLayerer, StretchWidthLayerer and InteractiveLayerer from the
// pinned Eclipse ELK p2layers package cited above (Kiel University and others, EPL-2.0).
fn commit_layers(
    graph: &mut LGraph,
    layers: Vec<Vec<usize>>,
    control: &mut dyn WorkControl,
) -> Result<(), LayeringError> {
    control.check(0)?;
    graph.clear_layers();
    for (rank, nodes) in layers.into_iter().enumerate() {
        for node in nodes {
            graph.set_node_layer(node, rank);
        }
    }
    Ok(())
}

fn width_pass_work(dag: &Dag) -> Result<usize, WorkError> {
    use crate::work::{checked_add, checked_mul, checked_sum};
    // At most two scans per inserted node; each scan visits at most V + E entries.
    let n = dag.order.len();
    checked_mul(
        checked_mul(n.max(1), 4)?,
        checked_add(n, checked_sum(dag.outgoing.iter().map(Vec::len))?)?.max(1),
    )
}

fn normalized_node_sizes(graph: &LGraph, dag: &Dag) -> (Vec<f64>, f64) {
    let minimum = dag
        .order
        .iter()
        .filter(|&&n| graph.layerless_nodes[n].kind == crate::graph::LNodeKind::Normal)
        .map(|&n| graph.layerless_nodes[n].size.height)
        .reduce(f64::min)
        .unwrap_or(1.0)
        .max(1.0);
    // Normalize after guarding zero-size nodes; dummy-only graphs still have real dimensions.
    (
        graph
            .layerless_nodes
            .iter()
            .map(|n| n.size.height / minimum)
            .collect(),
        graph.options.spacing.edge_edge / minimum,
    )
}

pub fn layer_min_width(
    graph: &mut LGraph,
    control: &mut dyn WorkControl,
) -> Result<(), LayeringError> {
    let dag = Dag::new(graph, control)?;
    if dag.order.is_empty() {
        return commit_layers(graph, vec![], control);
    }
    let (sizes, dummy) = normalized_node_sizes(graph, &dag);
    // Pinned elkjs 0.9.3 metadata defaults are 4 and 2, despite the Java class's stale
    // comment saying -1. Mermaid exposes neither tuning parameter, so no search is needed.
    let bound = 4.0 * dag.order.iter().map(|&n| sizes[n]).sum::<f64>() / dag.order.len() as f64;
    let work = width_pass_work(&dag)?;
    control.check(work)?;
    control.charge(work)?;
    let mut remaining = dag.order.clone();
    remaining.sort_unstable();
    remaining.sort_by_key(|&n| Reverse(dag.outgoing[n].len()));
    let mut previous = vec![false; sizes.len()];
    let mut layers = Vec::new();
    let mut layer = Vec::new();
    let (mut current_width, mut upper_width) = (0.0, 0.0);
    while !remaining.is_empty() {
        control.check(0)?;
        let selected = remaining
            .iter()
            .position(|&n| dag.outgoing[n].iter().all(|&(target, _)| previous[target]));
        let mut grow = selected.is_none();
        if let Some(index) = selected {
            let n = remaining.remove(index);
            layer.push(n);
            let outgoing = dag.outgoing[n].len() as f64 * dummy;
            current_width += sizes[n] - outgoing;
            upper_width += dag.incoming[n].len() as f64 * dummy;
            grow = remaining.is_empty()
                || (current_width >= bound && sizes[n] > outgoing)
                || upper_width >= 2.0 * bound;
        }
        if grow {
            if layer.is_empty() {
                return Err(LayeringError::Cycle);
            }
            for &n in &layer {
                previous[n] = true;
            }
            layers.push(std::mem::take(&mut layer));
            current_width = upper_width;
            upper_width = 0.0;
        }
    }
    layers.reverse();
    commit_layers(graph, layers, control)
}

pub fn layer_stretch_width(
    graph: &mut LGraph,
    control: &mut dyn WorkControl,
) -> Result<(), LayeringError> {
    let dag = Dag::new(graph, control)?;
    if dag.order.is_empty() {
        return commit_layers(graph, vec![], control);
    }
    let (sizes, dummy) = normalized_node_sizes(graph, &dag);
    let work = width_pass_work(&dag)?;
    control.check(work)?;
    control.charge(work)?;
    let degree: Vec<_> = dag.outgoing.iter().map(Vec::len).collect();
    let mut nodes = dag.order.clone();
    nodes.sort_unstable();
    nodes.sort_by_key(|&n| {
        Reverse(
            dag.incoming[n]
                .iter()
                .map(|&(source, _)| degree[source])
                .fold(degree[n], usize::max),
        )
    });
    let influence = degree.iter().sum::<usize>() as f64 / nodes.len() as f64;
    let mut max_width = nodes
        .iter()
        .filter(|&&n| graph.layerless_nodes[n].kind == crate::graph::LNodeKind::Normal)
        .map(|&n| sizes[n])
        .fold(1.0, f64::max);
    // Each retry follows the reference's unit increase. Charge it before execution so extreme
    // dimensions cannot turn the reference's reset loop into unbounded host work.
    'retry: loop {
        let mut remaining = nodes.clone();
        let mut out = degree.clone();
        let mut layers = vec![Vec::new()];
        let (mut current_width, mut upper_width) = (0.0, 0.0);
        while !remaining.is_empty() {
            control.check(0)?;
            let selected = remaining.iter().position(|&n| out[n] == 0);
            let grow = selected.is_some_and(|index| {
                let n = remaining[index];
                current_width - degree[n] as f64 * dummy + sizes[n] > max_width
                    || upper_width + dag.incoming[n].len() as f64 * dummy
                        > max_width * influence * dummy
            });
            if selected.is_none() || (grow && !layers.last().unwrap().is_empty()) {
                let layer = layers.last().unwrap();
                if layer.is_empty() {
                    return Err(LayeringError::Cycle);
                }
                for &n in layer {
                    for &(source, _) in &dag.incoming[n] {
                        out[source] -= 1;
                    }
                }
                layers.push(Vec::new());
                current_width = upper_width;
                upper_width = 0.0;
            } else if grow {
                let next = max_width + 1.0;
                if next == max_width || !next.is_finite() {
                    return Err(WorkError::ArithmeticOverflow.into());
                }
                control.check(work)?;
                control.charge(work)?;
                max_width = next;
                continue 'retry;
            } else {
                let n = remaining.remove(selected.unwrap());
                layers.last_mut().unwrap().push(n);
                current_width += sizes[n] - degree[n] as f64 * dummy;
                upper_width += dag.incoming[n].len() as f64 * dummy;
            }
        }
        layers.reverse();
        return commit_layers(graph, layers, control);
    }
}

pub fn layer_interactive(
    graph: &mut LGraph,
    control: &mut dyn WorkControl,
) -> Result<(), LayeringError> {
    let dag = Dag::new(graph, control)?;
    let mut nodes = dag.order.clone();
    nodes.sort_by(|&a, &b| {
        graph.layerless_nodes[a]
            .position
            .x
            .total_cmp(&graph.layerless_nodes[b].position.x)
            .then(a.cmp(&b))
    });
    let mut ranks = vec![0; graph.layerless_nodes.len()];
    let (mut rank, mut end) = (0, f64::NEG_INFINITY);
    for (index, &n) in nodes.iter().enumerate() {
        control.check(0)?;
        let node = &graph.layerless_nodes[n];
        if index > 0 && node.position.x >= end {
            rank += 1;
        }
        ranks[n] = rank;
        end = end.max(node.position.x + node.size.width.max(1.0));
    }
    // The DAG's topological order computes the same least rightward shifts as the reference's
    // repeated relaxation, without revisiting a long chain on every predecessor change.
    for &n in &dag.order {
        control.check(0)?;
        for &(target, _) in &dag.outgoing[n] {
            ranks[target] = ranks[target].max(ranks[n] + 1);
        }
    }
    let mut occupied: Vec<_> = nodes.iter().map(|&n| ranks[n]).collect();
    occupied.sort_unstable();
    occupied.dedup();
    let mut layers = vec![Vec::new(); occupied.len()];
    nodes.sort_unstable();
    for n in nodes {
        layers[occupied.binary_search(&ranks[n]).unwrap()].push(n);
    }
    commit_layers(graph, layers, control)
}

fn connected_components(graph: &LGraph, nodes: &[usize]) -> Vec<Vec<usize>> {
    let mut visited = vec![false; graph.layerless_nodes.len()];
    let mut components: VecDeque<Vec<usize>> = VecDeque::new();

    for node in nodes.iter().copied() {
        if visited[node] {
            continue;
        }

        let mut component = Vec::new();
        connected_components_dfs(graph, node, &mut visited, &mut component);
        if components
            .front()
            .map(|front| front.len() < component.len())
            .unwrap_or(true)
        {
            components.push_front(component);
        } else {
            components.push_back(component);
        }
    }

    components.into_iter().collect()
}

fn connected_components_dfs(
    graph: &LGraph,
    node: usize,
    visited: &mut [bool],
    component: &mut Vec<usize>,
) {
    visited[node] = true;
    component.push(node);

    for edge_index in graph.node_connected_edges(node) {
        let Some(opposite) = opposite_node(graph, node, edge_index) else {
            continue;
        };
        if !visited[opposite] {
            connected_components_dfs(graph, opposite, visited, component);
        }
    }
}

fn initialize(graph: &LGraph, nodes: &[usize]) -> NGraph {
    let mut ngraph = NGraph::new();
    let mut node_map = HashMap::new();

    for l_node in nodes {
        let n_node = ngraph.add_node(Some(*l_node));
        node_map.insert(*l_node, n_node);
    }

    for l_node in nodes {
        for edge_index in graph.node_outgoing_edges(*l_node) {
            let edge = &graph.edges[edge_index];
            if edge.source.node == edge.target.node {
                continue;
            }

            let Some(source) = node_map.get(&edge.source.node).copied() else {
                continue;
            };
            let Some(target) = node_map.get(&edge.target.node).copied() else {
                continue;
            };
            ngraph.add_edge(
                Some(edge_index),
                source,
                target,
                priority_shortness_weight(edge),
                1,
            );
        }
    }

    ngraph
}

fn opposite_node(graph: &LGraph, node: usize, edge_index: usize) -> Option<usize> {
    let edge = graph.edges.get(edge_index)?;
    if edge.source.node == node {
        Some(edge.target.node)
    } else if edge.target.node == node {
        Some(edge.source.node)
    } else {
        None
    }
}

fn priority_shortness_weight(edge: &LayeredEdge) -> f64 {
    edge.priority_shortness.max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importer::{ElkInputEdge, ElkInputGraph, ElkInputNode, import_graph};
    use crate::options::{ElkDirection, LayeredOptions};
    use crate::p1cycles::break_cycles_greedy;

    fn node(id: &str) -> ElkInputNode {
        ElkInputNode {
            id: id.to_string(),
            width: 80.0,
            height: 40.0,
            parent: None,
            direction: None,
            hierarchy_handling: None,
            layer_constraint: None,
            port_constraints: None,
            node_label_placement: crate::options::NodeLabelPlacement::Fixed,
            nested_spacing_base: None,
            label: None,
        }
    }

    fn edge(id: &str, source: &str, target: &str) -> ElkInputEdge {
        ElkInputEdge {
            id: id.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            label: None,
            minlen: 1,
            inside_self_loops_yo: false,
            model_order: None,
            priority_direction: 0,
            priority_shortness: 0,
            priority_straightness: 0,
        }
    }

    fn graph(nodes: Vec<ElkInputNode>, edges: Vec<ElkInputEdge>) -> LGraph {
        import_graph(&ElkInputGraph {
            id: "root".to_string(),
            options: LayeredOptions::mermaid_flowchart_defaults(ElkDirection::Down),
            nodes,
            edges,
        })
        .unwrap()
    }

    fn assert_width_strategy(strategy: crate::options::LayeringStrategy, expected: [usize; 6]) {
        let nodes = ["A", "B", "C", "D", "E", "F"].map(|id| {
            let mut n = node(id);
            n.width = 40.0;
            n
        });
        let mut graph = graph(
            nodes.to_vec(),
            vec![
                edge("ab", "A", "B"),
                edge("bc", "B", "C"),
                edge("cd", "C", "D"),
                edge("ae", "A", "E"),
                edge("ed", "E", "D"),
                edge("af", "A", "F"),
            ],
        );
        graph.options.layering_strategy = strategy;
        crate::pipeline::execute_processors_until(
            &mut graph,
            crate::pipeline::LayeredPhase::P2Layering,
        )
        .unwrap();
        let actual: Vec<_> = graph
            .layerless_nodes
            .iter()
            .map(|n| n.layer_index.unwrap())
            .collect();
        assert_eq!(
            actual, expected,
            "pinned elkjs 0.9.3 equal-size branch witness"
        );
    }

    #[test]
    fn interactive_layering_merges_overlapping_spans_then_shifts_only_required_nodes() {
        let mut actual = graph(
            ["A", "B", "C", "D", "E"].map(node).to_vec(),
            vec![edge("ca", "C", "A")],
        );
        for (n, (x, width)) in actual.layerless_nodes.iter_mut().zip([
            (0.0, 30.0),
            (10.0, 10.0),
            (30.0, 10.0),
            (40.0, 0.0),
            (35.0, 15.0),
        ]) {
            n.position.x = x;
            n.size.width = width;
        }
        layer_interactive(&mut actual, &mut crate::work::NoopWorkControl).unwrap();
        assert_eq!(
            actual
                .layerless_nodes
                .iter()
                .map(|n| n.layer_index.unwrap())
                .collect::<Vec<_>>(),
            [2, 0, 1, 1, 1]
        );
    }

    #[test]
    fn width_layerers_accept_zero_size_and_dummy_only_graphs() {
        for dummy in [false, true] {
            for (first, second) in [(0.0, 0.0), (0.0, 40.0), (40.0, 0.0)] {
                for choice in 4..7 {
                    let mut actual = graph(
                        vec![node("A"), node("B")],
                        vec![edge("ab", "A", "B"), edge("loop", "B", "B")],
                    );
                    for (n, height) in actual.layerless_nodes.iter_mut().zip([first, second]) {
                        n.size.width = 0.0;
                        n.size.height = height;
                        if dummy {
                            n.kind = crate::graph::LNodeKind::ExternalPort;
                        }
                    }
                    run_layerer(&mut actual, choice).unwrap();
                    assert_layer_order(&actual, "A", "B");
                    assert_eq!(actual.layers.len(), 2);
                }
            }
        }
    }

    #[test]
    fn minimum_width_layering_preserves_its_sink_branch_choice() {
        assert_width_strategy(
            crate::options::LayeringStrategy::MinWidth,
            [0, 1, 2, 3, 2, 3],
        );
    }

    #[test]
    fn stretch_width_layering_preserves_its_narrow_branch_choice() {
        assert_width_strategy(
            crate::options::LayeringStrategy::StretchWidth,
            [0, 1, 2, 3, 2, 4],
        );
    }

    #[test]
    fn interactive_layering_preserves_source_alignment_without_initial_positions() {
        assert_width_strategy(
            crate::options::LayeringStrategy::Interactive,
            [0, 1, 2, 3, 1, 1],
        );
    }

    fn run_layerer(graph: &mut LGraph, choice: usize) -> Result<(), LayeringError> {
        let mut control = crate::work::NoopWorkControl;
        match choice {
            0 => layer_longest_path(graph, false, &mut control),
            1 => layer_longest_path(graph, true, &mut control),
            2 | 3 => {
                graph.options.coffman_graham_layer_bound = choice - 1;
                layer_coffman_graham(graph, &mut control)
            }
            4 => layer_min_width(graph, &mut control),
            5 => layer_stretch_width(graph, &mut control),
            6 => layer_interactive(graph, &mut control),
            _ => unreachable!(),
        }
    }

    #[test]
    fn alternate_layerers_keep_every_small_dag_forward_and_bounded() {
        // All 1,024 five-node DAGs include disconnected, transitive and empty cases.
        let pairs: Vec<_> = (0..5)
            .flat_map(|a| (a + 1..5).map(move |b| (a, b)))
            .collect();
        for mask in 0..1 << pairs.len() {
            let nodes: Vec<_> = (0..5).map(|i| node(&i.to_string())).collect();
            let mut edges: Vec<_> = pairs
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(i, (a, b))| edge(&i.to_string(), &a.to_string(), &b.to_string()))
                .collect();
            edges.push(edge("loop", "0", "0"));
            if mask & 1 != 0 {
                edges.push(edge("parallel", "0", "1"));
            }
            let original = graph(nodes, edges);
            for choice in 0..7 {
                let mut actual = original.clone();
                run_layerer(&mut actual, choice).unwrap();
                assert_eq!(
                    actual.edges, original.edges,
                    "layering must retain original edges"
                );
                assert_eq!(
                    actual.layers.iter().map(|l| l.nodes.len()).sum::<usize>(),
                    5
                );
                for e in &actual.edges {
                    if e.source.node != e.target.node {
                        assert_layer_order(&actual, &e.source_node_id, &e.target_node_id);
                    }
                }
                if matches!(choice, 2 | 3) {
                    assert!(actual.layers.iter().all(|l| l.nodes.len() <= choice - 1));
                }
            }
        }
    }

    #[test]
    fn alternate_layerers_reject_cycles_without_mutation_and_accept_cycle_breaking() {
        let original = graph(
            vec![node("A"), node("B"), node("C")],
            vec![
                edge("ab", "A", "B"),
                edge("bc", "B", "C"),
                edge("ca", "C", "A"),
            ],
        );
        for choice in 0..7 {
            let mut actual = original.clone();
            actual.options.coffman_graham_layer_bound = if matches!(choice, 2 | 3) {
                choice - 1
            } else {
                i32::MAX as usize
            };
            let before = actual.clone();
            assert_eq!(run_layerer(&mut actual, choice), Err(LayeringError::Cycle));
            assert_eq!(actual, before);
            break_cycles_greedy(&mut actual);
            run_layerer(&mut actual, choice).unwrap();
            assert_eq!(actual.layers.len(), 3);
        }
    }

    #[test]
    fn longest_path_layerers_handle_long_chains_without_recursion() {
        std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(|| {
                let count = 4096;
                let original = graph(
                    (0..count).map(|i| node(&i.to_string())).collect(),
                    (1..count)
                        .map(|i| edge(&i.to_string(), &(i - 1).to_string(), &i.to_string()))
                        .collect(),
                );
                for choice in 0..2 {
                    let mut actual = original.clone();
                    run_layerer(&mut actual, choice).unwrap();
                    assert_eq!(actual.layers.len(), count);
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn network_simplex_layerer_assigns_forward_layers() {
        let mut graph = graph(
            vec![node("A"), node("B"), node("C")],
            vec![edge("A-B", "A", "B"), edge("B-C", "B", "C")],
        );

        layer_network_simplex(&mut graph);

        assert_eq!(graph.layers.len(), 3);
        assert_layer_order(&graph, "A", "B");
        assert_layer_order(&graph, "B", "C");
    }

    #[test]
    fn network_simplex_layerer_ignores_self_loops() {
        let mut graph = graph(
            vec![node("A"), node("B")],
            vec![edge("A-A", "A", "A"), edge("A-B", "A", "B")],
        );

        layer_network_simplex(&mut graph);

        assert_layer_order(&graph, "A", "B");
    }

    #[test]
    fn network_simplex_layerer_processes_connected_components() {
        let mut graph = graph(
            vec![node("A"), node("B"), node("C"), node("D")],
            vec![edge("A-B", "A", "B"), edge("C-D", "C", "D")],
        );

        layer_network_simplex(&mut graph);

        assert_layer_order(&graph, "A", "B");
        assert_layer_order(&graph, "C", "D");
        assert_eq!(graph.layers[0].nodes.len(), 2);
    }

    #[test]
    fn greedy_cycle_breaker_output_can_be_layered_by_network_simplex() {
        let mut graph = graph(
            vec![node("A"), node("B"), node("C")],
            vec![
                edge("A-B", "A", "B"),
                edge("B-C", "B", "C"),
                edge("C-A", "C", "A"),
            ],
        );

        break_cycles_greedy(&mut graph);
        layer_network_simplex(&mut graph);

        for edge in &graph.edges {
            if edge.source.node == edge.target.node {
                continue;
            }
            let source_layer = graph.layerless_nodes[edge.source.node].layer_index.unwrap();
            let target_layer = graph.layerless_nodes[edge.target.node].layer_index.unwrap();
            assert!(
                target_layer > source_layer,
                "edge {} should point from an earlier layer to a later layer",
                edge.id
            );
        }
    }

    fn assert_layer_order(graph: &LGraph, source: &str, target: &str) {
        let source = graph
            .layerless_nodes
            .iter()
            .find(|node| node.id == source)
            .unwrap();
        let target = graph
            .layerless_nodes
            .iter()
            .find(|node| node.id == target)
            .unwrap();
        assert!(target.layer_index.unwrap() > source.layer_index.unwrap());
    }
}
