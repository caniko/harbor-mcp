//! What changed in a package since its last release, and how much.
//!
//! Commits are read as Conventional Commits. API comparison is delegated:
//! cargo-semver-checks for Cargo libraries, griffe for Python packages.

use crate::inventory::{Ecosystem, Location, Package, git, version_at};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    None,
    Patch,
    Minor,
    Major,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Baseline {
    /// A release tag names the current version.
    Tag { tag: String, rev: String },
    /// The newest commit that changed the version: the last release commit.
    VersionChange { rev: String, from: String },
    /// The package arrived (imported or moved without history) already past
    /// a first version: released elsewhere, so this commit is its baseline.
    Introduced { rev: String, version: String },
    /// The version was never changed and no tag names it: never released.
    Unreleased,
}

/// Versions a new package starts at.
const INITIAL: &[&str] = &["0.0.0", "0.0.1", "0.1.0", "1.0.0"];

impl Baseline {
    pub fn rev(&self) -> Option<&str> {
        match self {
            Baseline::Tag { rev, .. }
            | Baseline::VersionChange { rev, .. }
            | Baseline::Introduced { rev, .. } => Some(rev),
            Baseline::Unreleased => None,
        }
    }
}

pub fn baseline(root: &Path, package: &Package, single: bool) -> Baseline {
    let Some(source) = &package.version_source else {
        return Baseline::Unreleased;
    };
    let v = &package.version;
    let mut tags = vec![
        format!("{}/v{v}", package.name),
        format!("{}-v{v}", package.name),
        format!("{}@{v}", package.name),
    ];
    if single || package.dir.is_empty() {
        tags.extend([format!("v{v}"), v.clone()]);
    }
    for tag in tags {
        if let Ok(rev) = git(
            root,
            &[
                "rev-parse",
                "-q",
                "--verify",
                &format!("refs/tags/{tag}^{{commit}}"),
            ],
        ) {
            return Baseline::Tag {
                tag,
                rev: rev.trim().to_owned(),
            };
        }
    }
    version_change(root, source)
}

fn version_change(root: &Path, source: &Location) -> Baseline {
    // -G narrows the walk to commits touching a `version =` line; parsing
    // then confirms the value at `source` really changed. --follow keeps
    // the history of a moved manifest.
    let Ok(log) = git(
        root,
        &[
            "log",
            "--follow",
            "--name-status",
            "--format=%x1e%H",
            "-G",
            r"^\s*version\s*=",
            "HEAD",
            "--",
            &source.file,
        ],
    ) else {
        return Baseline::Unreleased;
    };
    for record in log.split('\x1e') {
        let mut lines = record.lines().filter(|l| !l.is_empty());
        let (Some(rev), Some(status)) = (lines.next(), lines.next()) else {
            continue;
        };
        // `M\tpath`, `A\tpath` or `R100\told\tnew`.
        let fields: Vec<&str> = status.split('\t').collect();
        let (now_path, before_path) = match fields.as_slice() {
            [s, old, new] if s.starts_with(['R', 'C']) => (*new, Some(*old)),
            [s, path] if s.starts_with('A') => (*path, None),
            [_, path] => (*path, Some(*path)),
            _ => continue,
        };
        let at = |file: &str| Location {
            file: file.to_owned(),
            ..source.clone()
        };
        let now = version_at(root, rev, &at(now_path));
        let before = before_path.and_then(|p| version_at(root, &format!("{rev}^"), &at(p)));
        match (now, before) {
            (Some(now), Some(before)) if now != before => {
                return Baseline::VersionChange {
                    rev: rev.to_owned(),
                    from: before,
                };
            }
            // The version was introduced here: a first version means a new
            // package; anything later was released before it moved here.
            (Some(now), None) if INITIAL.contains(&now.as_str()) => return Baseline::Unreleased,
            (Some(version), None) => {
                return Baseline::Introduced {
                    rev: rev.to_owned(),
                    version,
                };
            }
            _ => {}
        }
    }
    Baseline::Unreleased
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub level: Level,
    pub source: String,
    pub detail: String,
}

