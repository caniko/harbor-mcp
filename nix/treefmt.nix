{
  harbor-meta,
  harbor-rs,
  rustfmtPackage,
}: _: {
  imports = [
    harbor-meta.treefmtModules.nix
    harbor-meta.treefmtModules.toml
    harbor-meta.treefmtModules.markdown
    harbor-meta.treefmtModules.shell
    harbor-meta.treefmtModules.svg
    harbor-rs.treefmtModules.rust
  ];
  projectRootFile = "flake.nix";
  # simit owns workflow serialization.
  settings.excludes = [".github/workflows/*.yaml"];
  programs.rustfmt = {
    package = rustfmtPackage;
    edition = "2024";
  };
}
