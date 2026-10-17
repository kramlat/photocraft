//! MyPaint brushes: a port of libmypaint's brush engine, so `.myb` files from MyPaint 2.x (and
//! its brush packs) paint the way they do there. See `NOTICE` for the licence of the original.

pub mod brush;
pub mod color;
pub mod mapping;
pub mod rng;
pub mod surface;
pub mod tables;

pub use brush::{Brush, DabSurface, Event};
pub use surface::TileSurface;
