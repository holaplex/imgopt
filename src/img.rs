use crate::utils::*;
use anyhow::Result;
use cmd_lib::*;
use image::io::Reader;
use image::ImageFormat;
use mp4::TrackType;
use png::BitDepth;
use png::ColorType;
use resize::Pixel;
use resize::Type::Triangle;
use rgb::FromSlice;
use std::fs;
use std::io::Cursor;
use std::time::Instant;

pub fn mp4_to_gif(input_path: &str, output_path: &str, width: u32) -> Result<Vec<u8>> {
    let start = Instant::now();
    log::info!("reading mp4 to retrieve dimensions");
    let f = fs::File::open(input_path).unwrap();
    let mp4 = mp4::read_mp4(f).unwrap();
    let video_tracks: Vec<_> = mp4
        .tracks()
        .values()
        .into_iter()
        .filter(|t| t.track_type().unwrap() == TrackType::Video)
        .collect();
    log::info!(
        "Detected dimensions: {}x{}",
        video_tracks[0].width(),
        video_tracks[0].height()
    );
    let (w, h) = calculate_dimensions(
        video_tracks[0].width() as u32,
        video_tracks[0].height() as u32,
        width,
    );
    log::info!("Able to scale to: {}x{}", w, h,);
    let mut handle = spawn!(./mp4-to-gif.sh ${input_path} ${w} ${h} ${output_path})?;
    if handle.wait().is_err() {
        log::error!("Unable to convert mp4 to gif with run_cmd.. Falling back to original file");
        read_from_file(input_path)
    } else {
        log::info!(
            "Converted mp4 to gif and resized to {} px in {}",
            width,
            Elapsed::from(&start)
        );
        read_from_file(output_path)
    }
}
pub fn scaledown_gif(input_path: &str, output_path: &str, width: u32) -> Result<Vec<u8>> {
    let start = Instant::now();
    //try to retrieve width and height.
    let file = fs::File::open(&input_path)?;
    let mut reader = {
        let mut options = gif::DecodeOptions::new();
        options.allow_unknown_blocks(true);
        options.read_info(file)?
    };
    let (w, h) = match reader.read_next_frame() {
        Ok(Some(frame)) => (frame.width, frame.height),
        Ok(None) => (width as u16, width as u16),
        Err(error) => {
            log::error!("error decoding gif frame - unable to detect width and height - forcing resize to square: {}", error);
            (width as u16, width as u16)
        }
    };
    let (w2, h2) = calculate_dimensions(w as u32, h as u32, width);
    let mut handle = spawn!(gifsicle ${input_path} -o ${output_path} --resize ${w2}x${h2})?;
    if handle.wait().is_err() {
        log::error!(
            "Unable to convert gif from path {} with run_cmd.. Falling back to original image",
            input_path
        );
        read_from_file(input_path)
    } else {
        log::info!("Resized gif to {} px in {}", width, Elapsed::from(&start));
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
    let png_bytes = pixmap.encode_png()?;
    Ok(png_bytes)
}

pub fn scaledown_static(data: &[u8], width: u32, format: ImageFormat) -> Result<Vec<u8>> {
    //moving to buffer
    let start = Instant::now();
    let reader = Reader::with_format(Cursor::new(data), format);
    let img = reader.decode()?;
    let format = match format {
        ImageFormat::WebP => ImageFormat::Jpeg,
        _ => format,
    };
    let (w, h) = calculate_dimensions(img.width(), img.height(), width);
    let mut buff = Cursor::new(Vec::new());
    img.thumbnail(w, h).write_to(&mut buff, format)?;
    log::info!("Resized to {} px in {}", width, Elapsed::from(&start));
    Ok(buff.into_inner())
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
pub fn scaledown_png(data: &[u8], width: u32) -> Result<Vec<u8>> {
    let start = Instant::now();
    let decoder = png::Decoder::new(Cursor::new(data));
    let (info, mut reader) = decoder.read_info()?;
    let mut src = vec![0; info.buffer_size()];
    reader.next_frame(&mut src)?;
    let (w2, h2) = {
        let x = calculate_dimensions(info.width, info.height, width);
        (x.0 as usize, x.1 as usize)
    };
    let (w1, h1) = (info.width as usize, info.height as usize);
    let mut dst = vec![0u8; w2 * h2 * info.color_type.samples()];

    assert_eq!(BitDepth::Eight, info.bit_depth);
    match info.color_type {
        ColorType::Grayscale => resize::new(w1, h1, w2, h2, Pixel::Gray8, Triangle)?
            .resize(src.as_gray(), dst.as_gray_mut())?,
        ColorType::RGB => resize::new(w1, h1, w2, h2, Pixel::RGB8, Triangle)?
            .resize(src.as_rgb(), dst.as_rgb_mut())?,
        ColorType::Indexed => {
            log::error!("Unimplemented conversion -> ColorType::Indexed");
            unimplemented!()
        }
        ColorType::GrayscaleAlpha => {
            log::error!("Unimplemented conversion -> ColorType::GrayscaleAlpha");
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
    log::info!("Resized to {} px in {}", width, Elapsed::from(&start));
    Ok(buff.into_inner())
}
