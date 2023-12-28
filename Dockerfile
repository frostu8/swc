FROM rust:1.74 AS builder

WORKDIR /usr/src/swc
COPY . .

RUN apt-get update && apt-get install -y cmake
RUN cargo install --path .

FROM debian:bookworm

RUN printf "deb http://deb.debian.org/debian bookworm-backports main" > /etc/apt/sources.list.d/backports.list
RUN apt-get update && apt-get install -y -t bookworm-backports yt-dlp ffmpeg && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/swc /usr/local/bin/swc

CMD ["swc"]
