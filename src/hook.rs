//! Claude Code hooks: what an agent should know about callers, given to it at the moment it matters, so it does
//! not depend on the agent choosing this tool over a text search.
//!
//! `after-change` (PostToolUse on Edit/MultiEdit/Write/Bash and the `callers` MCP tool): the direct callers of each
//! function the call changed, however it changed the file (an edit, a written file, a script); a `callers` question
//! the agent asked itself is noted.
//! `finish` (Stop): before the agent ends its turn, the callers of every function changed in the session (by it or
//! its subagents) that it has not been shown yet, once.
//! `before-change` (PreToolUse on Edit/MultiEdit): the direct callers of each function an edit is about to change,
//! read where its text stands in the file now, so the change is planned with them in view.
//! `before-search` (PreToolUse on Grep/Bash): a text search for a function name also gets that function's callers.
//! `prompt` (UserPromptSubmit): the repository's server is started again if it stopped while the session was idle,
//! and a prompt naming code (a stack-trace line `file:line`, `Class::method`) gets the callers of what it names.
//! `session-start` (SessionStart): the server of the session's repository starts reading the code, so the first
//! searches find it ready, and the session's changes are measured from here.
//!
//! Hooks never block the agent: the search or edit still runs, a server not ready in time adds nothing, and any
//! failure is silent. Only `finish` makes the agent go on, for one more round, and only when a changed function has
//! callers it has not seen.

use crate::changes::{holds_code, with_session, ChangedLines, MAX_LINES_PER_FILE};
use crate::daemon;
use crate::query::{QueryRequest, QueryResponse};
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
/// At the end of a turn the agent is done working: a longer wait buys the callers of everything it changed.
const FINISH_BUDGET: Duration = Duration::from_secs(20);
/// Words a search may start with that name no function.
const KEYWORDS: [&str; 23] = [
    "function", "class", "def", "func", "return", "public", "private", "protected", "static", "const", "import", "export", "async", "await", "interface", "trait",
    "struct", "enum", "namespace", "package", "module", "require", "include",
];
/// Lines of answer one hook adds at most: enough for the direct callers, small enough for the context.
const MAX_LINES: usize = 40;
/// Names asked about at most per search command: the server answers one question at a time, so each extra one can
/// keep the next search waiting.
const MAX_SEARCHED_NAMES: usize = 2;
/// Changed functions whose callers are added at most per tool call; the others wait for the end of the turn.
const MAX_PLACES: usize = 3;
/// At the end of a turn: changed functions asked about, and lines of answer added, at most.
const MAX_REMINDED: usize = 12;
const MAX_FINISH_LINES: usize = 150;

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
    /// End of a turn: every changed function's callers had been shown before.
    AllShown,
    /// End of a turn: the callers of changed functions not shown before were added.
    Reminded,
    /// End of a turn: the files the session changed hold no changed function (new files only, changes undone).
    NoChange,
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
            Outcome::AllShown => "all-shown",
            Outcome::Reminded => "reminded",
            Outcome::NoChange => "no-change",
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
        // `after-edit` is what an older setup installed.
        "after-change" | "after-edit" => after_change(&input),
        "finish" => finish(&input),
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
        "before-change" => before_change(&input),
        "prompt" => prompt(&input),
        "session-start" => session_start(&input),
        _ => None,
    };
    if let Some(hint) = context {
        let event_name = input.get("hook_event_name").and_then(Value::as_str).unwrap_or("PostToolUse");
        println!("{}", json!({ "systemMessage": hint.notice(), "hookSpecificOutput": { "hookEventName": event_name, "additionalContext": hint.context } }));
    }
    0
}

/// What a hook adds: the text for the agent, and the symbols it covers, named in one line the user sees so the work
/// done for the agent is not invisible.
struct Hint {
    context: String,
    symbols: Vec<String>,
}

impl Hint {
    fn notice(&self) -> String {
        const NAMED: usize = 3;
        let mut named: Vec<&str> = self.symbols.iter().take(NAMED).map(String::as_str).collect();
        let more = format!("+{} more", self.symbols.len().saturating_sub(NAMED));
        if self.symbols.len() > NAMED {
            named.push(&more);
        }
        format!("scout: callers of {} given to Claude", named.join(", "))
    }
}

fn session_id(input: &Value) -> &str {
    input.get("session_id").and_then(Value::as_str).unwrap_or_default()
}

