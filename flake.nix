{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = { self, flake-utils, nixpkgs }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        version = "1.0.0";
        pkgs = (import nixpkgs) { inherit system; };
        nativeBuildInputs = with pkgs; [ cmake pkg-config rustc cargo ];
        buildInputs = [ ];
        mkPackage = { name, buildInputs ? [ ] }: pkgs.rustPlatform.buildRustPackage {
          pname = name;
          inherit version;
          inherit buildInputs;
          inherit nativeBuildInputs;
          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "tree-sitter-matlab-1.0.5" = "sha256-iiELNwO4m0lr2Bcowu5zj0VdA2Eg2i5N58MwC7HiGbs=";
              "matlab_beautifier-1.0.0" = "sha256-NgGkjwO6SsevpuvQ6J17w029Pzfa1mszErcG3E42BqQ=";
            };
          };
          src = ./.;
          postInstall = "
            cp -r target/*/release/share $out/share
          ";
        };
      in
      rec {
        formatter = pkgs.nixpkgs-fmt;
        packages.matlab-lsp = mkPackage { name = "matlab-lsp"; };
        packages.default = packages.matlab-lsp;
        apps = rec {
          matlab-lsp = { type = "app"; program = "${packages.default}/bin/matlab-lsp"; };
          default = matlab-lsp;
        };
        devShell = pkgs.mkShell {
          nativeBuildInputs = nativeBuildInputs ++ (with pkgs; [ busybox ]);
          inherit buildInputs;
        };
      }
    );
}
