use anyhow::Result;
use cmd_lib::*;
use mime::Mime;
use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::time::{Duration, Instant};
pub struct Elapsed(Duration);
impl Elapsed {
    pub fn from(start: &Instant) -> Self {
        Elapsed(start.elapsed())
    }
}

impl fmt::Display for Elapsed {
    fn fmt(&self, out: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match (self.0.as_secs(), self.0.subsec_nanos()) {
            (0, n) if n < 1000 => write!(out, "{} ns", n),
            (0, n) if n < 1_000_000 => write!(out, "{} µs", n / 1000),
            (0, n) => write!(out, "{} ms", n / 1_000_000),
            (s, n) if s < 10 => write!(out, "{}.{:02} s", s, n / 10_000_000),
            (s, _) => write!(out, "{} s", s),
        }
    }
}
pub fn write_to_file(data: Vec<u8>, path: &str) -> Result<()> {
    let start = Instant::now();
    let mut file = File::create(path)?;
    file.write_all(&data)?;
    log::info!("it took {} to save data to disk", Elapsed::from(&start));
    Ok(())
}
pub fn read_from_file(path: &str) -> Result<Vec<u8>> {
    log::info!("Opening file:: {}", path);
    let mut image_data = Vec::new();
    let start = Instant::now();
    let mut f = File::open(path)?;
    f.read_to_end(&mut image_data)?;
    log::info!("reading image from file: took {}", Elapsed::from(&start));
    Ok(image_data)
}

pub fn guess_content_type(path: &str) -> Result<Mime, Box<dyn std::error::Error>> {
    let mut proc = spawn_with_output!(file --mime-type --mime-encoding -0 "${path}" | cut -d " " -f2 | tr -d ';')?;
    let output = proc.wait_with_output()?;
    Ok(output.parse::<Mime>()?)
}
