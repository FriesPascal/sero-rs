# build
FROM docker.io/rust:1.68-alpine3.17 AS build

ARG PROFILE=debug
ARG UID=10001
ARG GID=10001

RUN apk update && \
    apk add libc-dev libressl-dev 
RUN rustup target add x86_64-unknown-linux-musl
RUN addgroup -g $GID -S sero && \
    adduser -G sero -u $UID -S sero

WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --target x86_64-unknown-linux-musl && \
    mkdir -p /target && \
    cp /src/target/x86_64-unknown-linux-musl/$PROFILE/sero /target/

# package
FROM scratch

ARG UID=10001
ARG GID=10001

COPY --from=build /etc/passwd /etc/group /etc/
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=build /target/* /usr/bin/

USER $UID:$GID
LABEL org.opencontainers.image.source=https://github.com/FriesPascal/sero-rs

ENTRYPOINT ["sero"]
