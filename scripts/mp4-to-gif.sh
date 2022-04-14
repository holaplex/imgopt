#!/bin/bash
source_path=$1
width=$2
height=$3
output_path=$4
hash=$(md5sum ${source_path} | awk {'print $1'} | cut -c1-8)

mkdir -p tmp
#echo "working with hash ${hash}"
ffmpeg -loglevel quiet -i ${source_path} tmp/${hash}-frame-%04d.png
#echo "converted mp4 to pngs"
gifski --fps 30 --width ${width} --height ${height} -o "${output_path}" --quiet tmp/${hash}-frame-*
#echo "converted pngs to gif"
find tmp -type f -iname "${hash}*" -exec rm {} \;
#echo "removed leftover frames"
