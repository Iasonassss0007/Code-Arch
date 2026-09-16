//! `codearch` — the CLI.
//!
//! The whole intended experience is one command:
//!
//! ```text
//! $ codearch .
//! ```

use anyhow::Result;
use clap::{Parser, Subcommand};
use codearch::{query, Options, DEFAULT_BUDGET, DEFAULT_MAX_DOMAINS};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "codearch",
    about = "Analyze a repository locally and write a compact map for coding agents",
    version,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Repository root to analyze.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Also build the CODEBASE.md overview and index.json. Without it, only
    /// the lookup indexes are written.
    #[arg(long)]
    map: bool,

    /// Where to write the map (default: <repo>/CODEBASE.md).
    #[arg(short, long, requires = "map")]
    out: Option<PathBuf>,

    /// Hard token budget for the generated map.
    #[arg(short, long, default_value_t = DEFAULT_BUDGET, requires = "map")]
    budget: usize,

    /// Maximum number of top-level domains.
    #[arg(long, default_value_t = DEFAULT_MAX_DOMAINS, requires = "map")]
    max_domains: usize,

    /// Clustering seed. Changing it reshuffles tie-breaks, nothing else.
    #[arg(long, default_value_t = 0x5EED, requires = "map")]
    seed: u64,

    /// Skip writing .codearch/index.json.
    #[arg(long, requires = "map")]
    no_index: bool,

    /// Ignore git history: no co-change edges, no churn term.
    #[arg(long, requires = "map")]
    no_git: bool,

    /// Where the indexes (and index.json) are written (default: <repo>/.codearch).
    #[arg(long)]
    codearch_dir: Option<PathBuf>,

    /// Look up the index instead of analyzing: `importers` serves the
    /// reverse import index, `callers` the route index. A directory
    /// literally named `importers` or `callers` still analyzes as
    /// `codearch ./importers`.
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Every file importing FILE, directly (depth 1) or transitively.
    Importers {
        /// File to look up, as the agent has it (`src/a.ts`, `./src/a.ts`,
        /// `src\a.ts`, or an absolute path inside the repo).
        file: String,
        /// Deepest hop to report (default: no limit).
        #[arg(long)]
        depth: Option<usize>,
        /// Emit one JSON object instead of `depth  path` lines.
        #[arg(long)]
        json: bool,
        /// Repository root (default: `.`).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Where imports.md is read from (default: <repo>/.codearch).
        #[arg(long)]
        codearch_dir: Option<PathBuf>,
    },
    /// Frontend files calling a backend view, from the route index.
    Callers {
        /// View name, e.g. `TagViewSet`.
        view: String,
        /// Emit one JSON object instead of text blocks.
        #[arg(long)]
        json: bool,
        /// Repository root (default: `.`).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Where routes.md is read from (default: <repo>/.codearch).
        #[arg(long)]
        codearch_dir: Option<PathBuf>,
    },
    /// Serve the two lookups as MCP tools over stdio.
    Mcp {
        /// Repository root (default: `.`).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Where the index files are read from (default: <repo>/.codearch).
        #[arg(long)]
        codearch_dir: Option<PathBuf>,
    },
    /// Print the block that tells agents these lookups exist, or merge it
    /// into a file they read (e.g. AGENTS.md) with --write.
    Agents {
        /// File to create or update, relative to --repo. Only the
        /// codearch-marked block is replaced; other content is kept.
        #[arg(long)]
        write: Option<PathBuf>,
        /// Repository root (default: `.`).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Where the index files are read from (default: <repo>/.codearch).
        #[arg(long)]
        codearch_dir: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(command) = &cli.command {
        std::process::exit(run_query(command));
    }

    let opts = Options {
        root: cli.path,
        out: cli.out,
        budget: cli.budget,
        max_domains: cli.max_domains,
        seed: cli.seed,
        write_index: !cli.no_index,
        no_git: cli.no_git,
        codearch_dir: cli.codearch_dir,
        map: cli.map,
    };

    println!("Analyzing repository...");
    let report = codearch::run(&opts)?;

    if !report.map {
        print_index_report(&report, &opts);
        return Ok(());
    }

    println!();
    println!("Source files:        {}", report.files);
    println!("Lines:               {}", report.loc);
    println!(
        "Est. source context: {} tokens",
        report.source_tokens_estimate
    );
    println!(
        "Import resolution:   {:.0}%{}",
        report.resolution_rate * 100.0,
        if report.unresolved > 0 {
            format!(" ({} unresolved)", report.unresolved)
        } else {
            String::new()
        }
    );
    println!(
        "Git co-change:       {}",
        if report.commits_read == 0 {
            "none (no usable history)".to_string()
        } else {
            format!(
                "{} pairs from {} commits",
                report.cochange_pairs, report.commits_read
            )
        }
    );
    println!("Domains:             {}", report.domains);
    if report.split {
        println!("Split:               root routes to domain files");
    }
    if report.contracts > 0 {
        println!(
            "API contracts:       {} URL, {} symbol shapes",
            report.url_contracts, report.semantic_contracts
        );
    }
    println!(
        "Flows:               {}",
        if report.flows_dropped {
            format!("{} computed, dropped over budget", report.flows)
        } else {
            format!("{}", report.flows)
        }
    );
    println!(
        "Confidence:          {:.2}{}",
        report.confidence,
        if report.low_confidence_domains > 0 {
            format!(" ({} low-confidence domains)", report.low_confidence_domains)
        } else {
            String::new()
        }
    );
    println!();
    println!("Generated:");
    if let Some(p) = &report.out_path {
        println!("  {}", p.display());
    }
    if let Some(p) = &report.index_path {
        println!("  {}", p.display());
    }
    println!("  {}", report.imports_path.display());
    if let Some(p) = &report.routes_path {
        println!("  {} ({} views with frontend callers)", p.display(), report.route_views);
    }
    println!();
    println!("Generated context:   {} tokens", report.map_tokens);
    println!(
        "Import index:        {} imports, {} tokens, read on demand",
        report.import_edges, report.imports_tokens
    );

    if report.used_directory_fallback {
        println!();
        println!(
            "Note: no import edges were resolved, so domains fall back to \
directory structure. Treat the grouping as layout, not architecture."
        );
    }
    if report.over_domain_cap {
        println!();
        println!(
            "Note: {} domains exceeds the cap of {}. The remaining groups share no \
imports, so merging them further would assert a relationship the code does not have.",
            report.domains, opts.max_domains
        );
    }
    if report.truncated {
        println!();
        println!("Note: the file list per domain was shortened to fit the token budget.");
    }

    Ok(())
}

