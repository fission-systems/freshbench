# FreshBench build image: latest GCC release (16.2.0, official image) and the
# latest LLVM release (23.1.6, upstream binary tarball), x86-64 Linux.
# The fixed package set below IS the "few dependencies" rule: a project
# qualifies only if it builds with exactly this image, no per-project packages.
FROM gcc:16.2.0-trixie
ARG LLVM_VER=23.1.6
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      cmake meson ninja-build bison flex gettext autopoint texinfo pkg-config \
      python3 python3-pyelftools file xz-utils zstd ca-certificates git \
      libncurses-dev libreadline-dev zlib1g-dev libbz2-dev liblzma-dev libssl-dev \
      libcurl4-openssl-dev libpcre2-dev libexpat1-dev libsqlite3-dev libevent-dev \
      libjson-c-dev libyaml-dev libxml2-dev libpcap-dev libffi-dev libuv1-dev \
      libjansson-dev libmagic-dev libedit-dev libcap-dev libseccomp-dev libpopt-dev \
      libargon2-dev libsodium-dev libzstd-dev liblz4-dev libgmp-dev libmpfr-dev \
      libsdl2-dev libx11-dev libgtk-3-dev libglib2.0-dev libusb-1.0-0-dev \
      libmnl-dev libnl-3-dev libnl-genl-3-dev libudev-dev libsystemd-dev \
      libcmocka-dev check \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL -o /tmp/llvm.tar.xz \
      https://github.com/llvm/llvm-project/releases/download/llvmorg-${LLVM_VER}/LLVM-${LLVM_VER}-Linux-X64.tar.xz \
    && mkdir -p /opt/llvm && tar -xJf /tmp/llvm.tar.xz -C /opt/llvm --strip-components=1 \
    && rm /tmp/llvm.tar.xz && /opt/llvm/bin/clang --version
COPY fbcc.py /usr/local/bin/fbcc
RUN chmod +x /usr/local/bin/fbcc \
    && ln -s fbcc /usr/local/bin/fbcc-gcc && ln -s fbcc /usr/local/bin/fbcc-clang \
    && ln -s fbcc /usr/local/bin/fbcxx-gcc && ln -s fbcc /usr/local/bin/fbcxx-clang
COPY build_project.py /usr/local/bin/build_project
RUN chmod +x /usr/local/bin/build_project
