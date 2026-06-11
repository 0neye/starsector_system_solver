//! Standalone save-extraction CLI. Thin wrapper over `extract::cli`; the same
//! subcommands are available as `system_solver extract ...` in the main binary.

use clap::Parser;

use system_solver::extract::cli::{run, ExtractCommand};

#[derive(Parser, Debug)]
#[command(name = "extract", about = "Starsector save extraction tool")]
struct Cli {
    #[command(subcommand)]
    command: ExtractCommand,
}

fn main() {
    system_solver::cpu_affinity::prefer_performance_cores();

    let cli = Cli::parse();
    if let Err(err) = run(cli.command) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
