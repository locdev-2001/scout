//! Managing the tool itself: install it for the agents on this machine, see what runs, stop it, update it.

use crate::daemon;
use crate::hook;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What agents are told about this tool, in their global instructions, between these markers.
const BEGIN: &str = "<!-- scout: begin -->";
const END: &str = "<!-- scout: end -->";
const INSTRUCTIONS: &str = "## scout: who calls this code, what a change affects

- scout answers one question: what calls this function, method or class, up to the entry points (routes, commands, jobs, events). It follows types, imports, inheritance and framework wiring. It is not a text search: find text, definitions, config keys and log lines with grep/rg as usual.
- Before changing, renaming or deleting a function, method or class, run `scout callers <Class::method | name | path:line> --compact` (MCP tool `callers`). Read the `unresolved` lines too: they are calls the tool could not type, listed so none is missed.
- For a stack-trace line, `scout locate path:line` names the function it is in.
- The hooks bring callers without being asked: with a prompt naming a stack-trace line or `Class::method`, before an Edit (the function being edited), on a grep for a function name, after any change (edit, write, script), and at the end of a turn for changed functions not looked at yet. Check each caller still fits the change.";

/// The skill Claude Code lists in every session, so an agent finds the tool when the task calls for it.
const SKILL: &str = include_str!("skill.md");

fn skill_path(home: &Path) -> PathBuf {
    home.join(".claude/skills/scout/SKILL.md")
}

/// Writes the skill (or takes it out). A `scout` skill this tool did not write is left alone.
fn write_skill(home: &Path, remove: bool) -> Result<(), String> {
    let path = skill_path(home);
    let current = fs::read_to_string(&path).ok();
    if current.as_deref().is_some_and(|text| !text.contains("# scout: who calls this code")) {
        return Err(format!("{} is not this tool's", path.display()));
    }
    if remove {
        return match current {
            Some(_) => fs::remove_dir_all(path.parent().unwrap()).map_err(|error| error.to_string()),
            None => Ok(()),
        };
    }
    if current.as_deref() == Some(SKILL) {
        return Ok(());
    }
    fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    fs::write(&path, SKILL).map_err(|error| error.to_string())
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).ok_or_else(|| "no home directory".to_owned())
}

/// The command agents should run: the wrapper at the repository root (it rebuilds after an update), else the binary.
pub fn launcher() -> Result<PathBuf, String> {
    let binary = daemon::own_binary().map_err(|error| error.to_string())?;
    Ok(tool_root().map(|root| root.join("scout")).filter(|wrapper| wrapper.is_file()).unwrap_or(binary))
}

/// The tool's own repository: the binary is built into `<root>/target/release/`.
fn tool_root() -> Option<PathBuf> {
    let binary = daemon::own_binary().ok()?;
    binary.ancestors().nth(3).filter(|root| root.join("Cargo.toml").is_file() && root.join("drivers").is_dir()).map(Path::to_path_buf)
}

fn present(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok_and(|output| output.status.success())
}

/// Runs a step, or only says it with `--dry-run`.
fn step(dry_run: bool, description: &str, action: impl FnOnce() -> Result<(), String>) {
    if dry_run {
        println!("would {description}");
        return;
    }
    match action() {
        Ok(()) => println!("{description}: done"),
        Err(error) => println!("{description}: skipped ({error})"),
    }
}

fn run_quietly(program: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(program).args(args).output().map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().lines().last().unwrap_or("failed").to_owned())
    }
}

/// Puts the instructions block in a Markdown file (replacing an older one), or takes it out.
fn write_instructions(path: &Path, remove: bool) -> Result<(), String> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let without = match (text.find(BEGIN), text.find(END)) {
        (Some(start), Some(end)) if end > start => format!("{}{}", text[..start].trim_end(), &text[end + END.len()..]),
        _ => text.clone(),
    };
    let updated = if remove { without.trim_end().to_owned() + "\n" } else { format!("{}\n\n{BEGIN}\n{INSTRUCTIONS}\n{END}\n", without.trim_end()) };
    if updated == text {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, updated.trim_start()).map_err(|error| error.to_string())
}

