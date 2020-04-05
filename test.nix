# A fat and modifiable Nix image
with import <nixpkgs> { };

let
  bintools = binutils-unwrapped.overrideAttrs (o: rec { meta.priority = 9; });
  libclang = llvmPackages.libclang.overrideAttrs(o : rec {
    meta.priority = 4;
  });

  libc = glibc.overrideAttrs(o: rec{
    meta.priority = 4;
  });
  # useful things to be included within the container
  core = [
    bashInteractive
    bintools
    cacert
    coreutils
    findutils
    git
    gnugrep
    gnused
    gzip
    iproute # vscode
    less
    nix
    procps
    libc
    stdenv.cc
    stdenv.cc.cc.lib
    xz
  ];

  # things we need for rust
  rust = [ rustup libclang protobuf ];
  node = [ nodejs ];

  # generate a user profile for the image
  profile = mkUserEnvironment {
    derivations = [
      libaio
      libiscsi.lib
      libspdk
      liburing
      numactl
      openssl
      rdma-core
      utillinux
      utillinux.dev
    ] ++ core ++ rust ++ node;
  };

  image = dockerTools.buildImage {
    name = "mayadata/ms-buildenv";
    tag = "nix";

    extraCommands = ''
       # create the Nix DB
       export NIX_REMOTE=local?root=$PWD
       export USER=nobody
       ${nix}/bin/nix-store --load-db < ${
         closureInfo { rootPaths = [ profile ]; }
       }/registration

       # set the user profile
       ${profile}/bin/nix-env --profile nix/var/nix/profiles/default --set ${profile}

       # minimal
       mkdir -p bin usr/bin
       ln -s /nix/var/nix/profiles/default/bin/sh bin/sh
       ln -s /nix/var/nix/profiles/default/bin/env usr/bin/env
       ln -s /nix/var/nix/profiles/default/bin/bash bin/bash

      # setup shadow, bashrc
       cp -r ${./root/etc} etc
       chmod +w etc etc/group etc/passwd etc/shadow
       # make sure /tmp exists which is used by cargo
       mkdir -m 0777 tmp
       # used for our RPC socket
       mkdir -p -m 0777 var/tmp

       # allow ubuntu ELF binaries to run. VSCode copies it's own.
       mkdir -p lib64
       ln -s ${stdenv.glibc}/lib64/ld-linux-x86-64.so.2 lib64/ld-linux-x86-64.so.2

       # VSCode assumes that /sbin/ip exists
       mkdir sbin
       ln -s /nix/var/nix/profiles/default/bin/ip sbin/ip

       # you must still call nix-channel update if you wish to install something
       mkdir root
       echo 'https://nixos.org/channels/nixpkgs-unstable' > root/.nix-channels
    '';

    config = {
      Cmd = [ "/nix/var/nix/profiles/default/bin/bash" ];
      Env = [
        # set some environment variables so that cargo can find them
        "PROTOC_INCLUDE=${protobuf}/include"
        "LIBCLANG_PATH=${llvmPackages.libclang}/lib"
        "LOCAL_ACRHIVE=${glibc}/lib/locale/locale-archive"
        "C_INCLUDE_PATH=/nix/var/nix/profiles/default/include/spdk:/nix/var/nix/profiles/default/include"
        "LIBRARY_PATH=/nix/var/nix/profiles/default/lib"
        "LD_LIBRARY_PATH=/nix/var/nix/profiles/default/lib"
        "ENV=/nix/var/nix/profiles/default/etc/profile.d/nix.sh"

        "PAGER=less"
        "PATH=/nix/var/nix/profiles/default/bin"
        "SSL_CERT_FILE=/nix/var/nix/profiles/default/etc/ssl/certs/ca-bundle.crt"
        "GIT_SSL_CAINFO=/nix/var/nix/profiles/default/etc/ssl/certs/ca-bundle.crt"
        "RUST_BACKTRACE=1"
      ];
      Labels = {
        # https://github.com/microscaling/microscaling/blob/55a2d7b91ce7513e07f8b1fd91bbed8df59aed5a/Dockerfile#L22-L33
        "org.label-schema.vcs-ref" = "master";
        "org.label-schema.vcs-url" = "https://github.com/gila/mayastor";
      };
    };
  };
in image // {
  meta = image.meta // { description = "Mayastor development container"; };
}