/// The default run's summary: what the lookups can see, and what to do next.
fn print_index_report(report: &codearch::RunReport, opts: &Options) {
    println!();
    println!("Source files:        {}", report.files);
    println!(
        "Import resolution:   {:.0}%{}",
        report.resolution_rate * 100.0,
        if report.unresolved > 0 {
            format!(" ({} unresolved)", report.unresolved)
        } else {
            String::new()
        }
    );
    println!("Import index:        {} imports", report.import_edges);
    println!(
        "Route callers:       {}",
        if report.route_views > 0 {
            format!("{} backend views with frontend callers", report.route_views)
        } else {
            "none found".to_string()
        }
    );
    println!();
    println!("Wrote:");
    println!("  {}", report.imports_path.display());
    if let Some(p) = &report.routes_path {
        println!("  {}", p.display());
    }
    println!();
    println!("Next:");
    println!("  codearch importers <file>             who depends on a file");
    if report.routes_path.is_some() {
        println!("  codearch callers <View>               which frontend files call a backend view");
    }
    println!("  codearch agents --write AGENTS.md     tell your agents about these lookups");
    println!("  codearch --map                        also write the CODEBASE.md overview");

    // An older run's map is left alone, but it no longer tracks the code.
    let root = opts.root.canonicalize().unwrap_or_else(|_| opts.root.clone());
    let old_map = root.join("CODEBASE.md");
    let generated = std::fs::read_to_string(&old_map)
        .map(|t| t.starts_with("# Codebase Map") && t.contains("Generated by Code Arch"))
        .unwrap_or(false);
    if generated {
        println!();
        println!(
            "Note: CODEBASE.md is from an earlier run and is no longer updated; re-run with \
--map to refresh it, or delete it."
        );
    }
}

