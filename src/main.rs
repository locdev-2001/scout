mod cargo;
mod changes;
mod daemon;
mod cli;
mod driver;
mod freshness;
mod generic;
mod hook;
mod ipc;
mod lsp;
mod mcp;
mod output;
mod query;
mod resources;
mod slots;
mod trace;
mod types;

use query::QueryRequest;
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "Usage:
  scout locate <file:line>                 which symbol this line belongs to
  scout callers <symbol|file:line>         who calls it, walking back to entry points
    --depth N       maximum levels (default: unlimited)
    --max-nodes N   maximum symbols (default: unlimited)
    --tests         include callers inside tests
    --compact       short form for agents: direct callers with code, then every other
                    affected symbol grouped by file, then unresolved lines by file
  scout mcp                                Model Context Protocol server on stdio (for agents)
  scout doctor                             which language engines this machine can use
  scout setup                              install for the agents here: MCP (Claude Code, Codex), hooks,
                                               instructions, the `scout` command (--dry-run to preview)
  scout uninstall                          take all of that back
  scout status                             what is installed, which servers run
  scout stop [repo]                        stop the server of a repository, or all
  scout logs [repo]                        the end of the servers' logs
  scout update                             pull the tool's repository, rebuild, set up again
  scout --version
  scout hook before-change|after-change|before-search|prompt|finish|session-start
                                           Claude Code hook (reads the hook's JSON on stdin)
  scout claude-hooks install|remove        add or remove those hooks in ~/.claude/settings.json
  scout hold-slot                          for drivers: wait for a machine-wide work slot, hold it until stdin closes
                                               (SCOUT_MAX_WORKERS slots, default half the cores, 0 = no limit;
                                               none while less than SCOUT_MIN_AVAILABLE_MB of memory, default 1536)
    --dry-run       print the settings it would write
  --repo DIR        repository to inspect (default: the one containing the current directory)";

struct Options {
    command: String,
    query: String,
    repo: Option<PathBuf>,
    max_depth: usize,
    max_nodes: usize,
    include_tests: bool,
    compact: bool,
    dry_run: bool,
}

fn parse_options(args: Vec<String>) -> Result<Options, String> {
    let mut positionals = Vec::new();
    let mut options = Options { command: String::new(), query: String::new(), repo: None, max_depth: usize::MAX, max_nodes: usize::MAX, include_tests: false, compact: false, dry_run: false };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--repo" => options.repo = Some(PathBuf::from(value("--repo")?)),
            "--depth" => options.max_depth = value("--depth")?.parse().map_err(|_| "--depth must be a number")?,
            "--max-nodes" => options.max_nodes = value("--max-nodes")?.parse().map_err(|_| "--max-nodes must be a number")?,
            "--tests" => options.include_tests = true,
            "--compact" => options.compact = true,
            "--dry-run" => options.dry_run = true,
            "-h" | "--help" => return Err(String::new()),
            _ => positionals.push(arg),
        }
    }
    match positionals.as_slice() {
        [command, repo] if command == "driver-generic" || command == "driver-lsp" => {
            options.command = command.clone();
            options.query = repo.clone();
            Ok(options)
        }
        [command] if ["mcp", "doctor", "setup", "uninstall", "status", "stop", "logs", "update", "hold-slot", "--version"].contains(&command.as_str()) => {
            options.command = command.clone();
            Ok(options)
        }
        [command, query] if ["locate", "callers", "serve", "hook", "claude-hooks", "stop", "logs"].contains(&command.as_str()) => {
            options.command = command.clone();
            options.query = query.clone();
            Ok(options)
        }
        _ => Err(String::new()),
    }
}

fn find_repo_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|directory| directory.join(".git").exists()).map(Path::to_path_buf)
}

fn drivers_directory() -> PathBuf {
    match env::var_os("SCOUT_HOME") {
        Some(home) => PathBuf::from(home).join("drivers"),
        None => daemon::own_binary().ok().and_then(|exe| exe.parent().map(|bin| bin.join("../../drivers"))).unwrap_or_default(),
    }
}

fn run(options: Options) -> Result<u8, String> {
    if options.command == "driver-generic" {
        let claimed = env::var("SCOUT_CLAIMED_EXTENSIONS").unwrap_or_default();
        generic::serve(Path::new(&options.query), claimed.split(',').filter(|extension| !extension.is_empty()).map(str::to_owned).collect());
        return Ok(0);
    }
    if options.command == "driver-lsp" {
        lsp::serve(Path::new(&options.query));
        return Ok(0);
    }
    if options.command == "mcp" {
        mcp::serve();
        return Ok(0);
    }
    let argument = (!options.query.is_empty()).then_some(options.query.as_str());
    match options.command.as_str() {
        "--version" => {
            println!("scout {}", env!("CARGO_PKG_VERSION"));
            return Ok(0);
        }
        "setup" => return cli::setup(options.dry_run),
        "uninstall" => return cli::uninstall(options.dry_run),
        "status" => return cli::status(),
        "stop" => return cli::stop(argument, options.dry_run),
        "logs" => return cli::logs(argument),
        "update" => return cli::update(),
        "hold-slot" => return Ok(slots::hold()),
        _ => {}
    }
    if options.command == "hook" {
        return Ok(hook::run(&options.query, options.dry_run));
    }
    if options.command == "claude-hooks" {
        return hook::configure(&options.query, options.dry_run);
    }
    if options.command == "doctor" {
        return doctor(options.repo);
    }
    if options.command == "serve" {
        daemon::serve(&drivers_directory(), Path::new(&options.query))?;
        return Ok(0);
    }
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    let query_file = options.query.rsplit_once(':').map_or(options.query.as_str(), |(file, _)| file);
    let search_start = match &options.repo {
        Some(repo) => repo.clone(),
        None if cwd.join(query_file).is_file() => cwd.join(query_file).parent().unwrap().to_path_buf(),
        None => cwd.clone(),
    };
    let repo_root = find_repo_root(&search_start.canonicalize().map_err(|error| format!("{}: {error}", search_start.display()))?)
        .ok_or("No repository found: run inside one or pass --repo.")?;

    let request = QueryRequest {
        command: options.command,
        query: options.query,
        cwd,
        max_depth: options.max_depth,
        max_nodes: options.max_nodes,
        include_tests: options.include_tests,
        compact: options.compact,
    };
    let response = daemon::query(&repo_root, &request)?;
    print!("{}", response.stdout);
    eprint!("{}", response.stderr);
    let _ = std::io::stdout().flush();
    Ok(response.exit_code)
}

/// Starts the drivers that match the repo (or the current directory) and prints what each engine can do here.
fn doctor(repo: Option<PathBuf>) -> Result<u8, String> {
    let start = repo.unwrap_or(env::current_dir().map_err(|error| error.to_string())?);
    let repo_root = find_repo_root(&start.canonicalize().map_err(|error| error.to_string())?).ok_or("run inside a repository or pass --repo")?;
    println!("repository: {}", repo_root.display());
    let mut pool = driver::DriverPool::start(&drivers_directory(), &repo_root)?;
    for answer in pool.ask("probe", serde_json::json!({})) {
        println!("{}", serde_json::to_string_pretty(&*answer).unwrap_or_default());
    }
    for warning in &pool.warnings {
        println!("warning: {warning}");
    }
    Ok(0)
}

fn main() -> ExitCode {
    let options = match parse_options(env::args().skip(1).collect()) {
        Ok(options) => options,
        Err(message) => {
            if !message.is_empty() {
                eprintln!("{message}");
            }
            println!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(options) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}
