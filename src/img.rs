use crate::utils::*;
use anyhow::Result;
use cmd_lib::*;
use image::io::Reader;
use image::ImageFormat;
use png::BitDepth;
use png::ColorType;
use resize::Pixel;
use resize::Type::Triangle;
use rgb::FromSlice;
use std::io::Cursor;
use std::time::Instant;

pub fn mp4_to_gif(input_path: &str, output_path: &str, width: u32) -> Result<Vec<u8>> {
    let start = Instant::now();
    let mut handle = spawn!(./mp4-to-gif.sh ${input_path} ${width} ${output_path})?;
    if handle.wait().is_err() {
        log::error!("Unable to convert mp4 to gif with run_cmd.. Falling back to original file");
        read_from_file(input_path)
    } else {
        log::info!(
            "Converted mp4 to gif and resized to {} px in {}",
            width,
            Elapsed::from(&start)
        );
        read_from_file(&output_path)
    }
}
pub fn scaledown_gif(input_path: &str, output_path: &str, width: u32) -> Result<Vec<u8>> {
    let start = Instant::now();
    let mut handle = spawn!(gifsicle ${input_path} -o ${output_path} --resize ${width}x${width})?;
    if handle.wait().is_err() {
        log::error!("Unable to convert gif with run_cmd.. Falling back to original image");
        read_from_file(input_path)
    } else {
        log::info!("Resized gif to {} px in {}", width, Elapsed::from(&start));
        read_from_file(&output_path)
    }
}

pub fn svg_to_png(data: &Vec<u8>) -> Result<Vec<u8>> {
    let mut opt = usvg::Options::default();
    opt.fontdb.load_system_fonts();
    let rtree = usvg::Tree::from_data(&data, &opt.to_ref())?;
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

pub fn scaledown_static(data: &Vec<u8>, width: u32, format: ImageFormat) -> Result<Vec<u8>> {
    //moving to buffer
    let start = Instant::now();
    let reader = Reader::with_format(Cursor::new(data), format);
    let img = reader.decode()?;
    let format = match format {
        ImageFormat::WebP => ImageFormat::Jpeg,
        _ => format,
    };
    let mut buff = Cursor::new(Vec::new());
    img.thumbnail(width, width).write_to(&mut buff, format)?;
    log::info!("Resized to {} px in {}", width, Elapsed::from(&start));
    Ok(buff.into_inner())
}

pub fn scaledown_png(data: &Vec<u8>, width: u32) -> Result<Vec<u8>> {
    let start = Instant::now();
    let decoder = png::Decoder::new(Cursor::new(data));
    let (info, mut reader) = decoder.read_info()?;
    let mut src = vec![0; info.buffer_size()];
    reader.next_frame(&mut src)?;

    let (w1, h1) = (info.width as usize, info.height as usize);
    let (w2, h2) = (width as usize, width as usize);
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
