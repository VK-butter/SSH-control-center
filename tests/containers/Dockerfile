# Isolated acceptance image: build on Ubuntu 24.04 like the release DEB; executor_linux needs libsystemd >= 254.
FROM ubuntu:24.04 AS build
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends build-essential pkg-config libsystemd-dev openssh-client ca-certificates curl && rm -rf /var/lib/apt/lists/*
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.95.0
ENV PATH=/root/.cargo/bin:$PATH
WORKDIR /src
COPY . .
RUN cargo test --locked -p sshcc-core -- --test-threads=1
RUN cargo build --locked --release -p sshcc

FROM ubuntu:24.04 AS runtime
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends openssh-server openssh-client python3 ca-certificates procps libsystemd0 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/sshcc /usr/bin/sshcc
RUN useradd --create-home --uid 1100 --shell /bin/sh sshcc-bridge && useradd --create-home --uid 1101 --shell /bin/sh unprivileged && mkdir -p /run/sshd /run/sshcc && chmod 755 /run/sshcc
CMD ["sleep", "infinity"]
