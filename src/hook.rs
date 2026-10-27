//! Claude Code hooks: what an agent should know about callers, given to it at the moment it matters, so it does
//! not depend on the agent choosing this tool over a text search.
//!
//! `after-edit` (PostToolUse on Edit/MultiEdit): the direct callers of each function just edited.
//! `before-search` (PreToolUse on Grep/Bash): a text search for a function name also gets that function's callers.
//! `session-start` (SessionStart): the server of the session's repository starts reading the code, so the first
//! searches find it ready.
//!
//! Hooks never block or slow the agent: the search still runs, a server not ready in time adds nothing, and any
//! failure is silent.

use crate::daemon;
use crate::query::QueryRequest;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a hook waits for answers; past it the agent goes on without them (the server keeps warming up).
const BUDGET: Duration = Duration::from_secs(3);
/// Before a text search the agent is waiting: only a quick answer is worth adding.
const SEARCH_BUDGET: Duration = Duration::from_millis(1500);
/// Words a search may start with that name no function.
const KEYWORDS: [&str; 16] = ["function", "class", "def", "return", "public", "private", "protected", "static", "const", "import", "export", "async", "await", "interface", "trait", "struct"];
/// Lines of answer one hook adds at most: enough for the direct callers, small enough for the context.
const MAX_LINES: usize = 40;
/// Names asked about at most per search command: the server answers one question at a time, so each extra one can
/// keep the next search waiting.
const MAX_SEARCHED_NAMES: usize = 2;
/// Edited places looked up at most per edit.
const MAX_PLACES: usize = 3;

/// What became of one hook run, kept in the hook log so its usefulness can be measured.
#[derive(Clone, Copy, PartialEq)]
enum Outcome {
    /// Callers were added to the agent's context.
    Added,
    /// The command searched, but for no function name (a phrase, a regex, a word too short).
    NoName,
    /// The searched place is in no repository this tool can read.
    NoRepo,
    /// The repository's server could not be reached or started.
    NotRunning,
    /// The server did not answer within the hook's budget (often: still reading the code).
    Timeout,
    /// Answers came back but held nothing worth the context (nobody calls it, or only by name).
    NotUseful,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Outcome::Added => "added",
            Outcome::NoName => "no-name",
            Outcome::NoRepo => "no-repo",
            Outcome::NotRunning => "not-running",
            Outcome::Timeout => "timeout",
            Outcome::NotUseful => "not-useful",
        }
    }
}

/// `dry_run` (before-search only): print the repository and names the search would ask about, contact no server.
pub fn run(event: &str, dry_run: bool) -> u8 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let Ok(input) = serde_json::from_str::<Value>(&input) else { return 0 };
    let context = match event {
        "after-edit" => after_edit(&input),
        "before-search" if dry_run => {
            let planned = planned_search(&input);
            let shown = match &planned {
                Some(Ok(search)) => json!({ "repo": search.repo, "names": search.names }),
                Some(Err(outcome)) => json!({ "outcome": outcome.label() }),
                None => json!({ "outcome": "no-search" }),
            };
            println!("{shown}");
            None
        }
        "before-search" => before_search(&input),
        "session-start" => session_start(&input),
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
    if !holds_code(&file) {
        return None;
    }
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
    if questions.is_empty() {
        return None;
    }
    let started = Instant::now();
    let answers = ask(&repo, questions.clone(), BUDGET, true);
    record("after-edit", &repo, &questions, answers.as_ref().err().copied().unwrap_or(Outcome::Added), started);
    Some(format!("scout: direct callers of the code just edited (check they still fit the change):\n{}", answers.ok()?))
}

/// A doc comment, attribute or blank line belongs to the declaration under it: that line is asked about instead.
fn declared_below(content: &str, line: usize) -> usize {
    let leading = |text: &str| {
        let text = text.trim_start();
        text.is_empty() || ["//", "/*", "*", "#", "@"].iter().any(|prefix| text.starts_with(prefix))
    };
    content.lines().enumerate().skip(line - 1).find(|(_, text)| !leading(text)).map_or(line, |(index, _)| index + 1)
}

