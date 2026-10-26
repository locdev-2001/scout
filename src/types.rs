use serde::Deserialize;

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Exact,
    Likely,
    Unresolved,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SourcePosition {
    pub file: String,
    pub line: u32,
}

/// `kind == "entry"` marks a symbol the outside world calls (route, command, string event): the walk stops there.
/// `kind == "bridge"` marks a framework hop that is only worth showing when something reaches it.
#[derive(Deserialize, Clone, Debug)]
pub struct SymbolRef {
    pub symbol: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub kind: Option<String>,
}

impl SymbolRef {
    pub fn is_kind(&self, kind: &str) -> bool {
        self.kind.as_deref() == Some(kind)
    }

    pub fn position(&self) -> Option<SourcePosition> {
        Some(SourcePosition { file: self.file.clone()?, line: self.line? })
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct CallerEdge {
    pub from: SymbolRef,
    #[serde(default)]
    pub at: Option<SourcePosition>,
    pub kind: String,
    pub confidence: Confidence,
}

pub struct TraceBranch {
    pub edge: CallerEdge,
    pub node: Option<usize>,
    pub repeated: bool,
}

pub struct TraceNode {
    pub reference: SymbolRef,
    pub callers: Vec<TraceBranch>,
    pub truncated: bool,
}

/// Caller tree stored as an arena; node 0 is the queried symbol.
pub struct Trace {
    pub nodes: Vec<TraceNode>,
    pub hidden_test_callers: usize,
    /// The walk stopped at its time budget with symbols still to ask about.
    pub timed_out: bool,
}

impl Trace {
    /// Lowercased extensions of every file the answer names.
    pub fn extensions(&self) -> impl Iterator<Item = String> + '_ {
        let files = self.nodes.iter().flat_map(|node| node.reference.file.iter().chain(node.callers.iter().filter_map(|branch| branch.edge.at.as_ref().map(|at| &at.file))));
        files.filter_map(|file| std::path::Path::new(file).extension().map(|extension| extension.to_string_lossy().to_lowercase()))
    }
}
