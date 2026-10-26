use crate::driver::{Deferred, DriverPool};
use crate::freshness::Freshness;
use crate::output::{describe_ref, render_grouped, render_summary, render_tree, SourceLines};
use crate::resources::{profile_phase, QueryCost};
use crate::trace::{resolve_target, trace_callers, TraceLimits};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Serialize, Deserialize)]
pub struct QueryRequest {
    pub command: String,
    pub query: String,
    pub cwd: PathBuf,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub include_tests: bool,
    /// Leave out the calling line's code (smaller output for agents; position and hash stay).
    #[serde(default)]
    pub compact: bool,
}

#[derive(Serialize, Deserialize, Default)]
pub struct QueryResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
    /// How long the server took to answer, so the client can tell its own wait apart.
    #[serde(default)]
    pub answered_ms: u64,
}

/// `file:line` → (path relative to the repo root, line); anything else is a symbol query.
fn parse_position(query: &str, repo_root: &Path, cwd: &Path) -> Option<(String, u32)> {
    let (file, line) = query.rsplit_once(':')?;
    let line = line.parse().ok()?;
    let candidates = [cwd.join(file), repo_root.join(file)];
    let absolute = candidates.iter().find(|path| path.is_file())?.canonicalize().ok()?;
    let relative = absolute.strip_prefix(repo_root).ok()?;
    Some((relative.to_string_lossy().into_owned(), line))
}

/// Tells every driver which files changed since the previous query. The answers (extra inputs to watch,
/// warnings) are read at the end of the query: a driver handles the change before any later request, so
/// waiting here would only stop drivers with nothing to recompute from working in the meantime.
fn start_synchronize(pool: &mut DriverPool, freshness: &mut Freshness) -> Deferred {
    let started = Instant::now();
    let changed = freshness.sweep();
    profile_phase("sweep", started);
    pool.send_deferred("sync", json!({ "changed": changed }))
}

/// Brings the drivers up to date right away (called when the file watcher reports a change), so the next
/// query finds the work already done.
pub fn refresh(pool: &mut DriverPool, freshness: &mut Freshness) {
    let started = Instant::now();
    let synchronizing = start_synchronize(pool, freshness);
    finish_synchronize(pool, freshness, synchronizing);
    profile_phase("refresh on file change", started);
}

fn finish_synchronize(pool: &mut DriverPool, freshness: &mut Freshness, deferred: Deferred) {
    for (extensions, answer) in pool.collect_deferred(deferred) {
        if let Some(paths) = answer.get("watch").and_then(|watch| watch.as_array()) {
            freshness.watch(paths.iter().filter_map(|path| path.as_str().map(str::to_owned)));
        }
        let strings = |field: &str| answer.get(field).and_then(|warnings| warnings.as_array()).into_iter().flatten().filter_map(|warning| warning.as_str().map(str::to_owned)).collect::<Vec<_>>();
        pool.test_warnings.extend(strings("testWarnings"));
        for warning in strings("warnings") {
            if extensions.is_empty() {
                pool.warnings.insert(warning);
            } else {
                pool.language_warnings.insert(warning, extensions.clone());
            }
        }
    }
}

/// `hung_up` tells whether the asker left: a long walk then stops, so later questions do not wait behind it.
/// A name matching up to this many symbols is answered for each of them.
const SEVERAL: usize = 5;

pub fn execute(pool: &mut DriverPool, freshness: &mut Freshness, repo_root: &Path, request: &QueryRequest, hung_up: &dyn Fn() -> bool) -> QueryResponse {
    let cost = QueryCost::start(pool.process_ids());
    let mut response = QueryResponse::default();
    let synchronizing = start_synchronize(pool, freshness);
    let mut source = SourceLines::new(repo_root.to_path_buf());
    let position = parse_position(&request.query, repo_root, &request.cwd);
    let started = Instant::now();
    let targets = resolve_target(pool, &request.query, position);
    profile_phase("resolve", started);

    let out = &mut response.stdout;
    let mut not_found = false;
    let mut answered_extensions = std::collections::BTreeSet::new();
    match targets.as_slice() {
        [] => {
            let _ = writeln!(response.stderr, "Not found: \"{}\".", request.query);
        }
        [target] if request.command == "locate" => {
            let _ = writeln!(out, "{}", describe_ref(target, &mut source));
        }
        [target] => {
            let limits = TraceLimits { max_depth: request.max_depth, max_nodes: request.max_nodes, include_tests: request.include_tests };
            let started = Instant::now();
            let trace = trace_callers(pool, target.clone(), &limits, hung_up);
            answered_extensions.extend(trace.extensions());
            profile_phase("trace", started);
            // A name no driver knows and no line calls: a typo or something gone, not a function nobody calls.
            if target.is_kind("unclaimed") && trace.nodes[0].callers.is_empty() {
                let _ = writeln!(response.stderr, "Not found: \"{}\" is no symbol here, and no line calls it.", request.query);
                not_found = true;
            }
            let started = Instant::now();
            // Asked for the direct callers only, the unresolved lines are what is wanted: all of them, in place.
            let body = if request.compact { render_grouped(&trace, &mut source, request.max_depth == 1) } else { render_tree(&trace, &mut source) };
            if !not_found {
                let _ = writeln!(out, "{body}\n\n{}", render_summary(&trace));
            }
            profile_phase("render", started);
        }
        // A bare name defined a few times (`save` in two classes): the callers of each, as a text search
        // would show every use; more than that, the list to choose from.
        many if request.command == "callers" && many.len() <= SEVERAL => {
            let _ = writeln!(out, "\"{}\" names {} symbols; callers of each:", request.query, many.len());
            for target in many {
                let limits = TraceLimits { max_depth: request.max_depth, max_nodes: request.max_nodes, include_tests: request.include_tests };
                let trace = trace_callers(pool, target.clone(), &limits, hung_up);
                answered_extensions.extend(trace.extensions());
                let body = if request.compact { render_grouped(&trace, &mut source, request.max_depth == 1) } else { render_tree(&trace, &mut source) };
                let _ = writeln!(out, "\n{body}\n{}", render_summary(&trace));
            }
        }
        many => {
            let _ = writeln!(out, "\"{}\" matches several symbols, be more specific:", request.query);
            for target in many {
                let _ = writeln!(out, "- {}", describe_ref(target, &mut source));
            }
        }
    }
    finish_synchronize(pool, freshness, synchronizing);
    // Warnings say where callers may be unknown: that matters to a `callers` answer, not to `locate`.
    let mut warnings = std::mem::take(&mut pool.warnings);
    warnings.extend(pool.unavailable.iter().cloned());
    for (warning, extensions) in std::mem::take(&mut pool.language_warnings) {
        if extensions.iter().any(|extension| answered_extensions.contains(&extension.to_lowercase())) {
            warnings.insert(warning);
        }
    }
    let test_warnings = std::mem::take(&mut pool.test_warnings);
    if request.include_tests {
        warnings.extend(test_warnings);
    }
    if request.command != "locate" {
        for warning in warnings {
            let _ = writeln!(response.stderr, "Warning: {warning}");
        }
    }
    let _ = writeln!(response.stderr, "{}", cost.finish(pool.process_ids()));
    let answered = targets.len() == 1 || (request.command == "callers" && (2..=SEVERAL).contains(&targets.len()));
    response.exit_code = if answered && !not_found { 0 } else { 1 };
    response.answered_ms = cost.elapsed_ms();
    response
}