fn session_start(input: &Value) -> Option<String> {
    let cwd = PathBuf::from(input.get("cwd")?.as_str()?);
    let _ = daemon::ensure_running(&repo_root(&cwd)?);
    None
}

/// The function names one search command looks for, all in one repository.
struct PlannedSearch {
    repo: PathBuf,
    names: Vec<String>,
}

/// None when the tool call searches no code at all; else the names to ask about, or why there are none.
fn planned_search(input: &Value) -> Option<Result<PlannedSearch, Outcome>> {
    let tool_input = input.get("tool_input")?;
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let searches = match input.get("tool_name")?.as_str()? {
        "Grep" => vec![(tool_input.get("pattern")?.as_str()?.to_owned(), tool_input.get("path").and_then(Value::as_str).map(|path| cwd.join(path)).unwrap_or(cwd))],
        "Bash" => searched(tool_input.get("command")?.as_str()?, &cwd),
        _ => return None,
    };
    if searches.is_empty() {
        return None;
    }
    let mut names: Vec<String> = Vec::new();
    let mut repo = None;
    let mut named = false;
    let branches = searches.into_iter().flat_map(|(pattern, place)| alternatives(&pattern).into_iter().map(move |branch| (branch, place.clone())));
    for (pattern, place) in branches {
        let Some(name) = searched_name(&pattern) else { continue };
        named = true;
        if place.is_file() && !holds_code(&place) {
            continue;
        }
        let Some(place_repo) = place.parent().filter(|_| place.is_file()).or(Some(&place)).and_then(repo_root) else { continue };
        if repo.get_or_insert_with(|| place_repo.clone()) == &place_repo && !names.contains(&name) && names.len() < MAX_SEARCHED_NAMES {
            names.push(name);
        }
    }
    Some(match repo {
        Some(repo) if !names.is_empty() => Ok(PlannedSearch { repo, names }),
        _ if named => Err(Outcome::NoRepo),
        _ => Err(Outcome::NoName),
    })
}

fn before_search(input: &Value) -> Option<String> {
    let started = Instant::now();
    let PlannedSearch { repo, names } = match planned_search(input)? {
        Ok(search) => search,
        Err(outcome) => {
            record("before-search", Path::new(""), &[], outcome, started);
            return None;
        }
    };
    // A search in a repository whose server is not running starts it: this search goes on alone, the next ones
    // get answers. Waiting here would make a text search wait for a whole repository to be read.
    let answers = daemon::ensure_running(&repo).map_err(|_| Outcome::NotRunning).and_then(|()| ask(&repo, names.clone(), SEARCH_BUDGET, false));
    record("before-search", &repo, &names, answers.as_ref().err().copied().unwrap_or(Outcome::Added), started);
    let answers = answers.ok()?;
    let listed = names.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ");
    Some(format!("scout: who calls {listed} (exact, with entry points; unresolved lines are listed for reading, never dropped). For callers, `scout callers` answers more completely than a text search:\n{answers}"))
}

/// The branches of an alternation (`a\|b` in grep, `a|b` in rg and `grep -E`), each a search of its own.
fn alternatives(pattern: &str) -> Vec<String> {
    pattern.replace("\\|", "|").split('|').map(str::to_owned).collect()
}

