//! Stdio MCP server over the same planner the CLI uses.
use crate::inventory::Config;
use crate::plan::{self, Options, Plan};
use anyhow::Result;
use rmcp::{
    ServiceExt, handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio,
};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone)]
struct HarborMcp;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Scope {
    /// Repository checkouts to plan across (any path inside each repository).
    repos: Vec<String>,
    /// Extra harbor-mcp.toml applied on top of each repository's own.
    config: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct PlanArgs {
    #[serde(flatten)]
    scope: Scope,
    /// Run cargo-semver-checks and griffe against each release baseline (default true).
    api_checks: Option<bool>,
    /// Plan publishable units even though no API check compares them.
    allow_unchecked_publishable: Option<bool>,
    /// Revision to plan from (the previous release) instead of the detected
    /// last release.
    base: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ApplyArgs {
    /// A plan returned by `plan`, unmodified.
    plan: serde_json::Value,
    /// Write the files; false (the default) only lists the edits.
    write: Option<bool>,
}

async fn run(scope: Scope, opts: Options) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let config = scope
            .config
            .map(|c| Config::load(c.as_ref()))
            .transpose()?
            .unwrap_or_default();
        let repos: Vec<PathBuf> = scope.repos.iter().map(PathBuf::from).collect();
        Ok::<_, anyhow::Error>(serde_json::to_string(&plan::plan(&repos, &config, &opts)?)?)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("{e:#}"))
}

#[tool_router(server_handler)]
impl HarborMcp {
    #[tool(
        description = "Release units across repositories: packages, current versions, release baselines, lockstep units, internal dependencies, anchors and metadata drift. No commit history or API checks."
    )]
    async fn inventory(&self, Parameters(a): Parameters<Scope>) -> Result<String, String> {
        run(
            a,
            Options {
                classify: false,
                api_checks: false,
                base: None,
                allow_unchecked_publishable: false,
            },
        )
        .await
    }

    #[tool(
        description = "Semver bump plan: classifies each package's changes since its last release (Conventional Commits, cargo-semver-checks, griffe), applies pre-1.0 rules, propagates bumps to dependents, orders packages dependencies-first, and lists requirement edits and the steps other tools own. Optional `base` plans from an explicit revision instead. Read-only."
    )]
    async fn plan(&self, Parameters(a): Parameters<PlanArgs>) -> Result<String, String> {
        run(
            a.scope,
            Options {
                classify: true,
                api_checks: a.api_checks.unwrap_or(true),
                base: a.base,
                allow_unchecked_publishable: a.allow_unchecked_publishable.unwrap_or(false),
            },
        )
        .await
    }

    #[tool(
        description = "Write a plan's version, requirement and anchor edits into the working trees. Refuses if a repository moved past the plan or a touched file is dirty. Never commits, tags or publishes."
    )]
    async fn apply(&self, Parameters(a): Parameters<ApplyArgs>) -> Result<String, String> {
        let write = a.write.unwrap_or(false);
        tokio::task::spawn_blocking(move || {
            let plan: Plan = serde_json::from_value(a.plan)?;
            Ok::<_, anyhow::Error>(serde_json::to_string(&crate::apply::apply(&plan, write)?)?)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
    }
}

pub async fn serve() -> Result<()> {
    HarborMcp.serve(stdio()).await?.waiting().await?;
    Ok(())
}
