FROM instrumentisto/rust:nightly as build
ENV PKG_CONFIG_ALLOW_CROSS=1

WORKDIR /usr/src/imgopt
RUN cargo init
COPY ./Cargo.toml .
COPY ./src ./src
RUN cargo fetch
COPY ./docker/os_info-1.3.3 /usr/local/cargo/registry/src/github.com-1ecc6299db9ec823/os_info-1.3.3
RUN cargo build --release

FROM debian:stable-slim

WORKDIR /root
RUN apt update -y
RUN apt install gifsicle ffmpeg wget libavformat-dev libavfilter-dev libavdevice-dev libclang-dev clang -y
RUN wget --quiet https://github.com/ImageOptim/gifski/releases/download/1.6.4/gifski_1.6.4_amd64.deb -O gifski.deb
RUN dpkg -i gifski.deb && rm gifski.deb
COPY --from=build /usr/src/imgopt/target/release/imgopt imgopt
COPY config.toml .
COPY scripts/mp4-to-gif.sh .
RUN chmod +x mp4-to-gif.sh
EXPOSE 3030
CMD ["./imgopt"]