/// Each code search in a shell command (`rg`, `grep`, `ag`, `git grep`, also inside `$( ... )`): its pattern and
/// place (the first path after the pattern, else the directory a `cd` moved to; `git grep` searches the repository
/// it runs in, its other words being revisions and pathspecs). Shell variables set earlier in the command
/// (`B=/repo; grep x $B/app`) are read in paths. A grep reading another command's output (`... | grep x`) or keeping
/// the lines that do not match (`-v`) searches no code; nor does a search in a place named by an unknown variable.
fn searched(command: &str, cwd: &Path) -> Vec<(String, PathBuf)> {
    let words = shell_words(command);
    let mut directory = cwd.to_path_buf();
    let mut variables = std::collections::HashMap::new();
    let mut found = Vec::new();
    for (index, word) in words.iter().enumerate() {
        if let Some((name, value)) = word.split_once('=').filter(|(name, _)| is_variable_name(name)) {
            if let Some(value) = expanded(value, &variables) {
                variables.insert(name.to_owned(), value);
            }
            continue;
        }
        if word == "cd" {
            if let Some(target) = words.get(index + 1).filter(|target| !is_separator(target)).and_then(|target| expanded(target, &variables)) {
                directory = directory.join(target);
            }
        }
        let is_search = ["rg", "grep", "egrep", "ag", "ack"].contains(&word.as_str()) || word.ends_with("/rg") || word.ends_with("/grep");
        let reads_output = index > 0 && words[index - 1] == "|";
        if is_search && !reads_output {
            let in_git = index > 0 && words[index - 1] == "git";
            let arguments: Vec<&String> = words[index + 1..].iter().take_while(|word| !is_separator(word)).collect();
            let Some((pattern, path)) = one_search(&arguments) else { continue };
            let place = match path.filter(|_| !in_git) {
                Some(path) => match expanded(path, &variables) {
                    Some(path) => directory.join(path),
                    None => continue,
                },
                None => directory.clone(),
            };
            found.push((pattern, place));
        }
    }
    found
}

fn is_variable_name(name: &str) -> bool {
    name.chars().next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && name.chars().all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// `text` with `~`, `$NAME` and `${NAME}` replaced (variables set in the command first, then the environment); None
/// when a variable has no known value.
fn expanded(text: &str, variables: &std::collections::HashMap<String, String>) -> Option<String> {
    let value = |name: &str| variables.get(name).cloned().or_else(|| std::env::var(name).ok());
    let mut result = match text.strip_prefix('~').filter(|rest| rest.is_empty() || rest.starts_with('/')) {
        Some(rest) => format!("{}{rest}", value("HOME")?),
        None => text.to_owned(),
    };
    while let Some(start) = result.find('$') {
        let after = &result[start + 1..];
        let (name, length) = match after.strip_prefix('{') {
            Some(braced) => (braced.split_once('}')?.0, braced.split_once('}')?.0.len() + 2),
            None => {
                let length = after.find(|character: char| !(character.is_ascii_alphanumeric() || character == '_')).unwrap_or(after.len());
                (&after[..length], length)
            }
        };
        if !is_variable_name(name) {
            return None;
        }
        result = format!("{}{}{}", &result[..start], value(name)?, &result[start + 1 + length..]);
    }
    Some(result)
}

/// Whether a file searched by name can hold calls: settings, notes, data and logs cannot.
fn holds_code(file: &Path) -> bool {
    let name = file.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    let extension = file.extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
    !name.starts_with(".env") && !["md", "txt", "json", "yml", "yaml", "toml", "ini", "log", "csv", "lock", "xml", "sql", "neon"].contains(&extension.as_str())
}

fn is_separator(word: &str) -> bool {
    word == "|" || word == ";"
}

/// The pattern of one search and the first path after it, as written.
fn one_search<'a>(arguments: &[&'a String]) -> Option<(String, Option<&'a String>)> {
    let mut rest = arguments.iter();
    let mut pattern = None;
    while let Some(word) = rest.next() {
        if ["-v", "--invert-match", "--files-without-match"].contains(&word.as_str()) {
            return None;
        }
        if *word == "-e" || *word == "--regexp" {
            pattern = rest.next().map(|word| (*word).clone());
            break;
        }
        if word.starts_with('-') {
            // An option's value (`-g '*.php'`, `-t php`) is no pattern.
            if ["-g", "--glob", "-t", "--type", "-A", "-B", "-C", "-m", "--max-count", "--include"].contains(&word.as_str()) {
                rest.next();
            }
            continue;
        }
        pattern = Some((*word).clone());
        break;
    }
    let path = rest.find(|word| !word.starts_with('-')).copied();
    Some((pattern?, path))
}

