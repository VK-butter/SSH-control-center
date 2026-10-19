# Isolated acceptance image: build using Debian 12 so the CLI also runs on Ubuntu 24.04.
FROM rust:1.95-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends openssh-client && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo test --locked -p sshcc-core -- --test-threads=1
RUN cargo build --locked --release -p sshcc

FROM ubuntu:24.04 AS runtime
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends openssh-server openssh-client python3 ca-certificates procps && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/sshcc /usr/bin/sshcc
RUN useradd --create-home --uid 1100 --shell /bin/sh sshcc-bridge && useradd --create-home --uid 1101 --shell /bin/sh unprivileged && mkdir -p /run/sshd /run/sshcc && chmod 755 /run/sshcc
CMD ["sleep", "infinity"]
