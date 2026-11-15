//! Release units of Git repositories, read at `HEAD`: Cargo packages, Python
//! projects, where each version lives, their internal dependencies and the
//! anchor files that must repeat a version.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

pub const CONFIG_FILE: &str = "harbor-mcp.toml";

pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// File contents at `rev`, or `None` when the file or revision is absent.
pub fn show(root: &Path, rev: &str, path: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", &format!("{rev}:{path}")])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Manifest globs (repository-relative) to leave out, e.g. test fixtures.
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default, rename = "package")]
    pub packages: Vec<PackageConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageConfig {
    /// `cargo:<name>` or `pypi:<normalized name>`.
    pub id: String,
    /// Extra repository-relative paths whose commits count as this package's changes.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Files that repeat this package's version on a top-level `key: value` line.
    #[serde(default)]
    pub anchors: Vec<Anchor>,
    /// Release in lockstep with another package: share its version and level.
    pub follow: Option<String>,
    /// Set false to skip cargo-semver-checks / griffe for this package.
    pub api_check: Option<bool>,
    /// Keep a Changelog file whose `[Unreleased]` section describes this
    /// package; defaults to `CHANGELOG.md` in the package directory.
    pub changelog: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub file: String,
    pub key: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// `other` wins for package ids both configure.
    pub fn merge(mut self, other: &Config) -> Self {
        self.exclude.extend(other.exclude.iter().cloned());
        for package in &other.packages {
            self.packages.retain(|p| p.id != package.id);
            self.packages.push(package.clone());
        }
        self
    }

    pub fn package(&self, id: &str) -> Option<&PackageConfig> {
        self.packages.iter().find(|p| p.id == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Cargo,
    Python,
}

/// A TOML value addressed by a key path inside one repository file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Location {
    pub file: String,
    pub path: Vec<String>,
    /// Array index for PEP 508 requirement strings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dep {
    pub id: String,
    pub req: Option<String>,
    pub at: Location,
    /// Dev-dependencies keep their requirements current but never propagate.
    #[serde(default)]
    pub dev: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub ecosystem: Ecosystem,
    pub repo: usize,
    /// Repository-relative directory; empty for the root.
    pub dir: String,
    pub manifest: String,
    pub version: String,
    /// Where `version` is written; `None` when the manifest declares none.
    pub version_source: Option<Location>,
    pub publish: bool,
    /// Every surface an API checker can compare: the Cargo library, or each
    /// Python package and declared module.
    pub api_targets: Vec<ApiTarget>,
    /// A publishable project whose importable code could not be located: no
    /// API check can run, so planning it unchecked must not be confident.
    pub api_unknown: bool,
    pub deps: Vec<Dep>,
}

/// Where an API checker finds one surface: a Cargo library, or one Python
/// import under the directory griffe searches.
#[derive(Debug, Clone, Serialize)]
pub struct ApiTarget {
    /// `lib` for Cargo; the search path griffe gets for Python.
    pub search: String,
    /// Python module or package name; none for Cargo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub subject: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub path: PathBuf,
    pub head: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coordinate: Option<String>,
    pub dirty: bool,
    pub simit: bool,
    #[serde(skip)]
    pub files: Vec<String>,
}

pub struct Workspace {
    pub repos: Vec<Repo>,
    pub packages: Vec<Package>,
    pub config: Config,
    pub findings: Vec<Finding>,
}

pub fn normalize_python(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// `host/owner/name` from an SSH or HTTPS remote URL.
fn coordinate(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches(".git");
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("ssh://git@"))
        .map(str::to_owned)
        .or_else(|| url.strip_prefix("git@").map(|r| r.replacen(':', "/", 1)))?;
    Some(rest)
}

pub fn version_at(root: &Path, rev: &str, at: &Location) -> Option<String> {
    let doc: toml::Table = show(root, rev, &at.file)?.parse().ok()?;
    let mut value = doc.get(at.path.first()?)?;
    for key in &at.path[1..] {
        value = value.get(key)?;
    }
    value.as_str().map(str::to_owned)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

pub fn join(dir: &str, file: &str) -> String {
    if dir.is_empty() {
        file.to_owned()
    } else {
        format!("{dir}/{file}")
    }
}

fn strs(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub fn load(paths: &[PathBuf], extra: &Config) -> Result<Workspace> {
    let mut ws = Workspace {
        repos: Vec::new(),
        packages: Vec::new(),
        config: Config::default(),
        findings: Vec::new(),
    };
    for path in paths {
        let root = PathBuf::from(git(path, &["rev-parse", "--show-toplevel"])?.trim());
        let head = git(&root, &["rev-parse", "HEAD"])?.trim().to_owned();
        let files: Vec<String> = git(&root, &["ls-tree", "-r", "-z", "--name-only", "HEAD"])?
            .split('\0')
            .filter(|f| !f.is_empty())
            .map(str::to_owned)
            .collect();
        let repo_config = match show(&root, "HEAD", CONFIG_FILE) {
            Some(text) => toml::from_str::<Config>(&text)
                .with_context(|| format!("parsing {}/{CONFIG_FILE}", root.display()))?,
            None => Config::default(),
        };
        let index = ws.repos.len();
        let excludes: Vec<glob::Pattern> = repo_config
            .exclude
            .iter()
            .chain(&extra.exclude)
            .map(|g| glob::Pattern::new(g))
            .collect::<Result<_, _>>()?;
        let found = discover_cargo(&root, &files, index, &excludes, &mut ws.findings)
            .into_iter()
            .chain(discover_python(
                &root,
                &files,
                index,
                &excludes,
                &mut ws.findings,
            ));
        ws.packages.extend(found);
        ws.config = std::mem::take(&mut ws.config).merge(&repo_config);
        ws.repos.push(Repo {
            coordinate: git(&root, &["remote", "get-url", "origin"])
                .ok()
                .and_then(|u| coordinate(&u)),
            dirty: !git(&root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty(),
            simit: files.iter().any(|f| f == "simit.toml"),
            path: root,
            head,
            files,
        });
    }
    ws.config = std::mem::take(&mut ws.config).merge(extra);
    let mut seen = BTreeMap::new();
    for p in &ws.packages {
        if let Some(first) = seen.insert(p.id.clone(), (p.repo, p.manifest.clone())) {
            ws.findings.push(Finding {
                subject: p.id.clone(),
                message: format!(
                    "declared twice ({} and {}); dependencies resolve to the first",
                    first.1, p.manifest
                ),
            });
        }
    }
    // Drop external dependencies; keep the first declaration of duplicated ids.
    let mut kept = std::collections::BTreeSet::new();
    ws.packages.retain(|p| kept.insert(p.id.clone()));
    let ids: std::collections::BTreeSet<String> =
        ws.packages.iter().map(|p| p.id.clone()).collect();
    for p in &mut ws.packages {
        p.deps.retain(|d| ids.contains(&d.id) && d.id != p.id);
    }
    for c in &ws.config.packages {
        if !ids.contains(&c.id) {
            ws.findings.push(Finding {
                subject: c.id.clone(),
                message: "configured but not found in any repository".into(),
            });
        }
    }
    Ok(ws)
}

struct CargoWorkspace {
    members: Vec<glob::Pattern>,
    exclude: Vec<String>,
    package: Option<toml::Table>,
    deps: Option<toml::Table>,
}

fn discover_cargo(
    root: &Path,
    files: &[String],
    repo: usize,
    excludes: &[glob::Pattern],
    findings: &mut Vec<Finding>,
) -> Vec<Package> {
    let mut manifests = BTreeMap::new();
    for file in files
        .iter()
        .filter(|f| *f == "Cargo.toml" || f.ends_with("/Cargo.toml"))
    {
        if excludes.iter().any(|g| g.matches(file)) {
            continue;
        }
        match show(root, "HEAD", file).map(|t| t.parse::<toml::Table>()) {
            Some(Ok(doc)) => {
                manifests.insert(file.clone(), doc);
            }
            Some(Err(e)) => findings.push(Finding {
                subject: file.clone(),
                message: format!("unparsable: {e}"),
            }),
            None => {}
        }
    }
    let workspaces: BTreeMap<&str, CargoWorkspace> = manifests
        .iter()
        .filter_map(|(file, doc)| {
            let w = doc.get("workspace")?.as_table()?;
            Some((
                parent(file),
                CargoWorkspace {
                    members: strs(w.get("members"))
                        .iter()
                        .filter_map(|m| glob::Pattern::new(m.trim_end_matches('/')).ok())
                        .collect(),
                    exclude: strs(w.get("exclude")),
                    package: w.get("package").and_then(|v| v.as_table()).cloned(),
                    deps: w.get("dependencies").and_then(|v| v.as_table()).cloned(),
                },
            ))
        })
        .collect();
    // Nearest enclosing workspace, as Cargo resolves it.
    let nearest = |dir: &str| {
        let mut at = Some(dir);
        while let Some(d) = at {
            if let Some((k, w)) = workspaces.get_key_value(d) {
                return Some((*k, w));
            }
            at = (!d.is_empty()).then(|| parent(d));
        }
        None
    };
    // (workspace dir, listed member, excluded) for a package directory.
    let membership = |dir: &str| {
        nearest(dir).map(|(wdir, w)| {
            let rel = dir
                .strip_prefix(wdir)
                .unwrap_or(dir)
                .trim_start_matches('/');
            let member = wdir == dir || w.members.iter().any(|m| m.matches(rel));
            let excluded = w
                .exclude
                .iter()
                .any(|e| rel == e || rel.starts_with(&format!("{e}/")));
            (wdir, member, excluded)
        })
    };
    // Cargo also makes members' path dependencies inside the workspace members.
    let package_dir = |d: &str| {
        manifests
            .get(&join(d, "Cargo.toml"))
            .is_some_and(|m| m.contains_key("package"))
    };
    let mut implicit = BTreeSet::new();
    let mut queue: Vec<String> = manifests
        .keys()
        .map(|f| parent(f).to_owned())
        .filter(|d| package_dir(d) && matches!(membership(d), Some((_, true, false))))
        .collect();
    while let Some(dir) = queue.pop() {
        let ws = nearest(&dir);
        for (_, table) in dep_tables(&manifests[&join(&dir, "Cargo.toml")]) {
            for (key, value) in table {
                let path = if value.get("workspace").and_then(|v| v.as_bool()) == Some(true) {
                    ws.and_then(|(wdir, w)| {
                        let path = w.deps.as_ref()?.get(key)?.get("path")?.as_str()?;
                        Some(join(wdir, path))
                    })
                } else {
                    let path = value.get("path").and_then(|v| v.as_str());
                    path.map(|p| join(&dir, p))
                };
                let Some(dep) = path.map(|p| normalize(&p)) else {
                    continue;
                };
                let unlisted = membership(&dep).is_some_and(|(wdir, member, excluded)| {
                    !member && !excluded && Some(wdir) == ws.map(|w| w.0)
                });
                if unlisted && package_dir(&dep) && implicit.insert(dep.clone()) {
                    queue.push(dep);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (file, doc) in &manifests {
        let Some(package) = doc.get("package").and_then(|v| v.as_table()) else {
            continue;
        };
        let Some(name) = package.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let dir = parent(file);
        let mut owner = nearest(dir);
        if let Some((_, member, excluded)) = membership(dir) {
            if !member && !excluded && !implicit.contains(dir) {
                continue;
            }
            if excluded {
                owner = None;
            }
        }
        let wfile = owner.map(|(d, _)| join(d, "Cargo.toml"));
        let inherited = |key: &str| {
            package
                .get(key)
                .and_then(|v| v.get("workspace"))
                .and_then(|v| v.as_bool())
                == Some(true)
        };
        let (version, version_source) = match package.get("version") {
            Some(toml::Value::String(v)) => (
                v.clone(),
                Some(Location {
                    file: file.clone(),
                    path: vec!["package".into(), "version".into()],
                    index: None,
                }),
            ),
            _ if inherited("version") => {
                match owner.and_then(|(_, w)| w.package.as_ref()?.get("version")?.as_str()) {
                    Some(v) => (
                        v.to_owned(),
                        Some(Location {
                            file: wfile.clone().unwrap_or_default(),
                            path: vec!["workspace".into(), "package".into(), "version".into()],
                            index: None,
                        }),
                    ),
                    None => {
                        findings.push(Finding {
                            subject: file.clone(),
                            message: "inherits a version its workspace does not declare".into(),
                        });
                        continue;
                    }
                }
            }
            _ => ("0.0.0".into(), None),
        };
        let publish_value = if inherited("publish") {
            owner.and_then(|(_, w)| w.package.as_ref()?.get("publish").cloned())
        } else {
            package.get("publish").cloned()
        };
        let publish = version_source.is_some()
            && match publish_value {
                Some(toml::Value::Boolean(b)) => b,
                Some(toml::Value::Array(a)) => !a.is_empty(),
                _ => true,
            };
        let lib = doc.contains_key("lib") || files.iter().any(|f| *f == join(dir, "src/lib.rs"));
        let mut deps = Vec::new();
        for (path, table) in dep_tables(doc) {
            for (key, value) in table {
                let (spec, at) = if value.get("workspace").and_then(|v| v.as_bool()) == Some(true) {
                    let Some(spec) = owner.and_then(|(_, w)| w.deps.as_ref()?.get(key)) else {
                        continue;
                    };
                    let at = Location {
                        file: wfile.clone().unwrap_or_default(),
                        path: vec!["workspace".into(), "dependencies".into(), key.clone()],
                        index: None,
                    };
                    (spec, at)
                } else {
                    let mut p = path.clone();
                    p.push(key.clone());
                    (
                        value,
                        Location {
                            file: file.clone(),
                            path: p,
                            index: None,
                        },
                    )
                };
                let dep = spec.get("package").and_then(|v| v.as_str()).unwrap_or(key);
                let req = spec
                    .as_str()
                    .or_else(|| spec.get("version")?.as_str())
                    .map(str::to_owned);
                deps.push(Dep {
                    id: format!("cargo:{dep}"),
                    req,
                    at,
                    dev: path.iter().any(|k| k == "dev-dependencies"),
                });
            }
        }
        out.push(Package {
            id: format!("cargo:{name}"),
            name: name.to_owned(),
            ecosystem: Ecosystem::Cargo,
            repo,
            dir: dir.to_owned(),
            manifest: file.clone(),
            version,
            version_source,
            publish,
            api_targets: lib
                .then(|| ApiTarget {
                    search: "lib".into(),
                    import: None,
                })
                .into_iter()
                .collect(),
            api_unknown: false,
            deps,
        });
    }
    out
}

/// Every dependency table of a manifest, with its key path.
fn dep_tables(doc: &toml::Table) -> Vec<(Vec<String>, &toml::Table)> {
    let mut tables = Vec::new();
    for kind in ["dependencies", "build-dependencies", "dev-dependencies"] {
        if let Some(t) = doc.get(kind).and_then(|v| v.as_table()) {
            tables.push((vec![kind.into()], t));
        }
        if let Some(targets) = doc.get("target").and_then(|v| v.as_table()) {
            for (cfg, t) in targets {
                if let Some(t) = t.get(kind).and_then(|v| v.as_table()) {
                    tables.push((vec!["target".into(), cfg.clone(), kind.into()], t));
                }
            }
        }
    }
    tables
}

/// Resolve `.` and `..` in a repository-relative path.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

/// Every repository-relative path at `rev`.
fn files_at(root: &Path, rev: &str) -> Vec<String> {
    git(root, &["ls-tree", "-r", "-z", "--name-only", rev])
        .map(|out| {
            out.split('\0')
                .filter(|f| !f.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn has_file(files: &[String], path: &str) -> bool {
    files.iter().any(|f| f == path)
}

/// `path` is a directory holding Python sources at this revision.
fn has_python(files: &[String], path: &str) -> bool {
    let prefix = format!("{path}/");
    files.iter().any(|f| {
        f.strip_prefix(&prefix)
            .is_some_and(|rest| rest.ends_with(".py"))
    })
}

/// Search path for a dotted import: the first root whose directory for
/// `import` holds Python sources, with or without `__init__.py` (PEP 420).
fn locate(roots: &[String], import: &str, files: &[String]) -> Option<String> {
    let path = import.replace('.', "/");
    roots
        .iter()
        .find(|root| has_python(files, &join(root, &path)))
        .cloned()
}

/// Record one (search path, import) target once; the first location wins.
fn push_target(targets: &mut Vec<(String, String)>, search: String, import: String) {
    if !targets.iter().any(|(_, known)| *known == import) {
        targets.push((search, import));
    }
}

/// Importable code a Python project declares under `dir`, each with the search
/// path it lives under: a flat or `src/` package, a `py-modules` module, and
/// the package directories setuptools, Poetry, Hatch and maturin declare.
/// `files` answers for the repository at whatever revision is inspected.
/// Ambiguous layouts produce no target, so the package stays `api_unknown`.
fn python_targets(dir: &str, doc: &toml::Table, files: &[String]) -> Vec<(String, String)> {
    let Some(project) = doc.get("project").and_then(|v| v.as_table()) else {
        return Vec::new();
    };
    let Some(name) = project.get("name").and_then(|v| v.as_str()) else {
        return Vec::new();
    };
    let tool = |key: &str| doc.get("tool").and_then(|t| t.get(key));
    let setuptools = tool("setuptools");
    let roots = [join(dir, "src"), dir.to_owned()];
    let import = name.replace(['-', '.'], "_").to_ascii_lowercase();
    let mut targets = Vec::new();
    // A flat or `src/` package, and every single module.
    let package = join(&import, "__init__.py");
    if let Some(root) = roots.iter().find(|r| has_file(files, &join(r, &package))) {
        push_target(&mut targets, root.clone(), import.clone());
    }
    for module in setuptools
        .map(|t| strs(t.get("py-modules")))
        .unwrap_or_default()
    {
        let file = format!("{module}.py");
        if let Some(root) = roots.iter().find(|r| has_file(files, &join(r, &file))) {
            push_target(&mut targets, root.clone(), module);
        }
    }
    // setuptools: an explicit `packages` list, or `packages.find` whose
    // namespace lookup takes the distribution's own dotted directory.
    if let Some(packages) = setuptools.and_then(|t| t.get("packages")) {
        let mut roots = roots.to_vec();
        // `package-dir = {"" = "<dir>"}` moves every package's root; a
        // per-prefix mapping is left alone.
        if let Some(moved) = setuptools
            .and_then(|t| t.get("package-dir"))
            .and_then(|t| t.get(""))
            .and_then(|v| v.as_str())
        {
            roots.insert(0, join(dir, moved));
        }
        if let Some(list) = packages.as_array() {
            for import in list.iter().filter_map(|v| v.as_str()) {
                // A pattern is a `find` input, not a package name.
                if import.contains(['*', '?']) {
                    continue;
                }
                if let Some(root) = locate(&roots, import, files) {
                    push_target(&mut targets, root, import.to_owned());
                }
            }
        } else if let Some(find) = packages.get("find") {
            // `namespaces` defaults to true under `packages.find`: a directory
            // without `__init__.py` is still the package.
            if find.get("namespaces").and_then(|v| v.as_bool()) != Some(false)
                && let Some(root) = locate(&roots, &import, files)
            {
                push_target(&mut targets, root, import.clone());
            }
        }
    }
    // Poetry: declared `packages` (an `include` directory or module under
    // `from`), or its default package named after the project.
    if let Some(poetry) = tool("poetry") {
        match poetry.get("packages").and_then(|v| v.as_array()) {
            Some(list) => {
                for entry in list {
                    let Some(include) = entry.get("include").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let from = entry.get("from").and_then(|v| v.as_str()).unwrap_or("");
                    let root = normalize(&join(dir, from));
                    if let Some(module) = include.strip_suffix(".py") {
                        if has_file(files, &join(&root, include)) {
                            push_target(&mut targets, root, module.replace('/', "."));
                        }
                    } else if has_python(files, &join(&root, include)) {
                        push_target(&mut targets, root, include.replace('/', "."));
                    }
                }
            }
            None => {
                if let Some(root) = locate(&roots, &import, files) {
                    push_target(&mut targets, root, import.clone());
                }
            }
        }
    }
    // Hatch: wheel `packages` naming one package directory under the project
    // or its `src/`. Remapped and nested entries stay unknown.
    if let Some(wheel) = tool("hatch")
        .and_then(|t| t.get("build"))
        .and_then(|t| t.get("targets"))
        .and_then(|t| t.get("wheel"))
    {
        for entry in strs(wheel.get("packages")) {
            let entry = normalize(&entry);
            let root = match entry.rsplit_once('/') {
                Some((parent, _)) => normalize(&join(dir, parent)),
                None => dir.to_owned(),
            };
            if root != dir && root != join(dir, "src") {
                continue;
            }
            let Some(name) = entry.rsplit('/').next() else {
                continue;
            };
            if has_python(files, &join(&root, name)) {
                push_target(&mut targets, root, name.to_owned());
            }
        }
    }
    // maturin: the Python package beside the compiled extension under
    // `python-source`, named by `module-name`'s first component or the project.
    if let Some(source) = tool("maturin")
        .and_then(|t| t.get("python-source"))
        .and_then(|v| v.as_str())
    {
        let root = normalize(&join(dir, source));
        let package = tool("maturin")
            .and_then(|t| t.get("module-name"))
            .and_then(|v| v.as_str())
            .map_or_else(
                || import.clone(),
                |m| m.split('.').next().unwrap_or(m).replace('-', "_"),
            );
        if has_python(files, &join(&root, &package)) {
            push_target(&mut targets, root, package);
        }
    }
    // A pure-Rust maturin project has no Python sources; griffe cannot inspect
    // a compiled module from source, so it stays api_unknown.
    targets
}

/// API surfaces `package` had at `rev` (none when nothing was published yet).
pub fn api_targets_at(root: &Path, rev: &str, package: &Package) -> Vec<ApiTarget> {
    match package.ecosystem {
        Ecosystem::Cargo => {
            let lib = show(root, rev, &package.manifest)
                .and_then(|text| text.parse::<toml::Table>().ok())
                .is_some_and(|doc| doc.contains_key("lib"));
            let had = lib || show(root, rev, &join(&package.dir, "src/lib.rs")).is_some();
            had.then(|| ApiTarget {
                search: "lib".into(),
                import: None,
            })
            .into_iter()
            .collect()
        }
        Ecosystem::Python => show(root, rev, &package.manifest)
            .and_then(|text| text.parse::<toml::Table>().ok())
            .map(|doc| {
                python_targets(&package.dir, &doc, &files_at(root, rev))
                    .into_iter()
                    .map(|(search, import)| ApiTarget {
                        search,
                        import: Some(import),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn discover_python(
    root: &Path,
    files: &[String],
    repo: usize,
    excludes: &[glob::Pattern],
    findings: &mut Vec<Finding>,
) -> Vec<Package> {
    let mut out = Vec::new();
    for file in files
        .iter()
        .filter(|f| *f == "pyproject.toml" || f.ends_with("/pyproject.toml"))
    {
        if excludes.iter().any(|g| g.matches(file)) {
            continue;
        }
        let Some(Ok(doc)) = show(root, "HEAD", file).map(|t| t.parse::<toml::Table>()) else {
            continue;
        };
        let Some(project) = doc.get("project").and_then(|v| v.as_table()) else {
            continue;
        };
        let Some(name) = project.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        // uv builds a project only when it declares a build system (or opts in).
        let uv_package = doc
            .get("tool")
            .and_then(|t| t.get("uv"))
            .and_then(|t| t.get("package"))
            .and_then(|v| v.as_bool());
        if uv_package == Some(false)
            || (!doc.contains_key("build-system") && uv_package != Some(true))
        {
            continue;
        }
        if strs(project.get("dynamic")).iter().any(|d| d == "version") {
            findings.push(Finding {
                subject: file.clone(),
                message: "dynamic version: not planned".into(),
            });
            continue;
        }
        let Some(version) = project.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let dir = parent(file);
        let mut deps = Vec::new();
        let mut lists: Vec<(Vec<String>, Vec<String>)> = vec![(
            vec!["project".into(), "dependencies".into()],
            strs(project.get("dependencies")),
        )];
        if let Some(extras) = project
            .get("optional-dependencies")
            .and_then(|v| v.as_table())
        {
            for (extra, list) in extras {
                lists.push((
                    vec![
                        "project".into(),
                        "optional-dependencies".into(),
                        extra.clone(),
                    ],
                    strs(Some(list)),
                ));
            }
        }
        for (path, list) in lists {
            for (index, spec) in list.iter().enumerate() {
                match pep508_rs::Requirement::<pep508_rs::VerbatimUrl>::from_str(spec) {
                    Ok(req) => {
                        let specifiers = match req.version_or_url {
                            Some(pep508_rs::VersionOrUrl::VersionSpecifier(s)) if !s.is_empty() => {
                                Some(s.to_string())
                            }
                            _ => None,
                        };
                        deps.push(Dep {
                            id: format!("pypi:{}", normalize_python(req.name.as_ref())),
                            req: specifiers,
                            at: Location {
                                file: file.clone(),
                                path: path.clone(),
                                index: Some(index),
                            },
                            dev: false,
                        });
                    }
                    Err(e) => findings.push(Finding {
                        subject: file.clone(),
                        message: format!("requirement {spec:?}: {e}"),
                    }),
                }
            }
        }
        let api_targets: Vec<ApiTarget> = python_targets(dir, &doc, files)
            .into_iter()
            .map(|(search, import)| ApiTarget {
                search,
                import: Some(import),
            })
            .collect();
        let api_unknown = api_targets.is_empty();
        out.push(Package {
            id: format!("pypi:{}", normalize_python(name)),
            name: name.to_owned(),
            ecosystem: Ecosystem::Python,
            repo,
            dir: dir.to_owned(),
            manifest: file.clone(),
            version: version.to_owned(),
            version_source: Some(Location {
                file: file.clone(),
                path: vec!["project".into(), "version".into()],
                index: None,
            }),
            publish: !strs(project.get("classifiers"))
                .iter()
                .any(|c| c.starts_with("Private ::")),
            api_unknown,
            api_targets,
            deps,
        });
    }
    out
}

/// Value of a top-level `key: value` line (YAML such as CITATION.cff).
pub fn anchor_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    text.lines()
        .find_map(|l| l.strip_prefix(&prefix))
        .map(|v| v.trim().trim_matches(['"', '\'']).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_remotes() {
        assert_eq!(normalize_python("SynDB__Client.x"), "syndb-client-x");
        assert_eq!(
            coordinate("git@github.com:caniko/harbor.git").as_deref(),
            Some("github.com/caniko/harbor")
        );
        assert_eq!(
            coordinate("https://github.com/caniko/simit").as_deref(),
            Some("github.com/caniko/simit")
        );
        assert_eq!(
            anchor_value("cff: 1\nversion: \"0.10.107\"\n", "version").as_deref(),
            Some("0.10.107")
        );
    }
}