/// Level of one Conventional Commit; anything unparsable counts as a patch.
pub fn commit_level(subject: &str, body: &str) -> Level {
    let breaking_body = body
        .lines()
        .any(|l| l.starts_with("BREAKING CHANGE:") || l.starts_with("BREAKING-CHANGE:"));
    let Some((head, _)) = subject.split_once(':') else {
        return if breaking_body {
            Level::Major
        } else {
            Level::Patch
        };
    };
    let bang = head.ends_with('!');
    let kind = head.trim_end_matches('!');
    let kind = match kind.split_once('(') {
        Some((k, scope)) if scope.ends_with(')') => k,
        Some(_) => return Level::Patch,
        None => kind,
    };
    if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_alphabetic()) {
        return if breaking_body {
            Level::Major
        } else {
            Level::Patch
        };
    }
    if bang || breaking_body {
        return Level::Major;
    }
    match kind.to_ascii_lowercase().as_str() {
        "feat" => Level::Minor,
        "docs" | "test" | "tests" | "ci" | "style" => Level::None,
        _ => Level::Patch,
    }
}

/// Highest commit level in `base..HEAD` on `scope` (minus `exclude`), with
/// the commits that set it.
pub fn commits(
    root: &Path,
    base: &str,
    scope: &[String],
    exclude: &[String],
) -> anyhow::Result<Signal> {
    let range = format!("{base}..HEAD");
    let mut args = vec![
        "log",
        "--no-merges",
        "--format=%H%x1f%s%x1f%b%x1e",
        &range,
        "--",
    ];
    let excludes: Vec<String> = exclude.iter().map(|e| format!(":(exclude){e}")).collect();
    args.extend(scope.iter().map(String::as_str));
    args.extend(excludes.iter().map(String::as_str));
    let log = git(root, &args)?;
    let mut level = Level::None;
    let mut top = Vec::new();
    let mut count = 0;
    for record in log.split('\x1e').map(str::trim).filter(|r| !r.is_empty()) {
        let mut parts = record.splitn(3, '\x1f');
        let (sha, subject, body) = (
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
        );
        count += 1;
        let l = commit_level(subject, body);
        if l > level {
            level = l;
            top.clear();
        }
        if l == level && l > Level::None && top.len() < 3 {
            top.push(format!("{} {subject}", &sha[..sha.len().min(9)]));
        }
    }
    let detail = if count == 0 {
        "no commits since the release".into()
    } else {
        format!("{count} commits; highest: {}", top.join("; "))
    };
    Ok(Signal {
        level,
        source: "commits".into(),
        detail,
    })
}

/// Level of a Keep a Changelog `[Unreleased]` section: `Removed` or a
/// `BREAKING` note is major, `Added`/`Changed`/`Deprecated` minor, anything
/// else a patch. `None` when there is no such section or it is empty.
pub fn changelog(text: &str) -> Option<Signal> {
    let mut lines = text.lines().skip_while(|l| {
        !(l.starts_with("## ")
            && l[3..]
                .trim()
                .trim_matches(['[', ']'])
                .eq_ignore_ascii_case("unreleased"))
    });
    lines.next()?;
    let mut level = Level::None;
    let mut sections: Vec<String> = Vec::new();
    let mut section = String::new();
    for line in lines.take_while(|l| !l.starts_with("## ")) {
        if let Some(name) = line.strip_prefix("### ") {
            section = name.trim().to_owned();
            continue;
        }
        if line.trim().is_empty() || section.is_empty() {
            continue;
        }
        let l = match section.to_ascii_lowercase().as_str() {
            _ if line.contains("BREAKING") => Level::Major,
            "removed" => Level::Major,
            "added" | "changed" | "deprecated" => Level::Minor,
            _ => Level::Patch,
        };
        level = level.max(l);
        if !sections.contains(&section) {
            sections.push(section.clone());
        }
    }
    (level > Level::None).then(|| Signal {
        level,
        source: "changelog".into(),
        detail: format!("[Unreleased] has {}", sections.join(", ")),
    })
}

