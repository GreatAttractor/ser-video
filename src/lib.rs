//
// ser-video - SER video library
// Copyright (c) 2025 Filip Szczerek <ga.software@yahoo.com>
//
// This project is licensed under the terms of the MIT license
// (see the LICENSE file for details).
//

//!
//! Library main file.
//!

use std::{error::Error, io::{Read, Seek, SeekFrom, Write}};

pub use ga_image;

#[derive(Copy, Clone, num_derive::FromPrimitive, PartialEq)]
pub enum SerColorFormat {
    Mono      = 0,
    BayerRGGB = 8,
    BayerGRBG = 9,
    BayerGBRG = 10,
    BayerBGGR = 11,
    // unsupported:
    // BayerCYYM = 16,
    // BayerYCMY = 17,
    // BayerYMCY = 18,
    // BayerMYYC = 19,
    RGB       = 100,
    BGR       = 101
}

// see comment for `SerHeader::little_endian`
const SER_LITTLE_ENDIAN: u32 = 0;
const SER_BIG_ENDIAN: u32 = 1;

macro_rules! str_as_byte_array {
    ($string:expr, $len:expr) => {
        {
            let mut array = [0u8; $len];
            let bytes = $string.as_bytes();
            for i in 0..std::cmp::min($len, $string.len()) {
                array[i] = bytes[i];
            }
            array
        }
    }
}

#[repr(C, packed)]
struct SerHeader {
    signature: [u8; 14],
    camera_series_id: u32,
    color_id: u32,
    // Online documentation claims this is 0 when 16-bit pixel data
    // is big-endian, but the meaning is actually reversed.
    little_endian: u32,
    img_width: u32,
    img_height: u32,
    bits_per_channel: u32,
    frame_count: u32,
    observer: [u8; 40],
    instrument: [u8; 40],
    telescope: [u8; 40],
    date_time: i64,
    date_time_utc: i64
}

#[derive(Clone)]
pub struct SerMetadata {
    pub little_endian: bool,
    pub ser_color_fmt: SerColorFormat,
    pub pix_fmt: ga_image::PixelFormat,
    pub num_images: usize,
    pub width: u32,
    pub height: u32
}


pub trait ReadSeek: Read + Seek {}

impl<T: Read + Seek> ReadSeek for T {}

pub trait WriteSeek: Write + Seek {}

impl<T: Write + Seek> WriteSeek for T {}

pub struct SerVideoReader {
    metadata: SerMetadata,
    reader: Box<dyn ReadSeek + Send>
}

impl SerVideoReader {
    pub fn new(mut reader: Box<dyn ReadSeek + Send>) -> Result<SerVideoReader, Box<dyn Error>> {
        let header: SerHeader = ga_image::utils::read_struct(&mut reader)?;
        Ok(SerVideoReader{ reader, metadata: get_metadata(&header)? })
    }

    pub fn from_path<P: AsRef<std::path::Path>>(path: P) -> Result<SerVideoReader, Box<dyn Error>> {
        let reader = Box::new(std::io::BufReader::new(std::fs::File::open(path)?));
        SerVideoReader::new(reader)
    }

    pub fn metadata(&self) -> SerMetadata {
        self.metadata.clone()
    }

    // TODO: read_next_frame() -> Result<Option<...>>

    pub fn read_frame(&mut self, frame_idx: usize) -> Result<ga_image::Image, Box<dyn Error>> {
        let mut img =
            ga_image::Image::new(self.metadata.width, self.metadata.height, None, self.metadata.pix_fmt, None, false);

        let frame_size =
            (self.metadata.width * self.metadata.height) as usize * self.metadata.pix_fmt.bytes_per_pixel();

        self.reader.seek(SeekFrom::Start((size_of::<SerHeader>() + frame_idx * frame_size) as u64))?;

        for y in 0..self.metadata.height {
            self.reader.read_exact(img.line_raw_mut(y))?;

            if self.metadata.ser_color_fmt == SerColorFormat::BGR {
                match self.metadata.pix_fmt {
                    ga_image::PixelFormat::RGB8 => reverse_rgb(img.line_mut::<u8>(y)),
                    ga_image::PixelFormat::RGB16 => reverse_rgb(img.line_mut::<u16>(y)),
                    _ => unreachable!()
                }
            }
        }

        if self.metadata.pix_fmt.bytes_per_channel() > 1
            && (ga_image::utils::is_machine_big_endian() ^ !self.metadata.little_endian) {

            ga_image::utils::swap_words16(&mut img);
        }

        Ok(img)
    }
}

pub struct SerVideoWriter {
    writer: Box<dyn WriteSeek + Send>,
    num_images: usize,
    width: u32,
    height: u32,
    pixel_fmt: ga_image::PixelFormat
}

pub struct WriterParameters {
    pub pixel_fmt: ga_image::PixelFormat,
    pub width: u32,
    pub height: u32
}

impl SerVideoWriter {
    pub fn new(mut writer: Box<dyn WriteSeek + Send>, params: &WriterParameters ) -> Result<SerVideoWriter, Box<dyn Error>> {
        let (color_format, bits_per_channel) = from_pixel_format(params.pixel_fmt)?;

        let header = SerHeader{
            signature: [b' '; 14],
            camera_series_id: u32::to_le(0),
            color_id: u32::to_le(color_format as u32),
            little_endian:
                u32::to_le(if ga_image::utils::is_machine_big_endian() { SER_BIG_ENDIAN } else { SER_LITTLE_ENDIAN }),
            img_width: u32::to_le(params.width),
            img_height: u32::to_le(params.height),
            bits_per_channel: u32::to_le(bits_per_channel),
            frame_count: u32::to_le(0), // will be updated later
            observer: [b' '; 40], // TODO
            instrument: [b' '; 40], // TODO
            telescope: [b' '; 40], // TODO
            date_time: i64::to_le(0), // TODO
            date_time_utc: i64::to_le(0), // TODO
        };

        ga_image::utils::write_struct(&header, &mut writer)?;

        Ok(SerVideoWriter{
            writer,
            num_images: 0,
            width: params.width,
            height: params.height,
            pixel_fmt: params.pixel_fmt
        })
    }

