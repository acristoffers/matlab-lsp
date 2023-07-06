{
  inputs = {
    flake-utils.url = github:numtide/flake-utils;
    naersk.url = github:nix-community/naersk;
    nixpkgs.url = github:NixOS/nixpkgs/nixpkgs-unstable;
  };
  outputs = { self, flake-utils, naersk, nixpkgs }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        version = "1.0.0";
        pkgs = (import nixpkgs) { inherit system; };
        naersk' = pkgs.callPackage naersk { };
        buildInputs = with pkgs; [ ];
        mkPackage = { name, buildInputs ? [ ] }: naersk'.buildPackage {
          inherit buildInputs;
          inherit name;
          inherit version;
          nativeBuildInputs = with pkgs;[ cmake pkgconfig ];
          src = ./.;
          postInstall = "
            cp -r target/release/share $out/share
          ";
        };
      in
      rec {
        formatter = nixpkgs.legacyPackages.${system}.nixpkgs-fmt;
        packages.matlab-lsp = mkPackage { name = "matlab-lsp"; };
        packages.default = packages.matlab-lsp;
        devShell = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [ rustc cargo ];
          inherit buildInputs;
        };
      }
    );
}
