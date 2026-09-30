# syntax=docker/dockerfile:1
# The benchmark sources and generated artifacts are bind-mounted at runtime.
# Rust matches the crate's minimum version; Go matches tools/go-bench/go.mod.
FROM golang:1.25.0-bookworm AS go-toolchain

FROM rust:1.98.1-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    autoconf \
    automake \
    build-essential \
    ca-certificates \
    curl \
    fontconfig \
    fonts-dejavu-core \
    git \
    libtool \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY --from=go-toolchain /usr/local/go /usr/local/go
ENV PATH="/usr/local/go/bin:${PATH}" \
    CGO_ENABLED=1

WORKDIR /workspace
CMD ["make", "bench-compare"]
