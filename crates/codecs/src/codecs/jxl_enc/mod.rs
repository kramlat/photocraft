//! Fork of `zune-jpegxl` 0.5.2 (MIT OR Apache-2.0 OR Zlib, Caleb Etemesi), a pure-Rust lossless
//! JPEG XL encoder, with one fix: 16-bit alpha channels are declared as 16-bit (see `encoder.rs`).
#![allow(dead_code, clippy::all, unreachable_pub, missing_docs, clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::todo, clippy::unimplemented, clippy::unreachable)]

mod bit_depth;
mod bit_writer;
mod color_convert;
mod encoder;
mod errors;

pub(crate) use encoder::JxlSimpleEncoder;
pub(crate) use errors::JxlEncodeErrors;
