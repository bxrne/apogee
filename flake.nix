{
  description = "apogee: minimal x86-64 bare-metal kernel";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # Keep this in sync with rust-toolchain.toml. Pinning the date means
        # `nix develop` and `rustup` users compile with the same nightly.
        rustToolchain = pkgs.rust-bin.nightly."2026-05-06".default.override {
          extensions = [
            "rust-src"
            "llvm-tools-preview"
            "rust-analyzer"
          ];
        };

        # bootimage is not packaged in nixpkgs, so build the published crate.
        # 0.10.x is what pairs with the bootloader 0.9 dependency in Cargo.toml.
        bootimage = pkgs.rustPlatform.buildRustPackage {
          pname = "bootimage";
          version = "0.10.5";

          src = pkgs.fetchCrate {
            pname = "bootimage";
            version = "0.10.5";
            hash = "sha256-YvTj/1GKird09nBKv9BHqE/uCTIoj/bTabxz/QFFfBM=";
          };

          cargoHash = "sha256-UylacTDWZVkoyAA7fDsX1v+8TMnynIIfE3v+vxDXxyY=";
          doCheck = false;
        };
      in
      {
        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
            bootimage
            pkgs.qemu
            pkgs.gdb
          ];

          # rust-analyzer uses this to find core/alloc when building with -Zbuild-std.
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
        };

        formatter = pkgs.nixfmt;
      }
    );
}
