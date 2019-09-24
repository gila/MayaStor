{ stdenv, binutils, libaio, libuuid, numactl, openssl, python, rdma-core
, fetchFromGitHub, callPackage}:

let
  libiscsi = callPackage ../libiscsi{};
in

stdenv.mkDerivation rec {
  version = "19.07.x-mayastor";
  pname = "spdk";
  src = fetchFromGitHub {
    rev = "1274d250a6f49731aecbbcc925fff208a25f4b95";
    repo = "spdk";
    owner = "openebs";
    sha256 = "148dp6nm8a2dglc8wk5yakjjd8r6s6drn1afff5afafa50fkcjgd";
    fetchSubmodules = true;
  };

  buildInputs = [
    libiscsi
    binutils
    libaio
    libuuid
    numactl
    openssl
    python
    rdma-core
  ];

  RTE_TARGET = "x86_64-nhm-linuxapp-gcc";

  CONFIGURE_OPTS =
    "--enable-debug --with-iscsi-initiator --with-rdma --with-internal-vhost-lib --disable-tests";
  enableParallelBuilding = true;
  postPatch = "patchShebangs configure scripts/detect_cc.sh";

  #preConfigure = ''
  #	cat >>${src}/dpdk/config/defconfig_$RTE_TARGET <<EOF
  #	include "common_linux"
  #	CONFIG_RTE_MACHINE="default"
  #	CONFIG_RTE_ARCH="x86_64"
  #	CONFIG_RTE_ARCH_X86_64=y
  #	CONFIG_RTE_ARCH_X86=y
  #	CONFIG_RTE_ARCH_64=y
  #	CONFIG_RTE_TOOLCHAIN="gcc"
  #	CONFIG_RTE_TOOLCHAIN_GCC=y
  #	EOF
  #	'';

  NIX_CFLAGS_COMPILE = "-mno-movbe -mno-lzcnt -mno-bmi -mno-bmi2 -march=corei7";
  hardeningDisable = [ "all" ];

  configurePhase = ''
    ./configure $CONFIGURE_OPTS
  '';

  buildPhase = ''
    TARGET_ARCHITECTURE=corei7 make -j4

    find . -type f -name 'libspdk_ut_mock.a' -delete
    find . -type f -name 'librte_vhost.a' -delete

    $CC -shared -o libspdk_fat.so \
    -lc -lrdmacm -laio -libverbs -liscsi -lnuma -ldl -lrt -luuid -lpthread -lcrypto \
    -Wl,--whole-archive $(find build/lib -type f -name 'libspdk_*.a*' -o -name 'librte_*.a*') \
    -Wl,--whole-archive $(find dpdk/build/lib -type f -name 'librte_*.a*') \
    -Wl,--no-whole-archive
  '';

  installPhase = ''
    mkdir -p $out/lib
    cp libspdk_fat.so $out/lib
  '';

  dontStrip = true;
}


