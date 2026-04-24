{
  pkgs ? import <nixpkgs> { },
}:

pkgs.mkShell {
  packages = with pkgs; [
    cargo
    rustc
    rustfmt
    pkg-config
    SDL2
  ];

  # Prefer native Wayland on GNOME. If needed, override at runtime with:
  #   SDL_VIDEODRIVER=x11 cargo run --release
  shellHook = ''
    export SDL_VIDEODRIVER=''${SDL_VIDEODRIVER:-wayland}
    echo "GoldLaceRust dev shell"
    echo "  cargo run --release"
    echo "  SDL_VIDEODRIVER=$SDL_VIDEODRIVER"
  '';
}