    pub fn from_path<P: AsRef<std::path::Path>>(
        path: P,
        params: &WriterParameters
    ) -> Result<SerVideoWriter, Box<dyn Error>> {
        let writer = Box::new(std::io::BufWriter::new(std::fs::File::create(path)?));
        SerVideoWriter::new(writer, params)
    }

    pub fn write_frame(&mut self, image: &ga_image::Image) -> Result<(),  Box<dyn Error>> {
        if image.width() != self.width || image.height() != self.height || image.pixel_format() != self.pixel_fmt {
            return Err("mismatched frame size or pixel format".into());
        }

        for y in 0..image.height() {
            let line = image.line_raw(y);
            match self.writer.write_all(line) {
                Err(err) => return Err(Box::new(err)),
                Ok(()) => ()
            }
        }

        self.num_images += 1;

        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), Box<dyn Error>> {
        self.writer.flush()?;
        Ok(())
    }
}

impl Drop for SerVideoWriter {
    fn drop(&mut self) {
        let _ = self.writer.seek(std::io::SeekFrom::Start(std::mem::offset_of!(SerHeader, frame_count) as u64));
        let _ = ga_image::utils::write_struct(&self.num_images, &mut self.writer);
    }
}

/// Reverses RGB<->BGR.
fn reverse_rgb<T>(line: &mut [T]) {
    for x in 0..line.len() / 3 {
        let r: *mut T = unsafe { line.get_unchecked_mut(3 * x) } as _;
        let b: *mut T = unsafe { line.get_unchecked_mut(3 * x + 2) } as _;
        unsafe {
            std::mem::swap(&mut *r, &mut *b);
        }
    }
}

fn as_pixel_format(ser_color_fmt: SerColorFormat, bits_per_channel: u32) -> ga_image::PixelFormat {
    type PF = ga_image::PixelFormat;

    match ser_color_fmt {
        SerColorFormat::Mono => if bits_per_channel <= 8 { PF::Mono8 } else { PF::Mono16 },
        SerColorFormat::RGB | SerColorFormat::BGR =>
            if bits_per_channel <= 8 { PF::RGB8 } else { PF::RGB16 },
        SerColorFormat::BayerBGGR => if bits_per_channel <= 8 { PF::CfaBGGR8 } else { PF::CfaBGGR16 },
        SerColorFormat::BayerGBRG => if bits_per_channel <= 8 { PF::CfaGBRG8 } else { PF::CfaGBRG16 },
        SerColorFormat::BayerGRBG => if bits_per_channel <= 8 { PF::CfaGRBG8 } else { PF::CfaGRBG16 },
        SerColorFormat::BayerRGGB => if bits_per_channel <= 8 { PF::CfaRGGB8 } else { PF::CfaRGGB16 },
    }
}

/// Returns (color format, bits per channel).
fn from_pixel_format(pixel_fmt: ga_image::PixelFormat) -> Result<(SerColorFormat, u32), Box<dyn Error>> {
    type PF = ga_image::PixelFormat;

    match pixel_fmt {
    PF::Mono8 => Ok((SerColorFormat::Mono, 8)),
    PF::RGB8 => Ok((SerColorFormat::RGB, 8)),
    PF::BGR8 => Ok((SerColorFormat::BGR, 8)),
    PF::Mono16 => Ok((SerColorFormat::Mono, 16)),
    PF::RGB16 => Ok((SerColorFormat::RGB, 16)),
    PF::RGBA16 => Ok((SerColorFormat::RGB, 16)),
    PF::CfaRGGB8 => Ok((SerColorFormat::BayerRGGB, 8)),
    PF::CfaGRBG8 => Ok((SerColorFormat::BayerGRBG, 8)),
    PF::CfaGBRG8 => Ok((SerColorFormat::BayerGBRG, 8)),
    PF::CfaBGGR8 => Ok((SerColorFormat::BayerBGGR, 8)),
    _ => Err(format!("unsupported pixel format: {:?}", pixel_fmt).into())
    }
}

fn get_metadata(header: &SerHeader) -> Result<SerMetadata, Box<dyn Error>> {
    let c_id = header.color_id;
    let ser_color_fmt = num::FromPrimitive::from_u32(u32::from_le(header.color_id))
        .ok_or::<Box<dyn Error>>(format!("unsupported pixel format {}", c_id).into())?;

    let bits_per_channel = u32::from_le(header.bits_per_channel);
    if bits_per_channel > 16 {
        return Err(format!("unsupported bit depth {}", bits_per_channel).into());
    }

    Ok(SerMetadata{
        little_endian: u32::from_le(header.little_endian) == SER_LITTLE_ENDIAN,
        ser_color_fmt,
        pix_fmt: as_pixel_format(ser_color_fmt, bits_per_channel),
        num_images: u32::from_le(header.frame_count) as usize,
        width: u32::from_le(header.img_width),
        height: u32::from_le(header.img_height)
    })
}
