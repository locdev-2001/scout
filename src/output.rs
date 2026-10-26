use crate::types::{CallerEdge, Confidence, SourcePosition, SymbolRef, Trace};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;

const SNIPPET_MAX_CHARS: usize = 120;
const UNRESOLVED_LISTED_INLINE: usize = 12;

/// Reads source lines once per file and stamps each position with a short hash of its content,
/// so a reader can tell whether the line still says what it said when it was reported.
pub struct SourceLines {
    repo_root: PathBuf,
    files: HashMap<String, Vec<String>>,
}

fn line_hash(text: &str) -> String {
    let mut hash: u32 = 0x811c9dc5;
    for byte in text.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    format!("{:04x}", (hash ^ (hash >> 16)) & 0xffff)
}

impl SourceLines {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root, files: HashMap::new() }
    }

    fn line_text(&mut self, position: &SourcePosition) -> &str {
        let repo_root = &self.repo_root;
        let lines = self.files.entry(position.file.clone()).or_insert_with(|| {
            fs::read_to_string(repo_root.join(&position.file)).map(|content| content.lines().map(str::to_owned).collect()).unwrap_or_default()
        });
        lines.get(position.line as usize - 1).map(|line| line.trim()).unwrap_or("")
    }

    pub fn anchor(&mut self, position: &SourcePosition) -> String {
        format!("{}:{}#{}", position.file, position.line, line_hash(self.line_text(position)))
    }

    fn snippet(&mut self, position: &SourcePosition) -> String {
        let text = self.line_text(position);
        if text.chars().count() > SNIPPET_MAX_CHARS {
            format!("{}…", text.chars().take(SNIPPET_MAX_CHARS).collect::<String>())
        } else {
            text.to_owned()
        }
    }
}

pub fn describe_ref(reference: &SymbolRef, source: &mut SourceLines) -> String {
    match reference.position() {
        Some(position) => format!("{}  {}", reference.symbol, source.anchor(&position)),
        None => reference.symbol.clone(),
    }
}

fn confidence_label(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Exact => "exact",
        Confidence::Likely => "likely",
        Confidence::Unresolved => "unresolved",
    }
}

fn edge_label(edge: &CallerEdge) -> String {
    match edge.confidence {
        Confidence::Exact => edge.kind.clone(),
        other => format!("{}, {}", edge.kind, confidence_label(other)),
    }
}

fn describe_edge(edge: &CallerEdge, source: &mut SourceLines) -> String {
    let label = edge_label(edge);
    match &edge.at {
        Some(at) => format!("{}  [{label}]  {}  {}", edge.from.symbol, source.anchor(at), source.snippet(at)),
        None => format!("{}  [{label}]", edge.from.symbol),
    }
}

pub fn render_tree(trace: &Trace, source: &mut SourceLines) -> String {
    let mut lines = vec![describe_ref(&trace.nodes[0].reference, source)];
    render_children(trace, 0, "", source, &mut lines);
    lines.join("\n")
}

fn render_children(trace: &Trace, index: usize, prefix: &str, source: &mut SourceLines, lines: &mut Vec<String>) {
    let node = &trace.nodes[index];
    for (position, branch) in node.callers.iter().enumerate() {
        let last = position + 1 == node.callers.len() && !node.truncated;
        let repeated = if branch.repeated { "  (shown above)" } else { "" };
        lines.push(format!("{prefix}{}{}{repeated}", if last { "└─ " } else { "├─ " }, describe_edge(&branch.edge, source)));
        if let Some(child) = branch.node {
            render_children(trace, child, &format!("{prefix}{}", if last { "   " } else { "│  " }), source, lines);
        }
    }
    if node.truncated {
        lines.push(format!("{prefix}└─ … (stopped by --depth/--max-nodes)"));
    }
}