fn session_start(input: &Value) -> Option<Hint> {
    let cwd = PathBuf::from(input.get("cwd")?.as_str()?);
    let repo = repo_root(&cwd)?;
    crate::changes::forget_old_sessions();
    with_session(session_id(input), |session| session.follow(&repo));
    let _ = daemon::ensure_running(&repo);
    None
}

/// The symbols a `callers` question names, when the tool call is one: the MCP tool, or `scout callers X` in a shell.
fn asked_callers(tool: &str, tool_input: &Value) -> Vec<String> {
    match tool {
        "mcp__scout__callers" => tool_input.get("target").and_then(Value::as_str).map(str::to_owned).into_iter().collect(),
        "Bash" => {
            let words = shell_words(tool_input.get("command").and_then(Value::as_str).unwrap_or_default());
            let asks = |index: usize| words[index] == "callers" && index > 0 && (words[index - 1] == "scout" || words[index - 1].ends_with("/scout"));
            (0..words.len()).filter(|index| asks(*index)).filter_map(|index| words[index + 1..].iter().take_while(|word| !is_separator(word)).find(|word| !word.starts_with('-')).cloned()).collect()
        }
        _ => Vec::new(),
    }
}

/// Before an edit lands: the direct callers of each function its `old_string`s sit in, read where they stand in the
/// file now, so the change is made knowing what depends on it. What is shown here is not shown again after it.
fn before_change(input: &Value) -> Option<Hint> {
    let started = Instant::now();
    let tool_input = input.get("tool_input")?;
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let file = cwd.join(tool_input.get("file_path")?.as_str()?);
    if !holds_code(&file) {
        return None;
    }
    let repo = repo_root(file.parent()?)?;
    let relative = file.canonicalize().ok()?.strip_prefix(&repo).ok()?.to_string_lossy().into_owned();
    let text = std::fs::read_to_string(&file).ok()?;
    let edits: Vec<&Value> = match tool_input.get("edits").and_then(Value::as_array) {
        Some(edits) => edits.iter().collect(),
        None => vec![tool_input],
    };
    let replaced = edits.iter().filter_map(|edit| Some((edit.get("old_string")?.as_str()?, edit.get("replace_all").and_then(Value::as_bool).unwrap_or(false))));
    let lines = lines_to_change(&text, replaced);
    if lines.is_empty() {
        return None;
    }
    let id = session_id(input);
    // Followed from before the edit, so the edit is measured as this session's change once it lands.
    with_session(id, |session| session.follow(&repo));
    // A server not running is started: a repository it saved the analysis of answers within the budget, a
    // larger warm-up goes on for the next edit, the edit never waits longer.
    let deadline = started + SEARCH_BUDGET;
    let symbols = match changed_symbols(&repo, vec![ChangedLines { repo: repo.clone(), file: relative, lines }], deadline, true) {
        Ok(symbols) => symbols,
        Err(outcome) => {
            record("before-change", &repo, &[], outcome, started);
            return None;
        }
    };
    let unshown: Vec<String> = with_session(id, |session| symbols.into_iter().filter(|symbol| !session.was_shown(symbol)).take(MAX_PLACES).collect()).unwrap_or_default();
    if unshown.is_empty() {
        return None;
    }
    let callers = ask(&repo, unshown.clone(), deadline, true);
    with_session(id, |session| session.mark_shown(callers.answered.iter().map(String::as_str)));
    record("before-change", &repo, &unshown, callers.outcome(), started);
    let answers = callers.text(MAX_LINES)?;
    Some(Hint { context: format!("scout: who calls the code this edit changes (keep each caller working, or change it too):\n{answers}"), symbols: callers.useful })
}

/// Occurrences of one old string an edit looks at, at most: `replace_all` on a common word must stay cheap.
const MAX_REPLACED: usize = 50;

/// The lines of `text` (from 1) that replacing these strings changes: every occurrence of an old string when all
/// are replaced, else the first. An old string an earlier edit of the same call puts in is not in `text`: skipped.
fn lines_to_change<'a>(text: &str, replaced: impl Iterator<Item = (&'a str, bool)>) -> Vec<u32> {
    let mut lines = std::collections::BTreeSet::new();
    for (old, all) in replaced.filter(|(old, _)| !old.trim().is_empty()) {
        let mut line = 1;
        let mut counted_to = 0;
        for (offset, _) in text.match_indices(old).take(if all { MAX_REPLACED } else { 1 }) {
            line += text[counted_to..offset].matches('\n').count() as u32;
            counted_to = offset;
            let last = line + old.trim_end_matches('\n').matches('\n').count() as u32;
            lines.extend(line..=last);
        }
    }
    lines.into_iter().take(MAX_LINES_PER_FILE).collect()
}

