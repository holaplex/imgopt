use crate::utils::*;
use anyhow::Result;
use cmd_lib::*;
use image::{imageops::FilterType, io::Reader, EncodableLayout, ImageFormat};
use log::{error, info};
use mp4::TrackType;
use png::ColorType;
use resize::{Pixel, Type::Triangle};
use rgb::FromSlice;
use std::fs;
use std::io::Cursor;
use std::time::Instant;
use webp_animation::prelude::*;

pub fn resize_webp(data: &[u8], width: u32, animated: bool) -> Result<Vec<u8>> {
    if !animated {
        let img = image::load_from_memory(data)?;
        //early exit
        if width == img.width() {
            return Ok(data.to_vec());
        };
        let (w, h) = calculate_dimensions(img.width(), img.height(), width);

        let img = img.resize_exact(w, h, FilterType::Lanczos3);
        let encoder = webp::Encoder::from_image(&img).unwrap();
        let memory = encoder.encode_lossless();
        let bytes = memory.as_bytes();
        Ok(bytes.to_vec())
    } else {
        let decoder = webp_animation::Decoder::new(data)?;
        //Get animation data
        let frames: Vec<_> = decoder.into_iter().collect();
        let timestamps: Vec<i32> = frames.iter().map(|f| f.timestamp()).collect();
        let frame = frames.get(0).unwrap();
        let img = frame.dimensions();

        //early exit
        if width == img.0 {
            return Ok(data.to_vec());
        };

        let (w, h) = calculate_dimensions(img.0, img.1, width);

        //init encoder
        let mut encoder = Encoder::new_with_options(
            (w, h),
            EncoderOptions {
                kmin: 3,
                kmax: 5,
                encoding_config: Some(EncodingConfig {
                    quality: 75.,
                    encoding_type: EncodingType::Lossy(LossyEncodingConfig {
                        segments: 2,
                        alpha_compression: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )?;

        for (i, frame) in webp_animation::Decoder::new(data)?.into_iter().enumerate() {
            let mut buff = Cursor::new(Vec::new());
            frame.into_image()?.write_to(&mut buff, ImageFormat::WebP)?;
            let img = image::load_from_memory(&buff.into_inner())?;
            let mut buff = Cursor::new(Vec::new());
            img.resize_exact(w, h, FilterType::Lanczos3)
                .write_to(&mut buff, ImageFormat::WebP)?;
            let frame = webp_animation::Decoder::new(&buff.into_inner())?
                .into_iter()
                .next()
                .unwrap();
            let timestamp = timestamps.get(i).unwrap();
            encoder.add_frame(frame.data(), *timestamp)?;
        }
        let final_timestamp = *timestamps.last().unwrap() * frames.len() as i32;
        let bytes = encoder.finalize(final_timestamp)?;
        Ok(bytes.to_vec())
    }
}

pub fn mp4_to_gif(input_path: &str, output_path: &str, width: u32) -> Result<Vec<u8>> {
    let start = Instant::now();
    let f = fs::File::open(input_path).unwrap();
    let mp4 = mp4::read_mp4(f).unwrap();
    let video_tracks: Vec<_> = mp4
        .tracks()
        .values()
        .into_iter()
        .filter(|t| t.track_type().unwrap() == TrackType::Video)
        .collect();

    let (w, h) = calculate_dimensions(
        video_tracks[0].width() as u32,
        video_tracks[0].height() as u32,
        width,
    );

    let mut handle = spawn!(./mp4-to-gif.sh ${input_path} ${w} ${h} ${output_path})?;

    if handle.wait().is_err() {
        error!("Unable to convert mp4 to gif with run_cmd.. Falling back to original file");
        read_from_file(input_path)
    } else {
        info!(
            "Converted mp4 to gif and resized to {} px in {}",
            width,
            Elapsed::from(&start)
        );
        read_from_file(output_path)
    }
}
pub fn resize_gif(input_path: &str, output_path: &str, width: u32) -> Result<Vec<u8>> {
    let start = Instant::now();
    let file = fs::File::open(input_path)?;
    let og_gif = read_from_file(input_path);
    let mut reader = {
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::Indexed);
        options.set_memory_limit(::gif::MemoryLimit(8000 * 8000));
        options.allow_unknown_blocks(true);
        match options.read_info(&file) {
            Ok(r) => r,
            Err(e) => {
                error!("Error decoding gif: {e}");
                return og_gif;
            }
        }
    };

    let (w, h) = match reader.read_next_frame() {
        Ok(Some(frame)) => (frame.width, frame.height),
        Ok(None) => (width as u16, width as u16),
        Err(error) => {
            error!("error decoding gif frame - unable to detect width and height - forcing resize to square: {}", error);
            (width as u16, width as u16)
        }
    };

    //early exit
    if width == w as u32 {
        return og_gif;
    };
    let (w2, h2) = calculate_dimensions(w as u32, h as u32, width);

    let mut handle = spawn!(gifsicle ${input_path} -o ${output_path} --resize ${w2}x${h2})?;
    if handle.wait().is_err() {
        error!(
            "Unable to convert gif from path {} with run_cmd.. Falling back to original image",
            input_path
        );
        og_gif
    } else {
        info!("Resized gif to {} px in {}", width, Elapsed::from(&start));
        read_from_file(output_path)
    }
}

pub fn svg_to_png(data: &[u8]) -> Result<Vec<u8>> {
    let mut opt = usvg::Options::default();
    opt.fontdb.load_system_fonts();
    let rtree = usvg::Tree::from_data(data, &opt.to_ref())?;
    let pixmap_size = rtree.svg_node().size.to_screen_size();
    let mut pixmap = tiny_skia::Pixmap::new(pixmap_size.width(), pixmap_size.height()).unwrap();

    resvg::render(
        &rtree,
        usvg::FitTo::Original,
        tiny_skia::Transform::default(),
        pixmap.as_mut(),
    )
    .unwrap();

    Ok(pixmap.encode_png()?)
}

pub fn is_webp_animated(data: &[u8]) -> bool {
    let (riff, webp, vp8x, anim);
    let buff = Cursor::new(data);
    //Read 4 bytes -> 'RIFF'
    riff = buff.get_ref()[0..4] == vec![0x52, 0x49, 0x46, 0x46];
    // Skip 4 bytes and read 4 bytes -> 'WEBP'
    webp = buff.get_ref()[8..12] == vec![0x57, 0x45, 0x42, 0x50];
    //Read 4 bytes -> 'VP8X' / 'VP8L' / 'VP8'
    vp8x = buff.get_ref()[12..16] == vec![0x56, 0x50, 0x38, 0x58];
    //skip 14 bytes and read 4 bytes -> 'ANIM'
    anim = buff.get_ref()[30..34] == vec![0x41, 0x4e, 0x49, 0x4d];

    riff && webp && anim && vp8x
}
pub fn resize_static(data: &[u8], width: u32, format: ImageFormat) -> Result<Vec<u8>> {
    let start = Instant::now();

    let bytes = match format {
        ImageFormat::Png => resize_png(data, width),
        ImageFormat::WebP => resize_webp(data, width, is_webp_animated(data)),
        _ => {
            let mut buff = Cursor::new(Vec::new());
            let reader = Reader::with_format(Cursor::new(data), format);
            let img = reader.decode()?;
            //early exit
            if width == img.width() {
                return Ok(data.to_vec());
            };
            let (w, h) = calculate_dimensions(img.width(), img.height(), width);

            img.thumbnail(w, h).write_to(&mut buff, format)?;
            Ok(buff.into_inner())
        }
    };
    info!("Resized to {} px in {}", width, Elapsed::from(&start));
    bytes
}

fn calculate_dimensions(imgw: u32, imgh: u32, width: u32) -> (u32, u32) {
    if imgw > width && imgw != imgh {
        ((imgw / (imgw / width)), (imgh / (imgw / width)))
    } else if imgh > width && imgw != imgh {
        ((imgw / (imgh / width)), (imgh / (imgh / width)))
    } else if imgh == imgw {
        (width, width)
    } else {
        (imgw, imgh)
    }
}
pub fn resize_png(data: &[u8], width: u32) -> Result<Vec<u8>> {
    let start = Instant::now();
    let decoder = png::Decoder::new(Cursor::new(data));
    let (info, mut reader) = decoder.read_info()?;
    let mut src = vec![0; info.buffer_size()];
    reader.next_frame(&mut src)?;
    let (w2, h2) = {
        let x = calculate_dimensions(info.width, info.height, width);
        (x.0 as usize, x.1 as usize)
    };
    //early exit
    if width == info.width {
        return Ok(data.to_vec());
    };
    let (w1, h1) = (info.width as usize, info.height as usize);
    let mut dst = vec![0u8; w2 * h2 * info.color_type.samples()];

    match info.color_type {
        ColorType::Grayscale => resize::new(w1, h1, w2, h2, Pixel::Gray8, Triangle)?
            .resize(src.as_gray(), dst.as_gray_mut())?,
        ColorType::RGB => resize::new(w1, h1, w2, h2, Pixel::RGB8, Triangle)?
            .resize(src.as_rgb(), dst.as_rgb_mut())?,
        ColorType::Indexed => {
            error!("Unimplemented conversion -> ColorType::Indexed");
            unimplemented!()
        }
        ColorType::GrayscaleAlpha => {
            error!("Unimplemented conversion -> ColorType::GrayscaleAlpha");
            unimplemented!()
        }
        ColorType::RGBA => resize::new(w1, h1, w2, h2, Pixel::RGBA8, Triangle)?
            .resize(src.as_rgba(), dst.as_rgba_mut())?,
    };

    let mut buff = Cursor::new(Vec::new());
    let mut encoder = png::Encoder::new(&mut buff, w2 as u32, h2 as u32);
    encoder.set_color(info.color_type);
    encoder.set_depth(info.bit_depth);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&dst)
        .unwrap();
    info!("Resized to {} px in {}", width, Elapsed::from(&start));
    Ok(buff.into_inner())
}
