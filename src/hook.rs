//! Claude Code hooks: what an agent should know about callers, given to it at the moment it matters, so it does
//! not depend on the agent choosing this tool over a text search.
//!
//! `after-edit` (PostToolUse on Edit/MultiEdit): the direct callers of each function just edited.
//! `before-search` (PreToolUse on Grep/Bash): a text search for a function name also gets that function's callers.
//!
//! Hooks never block or slow the agent: the search still runs, a server not ready in time adds nothing, and any
//! failure is silent.

use crate::daemon;
use crate::query::QueryRequest;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// How long a hook waits for answers; past it the agent goes on without them (the server keeps warming up).
const BUDGET: Duration = Duration::from_secs(3);
/// Before a text search the agent is waiting: only a quick answer is worth adding.
const SEARCH_BUDGET: Duration = Duration::from_millis(1500);
/// Words a search may start with that name no function.
const KEYWORDS: [&str; 16] = ["function", "class", "def", "return", "public", "private", "protected", "static", "const", "import", "export", "async", "await", "interface", "trait", "struct"];
/// Lines of answer one hook adds at most: enough for the direct callers, small enough for the context.
const MAX_LINES: usize = 40;
/// Edited places looked up at most per edit.
const MAX_PLACES: usize = 3;

pub fn run(event: &str) -> u8 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let Ok(input) = serde_json::from_str::<Value>(&input) else { return 0 };
    let context = match event {
        "after-edit" => after_edit(&input),
        "before-search" => before_search(&input),
        _ => None,
    };
    if let Some(context) = context {
        let event_name = if event == "after-edit" { "PostToolUse" } else { "PreToolUse" };
        println!("{}", json!({ "hookSpecificOutput": { "hookEventName": event_name, "additionalContext": context } }));
    }
    0
}

fn after_edit(input: &Value) -> Option<String> {
    let tool_input = input.get("tool_input")?;
    let file = PathBuf::from(tool_input.get("file_path")?.as_str()?);
    let replacements: Vec<&str> = match tool_input.get("edits").and_then(Value::as_array) {
        Some(edits) => edits.iter().filter_map(|edit| edit.get("new_string").and_then(Value::as_str)).collect(),
        None => tool_input.get("new_string").and_then(Value::as_str).into_iter().collect(),
    };
    let content = std::fs::read_to_string(&file).ok()?;
    let mut lines: Vec<usize> = replacements
        .iter()
        // Where the new text sits, when it sits in one place only: a text found twice could be either.
        .filter(|text| !text.trim().is_empty() && content.matches(*text).count() == 1)
        .filter_map(|text| content.find(*text).map(|offset| content[..offset].matches('\n').count() + 1))
        .map(|line| declared_below(&content, line))
        .collect();
    lines.sort_unstable();
    lines.dedup();
    let repo = repo_root(file.parent()?)?;
    let relative = file.strip_prefix(&repo).ok()?.to_string_lossy().into_owned();
    let questions: Vec<String> = lines.into_iter().take(MAX_PLACES).map(|line| format!("{relative}:{line}")).collect();
    let answers = ask(&repo, questions, BUDGET, true)?;
    Some(format!("scout: direct callers of the code just edited (check they still fit the change):\n{answers}"))
}

/// A doc comment, attribute or blank line belongs to the declaration under it: that line is asked about instead.
fn declared_below(content: &str, line: usize) -> usize {
    let leading = |text: &str| {
        let text = text.trim_start();
        text.is_empty() || ["//", "/*", "*", "#", "@"].iter().any(|prefix| text.starts_with(prefix))
    };
    content.lines().enumerate().skip(line - 1).find(|(_, text)| !leading(text)).map_or(line, |(index, _)| index + 1)
}

