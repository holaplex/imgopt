# Description
`imgopt` is an image processing proxy with a very simple API, created to download and dynamically scaledown, convert, and cache different media formats on the fly to allow faster delivery for the [Holaplex](holaplex.com) Storefront.

# Supported actions
  - Scaledown `JPEG`, `PNG`
  - Scaledown `GIF` (via [gifsicle](https://github.com/kohler/gifsicle))
  - Convert `MP4` to `GIF` (via [ffmpeg](https://github.com/FFmpeg/FFmpeg) and [gifski](https://github.com/ImageOptim/gifski))


# Getting started

Quickest way to start playing with `imgopt` is by launching the server via docker.
By default, only proxying images from `ipfs.io` service is allowed (This can be changed by creating your own config. Use [config-sample.toml](config-sample.toml) as a guide to create your own `config.toml` file.

## Quick start
```bash
docker run --network=host  mpwsh/imgopt:0.1.8
```
Open [http://localhost:3030/health](http://localhost:3030/health) to validate your server is running. You should see **200 OK**.


To get a scaled down version of an image or video, just make a GET request to your `imgopt` instance providing the desired `width` as a query parameter and the `service` in the URL path.
The URL structure should look like this:

```text
http://localhost:3030/<service>/<image-to-scaledown>?width=<desired-width>
```

In order to, for example, scale down [this JPG image](https://ipfs.io/ipfs/bafybeih26pot7dyvqkjabsx75fuypf6cy6derd6tojnfpctja75a2j7uk4), you should point your browser to:
```text
http://localhost:3030/ipfs/bafybeih26pot7dyvqkjabsx75fuypf6cy6derd6tojnfpctja75a2j7uk4?width=600
```

[Click here to see it in action](http://localhost:3030/ipfs/bafybeih26pot7dyvqkjabsx75fuypf6cy6derd6tojnfpctja75a2j7uk4?width=600)

Change the width to get the image size you want (Use a size from the `allowed_sizes` array in your config file. The following requests with same `width` will be served from cache directly and skip conversion entirely.
To get the original image **remove** the `?width=` parameter.

`MP4` files work the same way, but those will be converted to `GIF` automatically and then scaled down to the desired `width`.

`imgopt` will create two folders inside the path specified in `storage_path` variable on the `config.toml` file to store the original and modified images and videos on start-up.
If you will run `imgopt` from the container image, remember to mount a volume to persist the cached and original files in a folder on your control and send the modified config to the container as well.

```bash
mkdir imgopt-data
docker run -d --network=host -v $(pwd)/imgopt-data:/root/imgopt-data -v $(pwd)/config.toml:/root/config.toml mpwsh/imgopt:0.1.6
```
## Customizing your configuration
The config file is pretty straightforward and all values are commented with a small description for ease of customization.
To add more services to **proxy** and process through `imgopt` just add a new object as the one below, specifying name and uri endpoint.
The endpoint should **NOT** contain a closing forward slash.
```toml
[[services]]
name = "ipfs"
endpoint = "https://ipfs.io/ipfs"
#max age header for media files (Optional, default 31536000 seconds)
cache.max_age = 31536000

[[services]]
name = "arweave"
endpoint = "https://arweave.net"

[[services]]
name = "yourservice"
endpoint = "https://servicewebsite.com"

```

## Twitter request caching support
If you want to use `imgopt` to cache API calls to twitter, you need to set up the env var `TWITTER_BEARER_TOKEN` when executing.


# Building from source
The code in this repository can be built using `cargo` without any further dependencies. Just clone the repo and execute `cargo build --release`.
If you only need `JPEG` and `PNG` resizing you can stop installing things here and just just run the server located in `./target/release/`.
If `MP4` and `GIF` are required on your setup then carry on.

Keep in mind that (as mentioned above) `ffmpeg`, `gifsicle` and `gifski` are required to trigger some conversions, which have their own dependencies.

### Tooling dependencies
`ffmpeg` and `gifsicle` can be installed via `apt` in debian based systems.

```bash
apt install ffmpeg gifsicle libavformat-dev libavfilter-dev libavdevice-dev libclang-dev clang -y
```
`gifski` can be installed using `brew`, or you can download the binary directly from their github repo.

```bash
wget --quiet https://github.com/ImageOptim/gifski/releases/download/1.6.4/gifski_1.6.4_amd64.deb
dpkg -i gifski_1.6.4_amd64.deb
```

Last piece of the puzzle is copying a small wrapper script located in `scripts/mp4-to-gif.sh`, which will take care of calling `ffmpeg` and `gifski` to convert mp4 to gif.

Copy the script on the alongside `imgopt` (both should live in the same folder).
You folder structure should look like this:

```bash
mp4-to-gif.sh
config.toml
imgopt
```

Once everything is setup, you should be able to just execute `./imgopt`.
The configuration being used is printed on startup when using `log_level = "debug"` to help troubleshooting.


# License
See [LICENSE](LICENSE)
