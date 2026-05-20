# syntax=docker/dockerfile:1.23
FROM docker.io/library/rust:alpine AS builder
# add only what's necessary
COPY --parents src/ Cargo.lock Cargo.toml /source/
WORKDIR /source
# compile
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=alpine_cargo_dir \
    --mount=type=cache,target=/source/target,id=automerge_target \
    --mount=type=bind,target=/context \
    touch src/build_script.rs && \
    GIT_DIR=/context/.git cargo build --release --locked && \
    cp /source/target/release/devops-automerge /

# prod image
FROM scratch
COPY --from=builder /devops-automerge /devops-automerge
ENTRYPOINT ["/devops-automerge"]
HEALTHCHECK --start-period=1s --interval=15s --timeout=1s CMD ["/devops-automerge", "health"]