/// The same trace in far fewer words, for agents: the direct callers with their code, then every other affected
/// symbol once, grouped by file, then every unresolved line grouped by file. Nothing in the trace is left out;
/// how a deeper symbol connects is one more `callers` query away.
pub fn render_grouped(trace: &Trace, source: &mut SourceLines, list_every_unresolved: bool) -> String {
    let mut lines = vec![describe_ref(&trace.nodes[0].reference, source)];

    let mut direct: Vec<(&SymbolRef, BTreeSet<String>, Vec<SourcePosition>)> = Vec::new();
    for branch in &trace.nodes[0].callers {
        let edge = &branch.edge;
        let index = match direct.iter().position(|(from, _, _)| from.symbol == edge.from.symbol) {
            Some(index) => index,
            None => {
                direct.push((&edge.from, BTreeSet::new(), Vec::new()));
                direct.len() - 1
            }
        };
        direct[index].1.insert(edge_label(edge));
        direct[index].2.extend(edge.at.clone());
    }
    lines.push(format!("\nDirect callers ({}):", direct.len()));
    for (from, labels, positions) in direct {
        let labels = labels.into_iter().collect::<Vec<_>>().join("; ");
        let Some(first) = positions.first() else {
            lines.push(format!("- {}  [{labels}]", from.symbol));
            continue;
        };
        let more: Vec<String> = positions[1..].iter().map(|position| format!("{}#{}", position.line, line_hash(source.line_text(position)))).collect();
        let more = if more.is_empty() { String::new() } else { format!(",{}", more.join(",")) };
        lines.push(format!("- {}  [{labels}]  {}{more}  {}", from.symbol, source.anchor(first), source.snippet(first)));
    }

    // Each further symbol once, by file, with the line it starts on: enough to open it or ask about it. A bridge
    // (`Job::dispatch`) is the framework's, not code to open: the lines that reach it are listed instead.
    let mut affected: BTreeMap<String, BTreeSet<(u32, String)>> = BTreeMap::new();
    let further = trace.nodes.iter().skip(1).filter(|node| !node.reference.is_kind("bridge") && !trace.nodes[0].callers.iter().any(|branch| branch.edge.from.symbol == node.reference.symbol));
    for node in further {
        let file = node.reference.file.clone().unwrap_or_default();
        let name = node.reference.symbol.strip_prefix(&format!("{file}#")).unwrap_or(&node.reference.symbol);
        let name = if name == file {
            "(file)".to_owned()
        } else if name.strip_prefix(&format!("{file}:")).is_some_and(|rest| rest.parse::<u32>().is_ok()) {
            "(closure)".to_owned()
        } else {
            name.to_owned()
        };
        affected.entry(file).or_default().insert((node.reference.line.unwrap_or(0), name));
    }
    if !affected.is_empty() {
        lines.push(format!("\nFurther up, {} symbols in {} files:", affected.values().map(BTreeSet::len).sum::<usize>(), affected.len()));
        for (file, names) in affected {
            let names: Vec<String> = names.into_iter().map(|(line, name)| if line > 0 { format!("{name}:{line}") } else { name }).collect();
            let file = if file.is_empty() { "(no file)".to_owned() } else { file };
            lines.push(format!("- {file}: {}", names.join(", ")));
        }
    }

    // Grouped by the symbol they may call: a common member name can match thousands of lines, which are
    // counted here and listed by asking for that symbol alone. The asked symbol's own are under Direct callers.
    let mut unresolved_total = 0;
    let mut unresolved_lines = Vec::new();
    for node in trace.nodes.iter().skip(1) {
        let mut by_file: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
        for branch in node.callers.iter().filter(|branch| branch.edge.confidence == Confidence::Unresolved) {
            if let Some(at) = &branch.edge.at {
                by_file.entry(&at.file).or_default().insert(at.line);
            }
        }
        let count: usize = by_file.values().map(BTreeSet::len).sum();
        if count == 0 {
            continue;
        }
        unresolved_total += count;
        let symbol = &node.reference.symbol;
        if count <= UNRESOLVED_LISTED_INLINE || list_every_unresolved {
            let places: Vec<String> = by_file.iter().map(|(file, file_lines)| format!("{file}:{}", file_lines.iter().map(u32::to_string).collect::<Vec<_>>().join(","))).collect();
            unresolved_lines.push(format!("- {symbol}: {}", places.join("  ")));
        } else {
            unresolved_lines.push(format!("- {symbol}: {count} lines in {} files, listed by callers of '{symbol}' with depth 1", by_file.len()));
        }
    }
    if unresolved_total > 0 {
        lines.push(format!("\nUnresolved, read by hand ({unresolved_total} lines), by the symbol they may call:"));
        lines.extend(unresolved_lines);
    }
    if trace.nodes.iter().any(|node| node.truncated) {
        lines.push("\n… (stopped by --depth/--max-nodes)".to_owned());
    }
    lines.join("\n")
}

pub fn render_summary(trace: &Trace) -> String {
    let mut counts = [0usize; 3];
    let mut entries = BTreeSet::new();
    let mut truncated = false;
    for node in &trace.nodes {
        truncated |= node.truncated;
        for branch in node.callers.iter().filter(|branch| !branch.repeated) {
            counts[branch.edge.confidence as usize] += 1;
            if branch.edge.from.is_kind("entry") {
                entries.insert(branch.edge.from.symbol.clone());
            }
            let unreached = branch.node.is_some_and(|child| trace.nodes[child].callers.is_empty() && !trace.nodes[child].truncated);
            let whole_file = branch.edge.from.file.as_deref() == Some(branch.edge.from.symbol.as_str());
            if branch.edge.from.is_kind("file") && whole_file && unreached {
                entries.insert(format!("{} (top-level code: runs when the file is loaded)", branch.edge.from.symbol));
            }
        }
    }
    let mut lines = vec![format!(
        "Total: {} exact, {} likely, {} unresolved (receiver type unknown, read by hand).",
        counts[0], counts[1], counts[2]
    )];
    if !entries.is_empty() {
        lines.push(format!("Affected entry points ({}):", entries.len()));
        lines.extend(entries.into_iter().map(|entry| format!("- {entry}")));
    }
    if trace.hidden_test_callers > 0 {
        lines.push(format!("Hidden: {} callers inside tests (--tests to show).", trace.hidden_test_callers));
    }
    if trace.timed_out {
        lines.push("Stopped at the 10 s budget with callers still to walk: ask about one of the callers above to go further.".to_owned());
    } else if truncated {
        lines.push("Some branches were cut: raise --depth or --max-nodes to go further.".to_owned());
    }
    lines.join("\n")
}