/// Code a prompt names, asked about at most: a pasted stack trace names many frames, the first ones matter.
const MAX_PROMPT_TARGETS: usize = 3;

/// When the user writes: the repository's server is started again if it stopped while the session was idle, and
/// code the prompt names gets its callers, a stack-trace line (`app/Foo.php:57`, `Foo.php(57)`, Python's
/// `File "x.py", line 57`) or a `Class::method`.
fn prompt(input: &Value) -> Option<Hint> {
    let started = Instant::now();
    let cwd = PathBuf::from(input.get("cwd")?.as_str()?);
    let repo = repo_root(&cwd)?;
    let targets = named_code(input.get("prompt")?.as_str()?, &repo);
    if targets.is_empty() {
        let _ = daemon::ensure_running(&repo);
        return None;
    }
    let callers = ask(&repo, targets.clone(), started + SEARCH_BUDGET, true);
    record("prompt", &repo, &targets, callers.outcome(), started);
    with_session(session_id(input), |session| session.mark_shown(callers.answered.iter().map(String::as_str)));
    let answers = callers.text(MAX_LINES)?;
    Some(Hint { context: format!("scout: who calls the code this prompt names (exact, with entry points; unresolved lines are listed for reading):\n{answers}"), symbols: callers.useful })
}

/// What a prompt names in the repository, in order and each once: `file:line` for a line of a code file there (a
/// path from another machine is matched by its end), `Class::method` (or Laravel's `Class@method`) for a method.
fn named_code(text: &str, repo: &Path) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    let words: Vec<&str> = text.split(|character: char| character.is_whitespace() || "`'\"<>[]{},;".contains(character)).filter(|word| !word.is_empty()).collect();
    for (index, word) in words.iter().enumerate() {
        // Python: `File "app/x.py", line 57`: the quotes and the comma split it into `app/x.py` `line` `57`.
        let python_line = (words.get(index + 1) == Some(&"line")).then(|| words.get(index + 2)).flatten().and_then(|line| line.parse::<u32>().ok());
        let target = match python_line {
            Some(line) => repo_file(word, repo).map(|file| format!("{file}:{line}")),
            None => file_line(word, repo).or_else(|| method_name(word)),
        };
        if let Some(target) = target.filter(|target| !targets.contains(target)) {
            targets.push(target);
            if targets.len() == MAX_PROMPT_TARGETS {
                break;
            }
        }
    }
    targets
}

/// `path:line`, `path:line:column` or `path(line)` (PHP traces), from inside a frame's parentheses too.
fn file_line(word: &str, repo: &Path) -> Option<String> {
    let word = word.trim_start_matches('(').trim_end_matches([')', ':', '.']);
    let (path, line) = match word.split_once('(') {
        Some((path, line)) => (path, line.trim_end_matches(')')),
        None => {
            let mut parts = word.splitn(3, ':');
            (parts.next()?, parts.next()?)
        }
    };
    let line: u32 = line.parse().ok().filter(|line| *line > 0)?;
    Some(format!("{}:{line}", repo_file(path, repo)?))
}

/// A code file of the repository the path names, relative to it: as written, or (a path on another machine, such
/// as a server's `/var/www/app/...`) by the longest end of it that exists here. Dependencies are not the code asked
/// about.
fn repo_file(path: &str, repo: &Path) -> Option<String> {
    let path = Path::new(path.strip_prefix("file://").unwrap_or(path));
    if path.extension().is_none() || !holds_code(path) {
        return None;
    }
    let components: Vec<_> = path.components().filter(|part| matches!(part, std::path::Component::Normal(_))).collect();
    let relative = match path.strip_prefix(repo) {
        Ok(inside) => inside.to_path_buf(),
        Err(_) => (0..components.len()).map(|start| components[start..].iter().collect::<PathBuf>()).find(|candidate| repo.join(candidate).is_file())?,
    };
    let dependency = relative.components().any(|part| ["vendor", "node_modules"].contains(&part.as_os_str().to_string_lossy().as_ref()));
    (repo.join(&relative).is_file() && !dependency).then(|| relative.to_string_lossy().into_owned())
}