/// Delegate an API comparison against `base`. `Err` explains why none ran.
pub fn api(root: &Path, package: &Package, base: &str) -> Result<Signal, String> {
    let target = package.api_target.as_deref().ok_or("no API target")?;
    match package.ecosystem {
        Ecosystem::Cargo => {
            let manifest = root.join(&package.manifest);
            let out = Command::new("cargo")
                .current_dir(root)
                .args([
                    "semver-checks",
                    "--color",
                    "never",
                    "--baseline-rev",
                    base,
                    "-p",
                    &package.name,
                    "--manifest-path",
                ])
                .arg(&manifest)
                .output()
                .map_err(|e| format!("cargo semver-checks unavailable: {e}"))?;
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let summary = text
                .lines()
                .find(|l| l.trim_start().starts_with("Summary"))
                .map(str::trim);
            let level = match summary {
                Some(s) if s.contains("requires new major version") => Level::Major,
                Some(s) if s.contains("requires new minor version") => Level::Minor,
                Some(s) if out.status.success() => {
                    return Ok(Signal {
                        level: Level::None,
                        source: "cargo-semver-checks".into(),
                        detail: s.into(),
                    });
                }
                _ => {
                    let tail: Vec<&str> = text.lines().rev().take(3).collect();
                    return Err(format!(
                        "cargo semver-checks failed: {}",
                        tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
                    ));
                }
            };
            Ok(Signal {
                level,
                source: "cargo-semver-checks".into(),
                detail: summary.unwrap_or_default().into(),
            })
        }
        Ecosystem::Python => {
            let import = package.import_name.as_deref().ok_or("no import name")?;
            let out = Command::new("griffe")
                .current_dir(root)
                .args([
                    "check",
                    import,
                    "--against",
                    base,
                    "--search",
                    target,
                    "--format",
                    "oneline",
                    "--no-color",
                ])
                .output()
                .map_err(|e| format!("griffe unavailable: {e}"))?;
            let err = String::from_utf8_lossy(&out.stderr).into_owned();
            // griffe exits 1 and prints one `path:line: object: reason` line
            // per breakage on stderr; other exits or lines are failures.
            let breaks: Vec<&str> = err
                .lines()
                .filter(|l| {
                    l.split_once(": ")
                        .and_then(|(at, _)| at.rsplit_once(':'))
                        .is_some_and(|(_, n)| n.parse::<u32>().is_ok())
                })
                .collect();
            match out.status.code() {
                Some(0) => Ok(Signal {
                    level: Level::None,
                    source: "griffe".into(),
                    detail: "no breaking API changes".into(),
                }),
                Some(1) if !breaks.is_empty() => Ok(Signal {
                    level: Level::Major,
                    source: "griffe".into(),
                    detail: breaks
                        .iter()
                        .take(3)
                        .copied()
                        .collect::<Vec<_>>()
                        .join("; "),
                }),
                _ => Err(format!(
                    "griffe check failed: {}",
                    err.trim().lines().last().unwrap_or_default()
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_sections() {
        let text = "# Log\n\n## [Unreleased]\n\n### Fixed\n\n- a\n\n### Removed\n\n- b\n\n## [1.0.0]\n\n### Added\n\n- c\n";
        let s = changelog(text).unwrap();
        assert_eq!(
            (s.level, s.detail.as_str()),
            (Level::Major, "[Unreleased] has Fixed, Removed")
        );
        assert!(changelog("## [Unreleased]\n\n### Added\n\n## [1.0.0]\n- x\n").is_none());
        assert_eq!(
            changelog("## Unreleased\n### Added\n- x\n").unwrap().level,
            Level::Minor
        );
        assert!(changelog("## [1.0.0]\n### Removed\n- x\n").is_none());
    }

    #[test]
    fn conventional_commits() {
        assert_eq!(commit_level("feat(ui): add", ""), Level::Minor);
        assert_eq!(commit_level("fix!: drop", ""), Level::Major);
        assert_eq!(commit_level("refactor(db)!: drop", ""), Level::Major);
        assert_eq!(
            commit_level("chore: x", "BREAKING CHANGE: gone"),
            Level::Major
        );
        assert_eq!(commit_level("docs(readme): x", ""), Level::None);
        assert_eq!(commit_level("Merge stuff", ""), Level::Patch);
        assert_eq!(commit_level("build(deps): bump", ""), Level::Patch);
        assert_eq!(commit_level("weird text: here", ""), Level::Patch);
    }
}
