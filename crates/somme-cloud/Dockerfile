# Somme Cloud server image. Built from the repo root by `fly deploy`.
FROM rust:1.97-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/somme-cloud crates/somme-cloud
# Only the cloud crate is copied in, so cut the workspace down to it.
RUN sed -i '/^members = \[/,/^\]/c members = ["crates/somme-cloud"]' Cargo.toml \
    && cargo build --release -p somme-cloud

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/somme-cloud /usr/local/bin/somme-cloud
ENV SOMME_CLOUD_ADDR=0.0.0.0:8080 SOMME_CLOUD_DATA=/data
EXPOSE 8080
CMD ["somme-cloud"]
