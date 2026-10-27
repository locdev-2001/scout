use crate::driver::DriverPool;
use crate::types::{CallerEdge, Confidence, SymbolRef, Trace, TraceBranch, TraceNode};
use serde_json::{json, Value};
use crate::resources::profile_phase;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

/// Longest a large walk keeps asking: a widely used function walked without a depth limit can take minutes. A
/// small walk that is slow because the drivers are still reading the project is never cut.
const TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
const LARGE_WALK: usize = 500;
/// Symbols asked per driver round within a level.
const LEVEL_PART: usize = 200;

pub struct TraceLimits {
    pub max_depth: usize,
    pub max_nodes: usize,
    pub include_tests: bool,
}

/// Reads driver answers in place (they may be kept and shared): one item or an array of items each.
fn parse_all<T: serde::de::DeserializeOwned>(values: Vec<Arc<Value>>, pool: &mut DriverPool) -> Vec<T> {
    let mut parsed = Vec::new();
    for value in values {
        let items = if value.is_array() { Vec::<T>::deserialize(&*value) } else { T::deserialize(&*value).map(|item| vec![item]) };
        match items {
            Ok(items) => parsed.extend(items),
            Err(error) => {
                pool.warnings.insert(format!("malformed driver result: {error}"));
            }
        }
    }
    parsed
}

/// `file:line` (or `file:line,line,…`) is located through the drivers, each symbol around those lines once;
/// anything else is resolved as a symbol name.
pub fn resolve_target(pool: &mut DriverPool, query: &str, position: Option<(String, Vec<u32>)>) -> Vec<SymbolRef> {
    let mut targets: Vec<SymbolRef> = match position {
        Some((file, lines)) => {
            let requests: Vec<Value> = lines.iter().map(|line| json!({ "file": file, "line": line })).collect();
            let located = pool.ask_many_about_file("locate", &requests, &file);
            located.into_iter().flat_map(|answers| parse_all::<SymbolRef>(answers, pool).into_iter().take(1)).collect()
        }
        None => {
            let resolved: Vec<SymbolRef> = {
                let answers = pool.ask("resolve", json!({ "query": query }));
                parse_all(answers, pool)
            };
            if resolved.is_empty() && (query.contains('#') || query.contains('\\')) {
                // `file#name` or `Namespace\\Class::method` names one symbol: if no driver has it, it does not exist
                // (a typo, or gone).
                Vec::new()
            } else if resolved.is_empty() {
                // No language driver knows this name: ask by the name itself, so the generic driver can still
                // list every call to it.
                vec![SymbolRef { symbol: query.to_owned(), file: None, line: None, kind: Some("unclaimed".to_owned()) }]
            } else {
                resolved
            }
        }
    };
    let mut seen = HashSet::new();
    targets.retain(|target| seen.insert(target.symbol.clone()));
    targets
}

fn edge_key(edge: &CallerEdge) -> String {
    match &edge.at {
        Some(at) => format!("{}@{}:{}", edge.from.symbol, at.file, at.line),
        None => edge.from.symbol.clone(),
    }
}

fn is_expandable(edge: &CallerEdge) -> bool {
    !edge.from.is_kind("entry") && edge.confidence != Confidence::Unresolved
}

