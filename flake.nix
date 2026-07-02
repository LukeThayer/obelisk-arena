{
  description = "obelisk-arena dev environment (game + skill-designer editor)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        darwinDeps = pkgs.lib.optionals pkgs.stdenv.isDarwin [
          pkgs.apple-sdk_15
          pkgs.libiconv
        ];

        # Bevy runtime/link deps on Linux (windowed client + editor: winit/wgpu/audio/input)
        linuxDeps = pkgs.lib.optionals pkgs.stdenv.isLinux (with pkgs; [
          vulkan-loader
          xorg.libX11
          xorg.libXcursor
          xorg.libXi
          xorg.libXrandr
          libxkbcommon
          wayland
          alsa-lib
          udev
          libglvnd # EGL for Wayland
        ]);
      in {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = darwinDeps ++ linuxDeps;
          packages = [ rustToolchain pkgs.cargo-watch pkgs.cargo-edit pkgs.git ];

          # Bevy dlopens vulkan/x11/wayland at runtime on Linux
          LD_LIBRARY_PATH = pkgs.lib.optionalString pkgs.stdenv.isLinux
            (pkgs.lib.makeLibraryPath linuxDeps);

          shellHook = ''
            echo "obelisk-arena dev shell — $(rustc --version)"
          '' + pkgs.lib.optionalString pkgs.stdenv.isLinux ''
            export WAYLAND_DISPLAY=''${WAYLAND_DISPLAY:-wayland-1}
            export XDG_RUNTIME_DIR=''${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
          '';
        };
      });
}