/// Words of a command line, with simple quoting undone (enough to find search patterns). A pipe becomes the word
/// `|`; every other end of a command (`;`, `&&`, `||`, a newline, `(`, `)`, `$(`, a backtick) becomes `;`.
fn shell_words(command: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let end_word = |word: &mut String, words: &mut Vec<String>| {
        if !word.is_empty() {
            words.push(std::mem::take(word));
        }
    };
    for character in command.chars() {
        match (quote, character) {
            (Some(open), close) if open == close => quote = None,
            (Some(_), _) => word.push(character),
            (None, '\'' | '"') => quote = Some(character),
            (None, ' ' | '\t') => end_word(&mut word, &mut words),
            (None, '|') => {
                end_word(&mut word, &mut words);
                // `||` runs the next command on failure: an end, not a pipe.
                if words.last().is_some_and(|last| last == "|") {
                    *words.last_mut().unwrap() = ";".to_owned();
                } else {
                    words.push("|".to_owned());
                }
            }
            (None, ';' | '&' | '\n' | '(' | ')' | '`') => {
                end_word(&mut word, &mut words);
                if words.last().is_none_or(|last| last != ";") {
                    words.push(";".to_owned());
                }
            }
            (None, _) => word.push(character),
        }
    }
    end_word(&mut word, &mut words);
    words
}

/// The function name a search pattern looks for (`saveOrder(`, `->saveOrder`, `\bsave_order\(`, `def save_order`),
/// or None when it looks for something else (a phrase, a regex, a word too short to mean one function).
fn searched_name(pattern: &str) -> Option<String> {
    let mut name = pattern.trim().to_owned();
    // `public static function save`, `pub async fn save`: the modifiers before the keyword say nothing of the name.
    for modifier in ["public ", "private ", "protected ", "static ", "final ", "abstract ", "pub ", "pub(crate) ", "async ", "export ", "default "] {
        while let Some(rest) = name.strip_prefix(modifier) {
            name = rest.trim_start().to_owned();
        }
    }
    // `function (save|load)`: a group around an alternation leaves a parenthesis on each branch.
    for prefix in ["function ", "def ", "fn ", "func ", "class ", "const ", "let ", "var ", "interface ", "trait ", "struct ", "enum ", "type ", "new ", "->", "?->", "::", ".", "\\.", "\\(", "(", "\\b"] {
        name = name.strip_prefix(prefix).unwrap_or(&name).to_owned();
    }
    for suffix in ["\\)", ")", "\\b", "\\s*\\(", "\\(", "(", "\\s*"] {
        name = name.strip_suffix(suffix).unwrap_or(&name).to_owned();
    }
    let identifier = name.chars().next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && name.chars().all(|character| character.is_ascii_alphanumeric() || character == '_');
    (identifier && name.len() >= 4 && !KEYWORDS.contains(&name.to_lowercase().as_str())).then_some(name)
}

/// The repository holding `start`; a place that does not exist as written (a glob such as `src/**/*.rs`) is looked
/// up from its nearest existing folder.
fn repo_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|place| place.exists())?.canonicalize().ok()?.ancestors().find(|directory| directory.join(".git").exists()).map(Path::to_path_buf)
}

/// Direct callers of each question, all within one time budget; the outcome when nothing useful came back in time.
/// `start` lets the question start a server for the repository (it keeps reading the code for the next one).
fn ask(repo: &Path, questions: Vec<String>, budget: Duration, start: bool) -> Result<String, Outcome> {
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
    let (mut answered, mut timed_out) = (false, false);
    let deadline = Instant::now() + budget;
    loop {
        let answer = match answers.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(answer) => answer,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                timed_out = true;
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let Ok(response) = answer else { continue };
        answered = true;
        // A file-level symbol, one nobody calls, or calls found by name alone (what the search itself prints)
        // add nothing worth the context. A name may stand for several symbols, each with its own total.
        let mut totals = response.stdout.lines().filter(|line| line.starts_with("Total:")).peekable();
        let by_name_only = totals.peek().is_some() && totals.all(|line| line.starts_with("Total: 0 exact, 0 likely"));
        let useful = response.exit_code == 0 && !by_name_only && response.stdout.lines().any(|line| line.starts_with("- ")) && !response.stdout.contains("(file)");
        if useful {
            text.extend(response.stdout.lines().filter(|line| !line.trim().is_empty()).map(str::to_owned));
        }
    }
    if text.is_empty() {
        return Err(if timed_out {
            Outcome::Timeout
        } else if answered {
            Outcome::NotUseful
        } else {
            Outcome::NotRunning
        });
    }
    if text.len() > MAX_LINES {
        let more = text.len() - MAX_LINES;
        text.truncate(MAX_LINES);
        text.push(format!("… {more} more lines: run `scout callers` for the full list"));
    }
    Ok(text.join("\n"))
}

