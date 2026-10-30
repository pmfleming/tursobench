# Legacy nix-shell entry point; shares the flake's pinned inputs and shell.
let
  lock = builtins.fromJSON (builtins.readFile ./flake.lock);
  fetchInput = name:
    let source = lock.nodes.${name}.locked;
    in builtins.fetchTarball {
      url = "https://github.com/${source.owner}/${source.repo}/archive/${source.rev}.tar.gz";
      sha256 = source.narHash;
    };
  pkgs = import (fetchInput "nixpkgs") {
    overlays = [ (import (fetchInput "rust-overlay")) ];
  };
in import ./nix/dev-shell.nix { inherit pkgs; }
