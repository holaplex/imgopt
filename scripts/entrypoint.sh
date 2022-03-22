#!/bin/bash
#set env vars from secrets
#AWS_ACCESS_KEY_ID
#AWS_SECRET_ACCESS_KEY
#AWS_REGION
#set env vars from manifest
#USE_S3
#BUCKET_NAME
#STORAGE_PATH
#LOG_LEVEL
mkdir -p $STORAGE_PATH
if $USE_S3;then
goofys --region $AWS_REGION $BUCKET_NAME $STORAGE_PATH
fi
RUST_LOG=$LOG_LEVEL ./imgopt