fn before_search(input: &Value) -> Option<String> {
    let tool_input = input.get("tool_input")?;
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let (pattern, place) = match input.get("tool_name")?.as_str()? {
        "Grep" => (tool_input.get("pattern")?.as_str()?.to_owned(), tool_input.get("path").and_then(Value::as_str).map(|path| cwd.join(path)).unwrap_or(cwd)),
        "Bash" => searched(tool_input.get("command")?.as_str()?, &cwd)?,
        _ => return None,
    };
    let name = searched_name(&pattern)?;
    let repo = repo_root(if place.is_file() { place.parent()? } else { &place })?;
    let answers = ask(&repo, vec![name.clone()], SEARCH_BUDGET, false)?;
    Some(format!("scout: who calls `{name}` (exact, with entry points; unresolved lines are listed for reading, never dropped). For callers, `scout callers` answers more completely than a text search:\n{answers}"))
}

/// The pattern and the place searched by `rg ...`, `grep ...`, `git grep ...`, `ag ...` in a shell command: the
/// first argument that is not an option, and the first path after it (else the directory a `cd` moved to).
fn searched(command: &str, cwd: &Path) -> Option<(String, PathBuf)> {
    let words = shell_words(command);
    let mut directory = cwd.to_path_buf();
    for pair in words.windows(2) {
        if pair[0] == "cd" {
            directory = directory.join(&pair[1]);
        }
    }
    let start = words.iter().position(|word| ["rg", "grep", "egrep", "ag", "ack"].contains(&word.as_str()) || word.ends_with("/rg") || word.ends_with("/grep"))?;
    let mut rest = words[start + 1..].iter();
    let mut pattern = None;
    while let Some(word) = rest.next() {
        if word == "-e" || word == "--regexp" {
            pattern = rest.next().cloned();
            break;
        }
        if word.starts_with('-') {
            // An option's value (`-g '*.php'`, `-t php`) is no pattern.
            if ["-g", "--glob", "-t", "--type", "-A", "-B", "-C", "-m", "--max-count", "--include"].contains(&word.as_str()) {
                rest.next();
            }
            continue;
        }
        pattern = Some(word.clone());
        break;
    }
    let place = rest.find(|word| !word.starts_with('-')).map(|path| directory.join(path)).unwrap_or(directory);
    Some((pattern?, place))
}

