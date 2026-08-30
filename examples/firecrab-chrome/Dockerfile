FROM rust:1.85-alpine AS build

RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM alpine:3.24

RUN apk add --no-cache xvfb chromium
COPY --from=build /src/target/release/microbox /usr/local/bin/microbox

EXPOSE 5943
ENTRYPOINT ["microbox", "agent", "chromium", "--", "--no-sandbox", "--disable-dev-shm-usage"]
