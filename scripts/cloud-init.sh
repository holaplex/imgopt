sudo apt-get update -y
sudo apt-get install automake autotools-dev fuse g++ git libcurl4-gnutls-dev libfuse-dev libssl-dev libxml2-dev make pkg-config binutils git -y
#FFMPEG / Gif to mp4 Dependencies
sudo apt install -y libavformat-dev gifsicle libavfilter-dev  libavdevice-dev libclang-dev clang ffmpeg -y
git clone https://github.com/s3fs-fuse/s3fs-fuse.git
cd s3fs-fuse && ./autogen.sh && ./configure --prefix=/usr --with-openssl && make
sudo make install && which s3fs
cd ../
#Installing goofys (to mount s3 bucket in filesystem)
wget --quiet https://github.com/kahing/goofys/releases/latest/download/goofys -O goofys
sudo cp ./goofys /usr/local/bin/goofys
sudo chmod +x /usr/local/bin/goofys
#Installing giski from package
wget --quiet https://github.com/ImageOptim/gifski/releases/download/1.6.4/gifski_1.6.4_amd64.deb -O gifski.deb
sudo dpkg -i gifski.deb
#Mounting s3 bucket
rm -rf /home/ubuntu/s3
mkdir -p s3
goofys imgopt-data2 s3
mkdir -p s3/logs
chown -R ubuntu:ubuntu s3
#Retrieving latest version of imgopt, config and tooling
cp s3/imgopt-server .
cp s3/mp4-to-gif.sh .
cp s3/config.toml .
chmod +x ./imgopt-server
chmod +x ./mp4-to-gif.sh
#Retrieving rc.local script to start imgopt on boot
cp s3/auto-start.sh .
nohup ./imgopt-server >> s3/logs/$(date +%Y-%m-%d-%H_%M)-$(hostname).log  2>&1 &
sudo su
cp auto-start.sh /etc/rc.local
chmod 755 /etc/rc.local
chmod +x /etc/rc.local
#Disable cloud init after initial setup
#touch /etc/cloud/cloud-init.disabled
#dpkg-reconfigure cloud-init
# Add s3 mount to fstab
#echo 'goofys#imgopt-data2     /home/ubuntu/s3  fuse     _netdev,allow_other,--file-mode=0666,--dir-mode=0777    0       0' >> /etc/fstab
exit
#Starting server