/// Walks callers breadth-first, one batched driver round per level, until entries, the depth limit or the node budget.
/// A bridge (a framework hop such as `Job::dispatch`) is no level of its own: `--depth 1` on a job's `handle` still
/// reaches the lines that dispatch it.
pub fn trace_callers(pool: &mut DriverPool, root: SymbolRef, limits: &TraceLimits, hung_up: &dyn Fn() -> bool) -> Trace {
    let mut trace = Trace { nodes: vec![TraceNode { reference: root.clone(), callers: Vec::new(), truncated: false }], hidden_test_callers: 0, timed_out: false };
    let started_walk = Instant::now();
    let mut seen_symbols = HashSet::from([root.symbol]);
    // Nodes to ask about, with their depth.
    let mut frontier = vec![(0usize, 0usize)];

    loop {
        let (asked, beyond): (Vec<_>, Vec<_>) = frontier.into_iter().partition(|&(_, depth)| depth < limits.max_depth);
        for (index, _) in beyond {
            trace.nodes[index].truncated = true;
        }
        frontier = asked;
        if frontier.is_empty() {
            break;
        }
        if hung_up() {
            for &(index, _) in &frontier {
                trace.nodes[index].truncated = true;
            }
            break;
        }
        let mut next_frontier = Vec::new();
        // A level is asked in parts, so a walk that outlives its time budget stops between them: the server is
        // free for the next question, and the answer says where it was cut.
        for part in frontier.chunks(LEVEL_PART) {
            if started_walk.elapsed() > TIME_BUDGET && trace.nodes.len() > LARGE_WALK {
                for &(index, _) in part {
                    trace.nodes[index].truncated = true;
                }
                trace.timed_out = true;
                continue;
            }
            let requests: Vec<Value> = part
                .iter()
                .map(|&(index, _)| json!({ "symbol": trace.nodes[index].reference.symbol, "unclaimed": trace.nodes[index].reference.is_kind("unclaimed") }))
                .collect();
            let started = Instant::now();
            let answers = pool.ask_many("callers", &requests);
            profile_phase(&format!("trace level: {} symbols asked", requests.len()), started);

            for (&(node_index, depth), answer) in part.iter().zip(answers) {
                let mut seen_edges = HashSet::new();
                let mut edges: Vec<CallerEdge> = parse_all(answer, pool);
                // Drivers answer in whatever order their caches hold; the tree must not depend on it.
                edges.sort_by(|left, right| {
                    let position = |edge: &CallerEdge| edge.at.as_ref().map(|at| (at.file.clone(), at.line));
                    (position(left).is_none(), position(left), &left.from.symbol).cmp(&(position(right).is_none(), position(right), &right.from.symbol))
                });
                for edge in edges.into_iter().filter(|edge| seen_edges.insert(edge_key(edge))) {
                    if !limits.include_tests && edge.from.is_kind("test") {
                        trace.hidden_test_callers += 1;
                        continue;
                    }
                    let mut branch = TraceBranch { edge, node: None, repeated: false };
                    if is_expandable(&branch.edge) {
                        if seen_symbols.contains(&branch.edge.from.symbol) {
                            branch.repeated = true;
                        } else if trace.nodes.len() >= limits.max_nodes {
                            trace.nodes[node_index].truncated = true;
                        } else {
                            seen_symbols.insert(branch.edge.from.symbol.clone());
                            trace.nodes.push(TraceNode { reference: branch.edge.from.clone(), callers: Vec::new(), truncated: false });
                            branch.node = Some(trace.nodes.len() - 1);
                            let child_depth = if branch.edge.from.is_kind("bridge") { depth } else { depth + 1 };
                            next_frontier.push((trace.nodes.len() - 1, child_depth));
                        }
                    }
                    trace.nodes[node_index].callers.push(branch);
                }
            }
        }
        frontier = next_frontier;
    }
    prune_dead_bridges(&mut trace, 0);
    trace
}

/// Drops bridge hops that lead nowhere; returns whether the node still has something to show.
fn prune_dead_bridges(trace: &mut Trace, index: usize) -> bool {
    let child_indexes: Vec<usize> = trace.nodes[index].callers.iter().filter_map(|branch| branch.node).collect();
    let mut alive = HashSet::new();
    for child in child_indexes {
        if prune_dead_bridges(trace, child) {
            alive.insert(child);
        }
    }
    let node = &mut trace.nodes[index];
    node.callers.retain(|branch| {
        !branch.edge.from.is_kind("bridge") || branch.repeated || branch.node.is_some_and(|child| alive.contains(&child))
    });
    !node.callers.is_empty() || node.truncated
}
