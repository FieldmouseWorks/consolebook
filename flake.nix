{
  description = "Consolebook";

  inputs.burrow.url = "git+https://github.com/FieldmouseWorks/burrow";

  # `nix develop` for the toolchains, `nix flake check` for the gate (see
  # burrow's lib/project.nix). The web, embedded-asset, browser and
  # installer gates need node_modules, Chromium or systemd, so they stay in
  # .github/workflows/ and CONTRIBUTING.md's sequence.
  outputs =
    { self, burrow }:
    burrow.lib.project {
      inherit self;
      # The proof runner's tests (crates/xtask) make throwaway git repositories.
      rust = {
        testInputs = pkgs: [ pkgs.git ];
      };
      python = { };
      node = { };
      # The repository guards read the tracked tree through git, so each runs
      # in a fresh commit of the flake's source (which is the tracked files).
      checks =
        pkgs:
        let
          inRepo =
            name: script:
            pkgs.runCommand name
              {
                nativeBuildInputs = [ pkgs.git ];
              }
              ''
                export HOME=$TMPDIR
                cp -r ${self} repo
                chmod -R u+w repo
                cd repo
                git init -q
                git add -A
                git -c user.name=Invented -c user.email=invented@example.invalid commit -q -m candidate
                ${script}
                touch $out
              '';
        in
        {
          guidance-names = inRepo "guidance-names" "sh scripts/check-guidance-names.sh";
        };
    };
}
