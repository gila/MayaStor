{ nospdk ? false, norust ? false }:
let
  sources = import ./nix/sources.nix;
  pkgs = import sources.nixpkgs {
    overlays =
      [ (_: _: { inherit sources; }) (import ./nix/mayastor-overlay.nix) ];
  };
in with pkgs;
let
  nospdk_moth =
    "You have requested environment without SPDK, you should provide it!";
  norust_moth =
    "You have requested environment without RUST, you should provide it!";
  channel = import ./nix/lib/rust.nix { inherit sources; };
  # python environment for test/python
  pytest_inputs = python3.withPackages
    (ps: with ps; [ virtualenv grpcio grpcio-tools asyncssh ]);
in mkShell {

  # fortify does not work with -O0 which is used by spdk when --enable-debug
  hardeningDisable = [ "fortify" ];
  buildInputs = [
    clang_11
    cowsay
    docker
    docker-compose
    e2fsprogs
    envsubst # for e2e tests
    etcd
    fio
    gdb
    git
    go
    gptfdisk
    kind
    kubectl
    kubernetes-helm
    libaio
    libiscsi
    libudev
    liburing
    llvmPackages_11.libclang
    meson
    nats-server
    ninja
    nodejs-16_x
    numactl
    nvme-cli
    nvmet-cli
    openssl
    pkg-config
    pre-commit
    procps
    python3
    pytest_inputs
    utillinux
    xfsprogs
  ] ++ (if (nospdk) then [ libspdk-dev.buildInputs ] else [ libspdk-dev ])
    ++ pkgs.lib.optional (!norust) channel.nightly.rust;

  LIBCLANG_PATH = mayastor.LIBCLANG_PATH;
  PROTOC = mayastor.PROTOC;
  PROTOC_INCLUDE = mayastor.PROTOC_INCLUDE;
  SPDK_PATH = if nospdk then null else "${libspdk-dev}";

  shellHook = ''
    ${pkgs.lib.optionalString (nospdk) "cowsay ${nospdk_moth}"}
    ${pkgs.lib.optionalString (nospdk) "export CFLAGS=-msse4"}
    ${pkgs.lib.optionalString (nospdk)
    ''export RUSTFLAGS="-C link-args=-Wl,-rpath,$(pwd)/spdk-sys/spdk"''}
    ${pkgs.lib.optionalString (nospdk) "echo"}
    ${pkgs.lib.optionalString (norust) "cowsay ${norust_moth}"}
    ${pkgs.lib.optionalString (norust) "echo 'Hint: use rustup tool.'"}
    ${pkgs.lib.optionalString (norust) "echo"}

    # SRCDIR is needed by docker-compose files as it requires absolute paths
    export SRCDIR=`pwd`
    # python compiled proto files needed by pytest
    python -m grpc_tools.protoc -I `realpath rpc/proto` --python_out=test/python --grpc_python_out=test/python mayastor.proto
    virtualenv --no-setuptools test/python/venv
    source test/python/venv/bin/activate 
    pip install -r test/python/requirements.txt
    pre-commit install
    pre-commit install --hook commit-msg
  '';
}
