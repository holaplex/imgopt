#!/bin/bash
set -e
cd /home/ubuntu
sleep 5
cp s3/config.toml .
cp s3/mp4-to-gif.sh .
cp s3/gifski .
cp s3/imgopt-server .
chmod +x mp4-to-gif.sh
chmod +x gifski
chmod +x imgopt-server
chown ubuntu:ubuntu imgopt-server
while true;do
  sudo -u ubuntu /home/ubuntu/imgopt-server 2> s3/logs/$(date +%Y-%m-%d-%H_%M)-$(hostname).log
  sleep 10
done
