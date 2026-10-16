# Linux test image: builds texttop with every default feature (WebKitGTK
# browser engine, ALSA audio), runs the unit and golden render tests, then
# drives the real binary through a pseudo-terminal under a virtual X server.
#
#   docker build -f tests/linux/Dockerfile -t texttop-linux-test .
#   docker run --rm texttop-linux-test
#
# Mount a folder on /out to keep the screen dumps of every E2E step:
#
#   docker run --rm -v "$PWD/target/linux-e2e:/out" texttop-linux-test

FROM rust:1-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        libwebkit2gtk-4.1-dev libasound2-dev \
        xvfb xauth dbus-x11 \
        python3 python3-pip git ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && pip3 install --no-cache-dir --break-system-packages pyte==0.8.2

WORKDIR /src

# Build the dependencies once, in their own layer, so editing texttop's code
# does not rebuild WebKit bindings and friends.
COPY Cargo.toml Cargo.lock ./
COPY vendor vendor
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && cargo test --release --no-run \
    && rm -rf src

COPY . .
# Make sure cargo sees the real sources as newer than the placeholder.
RUN touch src/main.rs \
    && cargo build --release \
    && cargo test --release --no-run

ENV SHELL=/bin/bash
CMD ["sh", "tests/linux/run.sh"]
