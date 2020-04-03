with import <nixpkgs> { };
let
  nixos = pkgs.dockerTools.pullImage {
    imageName = "lnl7/nix:2020-03-07";
    imageDigest =
      "sha256:2ca53dfc7e80084484dabd9221d97d96b6466b453864801962234a75a047949f";
    sha256 = "04k94d1sxzsa467p451xs412qp50rs8id49d7dqgxghjr8ygk7js";

  };

  entry = pkgs.writeShellScriptBin "wrapper" ''
    #!${pkgs.bash}
       echo would run nix-shell --run "$@"
       exec "$@"
  '';
in
pkgs.dockerTools.buildImageWithNixDb {
  name = "jan";
  tag = "latest";
  fromImage = "nixos";

  contents = with pkgs; [ stdenv curl bash busybox cargo rustc libspdk ] ++ mayastor.buildInputs;

  config = {
    Cmd = [ "${entry}/bin/wrapper" ];
    EntryPoint = "${entry}/bin/wrapper";
    #WorkingDir = "/";
  };
}
