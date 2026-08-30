FROM rust:1.85-alpine AS build

RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM alpine:3.24

RUN apk add --no-cache xvfb firefox ttf-freefont
COPY --from=build /src/target/release/microbox /usr/local/bin/microbox

# Firefox's content sandbox needs user namespaces that containers commonly
# don't grant; without this it fails to start instead of falling back.
ENV MOZ_DISABLE_CONTENT_SANDBOX=1

EXPOSE 5943
ENTRYPOINT ["microbox", "agent", "firefox"]