/// Where each hook run is logged, one JSON line each: kept with the saved analysis so it survives a restart.
fn log_path() -> PathBuf {
    daemon::cache_directory().join("hooks.log")
}

/// The log keeps about this much: enough for weeks of searches.
const LOG_BYTES: u64 = 1 << 20;

/// Appends one run to the hook log; any failure is ignored (the log must never stop a hook).
fn record(event: &str, repo: &Path, asked: &[String], outcome: Outcome, started: Instant) {
    let path = log_path();
    let line = json!({
        "at": SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or_default(),
        "event": event,
        "repo": repo,
        "asked": asked,
        "outcome": outcome.label(),
        "ms": started.elapsed().as_millis() as u64,
    });
    if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > LOG_BYTES) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let half = text.len() / 2;
            let kept = text[half..].split_once('\n').map_or("", |(_, rest)| rest);
            let _ = std::fs::write(&path, kept);
        }
    }
    let _ = std::fs::create_dir_all(daemon::cache_directory());
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

/// Per day (UTC), per hook: how many runs and what became of them, newest day first, for `scout status`.
pub fn usage_by_day(days: u64) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(log_path()) else { return Vec::new() };
    let today = SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs() / 86_400).unwrap_or_default();
    let mut tallies: std::collections::BTreeMap<(u64, String), std::collections::BTreeMap<String, u64>> = Default::default();
    for entry in text.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
        let day = entry["at"].as_u64().unwrap_or_default() / 86_400;
        if day + days <= today {
            continue;
        }
        let key = (day, entry["event"].as_str().unwrap_or_default().to_owned());
        *tallies.entry(key).or_default().entry(entry["outcome"].as_str().unwrap_or_default().to_owned()).or_default() += 1;
    }
    tallies
        .iter()
        .rev()
        .map(|((day, event), outcomes)| {
            let runs: u64 = outcomes.values().sum();
            let added = outcomes.get("added").copied().unwrap_or_default();
            let others = outcomes.iter().filter(|(outcome, _)| *outcome != "added").map(|(outcome, count)| format!("{outcome} {count}")).collect::<Vec<_>>().join(", ");
            format!("{}  {event:<13} {runs:>5} runs, {:>3}% added{}", civil_date(*day), added * 100 / runs.max(1), if others.is_empty() { String::new() } else { format!("  ({others})") })
        })
        .collect()
}

/// `YYYY-MM-DD` of a day counted from 1970-01-01 (proleptic Gregorian calendar).
fn civil_date(days: u64) -> String {
    let shifted = days as i64 + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Hook entries this tool owns in Claude Code's settings, recognized by their command.
fn owned(entry: &Value) -> bool {
    entry.get("hooks").and_then(Value::as_array).into_iter().flatten().any(|hook| hook.get("command").and_then(Value::as_str).is_some_and(|command| command.contains(" hook after-edit") || command.contains(" hook before-search") || command.contains(" hook session-start")))
}

/// Adds (or with `remove`, takes out) this tool's hooks in `settings`, leaving every other hook as it was.
/// Returns the new settings, or None when nothing changes.
pub fn configured(settings: &Value, binary: &str, remove: bool) -> Option<Value> {
    let mut updated = settings.clone();
    if !updated.is_object() {
        updated = json!({});
    }
    let hooks = updated.as_object_mut()?.entry("hooks").or_insert_with(|| json!({}));
    let wanted = [("PreToolUse", "Grep|Bash", "before-search"), ("PostToolUse", "Edit|MultiEdit", "after-edit"), ("SessionStart", "startup|resume", "session-start")];
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