/// Answer one index lookup. Returns the process exit code: 0 for answers
/// and clean misses, 2 for a missing index, an out-of-repo path, or bad
/// arguments. Queries never write anything and never re-analyze.
fn run_query(command: &Commands) -> i32 {
    match command {
        Commands::Importers {
            file,
            depth,
            json,
            repo,
            codearch_dir,
        } => {
            let repo = repo.clone().unwrap_or_else(|| PathBuf::from("."));
            let dir = codearch_dir.clone().unwrap_or_else(|| repo.join(".codearch"));
            let text = match query::index_text(&dir, "imports.md", &repo.display().to_string()) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("{e}");
                    return 2;
                }
            };
            let target = match query::normalize_target(&repo, file) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("{e}");
                    return 2;
                }
            };
            let hits = query::importers(&query::parse_imports(&text), &target, *depth);
            if hits.is_empty() {
                eprintln!(
                    "no importers for '{target}': nothing imports it, or it was not analyzed"
                );
                if *json {
                    println!("{}", query::importers_json(&target, &hits));
                }
                return 0;
            }
            if *json {
                println!("{}", query::importers_json(&target, &hits));
            } else {
                print!("{}", query::importers_text(&hits));
            }
            if let Some(mins) = query::index_age_minutes(&dir, "imports.md") {
                eprintln!("index written {mins} minutes ago");
            }
            0
        }
        Commands::Callers {
            view,
            json,
            repo,
            codearch_dir,
        } => {
            let repo = repo.clone().unwrap_or_else(|| PathBuf::from("."));
            let dir = codearch_dir.clone().unwrap_or_else(|| repo.join(".codearch"));
            let text = match query::index_text(&dir, "routes.md", &repo.display().to_string()) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("{e}");
                    return 2;
                }
            };
            let index = query::parse_routes(&text);
            let matches = index.get(view.as_str()).cloned().unwrap_or_default();
            if matches.is_empty() {
                let mut reason = format!("no route in the index names this view '{view}'");
                let suggestions = query::suggest_views(&index, view);
                if !suggestions.is_empty() {
                    reason.push_str(&format!("; similar indexed views: {}", suggestions.join(", ")));
                }
                eprintln!("{reason}");
                if *json {
                    println!("{}", query::callers_json(view, &matches));
                }
                return 0;
            }
            if *json {
                println!("{}", query::callers_json(view, &matches));
            } else {
                print!("{}", query::callers_text(&matches));
            }
            if let Some(mins) = query::index_age_minutes(&dir, "routes.md") {
                eprintln!("index written {mins} minutes ago");
            }
            0
        }
        Commands::Mcp { repo, codearch_dir } => {
            let repo = repo.clone().unwrap_or_else(|| PathBuf::from("."));
            let server = codearch::mcp::Server::new(repo, codearch_dir.clone());
            let stdin = std::io::stdin();
            server.serve(
                std::io::BufReader::new(stdin.lock()),
                std::io::stdout(),
            );
            0
        }
        Commands::Agents {
            write,
            repo,
            codearch_dir,
        } => {
            let repo = repo.clone().unwrap_or_else(|| PathBuf::from("."));
            let dir = codearch_dir.clone().unwrap_or_else(|| repo.join(".codearch"));
            // The block advertises lookups; without an index they cannot answer.
            if let Err(e) = query::index_text(&dir, "imports.md", &repo.display().to_string()) {
                eprintln!("{e}");
                return 2;
            }
            let block = codearch::agents::block(dir.join("routes.md").is_file());
            let Some(target) = write else {
                println!("{block}");
                return 0;
            };
            let path = repo.join(target);
            let existing = match std::fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    eprintln!("cannot read {}: {e}", path.display());
                    return 2;
                }
            };
            let merged = match codearch::agents::merge(existing.as_deref(), &block) {
                Ok(text) => text,
                Err(e) => {
                    eprintln!("{}: {e}", path.display());
                    return 2;
                }
            };
            if existing.as_deref() == Some(merged.as_str()) {
                println!("{} already up to date", path.display());
                return 0;
            }
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    eprintln!("cannot create {}: {e}", parent.display());
                    return 2;
                }
            }
            if let Err(e) = std::fs::write(&path, merged) {
                eprintln!("cannot write {}: {e}", path.display());
                return 2;
            }
            println!(
                "{} {}",
                if existing.is_some() { "Updated" } else { "Created" },
                path.display()
            );
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        let mut full = vec!["codearch"];
        full.extend(args);
        Cli::try_parse_from(full).expect("parses")
    }

    #[test]
    fn analysis_invocations_still_parse_as_analysis() {
        assert!(cli(&["."]).command.is_none());
        assert!(!cli(&["."]).map, "indexes only by default");
        assert_eq!(cli(&["src"]).path, PathBuf::from("src"));
        assert!(cli(&["--map", "--no-git", "eval/fixtures/xlang"]).command.is_none());
        assert!(cli(&["--map", "--budget", "100", "."]).map);
        assert!(cli(&["--codearch-dir", "state", "."]).command.is_none());
    }

    #[test]
    fn map_only_options_require_map() {
        for args in [
            vec!["codearch", "--budget", "100", "."],
            vec!["codearch", "--no-git", "."],
            vec!["codearch", "--out", "m.md", "."],
        ] {
            let err = Cli::try_parse_from(&args).err().expect("rejected without --map");
            assert!(err.to_string().contains("--map"), "{args:?}: {err}");
        }
    }

    #[test]
    fn agents_subcommand_parses() {
        match cli(&["agents", "--write", "AGENTS.md"]).command {
            Some(Commands::Agents { write, .. }) => {
                assert_eq!(write, Some(PathBuf::from("AGENTS.md")));
            }
            other => panic!("unexpected parse: {other:?}"),
        }
        assert!(matches!(cli(&["agents"]).command, Some(Commands::Agents { write: None, .. })));
    }

    #[test]
    fn query_subcommands_parse_with_their_flags() {
        match cli(&["importers", "src/a.ts"]).command {
            Some(Commands::Importers { file, depth, json, .. }) => {
                assert_eq!(file, "src/a.ts");
                assert_eq!(depth, None);
                assert!(!json);
            }
            other => panic!("unexpected parse: {other:?}"),
        }
        match cli(&["callers", "TagViewSet", "--json"]).command {
            Some(Commands::Callers { view, json, .. }) => {
                assert_eq!(view, "TagViewSet");
                assert!(json);
            }
            other => panic!("unexpected parse: {other:?}"),
        }
    }

    #[test]
    fn dotted_directory_names_still_analyze() {
        // A directory literally named `importers` is a subcommand spelling;
        // `./importers` is the documented way to analyze it instead.
        assert!(cli(&["importers", "src/a.ts"]).command.is_some());
        let dotted = cli(&["./importers"]);
        assert!(dotted.command.is_none());
        assert_eq!(dotted.path, PathBuf::from("./importers"));
    }
}
