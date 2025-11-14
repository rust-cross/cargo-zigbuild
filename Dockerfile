ARG PREV_ZIGBUILD_IMG=start9/cargo-zigbuild
ARG RUST_VERSION=1.91.1
ARG BUILDPLATFORM

FROM --platform=$BUILDPLATFORM $PREV_ZIGBUILD_IMG AS builder

ARG TARGETARCH

RUN echo $(if [ $TARGETARCH = amd64 ]; then \
    echo x86_64; \
    elif [ $TARGETARCH = arm64 ]; then \
    echo aarch64; \
    elif [ $TARGETARCH = riscv64 ]; then \
    echo riscv64gc; \
    else \
    echo $TARGETARCH; \
    fi)-unknown-linux-gnu > /tmp/rust-target-triple

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN rustup target add $(cat /tmp/rust-target-triple)

# Compile dependencies only for build caching
ADD Cargo.toml /cargo-zigbuild/Cargo.toml
ADD Cargo.lock /cargo-zigbuild/Cargo.lock
RUN mkdir /cargo-zigbuild/src && \
    touch  /cargo-zigbuild/src/lib.rs && \
    cargo zigbuild --target=$(cat /tmp/rust-target-triple) --manifest-path /cargo-zigbuild/Cargo.toml --release

# Build cargo-zigbuild
ADD . /cargo-zigbuild/
# Manually update the timestamps as ADD keeps the local timestamps and cargo would then believe the cache is fresh
RUN touch /cargo-zigbuild/src/lib.rs /cargo-zigbuild/src/bin/cargo-zigbuild.rs
RUN cargo zigbuild --target=$(cat /tmp/rust-target-triple) --manifest-path /cargo-zigbuild/Cargo.toml --release

FROM rust:$RUST_VERSION-trixie

# Install Zig
ARG ZIG_VERSION=0.15.2
RUN curl -L "https://ziglang.org/download/${ZIG_VERSION}/zig-$(uname -m)-linux-${ZIG_VERSION}.tar.xz" | tar -J -x -C /usr/local && \
    ln -s "/usr/local/zig-$(uname -m)-linux-${ZIG_VERSION}/zig" /usr/local/bin/zig

# Install macOS SDKs
RUN curl -L "https://github.com/phracker/MacOSX-SDKs/releases/download/11.3/MacOSX10.9.sdk.tar.xz" | tar -J -x -C /opt
RUN curl -L "https://github.com/phracker/MacOSX-SDKs/releases/download/11.3/MacOSX11.3.sdk.tar.xz" | tar -J -x -C /opt
ENV SDKROOT=/opt/MacOSX11.3.sdk

# Install Rust targets
RUN rustup target add \
    x86_64-unknown-linux-gnu \
    x86_64-unknown-linux-musl \
    aarch64-unknown-linux-gnu \
    aarch64-unknown-linux-musl \
    riscv64gc-unknown-linux-gnu \
    riscv64gc-unknown-linux-musl \
    arm-unknown-linux-gnueabihf \
    arm-unknown-linux-musleabihf \
    x86_64-apple-darwin \
    aarch64-apple-darwin \
    x86_64-pc-windows-gnu \
    aarch64-pc-windows-gnullvm

RUN --mount=type=bind,from=builder,source=/,target=/mnt/ \
    cp /mnt/cargo-zigbuild/target/$(cat /mnt/tmp/rust-target-triple)/release/cargo-zigbuild /usr/local/cargo/bin/
