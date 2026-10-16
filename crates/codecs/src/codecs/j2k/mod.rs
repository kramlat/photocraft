//! Fork of `pure_jpeg2k` for JPEG 2000 writing: see [`encoder`].
#![allow(dead_code, clippy::all, unreachable_pub, missing_docs, clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::todo, clippy::unimplemented, clippy::unreachable)]

mod bitstream;
mod dwt;
mod ebcot;
pub(crate) mod encoder;
mod error;
mod geometry;
mod markers;
mod mq;
mod packet;
mod tagtree;
mod tile;

pub(crate) use encoder::encode_planes;
