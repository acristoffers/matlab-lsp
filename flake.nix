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
              "tree-sitter-matlab-1.0.7" = "sha256-7I6ihIWBx8PqULbPzlrlI4BkrnEf7geT5xx08WjWQWg=";
              "matlab_beautifier-1.0.0" = "sha256-jBbkb37PMBUJpmRKxWNYQlrnqNfBPxzsTCWfYcBqsBA=";
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
