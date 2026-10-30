# cryoem-context-bridge as a container.
#
# Build (repo root):
#   docker build -f deploy/Dockerfile -t scigantic-cryoem-context:local .
# Run against a broker:
#   docker run --rm -e CRYOEM_BRIDGE_BROKERS=broker:9092 scigantic-cryoem-context:local run
#
# librdkafka and OpenSSL are compiled into the binary (the crate's
# ssl-vendored and libz-static features), so the runtime image needs only a
# C library. distroless/cc has that, CA roots, and no shell. The HTTPS calls
# to EBI use rustls with bundled roots and do not depend on the image's CA
# bundle.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked && strip target/release/cryoem-context-bridge

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/cryoem-context-bridge /usr/local/bin/cryoem-context-bridge
USER nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/cryoem-context-bridge"]
CMD ["run"]
