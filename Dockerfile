#FROM rustlang/rust:nightly-buster-slim as build
FROM messense/rust-musl-cross:x86_64-musl as build
RUN rustup update beta && \
    rustup target add --toolchain beta x86_64-unknown-linux-musl
ENV PKG_CONFIG_ALLOW_CROSS=1

WORKDIR /usr/src/imgopt
RUN cargo init
COPY ./Cargo.toml .
COPY ./src ./src
RUN cargo fetch
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM debian:stable-slim

WORKDIR /root
RUN apt update -y
#Install gifski and dependencies for FFmpeg
RUN apt install gifsicle ffmpeg wget libavformat-dev libavfilter-dev libavdevice-dev libclang-dev clang git file -y
RUN wget --quiet https://github.com/ImageOptim/gifski/releases/download/1.6.4/gifski_1.6.4_amd64.deb -O gifski.deb
RUN dpkg -i gifski.deb && rm gifski.deb
#Preparing Env
RUN useradd --create-home --shell /bin/bash imgopt
WORKDIR /home/imgopt
COPY --from=build /usr/src/imgopt/target/x86_64-unknown-linux-musl/release/imgopt imgopt
#Config should be provided from mount point/configMap (Use config-sample.toml as guide)
COPY scripts/mp4-to-gif.sh .
RUN chown imgopt:imgopt mp4-to-gif.sh imgopt
RUN chmod +x mp4-to-gif.sh imgopt
USER imgopt
EXPOSE 3030
CMD ["./imgopt"]