/// `scout setup`: everything an agent on this machine needs, idempotent.
pub fn setup(dry_run: bool) -> Result<u8, String> {
    let launcher = launcher()?;
    let launcher_text = launcher.to_string_lossy().into_owned();
    let home = home()?;
    println!("scout: {launcher_text}");
    let link = home.join(".local/bin/scout");
    step(dry_run, &format!("link {} -> {launcher_text}", link.display()), || {
        fs::create_dir_all(link.parent().unwrap()).map_err(|error| error.to_string())?;
        let _ = fs::remove_file(&link);
        #[cfg(unix)]
        return std::os::unix::fs::symlink(&launcher, &link).map_err(|error| error.to_string());
        #[cfg(not(unix))]
        return fs::copy(&launcher, &link).map(|_| ()).map_err(|error| error.to_string());
    });
    // Parsers for JavaScript repositories that bring none of their own (`typescript`, `vue`, `svelte`).
    if let Some(root) = tool_root().filter(|_| present("npm")) {
        let javascript = root.join("drivers/javascript");
        if !javascript.join("node_modules/typescript").is_dir() {
            step(dry_run, "install the JavaScript parsers (npm)", || {
                run_quietly("npm", &["install", "--omit=dev", "--no-audit", "--no-fund", "--prefix", &javascript.to_string_lossy()])
            });
        }
    }
    if present("claude") {
        // Loaded with the session rather than deferred behind a tool search: an agent that must first look a tool up
        // reaches for grep instead.
        step(dry_run, "register the MCP server with Claude Code (always loaded)", || {
            let _ = run_quietly("claude", &["mcp", "remove", "--scope", "user", "scout"]);
            let server = serde_json::json!({ "type": "stdio", "command": launcher_text, "args": ["mcp"], "alwaysLoad": true });
            run_quietly("claude", &["mcp", "add-json", "--scope", "user", "scout", &server.to_string()])
        });
        step(dry_run, "add the hooks to Claude Code (~/.claude/settings.json)", || hook::configure("install", false).map(|_| ()));
        step(dry_run, "add the instructions to ~/.claude/CLAUDE.md", || write_instructions(&home.join(".claude/CLAUDE.md"), false));
        step(dry_run, &format!("add the skill {}", skill_path(&home).display()), || write_skill(&home, false));
    }
    if present("codex") {
        step(dry_run, "register the MCP server with Codex", || {
            let _ = run_quietly("codex", &["mcp", "remove", "scout"]);
            run_quietly("codex", &["mcp", "add", "scout", "--", &launcher_text, "mcp"])
        });
        step(dry_run, "add the instructions to ~/.codex/AGENTS.md", || write_instructions(&home.join(".codex/AGENTS.md"), false));
    }
    println!("Other agents: add an MCP stdio server with command `{launcher_text} mcp`.");
    println!("Language engines on this machine:");
    for (engine, purpose) in [("php", "PHP, Laravel"), ("node", "JavaScript, TypeScript, Vue, React, Svelte"), ("python3", "Python"), ("rust-analyzer", "Rust")] {
        println!("  {engine:<14} {:<9} {purpose}", if present(engine) { "found" } else { "missing" });
    }
    Ok(0)
}

/// `scout uninstall`: takes back everything setup added (the repository itself stays).
pub fn uninstall(dry_run: bool) -> Result<u8, String> {
    let home = home()?;
    stop(None, dry_run)?;
    step(dry_run, "remove ~/.local/bin/scout", || fs::remove_file(home.join(".local/bin/scout")).map_err(|error| error.to_string()));
    if present("claude") {
        step(dry_run, "unregister the MCP server from Claude Code", || run_quietly("claude", &["mcp", "remove", "--scope", "user", "scout"]));
        step(dry_run, "remove the hooks from Claude Code", || hook::configure("remove", false).map(|_| ()));
        step(dry_run, "remove the instructions from ~/.claude/CLAUDE.md", || write_instructions(&home.join(".claude/CLAUDE.md"), true));
        step(dry_run, &format!("remove the skill {}", skill_path(&home).display()), || write_skill(&home, true));
    }
    if present("codex") {
        step(dry_run, "unregister the MCP server from Codex", || run_quietly("codex", &["mcp", "remove", "scout"]));
        step(dry_run, "remove the instructions from ~/.codex/AGENTS.md", || write_instructions(&home.join(".codex/AGENTS.md"), true));
    }
    let cache = daemon::cache_directory();
    if cache.exists() {
        step(dry_run, &format!("remove the saved analysis in {}", cache.display()), || fs::remove_dir_all(&cache).map_err(|error| error.to_string()));
    }
    Ok(0)
}

fn folder_bytes(folder: &Path) -> u64 {
    fs::read_dir(folder).into_iter().flatten().filter_map(Result::ok).map(|entry| match entry.file_type() {
        Ok(kind) if kind.is_dir() => folder_bytes(&entry.path()),
        Ok(_) => entry.metadata().map(|metadata| metadata.len()).unwrap_or(0),
        Err(_) => 0,
    }).sum()
}

