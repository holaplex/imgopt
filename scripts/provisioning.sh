#!/bin/bash
bucket_name="imgopt-data2"
cargo build --release
aws s3 cp ./target/release/imgopt s3://${bucket_name}/imgopt-server-new
aws s3 cp config.toml s3://${bucket_name}/config.toml
aws s3 cp ./scripts/mp4-to-gif.sh s3://${bucket_name}/mp4-to-gif.sh
aws s3 cp ./scripts/auto-start.sh s3://${bucket_name}/auto-start.sh
