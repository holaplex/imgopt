#!/bin/bash
cd /home/ubuntu
sleep 5
rm -rf s3
goofys imgopt-data2 s3
cp s3/config.toml .
cp s3/mp4-to-gif.sh .
cp s3/imgopt-server .
chmod +x mp4-to-gif.sh
chmod +x imgopt-server
chown ubuntu:ubuntu imgopt-server
sudo chown -R ubuntu:ubuntu s3
while true;do
  sudo -u ubuntu /home/ubuntu/imgopt-server >> s3/logs/$(date +%Y-%m-%d-%H_%M)-$(hostname).log  2>&1
  sleep 10
done