/// `Class::method`, `Namespace\Class::method()` or `Class@method`: a method named with its class.
fn method_name(word: &str) -> Option<String> {
    let word = word.trim_end_matches(['(', ')', '.', ':']).replace('@', "::");
    let (class, method) = word.rsplit_once("::")?;
    let class = class.trim_start_matches('\\');
    let is_name = |text: &str| text.chars().next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && text.chars().all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '\\');
    (is_name(class) && is_name(method) && !method.contains('\\') && method.len() >= 2).then(|| format!("{class}::{method}"))
}

/// What the tool call changed in the code, found in the repositories rather than in the call: an edit, a written
/// file and a shell script that rewrites one all count. The callers of the functions it changed that the agent has
/// not been shown yet are added.
fn after_change(input: &Value) -> Option<Hint> {
    let tool = input.get("tool_name")?.as_str()?;
    let tool_input = input.get("tool_input")?;
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let asked = asked_callers(tool, tool_input);
    let edited = tool_input.get("file_path").and_then(Value::as_str).map(|file| cwd.join(file));
    let starting_points = [Some(cwd.as_path()), edited.as_deref().and_then(Path::parent)];
    let repositories: Vec<PathBuf> = starting_points.into_iter().flatten().filter_map(repo_root).collect();
    let id = session_id(input);
    let changed = with_session(id, |session| {
        session.mark_shown(asked.iter().map(String::as_str));
        // A file edited the moment its repository starts being followed is already in the snapshot: count it here.
        let mut edited_first = Vec::new();
        for repo in &repositories {
            let relative = edited.as_ref().and_then(|file| file.canonicalize().ok()?.strip_prefix(repo).ok().map(|file| file.to_string_lossy().into_owned()));
            if session.follow(repo) {
                if let Some(file) = relative.filter(|file| holds_code(Path::new(file))) {
                    session.note_changed(repo, &file);
                    edited_first.push((repo.clone(), file));
                }
            }
        }
        let mut changed = Vec::new();
        for repo in session.repositories() {
            let mut files = session.changes_since_last_look(&repo);
            files.extend(edited_first.iter().filter(|(edited_repo, _)| *edited_repo == repo).map(|(_, file)| file.clone()));
            changed.extend(session.changed_lines(&repo, &files));
        }
        changed
    })?;
    let started = Instant::now();
    let deadline = started + BUDGET;
    let mut added = Vec::new();
    let mut covered = Vec::new();
    for (repo, places) in by_repository(changed) {
        let symbols = match changed_symbols(&repo, places, deadline, true) {
            Ok(symbols) => symbols,
            Err(outcome) => {
                record("after-change", &repo, &[], outcome, started);
                continue;
            }
        };
        let unshown: Vec<String> = with_session(id, |session| symbols.into_iter().filter(|symbol| !session.was_shown(symbol)).take(MAX_PLACES).collect()).unwrap_or_default();
        if unshown.is_empty() {
            continue;
        }
        let callers = ask(&repo, unshown.clone(), deadline, true);
        with_session(id, |session| session.mark_shown(callers.answered.iter().map(String::as_str)));
        record("after-change", &repo, &unshown, callers.outcome(), started);
        added.extend(callers.text(MAX_LINES));
        covered.extend(callers.useful);
    }
    (!added.is_empty()).then(|| Hint { context: format!("scout: direct callers of the code just changed (check they still fit the change):\n{}", added.join("\n")), symbols: covered })
}