/// `scout status`: where it is installed and which servers run, with their memory.
pub fn status() -> Result<u8, String> {
    let home = home()?;
    println!("scout: {}", launcher()?.display());
    let contains = |path: PathBuf, needle: &str| fs::read_to_string(path).is_ok_and(|text| text.contains(needle));
    let json = |path: PathBuf| fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    // Claude Code also reads servers and hooks set for the repository the command runs in, not only those `setup` adds
    // for the user: each scope that has them is named.
    let repo = std::env::current_dir().ok().and_then(|directory| directory.ancestors().find(|directory| directory.join(".git").exists()).map(Path::to_path_buf));
    let claude = json(home.join(".claude.json"));
    // A server named scout, not any mention of scout (skill usage, project paths) elsewhere in the file.
    let mut mcp_scopes = Vec::new();
    if claude.as_ref().is_some_and(|config| config.pointer("/mcpServers/scout").is_some()) {
        mcp_scopes.push("user");
    }
    if let Some(repo) = &repo {
        if claude.as_ref().and_then(|config| config.get("projects")?.get(repo.to_string_lossy().as_ref())?.pointer("/mcpServers/scout")).is_some() {
            mcp_scopes.push("local");
        }
        if json(repo.join(".mcp.json")).is_some_and(|config| config.pointer("/mcpServers/scout").is_some()) {
            mcp_scopes.push("project .mcp.json");
        }
    }
    // Every setup installed the search hook; only the current one installs the hook before an edit.
    let (mut hook_scopes, mut current_hooks) = (Vec::new(), true);
    let settings_files = [("user", home.join(".claude/settings.json"))].into_iter().chain(repo.iter().flat_map(|repo| [".claude/settings.json", ".claude/settings.local.json"].map(|settings| (settings, repo.join(settings)))));
    for (scope, settings) in settings_files {
        if contains(settings.clone(), " hook before-search") {
            hook_scopes.push(scope);
            current_hooks &= contains(settings, " hook before-change");
        }
    }
    let scopes = |found: Vec<&str>, yes: &str, no: &str| if found.is_empty() { no.to_owned() } else { format!("{yes} ({})", found.join(", ")) };
    println!("  Claude Code MCP:   {}", scopes(mcp_scopes, "registered", "not registered"));
    println!("  Claude Code hooks: {}", scopes(hook_scopes, if current_hooks { "installed" } else { "out of date, run `scout setup`" }, "not installed"));
    let skill = fs::read_to_string(skill_path(&home)).ok();
    println!("  Claude Code skill: {}", match skill.as_deref() {
        Some(text) if text == SKILL => "installed",
        Some(_) => "out of date (run `scout setup`)",
        None => "not installed",
    });
    let always_loaded = claude.as_ref().and_then(|config| config.pointer("/mcpServers/scout/alwaysLoad")).and_then(serde_json::Value::as_bool).unwrap_or(false);
    println!("  MCP tools loaded:  {}", if always_loaded { "with every session" } else { "on demand (deferred; `scout setup` loads them with every session)" });
    println!("  Codex MCP:         {}", if contains(home.join(".codex/config.toml"), "mcp_servers.scout") { "registered" } else { "not registered" });
    let cache = daemon::cache_directory();
    println!("Saved analysis:    {} MB in {}", folder_bytes(&cache) / 1_048_576, cache.display());
    let running = daemon::running_servers();
    let spent: Vec<u64> = running.iter().map(|server| crate::resources::tree_memory_mb(server.pid)).collect();
    println!("Servers running: {} ({} MB with their drivers; budget {})", running.len(), spent.iter().sum::<u64>(), match daemon::memory_budget_mb() {
        0 => "none".to_owned(),
        budget => format!("{budget} MB"),
    });
    for (server, memory) in running.iter().zip(spent) {
        println!("  {:>8}  {}  ({memory} MB)", server.pid, server.repo);
    }
    let (busy, slots) = crate::slots::usage();
    if slots > 0 {
        println!("Work slots:        {busy} of {slots} in use");
    }
    let usage = crate::hook::usage_by_day(7);
    if !usage.is_empty() {
        println!("Hooks, last 7 days (UTC):");
        for line in usage {
            println!("  {line}");
        }
    }
    Ok(0)
}

/// `scout stop [repo]`: ends the server of one repository, or all of them (they start again when asked).
pub fn stop(repo: Option<&str>, dry_run: bool) -> Result<u8, String> {
    let wanted = repo.map(|repo| fs::canonicalize(repo).map(|path| path.to_string_lossy().into_owned()).unwrap_or_else(|_| repo.to_owned()));
    for server in daemon::running_servers().into_iter().filter(|server| wanted.as_ref().is_none_or(|wanted| &server.repo == wanted)) {
        step(dry_run, &format!("stop the server of {} (pid {})", server.repo, server.pid), || {
            daemon::stop_server(&server);
            Ok(())
        });
    }
    Ok(0)
}

/// `scout logs [repo]`: the end of each server's log (a hang names the question and the processes there).
pub fn logs(repo: Option<&str>) -> Result<u8, String> {
    for server in daemon::running_servers().into_iter().filter(|server| repo.is_none_or(|repo| server.repo.contains(repo))) {
        println!("== {} ({})", server.repo, server.log.display());
        let text = fs::read_to_string(&server.log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        for line in &lines[lines.len().saturating_sub(30)..] {
            println!("{line}");
        }
    }
    Ok(0)
}

/// `scout update`: newest code of the tool's repository, rebuilt, servers restarted on it.
pub fn update() -> Result<u8, String> {
    let root = tool_root().ok_or("not running from the tool's repository")?;
    let root_text = root.to_string_lossy().into_owned();
    run_quietly("git", &["-C", &root_text, "pull", "--ff-only"])?;
    println!("pulled {root_text}");
    let status = Command::new(root.join("scout")).arg("--version").status().map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("the rebuild failed".to_owned());
    }
    stop(None, false)?;
    setup(false)
}
