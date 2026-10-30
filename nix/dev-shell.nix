{ pkgs }:
let
  toolchainSpec = (builtins.fromTOML (builtins.readFile ../rust-toolchain.toml)).toolchain;
  toolchain = (pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml).override {
    extensions = toolchainSpec.components ++ [ "rust-src" "rust-analyzer" ];
  };
  desktopLibraries = with pkgs; [
    libxkbcommon
    wayland
    libGL
    libX11
    libXcursor
    libXi
    libXrandr
  ];
in pkgs.mkShell {
  name = "tursobench-dev";

  packages = with pkgs; [
    toolchain
    gcc
    pkg-config
    python3
    xvfb-run
  ];

  buildInputs = desktopLibraries;
  RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";

  # eframe loads these libraries dynamically. Preserve any existing driver paths.
  shellHook = ''
    export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath desktopLibraries}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  '';
}