/// Before the agent ends its turn: the callers of every function the session changed that it was not shown yet,
/// once. Asked again after this, the agent may stop.
fn finish(input: &Value) -> Option<Hint> {
    if input.get("stop_hook_active").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let id = session_id(input);
    let changed = with_session(id, |session| session.has_changes().then(|| session.repositories().iter().flat_map(|repo| session.all_changed_lines(repo)).collect::<Vec<_>>()))??;
    let started = Instant::now();
    let deadline = started + FINISH_BUDGET;
    let mut reminders = Vec::new();
    let mut covered = Vec::new();
    let mut not_fetched = Vec::new();
    for (repo, places) in by_repository(changed) {
        let symbols = match changed_symbols(&repo, places, deadline, true) {
            Ok(symbols) => symbols,
            Err(outcome) => {
                record("finish", &repo, &[], outcome, started);
                continue;
            }
        };
        let unshown: Vec<String> = with_session(id, |session| symbols.iter().filter(|symbol| !session.was_shown(symbol)).cloned().collect()).unwrap_or_default();
        if unshown.is_empty() {
            record("finish", &repo, &[], if symbols.is_empty() { Outcome::NoChange } else { Outcome::AllShown }, started);
            continue;
        }
        let callers = ask(&repo, unshown.iter().take(MAX_REMINDED).cloned().collect(), deadline, true);
        let missed: Vec<String> = unshown.iter().filter(|symbol| !callers.answered.contains(symbol)).cloned().collect();
        // Each changed function is brought up once: listed now, whether its callers came back in time or not.
        with_session(id, |session| session.mark_shown(unshown.iter().map(String::as_str)));
        let outcome = match callers.outcome() {
            Outcome::Added => Outcome::Reminded,
            _ if !missed.is_empty() => Outcome::Timeout,
            other => other,
        };
        record("finish", &repo, &unshown, outcome, started);
        reminders.extend(callers.text(MAX_FINISH_LINES));
        covered.extend(callers.useful);
        not_fetched.extend(missed);
    }
    if reminders.is_empty() && not_fetched.is_empty() {
        return None;
    }
    let mut text = "scout: functions changed in this session whose callers you have not looked at yet. Before finishing, check each caller still fits the change (read the `unresolved` lines too):".to_owned();
    for reminder in reminders {
        text.push('\n');
        text.push_str(&reminder);
    }
    if !not_fetched.is_empty() {
        text.push_str(&format!("\nAlso changed, callers not fetched in time (run `scout callers <symbol> --compact`): {}", not_fetched.join(", ")));
    }
    covered.extend(not_fetched);
    Some(Hint { context: text, symbols: covered })
}

fn by_repository(changed: Vec<ChangedLines>) -> std::collections::BTreeMap<PathBuf, Vec<ChangedLines>> {
    let mut grouped: std::collections::BTreeMap<PathBuf, Vec<ChangedLines>> = Default::default();
    for lines in changed {
        grouped.entry(lines.repo.clone()).or_default().push(lines);
    }
    grouped
}

