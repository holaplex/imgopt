use crate::utils::*;
use anyhow::Result;
use cmd_lib::*;
use image::io::Reader;
use image::{DynamicImage, ImageFormat};
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
    let img = reader.decode().unwrap_or_else(|e| {
        log::error!("Error decoding image: {}", e);
        //returning dummy
        DynamicImage::new_luma16(0, 0)
    });

    //if image is 0 bytes the decoding has failed. return the original image.
    if img.as_bytes().is_empty() {
        log::warn!("falling back to base image from storage");
        return Ok(data.clone());
    }
    let format = match format {
        ImageFormat::WebP => ImageFormat::Png,
        _ => format,
    };
    let mut buff = Cursor::new(Vec::new());
    img.thumbnail(width, width).write_to(&mut buff, format)?;
    log::info!("Resized to {} px in {}", width, Elapsed::from(&start));
    Ok(buff.into_inner())
}
