mod apply;
mod classify;
mod inventory;
mod mcp;
mod plan;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Plan and apply semver-correct, dependency-ordered version bumps.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct Scope {
    /// Repository to include; repeat for an ecosystem-wide plan.
    #[arg(long = "repo", default_value = ".")]
    repos: Vec<PathBuf>,
    /// Extra harbor-mcp.toml applied on top of each repository's own.
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// List release units, versions, baselines and drift.
    Inventory(Scope),
    /// Compute the bump plan (read-only).
    Plan {
        #[command(flatten)]
        scope: Scope,
        /// Skip cargo-semver-checks and griffe.
        #[arg(long)]
        no_api_checks: bool,
    },
    /// Write the version edits of a plan produced by `plan --json`.
    Apply {
        plan: PathBuf,
        /// List the edits without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Serve the MCP tools over stdio.
    Mcp,
}

fn run(scope: &Scope, opts: plan::Options) -> Result<()> {
    let config = scope
        .config
        .as_deref()
        .map(inventory::Config::load)
        .transpose()?
        .unwrap_or_default();
    let plan = plan::plan(&scope.repos, &config, &opts)?;
    if scope.json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        print!("{}", plan::render(&plan));
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Inventory(scope) => run(
            &scope,
            plan::Options {
                classify: false,
                api_checks: false,
            },
        ),
        Command::Plan {
            scope,
            no_api_checks,
        } => run(
            &scope,
            plan::Options {
                classify: true,
                api_checks: !no_api_checks,
            },
        ),
        Command::Apply { plan, dry_run } => {
            let plan: plan::Plan = serde_json::from_str(&std::fs::read_to_string(plan)?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&apply::apply(&plan, !dry_run)?)?
            );
            Ok(())
        }
        Command::Mcp => mcp::serve().await,
    }
}