/// The functions (and classes) around changed lines, each once; a change outside any (an import, a file-level
/// statement) names none.
/// `start` lets the question start the repository's server.
fn changed_symbols(repo: &Path, places: Vec<ChangedLines>, deadline: Instant, start: bool) -> Result<Vec<String>, Outcome> {
    let files: Vec<String> = places.iter().map(|place| place.file.clone()).collect();
    let questions = places.into_iter().map(|place| format!("{}:{}", place.file, place.lines.iter().map(u32::to_string).collect::<Vec<_>>().join(","))).collect();
    let replies = replies_within(repo, "locate", questions, deadline, start);
    if replies.answered.is_empty() {
        return Err(if replies.timed_out { Outcome::Timeout } else { Outcome::NotRunning });
    }
    let mut symbols: Vec<String> = Vec::new();
    for (_, response) in replies.answered {
        for symbol in response.stdout.lines().filter_map(|line| line.split("  ").next()).filter(|symbol| !symbol.is_empty() && !files.iter().any(|file| file == symbol)) {
            if !symbols.iter().any(|known| known == symbol) {
                symbols.push(symbol.to_owned());
            }
        }
    }
    Ok(symbols)
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
        // A file without an extension (a Makefile, a shell script) is no code any driver reads.
        if place.is_file() && (!holds_code(&place) || place.extension().is_none()) {
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

fn before_search(input: &Value) -> Option<Hint> {
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
    if daemon::ensure_running(&repo).is_err() {
        record("before-search", &repo, &names, Outcome::NotRunning, started);
        return None;
    }
    let callers = ask(&repo, names.clone(), started + SEARCH_BUDGET, false);
    record("before-search", &repo, &names, callers.outcome(), started);
    with_session(session_id(input), |session| session.mark_shown(callers.answered.iter().map(String::as_str)));
    let answers = callers.text(MAX_LINES)?;
    let listed = names.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ");
    Some(Hint { context: format!("scout: who calls {listed} (exact, with entry points; unresolved lines are listed for reading, never dropped). For callers, `scout callers` answers more completely than a text search:\n{answers}"), symbols: callers.useful })
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
        // `| command grep x` reads the output too: `command` only skips shell functions and aliases.
        let before = if index > 0 && words[index - 1] == "command" { index - 1 } else { index };
        let reads_output = before > 0 && words[before - 1] == "|";
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

fn is_separator(word: &str) -> bool {
    word == "|" || word == ";"
}

/// Options of grep, rg, ag and ack written apart from their value (`-g '*.php'`, `--max-depth 2`).
const VALUED_OPTIONS: [&str; 26] = [
    "-g", "--glob", "--iglob", "-t", "--type", "-T", "--type-not", "-A", "-B", "-C", "--context", "--after-context", "--before-context", "-m", "--max-count",
    "--include", "--exclude", "--exclude-dir", "-d", "--max-depth", "-j", "--threads", "-M", "--max-columns", "-r", "--replace",
];

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
            if VALUED_OPTIONS.contains(&word.as_str()) {
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
    // `^\s*function\s+save\s*\(`: anchors and whitespace classes say where the name stands, not what it is.
    let mut name = pattern.replace("\\s+", " ").replace("[[:space:]]+", " ").replace("\\s*", "").replace("[[:space:]]*", "");
    name = name.trim().trim_start_matches('^').trim_end_matches('$').trim().to_owned();
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

/// What one server answered to questions asked in turn, those that came back before the deadline.
struct Replies {
    answered: Vec<(String, QueryResponse)>,
    timed_out: bool,
}

/// `start` lets the question start a server for the repository (it keeps reading the code for the next one).
fn replies_within(repo: &Path, command: &str, questions: Vec<String>, deadline: Instant, start: bool) -> Replies {
    let (sender, replies) = mpsc::channel();
    let repo = repo.to_path_buf();
    let command = command.to_owned();
    thread::spawn(move || {
        for question in questions {
            let request = QueryRequest { command: command.clone(), query: question.clone(), cwd: repo.clone(), max_depth: 1, max_nodes: usize::MAX, include_tests: false, compact: true };
            let answer = if start { daemon::query(&repo, &request) } else { daemon::query_running(&repo, &request) };
            if sender.send((question, answer)).is_err() {
                return;
            }
        }
    });
    let mut answered = Vec::new();
    loop {
        match replies.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((question, Ok(response))) => answered.push((question, response)),
            Ok((_, Err(_))) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => return Replies { answered, timed_out: true },
            Err(mpsc::RecvTimeoutError::Disconnected) => return Replies { answered, timed_out: false },
        }
    }
}

/// Direct callers worth adding to the agent's context, and which questions the server answered in time.
struct Callers {
    lines: Vec<String>,
    answered: Vec<String>,
    /// The questions whose answers are among `lines`.
    useful: Vec<String>,
    timed_out: bool,
}

impl Callers {
    /// What became of the questions when nothing useful came back in time.
    fn outcome(&self) -> Outcome {
        if !self.lines.is_empty() {
            Outcome::Added
        } else if self.timed_out {
            Outcome::Timeout
        } else if !self.answered.is_empty() {
            Outcome::NotUseful
        } else {
            Outcome::NotRunning
        }
    }

    fn text(&self, max_lines: usize) -> Option<String> {
        if self.lines.is_empty() {
            return None;
        }
        let mut text: Vec<String> = self.lines.iter().take(max_lines).cloned().collect();
        if self.lines.len() > max_lines {
            text.push(format!("… {} more lines: run `scout callers` for the full list", self.lines.len() - max_lines));
        }
        Some(text.join("\n"))
    }
}

/// Direct callers of each question, all before one deadline.
fn ask(repo: &Path, questions: Vec<String>, deadline: Instant, start: bool) -> Callers {
    let replies = replies_within(repo, "callers", questions, deadline, start);
    let mut callers = Callers { lines: Vec::new(), answered: Vec::new(), useful: Vec::new(), timed_out: replies.timed_out };
    for (question, response) in replies.answered {
        callers.answered.push(question.clone());
        // A file-level symbol, one nobody calls, or calls found by name alone (what the search itself prints)
        // add nothing worth the context. A name may stand for several symbols, each with its own total.
        let mut totals = response.stdout.lines().filter(|line| line.starts_with("Total:")).peekable();
        let by_name_only = totals.peek().is_some() && totals.all(|line| line.starts_with("Total: 0 exact, 0 likely"));
        let useful = response.exit_code == 0 && !by_name_only && response.stdout.lines().any(|line| line.starts_with("- ")) && !response.stdout.contains("(file)");
        if useful {
            // The symbol a `file:line` question stood for, named by the answer's first line.
            let symbol = response.stdout.lines().next().and_then(|line| line.split("  ").next()).filter(|_| question.contains(':') && !question.contains("::"));
            callers.useful.push(symbol.map_or(question, str::to_owned));
            // Hooks ask for direct callers only: the lines saying deeper ones were cut tell the agent nothing.
            let worth_reading = |line: &&str| !line.trim().is_empty() && !line.starts_with("… (stopped") && !line.starts_with("Some branches were cut");
            callers.lines.extend(response.stdout.lines().filter(worth_reading).map(str::to_owned));
        }
    }
    callers
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
        // Cut as bytes at a line end: the middle byte may fall inside a character of a path or a name.
        if let Ok(bytes) = std::fs::read(&path) {
            let half = bytes.len() / 2;
            let kept = bytes[half..].iter().position(|byte| *byte == b'\n').map_or(&[][..], |end| &bytes[half + end + 1..]);
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
            // At the end of a turn the aim is that nothing is left to show: every changed function's callers seen.
            let aim = if event == "finish" { "all-shown" } else { "added" };
            let runs: u64 = outcomes.values().sum();
            let reached = outcomes.get(aim).copied().unwrap_or_default();
            let others = outcomes.iter().filter(|(outcome, _)| *outcome != aim).map(|(outcome, count)| format!("{outcome} {count}")).collect::<Vec<_>>().join(", ");
            format!("{}  {event:<13} {runs:>5} runs, {:>3}% {aim}{}", civil_date(*day), reached * 100 / runs.max(1), if others.is_empty() { String::new() } else { format!("  ({others})") })
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
    let modes = [" hook after-edit", " hook after-change", " hook finish", " hook before-search", " hook before-change", " hook prompt", " hook session-start"];
    entry.get("hooks").and_then(Value::as_array).into_iter().flatten().any(|hook| hook.get("command").and_then(Value::as_str).is_some_and(|command| modes.iter().any(|mode| command.contains(mode))))
}

/// Each hook: its event, the tools it runs for (none: every time), its mode, and its time limit in seconds.
/// No `SubagentStop`: a subagent's hooks carry its parent's session, so a reminder there would be marked shown and
/// never reach the agent that reports to the user; the parent's `Stop` covers the subagent's changes too.
const HOOKS: [(&str, &str, &str, u64); 6] = [
    ("PreToolUse", "Grep|Bash", "before-search", 10),
    ("PreToolUse", "Edit|MultiEdit", "before-change", 10),
    ("UserPromptSubmit", "", "prompt", 10),
    ("PostToolUse", "Edit|MultiEdit|Write|Bash|mcp__scout__callers", "after-change", 10),
    ("SessionStart", "startup|resume", "session-start", 10),
    ("Stop", "", "finish", 30),
];

/// Adds (or with `remove`, takes out) this tool's hooks in `settings`, leaving every other hook as it was.
/// Returns the new settings, or None when nothing changes.
pub fn configured(settings: &Value, binary: &str, remove: bool) -> Option<Value> {
    let mut updated = settings.clone();
    if !updated.is_object() {
        updated = json!({});
    }
    let hooks = updated.as_object_mut()?.entry("hooks").or_insert_with(|| json!({}));
    // This tool's entries go first, under every event: one an older setup used may no longer be wanted.
    for entries in hooks.as_object_mut()?.values_mut().filter_map(Value::as_array_mut) {
        entries.retain(|entry| !owned(entry));
    }
    for (event, matcher, mode, timeout) in HOOKS {
        let entries = hooks.as_object_mut()?.entry(event).or_insert_with(|| json!([])).as_array_mut()?;
        if !remove {
            let mut entry = json!({ "hooks": [{ "type": "command", "command": format!("{binary} hook {mode}"), "timeout": timeout }] });
            if !matcher.is_empty() {
                entry["matcher"] = json!(matcher);
            }
            entries.push(entry);
        }
    }
    hooks.as_object_mut()?.retain(|_, entries| entries.as_array().is_none_or(|entries| !entries.is_empty()));
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
