# Compatibility shim for `nix-shell`; the real environment lives in flake.nix.
(builtins.getFlake (toString ./.)).devShells.${builtins.currentSystem}.default
