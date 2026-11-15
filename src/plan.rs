//! Ecosystem-wide bump plan: classify each release unit, propagate bumps to
//! dependents until nothing changes, and order units dependencies-first.

use crate::classify::{self, Baseline, Level};
use crate::inventory::{self, Config, Ecosystem, Finding, Location, Repo, anchor_value, show};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr;

pub const SCHEMA: &str = "harbor-mcp.plan.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Released before and changed since: take `next`.
    Bump,
    /// Lockstep member whose declared version lags its unit: write `next`.
    Align,
    /// Never released: the first release takes the declared version.
    Initial,
    Unchanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorEntry {
    pub file: String,
    pub key: String,
    pub current: Option<String>,
    pub next: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub repo: usize,
    pub manifest: String,
    pub ecosystem: Ecosystem,
    pub publish: bool,
    /// An API checker compared the package against its baseline.
    #[serde(default)]
    pub api_checked: bool,
    /// Why no API check compared this package, when one could have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_unchecked: Option<String>,
    pub current: String,
    pub next: String,
    pub action: Action,
    /// Strongest change found (semver meaning, before pre-1.0 mapping).
    pub level: Level,
    /// Version component that moves: major, minor, patch, release or none.
    pub bump: String,
    pub baseline: Baseline,
    /// Release unit: the package whose version this one shares.
    pub unit: String,
    pub version_source: Option<Location>,
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<AnchorEntry>,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Requirement {
    pub package: String,
    pub dependency: String,
    pub at: Location,
    pub req: String,
    pub next: String,
    /// Rewritten requirement; `None` needs a manual edit.
    pub replacement: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub schema: String,
    pub repos: Vec<Repo>,
    /// Dependencies first.
    pub packages: Vec<Entry>,
    pub requirements: Vec<Requirement>,
    pub findings: Vec<Finding>,
    /// Steps harbor-mcp leaves to the tools that own them.
    pub delegations: Vec<String>,
}

pub struct Options {
    /// Read commits and run API checks; false gives the cheap inventory.
    pub classify: bool,
    pub api_checks: bool,
    /// Revision to plan from instead of each package's last release.
    pub base: Option<String>,
    /// Plan publishable units even though no API check compares them.
    pub allow_unchecked_publishable: bool,
}

/// Next version for a change of `level`, under Cargo's pre-1.0 rules:
/// in 0.y.z a breaking change moves y and anything else moves z; in 0.0.z
/// every change moves z. A prerelease is promoted to its release.
pub fn next_version(current: &str, level: Level) -> Option<(String, &'static str)> {
    let padded = match current.split('.').count() {
        1 => format!("{current}.0.0"),
        2 => format!("{current}.0"),
        _ => current.to_owned(),
    };
    let mut v = semver::Version::parse(&padded).ok()?;
    if level == Level::None {
        return Some((current.to_owned(), "none"));
    }
    v.build = semver::BuildMetadata::EMPTY;
    if !v.pre.is_empty() {
        v.pre = semver::Prerelease::EMPTY;
        return Some((v.to_string(), "release"));
    }
    let bump = match (v.major, v.minor, level) {
        (0, 0, _) => "patch",
        (0, _, Level::Major) => "minor",
        (0, _, _) => "patch",
        (_, _, Level::Major) => "major",
        (_, _, Level::Minor) => "minor",
        _ => "patch",
    };
    match bump {
        "major" => (v.major, v.minor, v.patch) = (v.major + 1, 0, 0),
        "minor" => (v.minor, v.patch) = (v.minor + 1, 0),
        _ => v.patch += 1,
    }
    Some((v.to_string(), bump))
}

fn satisfies(ecosystem: Ecosystem, req: &str, version: &str) -> Option<bool> {
    match ecosystem {
        Ecosystem::Cargo => Some(
            semver::VersionReq::parse(req)
                .ok()?
                .matches(&semver::Version::parse(version).ok()?),
        ),
        Ecosystem::Python => {
            use pep508_rs::pep440_rs::{Version, VersionSpecifiers};
            Some(
                VersionSpecifiers::from_str(req)
                    .ok()?
                    .contains(&Version::from_str(version).ok()?),
            )
        }
    }
}

fn find(leader: &mut [usize], mut i: usize) -> usize {
    while leader[i] != i {
        leader[i] = leader[leader[i]];
        i = leader[i];
    }
    i
}

pub fn plan(paths: &[PathBuf], extra: &Config, opts: &Options) -> Result<Plan> {
    let ws = inventory::load(paths, extra)?;
    let pkgs = &ws.packages;
    let n = pkgs.len();
    let mut findings = ws.findings.clone();
    let index: BTreeMap<&str, usize> = pkgs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect();
    let deps: Vec<Vec<usize>> = pkgs
        .iter()
        .map(|p| {
            let runtime = p.deps.iter().filter(|d| !d.dev);
            runtime.map(|d| index[d.id.as_str()]).collect()
        })
        .collect();

    // Release units: packages sharing a version source, or following another.
    let mut leader: Vec<usize> = (0..n).collect();
    let mut by_source = BTreeMap::new();
    for (i, p) in pkgs.iter().enumerate() {
        if let Some(source) = &p.version_source {
            let first = *by_source.entry((p.repo, source.clone())).or_insert(i);
            let root = find(&mut leader, first);
            leader[i] = root;
        }
    }
    for (i, p) in pkgs.iter().enumerate() {
        let Some(follow) = ws.config.package(&p.id).and_then(|c| c.follow.as_deref()) else {
            continue;
        };
        match index.get(follow) {
            Some(&j) => {
                let (a, b) = (find(&mut leader, i), find(&mut leader, j));
                if a != b {
                    leader[a] = b;
                }
            }
            None => findings.push(Finding {
                subject: p.id.clone(),
                message: format!("follows unknown package {follow}"),
            }),
        }
    }
    let unit: Vec<usize> = (0..n).map(|i| find(&mut leader, i)).collect();

    // Repository-wide tags (`vX`) apply when a repository has one version.
    let mut sources_per_repo: BTreeMap<usize, BTreeSet<&Location>> = BTreeMap::new();
    for p in pkgs {
        if let Some(source) = &p.version_source {
            sources_per_repo.entry(p.repo).or_default().insert(source);
        }
    }
    // An explicit base moves the comparison point of released packages only:
    // release status stays what tags and version history say, so unreleased
    // dependencies still ship inside their consumers.
    let explicit = match &opts.base {
        Some(base) => Some(
            ws.repos
                .iter()
                .map(|repo| classify::resolve_base(&repo.path, base))
                .collect::<Result<Vec<_>>>()?,
        ),
        None => None,
    };
    let baselines: Vec<Baseline> = pkgs
        .iter()
        .map(|p| {
            let detected = classify::baseline(
                &ws.repos[p.repo].path,
                p,
                sources_per_repo.get(&p.repo).map(BTreeSet::len) == Some(1),
            );
            match (&explicit, detected) {
                (Some(_), Baseline::Unreleased) => Baseline::Unreleased,
                (Some(revs), _) => Baseline::Explicit {
                    rev: revs[p.repo].clone(),
                },
                (None, detected) => detected,
            }
        })
        .collect();
    let mut unit_released = vec![false; n];
    for i in 0..n {
        unit_released[unit[i]] |= baselines[i] != Baseline::Unreleased;
    }

    // Commits alone cannot show a removed public item: a publishable unit
    // whose API would not be compared must not get a confident plan. A package
    // counts as having an API when it has one now or had one at its baseline.
    if opts.classify && !opts.api_checks && !opts.allow_unchecked_publishable {
        let mut unchecked = BTreeSet::new();
        for (i, p) in pkgs.iter().enumerate() {
            if !p.publish || ws.config.package(&p.id).and_then(|c| c.api_check) == Some(false) {
                continue;
            }
            let api = p.api_target.is_some()
                || p.api_unknown
                || baselines[i]
                    .rev()
                    .is_some_and(|rev| inventory::api_target_at(&ws.repos[p.repo].path, rev, p));
            if api {
                unchecked.insert(p.id.as_str());
            }
        }
        if !unchecked.is_empty() {
            bail!(
                "API checks are off, so commits alone would plan publishable units {}; a removed public item can understate the bump. Run with API checks or pass --allow-unchecked-publishable (MCP: allowUncheckedPublishable)",
                unchecked.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
    }

    // Intrinsic levels: commits on the package's scope, plus API checks.
    let mut level = vec![Level::None; n];
    let mut api_checked = vec![false; n];
    let mut reasons: Vec<Vec<String>> = vec![Vec::new(); n];
    for (i, p) in pkgs.iter().enumerate() {
        let Some(base) = baselines[i].rev().filter(|_| opts.classify) else {
            continue;
        };
        let repo = &ws.repos[p.repo];
        let config = ws.config.package(&p.id);
        // Unreleased same-repo dependencies ship inside this package.
        let mut scope = BTreeSet::from([if p.dir.is_empty() {
            ".".to_owned()
        } else {
            p.dir.clone()
        }]);
        scope.extend(config.map(|c| c.paths.clone()).unwrap_or_default());
        let mut stack = deps[i].clone();
        let mut seen = BTreeSet::new();
        while let Some(j) = stack.pop() {
            if seen.insert(j) && pkgs[j].repo == p.repo && !unit_released[unit[j]] {
                scope.insert(if pkgs[j].dir.is_empty() {
                    ".".into()
                } else {
                    pkgs[j].dir.clone()
                });
                stack.extend(&deps[j]);
            }
        }
        // Packages nested in this package's own directory release on their
        // own; configured paths and private dependencies are taken whole.
        let exclude: BTreeSet<String> = pkgs
            .iter()
            .filter(|q| q.repo == p.repo && q.dir != p.dir && !scope.contains(&q.dir))
            .filter(|q| p.dir.is_empty() || q.dir.starts_with(&format!("{}/", p.dir)))
            .map(|q| q.dir.clone())
            .collect();
        let scope: Vec<String> = scope.into_iter().collect();
        let exclude: Vec<String> = exclude.into_iter().collect();
        let signal = classify::commits(&repo.path, base, &scope, &exclude)?;
        level[i] = signal.level;
        reasons[i].push(format!("{}: {}", signal.source, signal.detail));
        let changelog = config
            .and_then(|c| c.changelog.clone())
            .unwrap_or_else(|| inventory::join(&p.dir, "CHANGELOG.md"));
        if let Some(entry) =
            show(&repo.path, "HEAD", &changelog).and_then(|t| classify::changelog(&t))
        {
            level[i] = level[i].max(entry.level);
            reasons[i].push(format!("{changelog} {}", entry.detail));
        }
        let api_wanted =
            opts.api_checks && p.publish && config.and_then(|c| c.api_check) != Some(false);
        if api_wanted && p.api_target.is_some() && signal.level > Level::None {
            // The checkers read the working tree; the plan describes HEAD.
            let api = if repo.dirty {
                Err("the working tree has uncommitted changes".to_owned())
            } else {
                classify::api(&repo.path, p, base)
            };
            match api {
                Ok(api) => {
                    api_checked[i] = true;
                    level[i] = level[i].max(api.level);
                    reasons[i].push(format!("{}: {}", api.source, api.detail));
                }
                Err(why) => findings.push(Finding {
                    subject: p.id.clone(),
                    message: format!("API check skipped: {why}"),
                }),
            }
        }
    }

    let mut unit_level = vec![Level::None; n];
    let mut unit_reasons: Vec<Vec<String>> = vec![Vec::new(); n];
    for i in 0..n {
        unit_level[unit[i]] = unit_level[unit[i]].max(level[i]);
        let prefix = if unit[i] == i {
            String::new()
        } else {
            format!("{}: ", pkgs[i].id)
        };
        unit_reasons[unit[i]].extend(reasons[i].iter().map(|r| format!("{prefix}{r}")));
    }
    let unit_next = |u: usize, l: Level| {
        next_version(
            &pkgs[u].version,
            if unit_released[u] { l } else { Level::None },
        )
    };

    // A released dependent of a bumped unit ships the new dependency: patch.
    loop {
        let mut changed = false;
        for i in 0..n {
            let ui = unit[i];
            for &j in &deps[i] {
                let uj = unit[j];
                if ui == uj
                    || !unit_released[ui]
                    || !unit_released[uj]
                    || unit_level[uj] == Level::None
                {
                    continue;
                }
                if unit_level[ui] < Level::Patch {
                    unit_level[ui] = Level::Patch;
                    let next = unit_next(uj, unit_level[uj])
                        .map(|(v, _)| v)
                        .unwrap_or_default();
                    unit_reasons[ui].push(format!(
                        "dependency {} moves {} -> {next}",
                        pkgs[j].id, pkgs[uj].version
                    ));
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Order units dependencies-first (Kahn); report any cycle.
    let mut edges: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for i in 0..n {
        edges.entry(unit[i]).or_default();
        for &j in &deps[i] {
            if unit[j] != unit[i] {
                edges.entry(unit[i]).or_default().insert(unit[j]);
            }
        }
    }
    let mut order = Vec::new();
    let mut done = BTreeSet::new();
    while done.len() < edges.len() {
        let ready: Vec<usize> = edges
            .iter()
            .filter(|(u, d)| !done.contains(*u) && d.iter().all(|x| done.contains(x)))
            .map(|(u, _)| *u)
            .collect();
        let batch = if ready.is_empty() {
            let rest: Vec<usize> = edges
                .keys()
                .filter(|u| !done.contains(*u))
                .copied()
                .collect();
            findings.push(Finding {
                subject: rest
                    .iter()
                    .map(|&u| pkgs[u].id.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                message: "dependency cycle: ordered by id".into(),
            });
            rest
        } else {
            ready
        };
        for u in batch {
            done.insert(u);
            order.push(u);
        }
    }

    let mut entries = Vec::new();
    let mut next_of = vec![String::new(); n];
    for &u in &order {
        let Some((next, bump)) = unit_next(u, unit_level[u]) else {
            findings.push(Finding {
                subject: pkgs[u].id.clone(),
                message: format!(
                    "version {} is not semver-like: left unchanged",
                    pkgs[u].version
                ),
            });
            for i in (0..n).filter(|&i| unit[i] == u) {
                next_of[i] = pkgs[i].version.clone();
            }
            continue;
        };
        let mut members: Vec<usize> = (0..n).filter(|&i| unit[i] == u).collect();
        members.sort_by_key(|&i| (i != u, pkgs[i].id.clone()));
        for i in members {
            let p = &pkgs[i];
            next_of[i] = next.clone();
            let action = if unit_released[u] && unit_level[u] > Level::None {
                Action::Bump
            } else if p.version != next {
                Action::Align
            } else if !unit_released[u] {
                Action::Initial
            } else {
                Action::Unchanged
            };
            let mut why = if i == u {
                unit_reasons[u].clone()
            } else {
                vec![format!("shares the version of {}", pkgs[u].id)]
            };
            if action == Action::Initial {
                why.push("no release found (no tag, version never changed): first release keeps the declared version".into());
            }
            if action == Action::Align {
                findings.push(Finding {
                    subject: p.id.clone(),
                    message: format!(
                        "declares {} but its unit {} is at {}",
                        p.version, pkgs[u].id, pkgs[u].version
                    ),
                });
            }
            let anchors = ws
                .config
                .package(&p.id)
                .map(|c| c.anchors.clone())
                .unwrap_or_default();
            let anchors: Vec<AnchorEntry> = anchors
                .into_iter()
                .map(|a| {
                    let current = show(&ws.repos[p.repo].path, "HEAD", &a.file)
                        .and_then(|t| anchor_value(&t, &a.key));
                    if current.as_deref() != Some(p.version.as_str()) {
                        findings.push(Finding {
                            subject: format!("{}:{}", a.file, a.key),
                            message: format!(
                                "anchor reads {:?} but {} is {}",
                                current, p.id, p.version
                            ),
                        });
                    }
                    AnchorEntry {
                        file: a.file,
                        key: a.key,
                        current,
                        next: next.clone(),
                    }
                })
                .collect();
            let api_unchecked = (opts.classify && p.publish && !api_checked[i]).then(|| {
                if ws.config.package(&p.id).and_then(|c| c.api_check) == Some(false) {
                    "api_check = false".to_owned()
                } else if !opts.api_checks {
                    "API checks off".to_owned()
                } else if p.api_unknown {
                    "layout not recognised".to_owned()
                } else if p.api_target.is_none() {
                    "no API target".to_owned()
                } else {
                    "check skipped".to_owned()
                }
            });
            entries.push(Entry {
                id: p.id.clone(),
                repo: p.repo,
                manifest: p.manifest.clone(),
                ecosystem: p.ecosystem,
                publish: p.publish,
                api_checked: api_checked[i],
                api_unchecked,
                current: p.version.clone(),
                next: next.clone(),
                action,
                level: unit_level[u],
                bump: if action == Action::Bump {
                    bump.into()
                } else {
                    "none".into()
                },
                baseline: baselines[i].clone(),
                unit: pkgs[u].id.clone(),
                version_source: p.version_source.clone(),
                reasons: why,
                anchors,
                depends_on: p
                    .deps
                    .iter()
                    .filter(|d| !d.dev)
                    .map(|d| d.id.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            });
        }
    }

    let mut requirements = Vec::new();
    for p in pkgs {
        for d in &p.deps {
            let j = index[d.id.as_str()];
            let Some(req) = &d.req else { continue };
            if satisfies(p.ecosystem, req, &pkgs[j].version) == Some(false) {
                findings.push(Finding {
                    subject: p.id.clone(),
                    message: format!(
                        "requires {} {req}, which excludes its current {}",
                        d.id, pkgs[j].version
                    ),
                });
            }
            if next_of[j] != pkgs[j].version
                && satisfies(p.ecosystem, req, &next_of[j]) == Some(false)
            {
                let replacement = match p.ecosystem {
                    Ecosystem::Cargo if req.starts_with('=') => Some(format!("={}", next_of[j])),
                    Ecosystem::Cargo => Some(next_of[j].clone()),
                    Ecosystem::Python => None,
                };
                let same = |r: &Requirement| {
                    r.at == d.at && pkgs[index[r.package.as_str()]].repo == p.repo
                };
                if requirements.iter().any(same) {
                    continue; // a shared [workspace.dependencies] entry
                }
                requirements.push(Requirement {
                    package: p.id.clone(),
                    dependency: d.id.clone(),
                    at: d.at.clone(),
                    req: req.clone(),
                    next: next_of[j].clone(),
                    replacement,
                });
            }
        }
    }

    let mut delegations = Vec::new();
    for (r, repo) in ws.repos.iter().enumerate() {
        let moved: Vec<&Entry> = entries
            .iter()
            .filter(|e| e.repo == r && matches!(e.action, Action::Bump | Action::Align))
            .collect();
        if moved.is_empty() {
            continue;
        }
        let at = repo.path.display();
        if repo.simit {
            for e in moved.iter().filter(|e| {
                e.action == Action::Bump && e.unit == e.id && e.ecosystem == Ecosystem::Cargo
            }) {
                let name = e.id.trim_start_matches("cargo:");
                delegations.push(format!("{at}: simit release {} --package {name} --no-tag records {} -> {} with its changelog; or `harbor-mcp apply` for the version edits alone", e.bump, e.current, e.next));
            }
        }
        if repo.files.iter().any(|f| f.ends_with("Cargo.lock"))
            && moved.iter().any(|e| e.ecosystem == Ecosystem::Cargo)
        {
            delegations.push(format!(
                "{at}: cargo update --workspace --offline (refresh Cargo.lock after the edits)"
            ));
        }
        if repo.files.iter().any(|f| f.ends_with("uv.lock"))
            && moved.iter().any(|e| e.ecosystem == Ecosystem::Python)
        {
            delegations.push(format!("{at}: uv lock (refresh uv.lock after the edits)"));
        }
        if let Some(coordinate) = &repo.coordinate {
            delegations.push(format!(
                "{at}: once the release commit lands, move consumers that pin {coordinate}: fleetix_supply_consumers then fleetix_supply_land (producer {coordinate}, target the release commit); canix pins go through the pin train (pin_request)"
            ));
        }
        delegations.push(format!(
            "{at}: tags, registry publishing and archive deposits stay with the operator"
        ));
    }

    Ok(Plan {
        schema: SCHEMA.into(),
        repos: ws.repos,
        packages: entries,
        requirements,
        findings,
        delegations,
    })
}

pub fn render(plan: &Plan) -> String {
    let mut out = String::new();
    for (i, r) in plan.repos.iter().enumerate() {
        out += &format!(
            "repo {i}: {} @ {}{}\n",
            r.path.display(),
            &r.head[..12.min(r.head.len())],
            if r.dirty { " (dirty)" } else { "" }
        );
    }
    out += "\npackage | current -> next | action | level | bump\n";
    for e in &plan.packages {
        out += &format!(
            "{} | {} -> {} | {:?} | {:?} | {}\n",
            e.id, e.current, e.next, e.action, e.level, e.bump
        );
        for r in &e.reasons {
            out += &format!("    {r}\n");
        }
        if let Some(why) = &e.api_unchecked {
            out += &format!("    api not checked ({why})\n");
        }
        for a in &e.anchors {
            out += &format!(
                "    anchor {}:{} {:?} -> {}\n",
                a.file, a.key, a.current, a.next
            );
        }
    }
    for r in &plan.requirements {
        out += &format!(
            "requirement {} on {}: {} excludes {} -> {}\n",
            r.package,
            r.dependency,
            r.req,
            r.next,
            r.replacement.as_deref().unwrap_or("edit by hand")
        );
    }
    for f in &plan.findings {
        out += &format!("finding {}: {}\n", f.subject, f.message);
    }
    for d in &plan.delegations {
        out += &format!("next: {d}\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_one_rules() {
        let n = |v, l| next_version(v, l).unwrap();
        assert_eq!(n("0.10.107", Level::Major), ("0.11.0".into(), "minor"));
        assert_eq!(n("0.10.107", Level::Minor), ("0.10.108".into(), "patch"));
        assert_eq!(n("0.0.3", Level::Major), ("0.0.4".into(), "patch"));
        assert_eq!(n("1.2.3", Level::Minor), ("1.3.0".into(), "minor"));
        assert_eq!(n("1.2.3", Level::Major), ("2.0.0".into(), "major"));
        assert_eq!(n("2.0.0-rc.1", Level::Patch), ("2.0.0".into(), "release"));
        assert_eq!(n("1.4", Level::Patch), ("1.4.1".into(), "patch"));
        assert_eq!(n("0.1.0", Level::None), ("0.1.0".into(), "none"));
        assert!(next_version("1.0.post1", Level::Patch).is_none());
        assert_eq!(
            satisfies(Ecosystem::Python, ">=0.1,<0.2", "0.2.0"),
            Some(false)
        );
        assert_eq!(satisfies(Ecosystem::Cargo, "0.10", "0.10.9"), Some(true));
    }
}
