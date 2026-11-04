{ pkgs }:
let
  toolchainSpec = (builtins.fromTOML (builtins.readFile ../rust-toolchain.toml)).toolchain;
  toolchain = (pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml).override {
    extensions = toolchainSpec.components ++ [ "rust-src" "rust-analyzer" ];
  };
  desktopLibraries = with pkgs; [
    dbus # rfd dynamically loads libdbus-1 to reach the desktop file chooser portal.
    libxkbcommon
    wayland
    libGL
  ];
in pkgs.mkShell {
  name = "tursobench-dev";

  packages = with pkgs; [
    toolchain
    gcc
    pkg-config
    python3
    weston
    wayland-utils
  ];

  buildInputs = desktopLibraries;
  RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";

  # eframe and rfd load these libraries dynamically. Preserve existing driver paths.
  shellHook = ''
    export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath desktopLibraries}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  '';
}
