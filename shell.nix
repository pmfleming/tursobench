{ pkgs ? import <nixpkgs> {} }:
pkgs.mkShell {
  packages = with pkgs; [ cargo rustc rustfmt clippy gcc pkg-config python3 ];
  buildInputs = with pkgs; [ libxkbcommon wayland libGL libX11 libXcursor libXi libXrandr ];
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [ libxkbcommon wayland libGL libX11 libXcursor libXi libXrandr ]);
}
