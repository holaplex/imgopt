#!/bin/bash
cargo build --release
aws s3 cp ./target/release/imgopt s3://imgopt-data/imgopt-server
aws s3 cp config.toml s3://imgopt-data/config.toml
aws s3 cp ./scripts/mp4-to-gif.sh s3://imgopt-data/mp4-to-gif.sh
aws s3 cp ./scripts/auto-start.sh s3://imgopt-data/auto-start.sh
