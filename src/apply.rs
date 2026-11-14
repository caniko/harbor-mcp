//! Write a plan's version edits: version sources, Cargo requirements and
//! anchors. Nothing is committed, tagged or published.

use crate::inventory::{Location, git};
use crate::plan::{Action, Plan};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct Edit {
    pub repo: usize,
    pub file: String,
    pub at: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct Applied {
    pub written: bool,
    pub edits: Vec<Edit>,
    /// Requirements the plan could not rewrite (PEP 508 specifiers).
    pub manual: Vec<String>,
}

enum Change {
    Toml(Location, String, String),
    Line(String, String, String, String),
}

/// Refuses when a repository moved past the plan's head or a touched file
/// has uncommitted changes, so edits never mix with unreviewed work.
pub fn apply(plan: &Plan, write: bool) -> Result<Applied> {
    if plan.schema != crate::plan::SCHEMA {
        bail!("plan schema {} is not {}", plan.schema, crate::plan::SCHEMA);
    }
    let mut changes: BTreeMap<usize, Vec<Change>> = BTreeMap::new();
    for e in plan
        .packages
        .iter()
        .filter(|e| matches!(e.action, Action::Bump | Action::Align))
    {
        let list = changes.entry(e.repo).or_default();
        if let Some(source) = &e.version_source {
            let duplicate = list
                .iter()
                .any(|c| matches!(c, Change::Toml(l, _, _) if l == source));
            if !duplicate {
                list.push(Change::Toml(
                    source.clone(),
                    e.current.clone(),
                    e.next.clone(),
                ));
            }
        }
        for a in &e.anchors {
            let from = a.current.clone().unwrap_or_default();
            list.push(Change::Line(
                a.file.clone(),
                a.key.clone(),
                from,
                a.next.clone(),
            ));
        }
    }
    let mut manual = Vec::new();
    for r in &plan.requirements {
        let repo = plan
            .packages
            .iter()
            .find(|e| e.id == r.package)
            .map(|e| e.repo)
            .context("requirement of unknown package")?;
        match &r.replacement {
            Some(to) => changes.entry(repo).or_default().push(Change::Toml(
                r.at.clone(),
                r.req.clone(),
                to.clone(),
            )),
            None => manual.push(format!(
                "{}: {} requires {} {}; widen it to admit {}",
                r.at.file, r.package, r.dependency, r.req, r.next
            )),
        }
    }

    let mut edits = Vec::new();
    let mut outputs: BTreeMap<(usize, String), String> = BTreeMap::new();
    for (&r, list) in &changes {
        let repo = plan
            .repos
            .get(r)
            .context("plan names an unknown repository")?;
        let head = git(&repo.path, &["rev-parse", "HEAD"])?;
        if head.trim() != repo.head {
            bail!(
                "{} moved from {} to {}; plan again",
                repo.path.display(),
                repo.head,
                head.trim()
            );
        }
        for change in list {
            let file = match change {
                Change::Toml(at, ..) => &at.file,
                Change::Line(file, ..) => file,
            };
            let key = (r, file.clone());
            if !outputs.contains_key(&key) {
                if !git(&repo.path, &["status", "--porcelain", "--", file])?.is_empty() {
                    bail!("{}/{file} has uncommitted changes", repo.path.display());
                }
                let text = std::fs::read_to_string(repo.path.join(file))
                    .with_context(|| format!("reading {file}"))?;
                outputs.insert(key.clone(), text);
            }
            let text = outputs.get_mut(&key).expect("loaded above");
            let (at, from, to) = match change {
                Change::Toml(at, from, to) => {
                    *text = set_toml(text, at, from, to)
                        .with_context(|| format!("{file}: {}", at.path.join(".")))?;
                    (at.path.join("."), from, to)
                }
                Change::Line(_, k, from, to) => {
                    *text = set_line(text, k, from, to).with_context(|| format!("{file}: {k}"))?;
                    (k.clone(), from, to)
                }
            };
            edits.push(Edit {
                repo: r,
                file: file.clone(),
                at,
                from: from.clone(),
                to: to.clone(),
            });
        }
    }
    if write {
        for ((r, file), text) in &outputs {
            std::fs::write(plan.repos[*r].path.join(file), text)
                .with_context(|| format!("writing {file}"))?;
        }
    }
    Ok(Applied {
        written: write,
        edits,
        manual,
    })
}

/// Replace the string at `at` (or its `version` key, for a dependency
/// table) when it still reads `from`, keeping surrounding formatting.
fn set_toml(text: &str, at: &Location, from: &str, to: &str) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    let mut item = doc.as_item_mut();
    for key in &at.path {
        item = item
            .get_mut(key.as_str())
            .with_context(|| format!("missing key {key}"))?;
    }
    let item = if item.is_table_like() {
        let table = item.as_table_like_mut().expect("checked above");
        table
            .get_mut("version")
            .context("dependency has no version requirement")?
    } else {
        item
    };
    let value = item.as_value_mut().context("not a value")?;
    let current = value.as_str().context("not a string")?;
    if current != from {
        bail!("reads {current:?}, plan expected {from:?}; plan again");
    }
    let decor = value.decor().clone();
    *value = to.into();
    *value.decor_mut() = decor;
    Ok(doc.to_string())
}

/// Rewrite a top-level `key: value` line, keeping any quotes.
fn set_line(text: &str, key: &str, from: &str, to: &str) -> Result<String> {
    let prefix = format!("{key}:");
    let mut out = String::new();
    let mut done = false;
    for line in text.split_inclusive('\n') {
        match line.strip_prefix(&prefix).filter(|_| !done) {
            Some(rest) => {
                let value = rest.trim();
                let bare = value.trim_matches(['"', '\'']);
                if bare != from {
                    bail!("reads {bare:?}, plan expected {from:?}; plan again");
                }
                let quote = if value.starts_with(['"', '\'']) {
                    &value[..1]
                } else {
                    ""
                };
                let eol = if line.ends_with('\n') { "\n" } else { "" };
                out += &format!("{prefix} {quote}{to}{quote}{eol}");
                done = true;
            }
            None => out += line,
        }
    }
    if !done {
        bail!("no top-level `{key}:` line");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_formatting() {
        let at = |p: &[&str]| Location {
            file: "Cargo.toml".into(),
            path: p.iter().map(|s| s.to_string()).collect(),
            index: None,
        };
        let cargo = "[package]\nversion = \"0.1.0\" # keep\n\n[dependencies]\nfoo = { path = \"../foo\", version = \"0.3\" }\n";
        let out = set_toml(cargo, &at(&["package", "version"]), "0.1.0", "0.1.1").unwrap();
        assert!(out.contains("version = \"0.1.1\" # keep"));
        let out = set_toml(&out, &at(&["dependencies", "foo"]), "0.3", "0.4.0").unwrap();
        assert!(out.contains("version = \"0.4.0\" }"));
        assert!(set_toml(cargo, &at(&["package", "version"]), "9", "1").is_err());
        let cff = "title: x\nversion: \"0.10.107\"\ndate-released: 2026-11-09\n";
        assert_eq!(
            set_line(cff, "version", "0.10.107", "0.10.108").unwrap(),
            "title: x\nversion: \"0.10.108\"\ndate-released: 2026-11-09\n"
        );
        assert!(set_line(cff, "version", "0.9.0", "1.0.0").is_err());
    }
}
