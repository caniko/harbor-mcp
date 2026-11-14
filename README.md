# harbor-mcp

<!-- simit:badges:start -->

[![CI](https://img.shields.io/badge/CI-managed-2088ff)](.github/workflows/ci.yaml)

<!-- simit:badges:end -->

Plans semver-correct version bumps across many repositories and applies
them in dependency order. It handles Cargo packages and Python (PEP 621)
projects, and runs as an MCP server for agents or as a CLI.

```sh
harbor-mcp plan --repo ../core --repo ../app        # read-only plan
harbor-mcp plan --repo . --json > plan.json
harbor-mcp apply plan.json                           # write the version edits
harbor-mcp inventory --repo .                        # versions, baselines, drift
harbor-mcp mcp                                       # stdio MCP server
```

## What it decides

For every package it finds:

1. **Baseline.** The last release is a tag naming the current version
   (`name/vX`, `name-vX`, `name@X`, or `vX`/`X` for a repository's root
   or only package). Without one, it is the newest commit that changed the
   version. A package whose version never changed is unreleased, and its
   first release keeps the declared version.
2. **Change level** since the baseline is the strongest signal from:
   - Conventional Commits on the package's paths: `!` or `BREAKING CHANGE`
     is major, `feat` minor, `docs`/`test`/`ci`/`style` nothing, and any
     other commit a patch. Unreleased dependencies in the same repository ship
     inside their dependents, so their paths count too.
   - API comparison against the baseline: `cargo semver-checks` for Cargo
     libraries, `griffe check` for Python packages. Both run only for
     packages that publish and changed.
3. **Release units.** Packages that share a version source (Cargo
   `version.workspace = true`) or `follow` another package move together at
   the strongest member's level.
4. **Propagation.** A released package that depends on a bumped unit gets at
   least a patch, repeated until nothing changes. Dev dependencies and
   dependency groups don't propagate.
5. **Version arithmetic**, with Cargo's pre-1.0 rules: in `0.y.z` a breaking
   change moves `y` and anything else moves `z`; in `0.0.z` every change
   moves `z`; a prerelease is promoted to its release.
6. **Requirements and anchors.** A dependent whose requirement excludes the
   new version gets a rewrite (Cargo) or a manual item (PEP 508). Anchor
   files such as `CITATION.cff` follow their package, and drift is
   reported.

The plan lists packages dependencies-first and names the steps other tools
own.

## What it owns and what it delegates

| Concern | Owner |
| --- | --- |
| Cross-repository package graph, baselines, level, propagation, ordering, pre-1.0 rules, anchors, version edits | harbor-mcp |
| Rust public-API breakage | [cargo-semver-checks](https://github.com/obi1kenobi/cargo-semver-checks) |
| Python public-API breakage | [griffe](https://mkdocstrings.github.io/griffe/) `check` |
| Changelog, release commit, signed tag, crates.io/PyPI publishing, workspace publish order | [simit](https://github.com/caniko/simit) (`simit release`, `simit release plan`) or the repository's own release tool |
| Lockfiles | `cargo update --workspace`, `uv lock` |
| Moving flake-input pins in consuming repositories | Fleetix supply graph (`fleetix_supply_consumers`, `fleetix_supply_land`) and Canix's pin train |

harbor-mcp never commits, tags, publishes or deposits archives.

## MCP tools

| Tool | Effect |
| --- | --- |
| `inventory` | Packages, versions, baselines, units, dependencies, anchors and drift. Reads no history. |
| `plan` | The full bump plan. Read-only. |
| `apply` | Writes a plan's edits. Lists them unless `write: true`. Refuses if a repository moved past the plan or a touched file is dirty. |

```json
{"mcpServers": {"harbor-mcp": {"command": "harbor-mcp", "args": ["mcp"]}}}
```

## Configuration

Each repository may commit a `harbor-mcp.toml`; `--config` (or the MCP
`config` argument) layers another on top.

```toml
exclude = ["tests/fixtures/**"]      # manifest globs to skip

[[package]]
id = "cargo:api"                     # or "pypi:<normalized-name>"
paths = ["."]                        # extra paths whose commits count
anchors = [{ file = "CITATION.cff", key = "version" }]
follow = "cargo:other"               # lockstep with another package
api_check = false                    # skip cargo-semver-checks / griffe
```

`examples/syndb.toml` is the SynDB release policy.

## Limits

- A dependency's breaking change reaches a dependent as a patch unless the
  dependent's own API check catches it. Re-exported types are not traced.
- Python requirement rewrites are listed, not written.
- Anchors are top-level `key: value` lines.
- Versions must be semver-like; others are reported and left alone.