/// Words of a command line, with simple quoting undone (enough to find a search pattern).
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    for character in command.chars() {
        match (quote, character) {
            (Some(open), close) if open == close => quote = None,
            (Some(_), _) => word.push(character),
            (None, '\'' | '"') => quote = Some(character),
            (None, ' ' | '\t' | '\n' | ';' | '|' | '&') => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            (None, _) => word.push(character),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// The function name a search pattern looks for (`saveOrder(`, `->saveOrder`, `\bsave_order\(`, `def save_order`),
/// or None when it looks for something else (a phrase, a regex, a word too short to mean one function).
fn searched_name(pattern: &str) -> Option<String> {
    let mut name = pattern.trim().to_owned();
    for prefix in ["function ", "def ", "fn ", "func ", "->", "?->", "::", ".", "\\.", "\\b"] {
        name = name.strip_prefix(prefix).unwrap_or(&name).to_owned();
    }
    for suffix in ["\\b", "\\s*\\(", "\\(", "(", "\\s*"] {
        name = name.strip_suffix(suffix).unwrap_or(&name).to_owned();
    }
    let identifier = name.chars().next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && name.chars().all(|character| character.is_ascii_alphanumeric() || character == '_');
    (identifier && name.len() >= 4 && !KEYWORDS.contains(&name.to_lowercase().as_str())).then_some(name)
}

fn repo_root(start: &Path) -> Option<PathBuf> {
    start.canonicalize().ok()?.ancestors().find(|directory| directory.join(".git").exists()).map(Path::to_path_buf)
}

/// Direct callers of each question, within the time budget; None when nothing useful came back in time. `start`
/// lets the question start a server for the repository (it keeps reading the code for the next one).
fn ask(repo: &Path, questions: Vec<String>, budget: Duration, start: bool) -> Option<String> {
    let (sender, answers) = mpsc::channel();
    let repo = repo.to_path_buf();
    thread::spawn(move || {
        for question in questions {
            let request = QueryRequest { command: "callers".to_owned(), query: question, cwd: repo.clone(), max_depth: 1, max_nodes: usize::MAX, include_tests: false, compact: true };
            let answer = if start { daemon::query(&repo, &request) } else { daemon::query_running(&repo, &request) };
            if sender.send(answer).is_err() {
                return;
            }
        }
    });
    let mut text = Vec::new();
    while let Ok(answer) = answers.recv_timeout(budget) {
        let Ok(response) = answer else { continue };
        // A file-level symbol, one nobody calls, or calls found by name alone (what the search itself prints)
        // add nothing worth the context.
        let by_name_only = response.stdout.contains("Total: 0 exact, 0 likely");
        let useful = response.exit_code == 0 && !by_name_only && response.stdout.lines().any(|line| line.starts_with("- ")) && !response.stdout.contains("(file)");
        if useful {
            text.extend(response.stdout.lines().filter(|line| !line.trim().is_empty()).map(str::to_owned));
        }
    }
    if text.is_empty() {
        return None;
    }
    if text.len() > MAX_LINES {
        let more = text.len() - MAX_LINES;
        text.truncate(MAX_LINES);
        text.push(format!("… {more} more lines: run `scout callers` for the full list"));
    }
    Some(text.join("\n"))
}

/// Hook entries this tool owns in Claude Code's settings, recognized by their command.
fn owned(entry: &Value) -> bool {
    entry.get("hooks").and_then(Value::as_array).into_iter().flatten().any(|hook| hook.get("command").and_then(Value::as_str).is_some_and(|command| command.contains(" hook after-edit") || command.contains(" hook before-search")))
}

/// Adds (or with `remove`, takes out) this tool's hooks in `settings`, leaving every other hook as it was.
/// Returns the new settings, or None when nothing changes.
pub fn configured(settings: &Value, binary: &str, remove: bool) -> Option<Value> {
    let mut updated = settings.clone();
    if !updated.is_object() {
        updated = json!({});
    }
    let hooks = updated.as_object_mut()?.entry("hooks").or_insert_with(|| json!({}));
    let wanted = [("PreToolUse", "Grep|Bash", "before-search"), ("PostToolUse", "Edit|MultiEdit", "after-edit")];
    for (event, matcher, mode) in wanted {
        let entries = hooks.as_object_mut()?.entry(event).or_insert_with(|| json!([])).as_array_mut()?;
        entries.retain(|entry| !owned(entry));
        if !remove {
            entries.push(json!({ "matcher": matcher, "hooks": [{ "type": "command", "command": format!("{binary} hook {mode}"), "timeout": 10 }] }));
        }
    }
    (updated != *settings).then_some(updated)
}

/// `scout claude-hooks install|remove [--dry-run]`: edits ~/.claude/settings.json (a backup is kept beside it).
pub fn configure(action: &str, dry_run: bool) -> Result<u8, String> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).ok_or("no home directory")?;
    let path = PathBuf::from(home).join(".claude/settings.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_owned());
    let settings: Value = serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let command = crate::cli::launcher()?;
    let Some(updated) = configured(&settings, &command.to_string_lossy(), action == "remove") else {
        println!("{}: already as wanted", path.display());
        return Ok(0);
    };
    let rendered = serde_json::to_string_pretty(&updated).map_err(|error| error.to_string())?;
    if dry_run {
        println!("would write {}:\n{rendered}", path.display());
        return Ok(0);
    }
    if path.is_file() {
        std::fs::copy(&path, path.with_extension("json.before-scout")).map_err(|error| error.to_string())?;
    }
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    std::fs::write(&path, rendered + "\n").map_err(|error| error.to_string())?;
    println!("{}: scout hooks {}", path.display(), if action == "remove" { "removed" } else { "installed" });
    Ok(0)
}
