//! End to end: a fixture repository with a released Cargo library, a
//! released dependent, an unreleased Python package and a CITATION anchor.
use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        // Hermetic: no global hooks, templates or signing from the host.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn harbor(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_harbor-mcp"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn harbor_fail(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_harbor-mcp"))
        .args(args)
        .output()
        .unwrap();
    assert!(!out.status.success(), "{args:?} should fail");
    String::from_utf8(out.stderr).unwrap()
}

fn entry<'a>(plan: &'a Value, id: &str) -> &'a Value {
    plan["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}

fn pyproject(modules: &[&str]) -> String {
    let list: Vec<String> = modules.iter().map(|m| format!("\"{m}\"")).collect();
    format!(
        "[project]\nname = \"shapes\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"setuptools\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[tool.setuptools]\npy-modules = [{}]\n",
        list.join(", ")
    )
}

#[test]
fn plans_propagates_and_applies() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let main = tmp.join("main");
    let d = main.as_path();
    std::fs::create_dir_all(d).unwrap();
    git(d, &["init", "-q", "-b", "trunk"]);
    // A shared workspace requirement, a dev-only user of it, and a member
    // that Cargo adds only because a member path-depends on it.
    write(
        d,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.dependencies]\ncore = { path = \"crates/core\", version = \"0.3\" }\n",
    );
    write(
        d,
        "extra/helper/Cargo.toml",
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n",
    );
    write(d, "extra/helper/src/lib.rs", "");
    write(
        d,
        "crates/tool/Cargo.toml",
        "[package]\nname = \"tool\"\nversion = \"0.1.0\"\n\n[dev-dependencies]\ncore.workspace = true\n",
    );
    write(d, "crates/tool/src/lib.rs", "");
    write(
        d,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.3.0\"\n",
    );
    write(d, "crates/core/src/lib.rs", "pub fn a() {}\n");
    write(
        d,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\n\n[dependencies]\ncore.workspace = true\nhelper = { path = \"../../extra/helper\" }\n",
    );
    write(d, "crates/app/src/main.rs", "fn main() {}\n");
    write(
        d,
        "py/client/pyproject.toml",
        "[project]\nname = \"Demo_Client\"\nversion = \"0.1.0\"\n\n[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n",
    );
    write(d, "CITATION.cff", "title: demo\nversion: 0.9.0\n");
    write(
        d,
        "harbor-mcp.toml",
        "[[package]]\nid = \"cargo:app\"\nanchors = [{ file = \"CITATION.cff\", key = \"version\" }]\n",
    );
    git(d, &["add", "-A"]);
    git(d, &["commit", "-qm", "feat: initial"]);
    // Release commits: versions change, so both crates count as released.
    write(
        d,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.3.1\"\n",
    );
    write(
        d,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"1.0.1\"\n\n[dependencies]\ncore.workspace = true\nhelper = { path = \"../../extra/helper\" }\n",
    );
    git(d, &["commit", "-qam", "chore: release"]);
    write(d, "crates/core/src/lib.rs", "pub fn b() {}\n");
    git(d, &["commit", "-qam", "feat(core)!: rename a to b"]);
    write(d, "py/client/README.md", "docs\n");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-qm", "docs(client): readme"]);
    // Imported already past a first version: released elsewhere.
    write(
        d,
        "crates/legacy/Cargo.toml",
        "[package]\nname = \"legacy\"\nversion = \"2.3.0\"\n",
    );
    write(d, "crates/legacy/src/lib.rs", "");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-qm", "chore: import legacy"]);
    write(d, "crates/legacy/src/lib.rs", "pub fn x() {}\n");
    git(d, &["commit", "-qam", "fix(legacy): x"]);

    // Two more repositories whose root manifests require core at the same
    // key path: each needs its own rewrite.
    let consumers: Vec<String> = ["c1", "c2"]
        .iter()
        .map(|name| {
            let c = tmp.join(name);
            std::fs::create_dir_all(&c).unwrap();
            git(&c, &["init", "-q", "-b", "trunk"]);
            write(
                &c,
                "Cargo.toml",
                &format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n\n[dependencies]\ncore = \"0.3\"\n"),
            );
            write(&c, "src/lib.rs", "");
            git(&c, &["add", "-A"]);
            git(&c, &["commit", "-qm", "feat: consumer"]);
            c.to_str().unwrap().to_owned()
        })
        .collect();
    let repo = d.to_str().unwrap();
    let plan: Value = serde_json::from_str(&harbor(&[
        "plan",
        "--repo",
        repo,
        "--repo",
        &consumers[0],
        "--repo",
        &consumers[1],
        "--json",
        "--no-api-checks",
        "--allow-unchecked-publishable",
    ]))
    .unwrap();
    let core = entry(&plan, "cargo:core");
    assert_eq!(
        (
            core["next"].as_str(),
            core["bump"].as_str(),
            core["level"].as_str()
        ),
        (Some("0.4.0"), Some("minor"), Some("major"))
    );
    let app = entry(&plan, "cargo:app");
    assert_eq!(
        (app["next"].as_str(), app["action"].as_str()),
        (Some("1.0.2"), Some("bump"))
    );
    assert!(
        app["reasons"]
            .to_string()
            .contains("dependency cargo:core moves 0.3.1 -> 0.4.0")
    );
    // The anchor drifted (0.9.0 vs 1.0.1) and follows app to its next version.
    assert_eq!(app["anchors"][0]["next"], "1.0.2");
    assert!(plan["findings"].to_string().contains("anchor reads"));
    let client = entry(&plan, "pypi:demo-client");
    assert_eq!(
        (client["action"].as_str(), client["next"].as_str()),
        (Some("initial"), Some("0.1.0"))
    );
    let legacy = entry(&plan, "cargo:legacy");
    assert_eq!(legacy["next"], "2.3.1");
    assert_eq!(legacy["baseline"]["kind"], "introduced");
    // Dependencies first.
    let order: Vec<&str> = plan["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert!(
        order.iter().position(|&i| i == "cargo:core")
            < order.iter().position(|&i| i == "cargo:app")
    );
    // app and tool share one [workspace.dependencies] requirement; each
    // consumer repository needs its own.
    let reqs = plan["requirements"].as_array().unwrap();
    assert_eq!(reqs.len(), 3, "{reqs:?}");
    assert_eq!(
        (
            reqs[0]["at"]["file"].as_str(),
            reqs[0]["req"].as_str(),
            reqs[0]["replacement"].as_str()
        ),
        (Some("Cargo.toml"), Some("0.3"), Some("0.4.0"))
    );
    assert!(app["depends_on"].to_string().contains("cargo:helper"));
    // A dev-dependency keeps its requirement current but does not order or bump.
    assert_eq!(
        entry(&plan, "cargo:tool")["depends_on"],
        serde_json::json!([])
    );

    let plan_file = d.join("plan.json");
    std::fs::write(&plan_file, plan.to_string()).unwrap();
    harbor(&["apply", plan_file.to_str().unwrap()]);
    let app_manifest = std::fs::read_to_string(d.join("crates/app/Cargo.toml")).unwrap();
    assert!(app_manifest.contains("version = \"1.0.2\""));
    assert!(
        std::fs::read_to_string(d.join("Cargo.toml"))
            .unwrap()
            .contains("version = \"0.4.0\" }")
    );
    assert!(
        std::fs::read_to_string(d.join("crates/core/Cargo.toml"))
            .unwrap()
            .contains("\"0.4.0\"")
    );
    assert!(
        std::fs::read_to_string(d.join("CITATION.cff"))
            .unwrap()
            .contains("version: 1.0.2")
    );
    for c in &consumers {
        let manifest = std::fs::read_to_string(Path::new(c).join("Cargo.toml")).unwrap();
        assert!(manifest.contains("core = \"0.4.0\""), "{manifest}");
    }
    // A second apply of the same plan refuses: the files are now dirty.
    let again = Command::new(env!("CARGO_BIN_EXE_harbor-mcp"))
        .args(["apply", plan_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!again.status.success());
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn refuses_unchecked_publishable_units() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-guard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    write(
        &tmp,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n",
    );
    write(
        &tmp,
        "crates/api/Cargo.toml",
        "[package]\nname = \"api\"\nversion = \"0.1.0\"\n",
    );
    write(&tmp, "crates/api/src/lib.rs", "pub fn a() {}\n");
    write(
        &tmp,
        "crates/cli/Cargo.toml",
        "[package]\nname = \"cli\"\nversion = \"0.1.0\"\npublish = false\n",
    );
    write(&tmp, "crates/cli/src/main.rs", "fn main() {}\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    let repo = tmp.to_str().unwrap();

    let err = harbor_fail(&["plan", "--repo", repo, "--json", "--no-api-checks"]);
    assert!(err.contains("cargo:api"), "{err}");
    assert!(!err.contains("cargo:cli"), "{err}");
    assert!(err.contains("--allow-unchecked-publishable"), "{err}");

    let plan: Value = serde_json::from_str(&harbor(&[
        "plan",
        "--repo",
        repo,
        "--json",
        "--no-api-checks",
        "--allow-unchecked-publishable",
    ]))
    .unwrap();
    assert_eq!(entry(&plan, "cargo:api")["api_checked"], false);
    let text = harbor(&[
        "plan",
        "--repo",
        repo,
        "--no-api-checks",
        "--allow-unchecked-publishable",
    ]);
    assert!(text.contains("api not checked (API checks off)"), "{text}");
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn checks_removed_python_modules() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-py-removed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    write(&tmp, "pyproject.toml", &pyproject(&["alpha", "beta"]));
    write(&tmp, "alpha.py", "def stable():\n    pass\n");
    write(&tmp, "beta.py", "def old():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v1.0.0"]);
    // The module itself is gone, though py-modules still lists it.
    std::fs::remove_file(tmp.join("beta.py")).unwrap();
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "fix: drop beta"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    let shapes = entry(&plan, "pypi:shapes");
    assert_eq!(shapes["api_checked"], true);
    assert_eq!(
        (shapes["level"].as_str(), shapes["next"].as_str()),
        (Some("major"), Some("2.0.0"))
    );
    assert!(
        shapes["reasons"].to_string().contains("beta: removed"),
        "{shapes}"
    );
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn checks_removals_beside_python_module_additions() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-py-added-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    write(&tmp, "pyproject.toml", &pyproject(&["alpha", "beta"]));
    write(
        &tmp,
        "alpha.py",
        "def old():\n    pass\n\ndef keep():\n    pass\n",
    );
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v1.0.0"]);
    // One module loses a public name while another is added: the addition
    // must not make the comparison fail.
    write(&tmp, "alpha.py", "def keep():\n    pass\n");
    write(&tmp, "beta.py", "def fresh():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "fix: reshape alpha"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    let shapes = entry(&plan, "pypi:shapes");
    assert_eq!(shapes["api_checked"], true);
    assert_eq!(
        (shapes["level"].as_str(), shapes["next"].as_str()),
        (Some("major"), Some("2.0.0"))
    );
    assert!(shapes["reasons"].to_string().contains("alpha:"), "{shapes}");
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn checks_python_module_additions_without_breaking() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-py-new-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    write(&tmp, "pyproject.toml", &pyproject(&["alpha", "beta"]));
    write(&tmp, "alpha.py", "def stable():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v1.0.0"]);
    write(&tmp, "beta.py", "def fresh():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: add beta"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    let shapes = entry(&plan, "pypi:shapes");
    assert_eq!(shapes["api_checked"], true);
    assert_eq!(
        (shapes["level"].as_str(), shapes["next"].as_str()),
        (Some("minor"), Some("1.1.0"))
    );
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn checks_setuptools_namespace_packages() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-ns-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // A dotted package declared under `package-dir`; it has no
    // `__init__.py`, so only the declaration can locate it.
    write(
        &tmp,
        "pyproject.toml",
        "[project]\nname = \"gadget-dist\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"setuptools\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[tool.setuptools]\npackages = [\"ns.gadget\"]\npackage-dir = {\"\" = \"lib\"}\n",
    );
    write(&tmp, "lib/ns/gadget/mod.py", "def old():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v1.0.0"]);
    write(&tmp, "lib/ns/gadget/mod.py", "def keep():\n    pass\n");
    git(&tmp, &["commit", "-qam", "fix: drop old"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    let gadget = entry(&plan, "pypi:gadget-dist");
    assert_eq!(gadget["api_checked"], true, "{gadget}");
    assert_eq!(
        (gadget["level"].as_str(), gadget["next"].as_str()),
        (Some("major"), Some("2.0.0")),
        "{gadget}"
    );
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn checks_poetry_packages() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-poetry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // A declared include under `from = "src"`, and Poetry's default package
    // named after the project, which has no `__init__.py`.
    write(
        &tmp,
        "declared/pyproject.toml",
        "[project]\nname = \"acme-client\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"poetry-core\"]\nbuild-backend = \"poetry.core.masonry.api\"\n\n[tool.poetry]\npackages = [{ include = \"acme\", from = \"src\" }]\n",
    );
    write(
        &tmp,
        "declared/src/acme/__init__.py",
        "def old():\n    pass\n",
    );
    write(
        &tmp,
        "default/pyproject.toml",
        "[project]\nname = \"gadget\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"poetry-core\"]\nbuild-backend = \"poetry.core.masonry.api\"\n\n[tool.poetry]\n",
    );
    write(&tmp, "default/src/gadget/mod.py", "def old():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "acme-client/v1.0.0"]);
    git(&tmp, &["tag", "gadget/v1.0.0"]);
    write(
        &tmp,
        "declared/src/acme/__init__.py",
        "def keep():\n    pass\n",
    );
    write(&tmp, "default/src/gadget/mod.py", "def keep():\n    pass\n");
    git(&tmp, &["commit", "-qam", "fix: drop public names"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    for id in ["pypi:acme-client", "pypi:gadget"] {
        let p = entry(&plan, id);
        assert_eq!(p["api_checked"], true, "{p}");
        assert_eq!(
            (p["level"].as_str(), p["next"].as_str()),
            (Some("major"), Some("2.0.0")),
            "{p}"
        );
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn checks_hatch_packages() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-hatch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // The wheel target names a package whose directory does not match the
    // distribution's normalized name.
    write(
        &tmp,
        "pyproject.toml",
        "[project]\nname = \"shapes-dist\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n\n[tool.hatch.build.targets.wheel]\npackages = [\"src/shapes\"]\n",
    );
    write(&tmp, "src/shapes/__init__.py", "def old():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v1.0.0"]);
    write(&tmp, "src/shapes/__init__.py", "def keep():\n    pass\n");
    git(&tmp, &["commit", "-qam", "fix: drop old"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    let shapes = entry(&plan, "pypi:shapes-dist");
    assert_eq!(shapes["api_checked"], true, "{shapes}");
    assert_eq!(
        (shapes["level"].as_str(), shapes["next"].as_str()),
        (Some("major"), Some("2.0.0")),
        "{shapes}"
    );
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn checks_maturin_python_sources() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-maturin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // A mixed project whose Python package takes `module-name`'s first
    // component, and one that falls back to the project name.
    write(
        &tmp,
        "named/pyproject.toml",
        "[project]\nname = \"shapes-ext\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"maturin>=1.0\"]\nbuild-backend = \"maturin\"\n\n[tool.maturin]\npython-source = \"python\"\nmodule-name = \"shapes._ext\"\n",
    );
    write(
        &tmp,
        "named/python/shapes/__init__.py",
        "def old():\n    pass\n",
    );
    write(
        &tmp,
        "unnamed/pyproject.toml",
        "[project]\nname = \"gadget-kit\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"maturin>=1.0\"]\nbuild-backend = \"maturin\"\n\n[tool.maturin]\npython-source = \"python\"\n",
    );
    write(
        &tmp,
        "unnamed/python/gadget_kit/__init__.py",
        "def old():\n    pass\n",
    );
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "shapes-ext/v1.0.0"]);
    git(&tmp, &["tag", "gadget-kit/v1.0.0"]);
    write(
        &tmp,
        "named/python/shapes/__init__.py",
        "def keep():\n    pass\n",
    );
    write(
        &tmp,
        "unnamed/python/gadget_kit/__init__.py",
        "def keep():\n    pass\n",
    );
    git(&tmp, &["commit", "-qam", "fix: drop public names"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    for id in ["pypi:shapes-ext", "pypi:gadget-kit"] {
        let p = entry(&plan, id);
        assert_eq!(p["api_checked"], true, "{p}");
        assert_eq!(
            (p["level"].as_str(), p["next"].as_str()),
            (Some("major"), Some("2.0.0")),
            "{p}"
        );
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn refuses_pure_rust_maturin_projects() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-maturin-rs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // Only a compiled module: no Python sources for griffe to read.
    write(
        &tmp,
        "pyproject.toml",
        "[project]\nname = \"widget-ext\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"maturin>=1.0\"]\nbuild-backend = \"maturin\"\n\n[tool.maturin]\nmodule-name = \"widget\"\n",
    );
    write(&tmp, "src/lib.rs", "");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    let repo = tmp.to_str().unwrap();

    let err = harbor_fail(&["plan", "--repo", repo, "--json", "--no-api-checks"]);
    assert!(err.contains("pypi:widget-ext"), "{err}");
    let plan: Value = serde_json::from_str(&harbor(&[
        "plan",
        "--repo",
        repo,
        "--json",
        "--no-api-checks",
        "--allow-unchecked-publishable",
    ]))
    .unwrap();
    assert_eq!(entry(&plan, "pypi:widget-ext")["api_checked"], false);
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn refuses_setuptools_package_discovery() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-find-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // `packages.find` discovers packages from the tree, which harbor-mcp does
    // not emulate: even a directory the heuristic could match stays unknown.
    write(
        &tmp,
        "pyproject.toml",
        "[project]\nname = \"shapes\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"setuptools\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[tool.setuptools.packages.find]\nnamespaces = true\n",
    );
    write(&tmp, "src/shapes/__init__.py", "def stable():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    let repo = tmp.to_str().unwrap();

    let err = harbor_fail(&["plan", "--repo", repo, "--json", "--no-api-checks"]);
    assert!(err.contains("pypi:shapes"), "{err}");
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn uses_declared_package_dir_over_the_name_heuristic() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-packagedir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // The declared package under `lib/` shares the distribution's name; a
    // same-named directory at the root must not be compared instead.
    write(
        &tmp,
        "pyproject.toml",
        "[project]\nname = \"shapes\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"setuptools\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[tool.setuptools]\npackages = [\"shapes\"]\npackage-dir = {\"\" = \"lib\"}\n",
    );
    write(&tmp, "lib/shapes/__init__.py", "def old():\n    pass\n");
    write(&tmp, "shapes/__init__.py", "def stable():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v1.0.0"]);
    write(&tmp, "lib/shapes/__init__.py", "def keep():\n    pass\n");
    git(&tmp, &["commit", "-qam", "fix: drop old"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    let shapes = entry(&plan, "pypi:shapes");
    assert_eq!(shapes["api_checked"], true, "{shapes}");
    assert_eq!(
        (shapes["level"].as_str(), shapes["next"].as_str()),
        (Some("major"), Some("2.0.0")),
        "{shapes}"
    );
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn refuses_poetry_glob_packages() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-poetry-glob-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // A glob include names no single import.
    write(
        &tmp,
        "pyproject.toml",
        "[project]\nname = \"acme\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"poetry-core\"]\nbuild-backend = \"poetry.core.masonry.api\"\n\n[tool.poetry]\npackages = [{ include = \"acme-*\", from = \"src\" }]\n",
    );
    write(
        &tmp,
        "src/acme_core/__init__.py",
        "def stable():\n    pass\n",
    );
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    let repo = tmp.to_str().unwrap();

    let err = harbor_fail(&["plan", "--repo", repo, "--json", "--no-api-checks"]);
    assert!(err.contains("pypi:acme"), "{err}");
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn skips_poetry_sdist_only_packages() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-poetry-format-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // Two includes, one of them sdist-only: its removal is not an API change.
    write(
        &tmp,
        "pyproject.toml",
        "[project]\nname = \"acme\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"poetry-core\"]\nbuild-backend = \"poetry.core.masonry.api\"\n\n[tool.poetry]\npackages = [{ include = \"acme\", from = \"src\", format = [\"sdist\", \"wheel\"] }, { include = \"tests\", format = \"sdist\" }]\n",
    );
    write(&tmp, "src/acme/__init__.py", "def stable():\n    pass\n");
    write(&tmp, "tests/helpers.py", "def old():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v1.0.0"]);
    write(&tmp, "tests/helpers.py", "def keep():\n    pass\n");
    git(&tmp, &["commit", "-qam", "fix: drop a test helper"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    let acme = entry(&plan, "pypi:acme");
    assert_eq!(acme["api_checked"], true, "{acme}");
    assert_eq!(
        (acme["level"].as_str(), acme["next"].as_str()),
        (Some("patch"), Some("1.0.1")),
        "{acme}"
    );
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn refuses_remapped_python_layouts() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-remap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // A Hatch `sources` remap at the wheel target and at the build level, and
    // a Poetry `to` remap, all move the import somewhere the declared name
    // does not name.
    write(
        &tmp,
        "hatch/pyproject.toml",
        "[project]\nname = \"shapes\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n\n[tool.hatch.build.targets.wheel]\npackages = [\"src/shapes\"]\nsources = {\"src\" = \"\"}\n",
    );
    write(
        &tmp,
        "hatch/src/shapes/__init__.py",
        "def stable():\n    pass\n",
    );
    write(
        &tmp,
        "widemap/pyproject.toml",
        "[project]\nname = \"gadget\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n\n[tool.hatch.build]\nsources = {\"src\" = \"\"}\n\n[tool.hatch.build.targets.wheel]\npackages = [\"src/gadget\"]\n",
    );
    write(
        &tmp,
        "widemap/src/gadget/__init__.py",
        "def stable():\n    pass\n",
    );
    write(
        &tmp,
        "poetry/pyproject.toml",
        "[project]\nname = \"acme\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"poetry-core\"]\nbuild-backend = \"poetry.core.masonry.api\"\n\n[tool.poetry]\npackages = [{ include = \"acme\", from = \"src\", to = \"renamed\" }]\n",
    );
    write(
        &tmp,
        "poetry/src/acme/__init__.py",
        "def stable():\n    pass\n",
    );
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    let repo = tmp.to_str().unwrap();

    let err = harbor_fail(&["plan", "--repo", repo, "--json", "--no-api-checks"]);
    assert!(err.contains("pypi:shapes"), "{err}");
    assert!(err.contains("pypi:gadget"), "{err}");
    assert!(err.contains("pypi:acme"), "{err}");
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn locates_hatch_packages_outside_src() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-hatch-lib-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    // The package directory names the import, not the distribution.
    write(
        &tmp,
        "pyproject.toml",
        "[project]\nname = \"shapes-dist\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n\n[tool.hatch.build.targets.wheel]\npackages = [\"lib/shapes\"]\n",
    );
    write(&tmp, "lib/shapes/__init__.py", "def old():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v1.0.0"]);
    write(&tmp, "lib/shapes/__init__.py", "def keep():\n    pass\n");
    git(&tmp, &["commit", "-qam", "fix: drop old"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    let shapes = entry(&plan, "pypi:shapes-dist");
    assert_eq!(shapes["api_checked"], true, "{shapes}");
    assert_eq!(
        (shapes["level"].as_str(), shapes["next"].as_str()),
        (Some("major"), Some("2.0.0")),
        "{shapes}"
    );
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn locates_poetry_default_packages() {
    let tmp =
        std::env::temp_dir().join(format!("harbor-mcp-poetry-default-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    let poetry = "[build-system]\nrequires = [\"poetry-core\"]\nbuild-backend = \"poetry.core.masonry.api\"\n\n[tool.poetry]\n";
    // The canonical project name as a directory, as a module, and under `src/`.
    write(
        &tmp,
        "canonical/pyproject.toml",
        &format!("[project]\nname = \"My--Shapes\"\nversion = \"1.0.0\"\n\n{poetry}"),
    );
    write(
        &tmp,
        "canonical/my_shapes/__init__.py",
        "def old():\n    pass\n",
    );
    write(
        &tmp,
        "module/pyproject.toml",
        &format!("[project]\nname = \"gadget\"\nversion = \"1.0.0\"\n\n{poetry}"),
    );
    write(&tmp, "module/gadget.py", "def old():\n    pass\n");
    write(
        &tmp,
        "src/pyproject.toml",
        &format!("[project]\nname = \"widget\"\nversion = \"1.0.0\"\n\n{poetry}"),
    );
    write(&tmp, "src/src/widget/__init__.py", "def old():\n    pass\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    for tag in ["My--Shapes/v1.0.0", "gadget/v1.0.0", "widget/v1.0.0"] {
        git(&tmp, &["tag", tag]);
    }
    write(
        &tmp,
        "canonical/my_shapes/__init__.py",
        "def keep():\n    pass\n",
    );
    write(&tmp, "module/gadget.py", "def keep():\n    pass\n");
    write(
        &tmp,
        "src/src/widget/__init__.py",
        "def keep():\n    pass\n",
    );
    git(&tmp, &["commit", "-qam", "fix: drop public names"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    for id in ["pypi:my-shapes", "pypi:gadget", "pypi:widget"] {
        let p = entry(&plan, id);
        assert_eq!(p["api_checked"], true, "{p}");
        assert_eq!(
            (p["level"].as_str(), p["next"].as_str()),
            (Some("major"), Some("2.0.0")),
            "{p}"
        );
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn plans_from_an_explicit_base() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-base-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    let manifest = |version: &str| format!("[package]\nname = \"lib\"\nversion = \"{version}\"\n");
    write(&tmp, "Cargo.toml", &manifest("0.1.0"));
    write(&tmp, "src/lib.rs", "pub fn a() {}\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v0.1.0"]);
    // The bump commit itself: at this head no revision is left to plan from.
    write(&tmp, "Cargo.toml", &manifest("0.2.0"));
    write(&tmp, "src/lib.rs", "pub fn b() {}\n");
    git(&tmp, &["commit", "-qam", "feat!: replace a with b"]);
    let repo = tmp.to_str().unwrap();
    let plan: Value = serde_json::from_str(&harbor(&[
        "plan",
        "--repo",
        repo,
        "--json",
        "--no-api-checks",
        "--allow-unchecked-publishable",
    ]))
    .unwrap();
    let at_head = entry(&plan, "cargo:lib");
    assert_eq!(at_head["action"], "unchanged");
    assert!(
        at_head["reasons"]
            .to_string()
            .contains("no commits since the release")
    );

    let plan: Value = serde_json::from_str(&harbor(&[
        "plan",
        "--repo",
        repo,
        "--json",
        "--base",
        "v0.1.0",
        "--no-api-checks",
        "--allow-unchecked-publishable",
    ]))
    .unwrap();
    let lib = entry(&plan, "cargo:lib");
    assert_eq!(lib["baseline"]["kind"], "explicit");
    assert_eq!(
        (lib["level"].as_str(), lib["next"].as_str()),
        (Some("major"), Some("0.3.0"))
    );

    let err = harbor_fail(&[
        "plan",
        "--repo",
        repo,
        "--json",
        "--base",
        "nope",
        "--no-api-checks",
        "--allow-unchecked-publishable",
    ]);
    assert!(err.contains("base nope does not resolve"), "{err}");
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn explicit_base_keeps_release_status() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-status-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    write(
        &tmp,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n",
    );
    write(
        &tmp,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\n\n[dependencies]\nhelper = { path = \"../helper\" }\n",
    );
    write(&tmp, "crates/app/src/main.rs", "fn main() {}\n");
    write(
        &tmp,
        "crates/helper/Cargo.toml",
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\npublish = false\n",
    );
    write(&tmp, "crates/helper/src/lib.rs", "pub fn a() {}\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "app/v1.0.0"]);
    // An unreleased private dependency: its breaking change ships inside app.
    write(&tmp, "crates/helper/src/lib.rs", "pub fn b() {}\n");
    git(&tmp, &["commit", "-qam", "refactor(helper)!: reshape"]);
    let repo = tmp.to_str().unwrap();

    let planned = |extra: &[&str]| -> Value {
        let mut args = vec!["plan", "--repo", repo, "--json"];
        args.extend_from_slice(extra);
        serde_json::from_str(&harbor(&args)).unwrap()
    };
    let auto = planned(&[]);
    let based = planned(&["--base", "app/v1.0.0"]);
    for id in ["cargo:app", "cargo:helper"] {
        let pick = |plan: &Value| {
            let e = entry(plan, id);
            (
                e["action"].clone(),
                e["next"].clone(),
                e["bump"].clone(),
                e["level"].clone(),
            )
        };
        assert_eq!(pick(&auto), pick(&based), "{id}");
    }
    assert_eq!(entry(&auto, "cargo:app")["next"], "2.0.0");
    assert_eq!(entry(&auto, "cargo:helper")["action"], "initial");
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn refuses_publishable_units_that_lost_their_api() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-lost-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    write(
        &tmp,
        "Cargo.toml",
        "[package]\nname = \"widget\"\nversion = \"1.0.0\"\n",
    );
    write(&tmp, "src/lib.rs", "pub fn a() {}\n");
    write(&tmp, "src/main.rs", "fn main() {}\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v1.0.0"]);
    // The crate is published, and its released API was a library.
    std::fs::remove_file(tmp.join("src/lib.rs")).unwrap();
    write(&tmp, "src/main.rs", "fn main() { println!(\"x\"); }\n");
    git(&tmp, &["commit", "-qam", "fix: bins only"]);
    let repo = tmp.to_str().unwrap();

    let err = harbor_fail(&["plan", "--repo", repo, "--json", "--no-api-checks"]);
    assert!(err.contains("cargo:widget"), "{err}");
    // With checks on, the library that was released is gone: breaking.
    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    let widget = entry(&plan, "cargo:widget");
    assert_eq!(widget["api_checked"], true);
    assert_eq!(
        (widget["level"].as_str(), widget["next"].as_str()),
        (Some("major"), Some("2.0.0"))
    );
    assert!(widget["reasons"].to_string().contains("lib: removed"));
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn refuses_unlocatable_python_apis() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-python-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    let project = |name: &str, tool: &str| {
        format!(
            "[project]\nname = \"{name}\"\nversion = \"1.0.0\"\n\n[build-system]\nrequires = [\"setuptools\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[tool.setuptools]\n{tool}"
        )
    };
    // A setuptools single-module distribution, and a layout harbor-mcp cannot
    // place at all.
    write(
        &tmp,
        "modules/pyproject.toml",
        &project("widget", "py-modules = [\"widget\"]\n"),
    );
    write(&tmp, "modules/widget.py", "def run():\n    pass\n");
    write(
        &tmp,
        "packaged/pyproject.toml",
        &project("gadget", "package-dir = {\"\" = \"lib\"}\n"),
    );
    write(&tmp, "packaged/lib/gadget/__init__.py", "");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    let repo = tmp.to_str().unwrap();

    let err = harbor_fail(&["plan", "--repo", repo, "--json", "--no-api-checks"]);
    assert!(err.contains("pypi:widget"), "{err}");
    assert!(err.contains("pypi:gadget"), "{err}");
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn rejects_a_base_outside_history() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-ancestor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    write(
        &tmp,
        "Cargo.toml",
        "[package]\nname = \"widget\"\nversion = \"0.1.0\"\n",
    );
    write(&tmp, "src/lib.rs", "pub fn a() {}\n");
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    write(&tmp, "src/lib.rs", "pub fn b() {}\n");
    git(&tmp, &["commit", "-qam", "feat: later"]);
    git(&tmp, &["tag", "later"]);
    // Drop the tagged commit: it is no longer an ancestor of HEAD.
    git(&tmp, &["reset", "--hard", "HEAD~1"]);
    let repo = tmp.to_str().unwrap();

    let err = harbor_fail(&["plan", "--repo", repo, "--json", "--base", "later"]);
    assert!(
        err.contains("base later is not an ancestor of HEAD"),
        "{err}"
    );
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn checks_every_python_module() {
    let tmp = std::env::temp_dir().join(format!("harbor-mcp-py-modules-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git(&tmp, &["init", "-q", "-b", "trunk"]);
    write(&tmp, "pyproject.toml", &pyproject(&["alpha", "beta"]));
    write(&tmp, "alpha.py", "def stable():\n    pass\n");
    write(
        &tmp,
        "beta.py",
        "def old():\n    pass\n\ndef keep():\n    pass\n",
    );
    git(&tmp, &["add", "-A"]);
    git(&tmp, &["commit", "-qm", "feat: initial"]);
    git(&tmp, &["tag", "v1.0.0"]);
    // Only the second module loses a public name, under a commit that is a
    // patch by itself.
    write(&tmp, "beta.py", "def keep():\n    pass\n");
    git(&tmp, &["commit", "-qam", "fix: drop beta.old"]);
    let repo = tmp.to_str().unwrap();

    let plan: Value = serde_json::from_str(&harbor(&["plan", "--repo", repo, "--json"])).unwrap();
    let shapes = entry(&plan, "pypi:shapes");
    assert_eq!(shapes["api_checked"], true);
    assert_eq!(
        (shapes["level"].as_str(), shapes["next"].as_str()),
        (Some("major"), Some("2.0.0"))
    );
    assert!(shapes["reasons"].to_string().contains("beta:"), "{shapes}");
    std::fs::remove_dir_all(&tmp).unwrap();
}
