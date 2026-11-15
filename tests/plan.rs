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
