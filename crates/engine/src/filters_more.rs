//! Further `filter.*` commands in Photoshop's vocabulary: Invert Brightness, Invert (Linear
//! Light), Replace Color Range, Channel Waves, Extract Channel, Edge Detection and Gaussian
//! Difference. They run through [`crate::filters::run_filter`] like every other filter, so
//! selections, smart filters, history and `filter.lastFilter` behave the same.

use photocraft_algo::{BendCurve, ChannelSource, DitherMethod, EdgeAlgorithm, FilterParams, NoiseKind, ScatterMode, StretchMode, TileGaps, WaveShape};
use serde_json::Value;

use crate::commands::CommandSpec;
use crate::{EngineError, Result};

fn f(p: &Value, k: &str, d: f32) -> f32 {
    p.get(k).and_then(Value::as_f64).map_or(d, |v| v as f32)
}
fn b(p: &Value, k: &str, d: bool) -> bool {
    p.get(k).and_then(Value::as_bool).unwrap_or(d)
}
fn s<'a>(p: &'a Value, k: &str, d: &'a str) -> &'a str {
    p.get(k).and_then(Value::as_str).unwrap_or(d)
}

/// `#rrggbb` or `[r, g, b]` (0..=1) as a colour.
fn rgb(p: &Value, k: &str, d: [f32; 3]) -> [f32; 3] {
    match p.get(k) {
        Some(Value::Array(a)) if a.len() >= 3 => {
            let c = |i: usize| a.get(i).and_then(Value::as_f64).map_or(0.0, |v| (v as f32).clamp(0.0, 1.0));
            [c(0), c(1), c(2)]
        }
        Some(Value::String(h)) if h.len() == 7 && h.starts_with('#') => {
            let ch = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).map_or(0.0, |v| f32::from(v) / 255.0);
            [ch(1), ch(3), ch(5)]
        }
        _ => d,
    }
}

/// Straight RGBA from `#rrggbb`, `[r, g, b(, a)]` (0..=1) or the default.
fn rgba(p: &Value, k: &str, d: [f32; 4]) -> [f32; 4] {
    match p.get(k) {
        Some(Value::Array(a)) if a.len() >= 3 => {
            let c = |i: usize, dv: f32| a.get(i).and_then(Value::as_f64).map_or(dv, |v| (v as f32).clamp(0.0, 1.0));
            [c(0, 0.0), c(1, 0.0), c(2, 0.0), c(3, 1.0)]
        }
        Some(Value::String(_)) => {
            let c = rgb(p, k, [d[0], d[1], d[2]]);
            [c[0], c[1], c[2], d[3]]
        }
        _ => d,
    }
}

const CHANNELS: &str = "red|green|blue|hue|saturation|brightness|lightness|luminosity|cyan|magenta|yellow|black|alpha";
const EDGE_ALGORITHMS: &str = "sobel|prewitt|roberts|gradient|differential|laplace";

fn channel(name: &str) -> ChannelSource {
    match name {
        "green" => ChannelSource::Green,
        "blue" => ChannelSource::Blue,
        "hue" => ChannelSource::Hue,
        "saturation" => ChannelSource::Saturation,
        "brightness" => ChannelSource::Value,
        "lightness" => ChannelSource::Lightness,
        "luminosity" => ChannelSource::Luma,
        "cyan" => ChannelSource::Cyan,
        "magenta" => ChannelSource::Magenta,
        "yellow" => ChannelSource::Yellow,
        "black" => ChannelSource::Black,
        "alpha" => ChannelSource::Alpha,
        _ => ChannelSource::Red,
    }
}

fn algorithm(name: &str) -> EdgeAlgorithm {
    match name {
        "prewitt" => EdgeAlgorithm::Prewitt,
        "roberts" => EdgeAlgorithm::Roberts,
        "gradient" => EdgeAlgorithm::Gradient,
        "differential" => EdgeAlgorithm::Differential,
        "laplace" => EdgeAlgorithm::Laplace,
        _ => EdgeAlgorithm::Sobel,
    }
}

/// Algorithm parameters for the ids this module owns; other ids pass on to the next batch.
pub fn params_for(id: &str, p: &Value) -> Option<FilterParams> {
    Some(match id {
        "filter.other.invertBrightness" => FilterParams::InvertBrightness,
        "filter.other.invertLinear" => FilterParams::InvertLinear,
        "filter.other.replaceColorRange" => FilterParams::ReplaceColorRange {
            from: rgb(p, "from", [0.5; 3]),
            to: rgb(p, "to", [1.0, 0.0, 0.0]),
            fuzziness: f(p, "fuzziness", 10.0).clamp(0.0, 100.0),
        },
        "filter.other.channelWaves" => {
            let (freq, phase) = (f(p, "frequency", 1.0).clamp(0.0, 20.0), f(p, "phase", 0.0).clamp(0.0, 360.0));
            FilterParams::ChannelWaves {
                frequency: [freq; 3],
                phase: [phase; 3],
                modify: [b(p, "red", true), b(p, "green", true), b(p, "blue", true)],
            }
        }
        "filter.other.extractChannel" => FilterParams::ExtractChannel { source: channel(s(p, "channel", "red")), invert: b(p, "invert", false) },
        "filter.stylize.edgeDetection" => {
            FilterParams::EdgeDetection { algorithm: algorithm(s(p, "algorithm", "sobel")), amount: f(p, "amount", 1.0).clamp(0.1, 10.0) }
        }
        "filter.other.gaussianDifference" => FilterParams::GaussianDifference {
            radius1: f(p, "radius", 1.0).clamp(0.1, 100.0),
            radius2: f(p, "outerRadius", 3.0).clamp(0.1, 100.0),
            normalize: b(p, "normalize", true),
            invert: b(p, "invert", false),
        },
        "filter.render.noisePattern" => FilterParams::NoisePattern {
            kind: match s(p, "type", "perlin") {
                "simplex" => NoiseKind::Simplex,
                "cell" => NoiseKind::Cell,
                "value" => NoiseKind::Value,
                _ => NoiseKind::Perlin,
            },
            size: f(p, "size", 40.0).clamp(1.0, 10_000.0),
            detail: f(p, "detail", 3.0).round().clamp(1.0, 8.0) as u32,
            seed: seed(p),
            foreground: rgba(p, "foreground", [0.0, 0.0, 0.0, 1.0]),
            background: rgba(p, "background", [1.0, 1.0, 1.0, 1.0]),
        },
        "filter.render.checkerboard" => FilterParams::Checkerboard {
            size: f(p, "size", 32.0).clamp(1.0, 10_000.0),
            foreground: rgba(p, "foreground", [0.0, 0.0, 0.0, 1.0]),
            background: rgba(p, "background", [1.0, 1.0, 1.0, 1.0]),
        },
        "filter.render.grid" => FilterParams::Grid {
            spacing: f(p, "spacing", 32.0).clamp(2.0, 10_000.0),
            line_width: f(p, "lineWidth", 1.0).clamp(1.0, 1000.0),
            color: rgba(p, "color", [0.0, 0.0, 0.0, 1.0]),
        },
        "filter.distort.kaleidoscope" => FilterParams::Kaleidoscope {
            segments: f(p, "segments", 6.0).round().clamp(2.0, 64.0) as u32,
            angle: f(p, "angle", 0.0).clamp(-360.0, 360.0),
            center_x: f(p, "centerX", 0.5).clamp(0.0, 1.0),
            center_y: f(p, "centerY", 0.5).clamp(0.0, 1.0),
            mirror: b(p, "mirror", false),
        },
        "filter.stylize.longShadow" => FilterParams::LongShadow {
            angle: f(p, "angle", 45.0).clamp(-360.0, 360.0),
            length: f(p, "length", 50.0).clamp(1.0, 2000.0),
            color: rgba(p, "color", [0.0, 0.0, 0.0, 0.6]),
            fade: b(p, "fade", true),
        },
        "filter.render.vignette" => FilterParams::Vignette {
            amount: f(p, "amount", -60.0).clamp(-100.0, 100.0),
            size: f(p, "size", 40.0).clamp(0.0, 100.0),
            softness: f(p, "softness", 60.0).clamp(0.0, 100.0),
            roundness: f(p, "roundness", 50.0).clamp(0.0, 100.0),
        },
        "filter.other.stretchLevels" => FilterParams::StretchLevels {
            mode: match s(p, "mode", "channels") {
                "together" => StretchMode::Together,
                "brightness" => StretchMode::Brightness,
                _ => StretchMode::Channels,
            },
        },
        "filter.other.enhanceColors" => FilterParams::EnhanceColors,
        "filter.other.dither" => FilterParams::Dither {
            method: match s(p, "method", "bayer4") {
                "bayer2" => DitherMethod::Bayer2,
                "bayer8" => DitherMethod::Bayer8,
                "random" => DitherMethod::Random,
                "floydSteinberg" => DitherMethod::FloydSteinberg,
                _ => DitherMethod::Bayer4,
            },
            levels: f(p, "levels", 4.0).round().clamp(2.0, 32.0) as u32,
            seed: seed(p),
        },
        "filter.noise.scatterPixels" => FilterParams::ScatterPixels {
            mode: match s(p, "mode", "random") {
                "pick" => ScatterMode::Pick,
                "smear" => ScatterMode::Smear,
                "spread" => ScatterMode::Spread,
                _ => ScatterMode::Random,
            },
            amount: f(p, "amount", 20.0).clamp(0.0, 100.0),
            radius: f(p, "radius", 4.0).clamp(1.0, 200.0),
            seed: seed(p),
        },
        "filter.render.plasma" => FilterParams::Plasma {
            turbulence: f(p, "turbulence", 2.0).clamp(0.1, 7.0),
            saturation: f(p, "saturation", 80.0).clamp(0.0, 100.0),
            seed: seed(p),
        },
        "filter.render.spiral" => FilterParams::Spiral {
            spacing: f(p, "spacing", 16.0).clamp(2.0, 10_000.0),
            thickness: f(p, "thickness", 8.0).clamp(1.0, 10_000.0),
            clockwise: b(p, "clockwise", true),
            foreground: rgba(p, "foreground", [0.0, 0.0, 0.0, 1.0]),
            background: rgba(p, "background", [1.0, 1.0, 1.0, 1.0]),
        },
        "filter.render.maze" => FilterParams::Maze {
            cell: f(p, "cellSize", 16.0).clamp(4.0, 2000.0),
            wall: f(p, "wallWidth", 2.0).clamp(1.0, 1000.0),
            seed: seed(p),
            foreground: rgba(p, "foreground", [0.0, 0.0, 0.0, 1.0]),
            background: rgba(p, "background", [1.0, 1.0, 1.0, 1.0]),
        },
        "filter.render.sineWaves" => FilterParams::SineWaves {
            shape: match s(p, "shape", "linear") {
                "radial" => WaveShape::Radial,
                "cross" => WaveShape::Cross,
                _ => WaveShape::Linear,
            },
            wavelength: f(p, "wavelength", 32.0).clamp(2.0, 10_000.0),
            angle: f(p, "angle", 0.0).clamp(-360.0, 360.0),
            phase: f(p, "phase", 0.0).clamp(-360.0, 360.0),
            contrast: f(p, "contrast", 100.0).clamp(0.0, 100.0),
            foreground: rgba(p, "foreground", [0.0, 0.0, 0.0, 1.0]),
            background: rgba(p, "background", [1.0, 1.0, 1.0, 1.0]),
        },
        "filter.distort.shiftRows" => {
            FilterParams::ShiftRows { vertical: s(p, "direction", "horizontal") == "vertical", distance: f(p, "distance", 10.0).clamp(0.0, 10_000.0), seed: seed(p) }
        }
        "filter.distort.bend" => FilterParams::Bend {
            amount: f(p, "amount", 40.0).clamp(-100.0, 100.0),
            // Bending "vertically" moves columns up and down along a curve across the width.
            vertical_axis: s(p, "direction", "horizontal") == "vertical",
            curve: if s(p, "curve", "arc") == "wave" { BendCurve::Wave } else { BendCurve::Arc },
        },
        "filter.distort.glassTiles" => FilterParams::GlassTiles { size: f(p, "tileSize", 32.0).clamp(2.0, 2000.0), refraction: f(p, "refraction", 40.0).clamp(0.0, 100.0) },
        "filter.distort.paperTiles" => FilterParams::PaperTiles {
            size: f(p, "tileSize", 32.0).clamp(2.0, 2000.0),
            jitter: f(p, "jitter", 8.0).clamp(0.0, 2000.0),
            gaps: match s(p, "gaps", "transparent") {
                "black" => TileGaps::Black,
                "white" => TileGaps::White,
                _ => TileGaps::Transparent,
            },
            seed: seed(p),
        },
        "filter.distort.illusion" => FilterParams::Illusion {
            copies: f(p, "copies", 4.0).round().clamp(2.0, 32.0) as u32,
            center_x: f(p, "centerX", 0.5).clamp(0.0, 1.0),
            center_y: f(p, "centerY", 0.5).clamp(0.0, 1.0),
        },
        "filter.stylize.engrave" => FilterParams::Engrave {
            spacing: f(p, "spacing", 6.0).clamp(2.0, 200.0),
            angle: f(p, "angle", 45.0).clamp(-180.0, 180.0),
            contrast: f(p, "contrast", 70.0).clamp(0.0, 100.0),
        },
        "filter.video.pixelDisplay" => FilterParams::PixelDisplay {
            cell: f(p, "cellSize", 3.0).clamp(1.0, 50.0),
            strength: f(p, "strength", 80.0).clamp(0.0, 100.0),
            scanlines: f(p, "scanlines", 30.0).clamp(0.0, 100.0),
        },
        "filter.render.supernova" => FilterParams::Supernova {
            center_x: f(p, "centerX", 0.5).clamp(0.0, 1.0),
            center_y: f(p, "centerY", 0.5).clamp(0.0, 1.0),
            radius: f(p, "radius", 60.0).clamp(2.0, 20_000.0),
            rays: f(p, "rays", 40.0).round().clamp(3.0, 200.0) as u32,
            randomness: f(p, "randomness", 60.0).clamp(0.0, 100.0),
            color: rgb(p, "color", [1.0, 1.0, 1.0]),
            seed: seed(p),
        },
        "filter.distort.littlePlanet" => FilterParams::LittlePlanet { scale: f(p, "scale", 100.0).clamp(10.0, 500.0), rotation: f(p, "rotation", 0.0).clamp(-360.0, 360.0) },
        "filter.distort.panoramaView" => FilterParams::PanoramaView {
            pan: f(p, "pan", 0.0).clamp(-360.0, 360.0),
            tilt: f(p, "tilt", 0.0).clamp(-90.0, 90.0),
            fov: f(p, "fieldOfView", 90.0).clamp(10.0, 170.0),
        },
        "filter.distort.lensRefraction" => FilterParams::LensRefraction {
            radius: f(p, "radius", 100.0).clamp(2.0, 20_000.0),
            index: f(p, "refractiveIndex", 1.5).clamp(1.0, 2.5),
            center_x: f(p, "centerX", 0.5).clamp(0.0, 1.0),
            center_y: f(p, "centerY", 0.5).clamp(0.0, 1.0),
        },
        "filter.other.makeSeamless" => FilterParams::MakeSeamless { blend: f(p, "blend", 50.0).clamp(10.0, 100.0) },
        "filter.pixelate.cubism" => FilterParams::Cubism { size: f(p, "tileSize", 12.0).clamp(2.0, 400.0), seed: seed(p) },
        "filter.stylize.bumpMap" => FilterParams::BumpMap {
            azimuth: f(p, "azimuth", 135.0).clamp(-180.0, 180.0),
            elevation: f(p, "elevation", 45.0).clamp(1.0, 90.0),
            depth: f(p, "depth", 10.0).clamp(0.0, 200.0),
            ambient: f(p, "ambient", 20.0).clamp(0.0, 100.0),
        },
        "filter.stylize.normalMap" => FilterParams::NormalMap { strength: f(p, "strength", 4.0).clamp(0.01, 100.0), flip_y: b(p, "flipY", false) },
        "filter.render.sparkle" => FilterParams::Sparkle {
            threshold: f(p, "threshold", 80.0).clamp(0.0, 100.0),
            length: f(p, "length", 30.0).clamp(1.0, 400.0),
            spikes: f(p, "spikes", 4.0).round().clamp(2.0, 16.0) as u32,
            angle: f(p, "angle", 0.0).clamp(-180.0, 180.0),
            intensity: f(p, "intensity", 70.0).clamp(0.0, 100.0),
        },
        _ => return crate::gallery_cmds::params_for(id, p),
    })
}

/// A `seed` param: any `u32`, else 0.
fn seed(p: &Value) -> u32 {
    p.get("seed").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0)
}

/// Bounds and choices of this module's params; `None` for ids it doesn't own.
pub(crate) fn validate(id: &str, p: &Value) -> Option<Result<()>> {
    let numbers: &[(&str, f64, f64)] = match id {
        "filter.other.replaceColorRange" => &[("fuzziness", 0.0, 100.0)],
        "filter.other.channelWaves" => &[("frequency", 0.0, 20.0), ("phase", 0.0, 360.0)],
        "filter.stylize.edgeDetection" => &[("amount", 0.1, 10.0)],
        "filter.other.gaussianDifference" => &[("radius", 0.1, 100.0), ("outerRadius", 0.1, 100.0)],
        "filter.render.noisePattern" => &[("size", 1.0, 10_000.0), ("detail", 1.0, 8.0)],
        "filter.render.checkerboard" => &[("size", 1.0, 10_000.0)],
        "filter.render.grid" => &[("spacing", 2.0, 10_000.0), ("lineWidth", 1.0, 1000.0)],
        "filter.distort.kaleidoscope" => &[("segments", 2.0, 64.0), ("angle", -360.0, 360.0), ("centerX", 0.0, 1.0), ("centerY", 0.0, 1.0)],
        "filter.stylize.longShadow" => &[("angle", -360.0, 360.0), ("length", 1.0, 2000.0)],
        "filter.render.vignette" => &[("amount", -100.0, 100.0), ("size", 0.0, 100.0), ("softness", 0.0, 100.0), ("roundness", 0.0, 100.0)],
        "filter.other.dither" => &[("levels", 2.0, 32.0)],
        "filter.noise.scatterPixels" => &[("amount", 0.0, 100.0), ("radius", 1.0, 200.0)],
        "filter.render.plasma" => &[("turbulence", 0.1, 7.0), ("saturation", 0.0, 100.0)],
        "filter.render.spiral" => &[("spacing", 2.0, 10_000.0), ("thickness", 1.0, 10_000.0)],
        "filter.render.maze" => &[("cellSize", 4.0, 2000.0), ("wallWidth", 1.0, 1000.0)],
        "filter.render.sineWaves" => &[("wavelength", 2.0, 10_000.0), ("angle", -360.0, 360.0), ("phase", -360.0, 360.0), ("contrast", 0.0, 100.0)],
        "filter.distort.shiftRows" => &[("distance", 0.0, 10_000.0)],
        "filter.distort.bend" => &[("amount", -100.0, 100.0)],
        "filter.distort.glassTiles" => &[("tileSize", 2.0, 2000.0), ("refraction", 0.0, 100.0)],
        "filter.distort.paperTiles" => &[("tileSize", 2.0, 2000.0), ("jitter", 0.0, 2000.0)],
        "filter.distort.illusion" => &[("copies", 2.0, 32.0), ("centerX", 0.0, 1.0), ("centerY", 0.0, 1.0)],
        "filter.stylize.engrave" => &[("spacing", 2.0, 200.0), ("angle", -180.0, 180.0), ("contrast", 0.0, 100.0)],
        "filter.video.pixelDisplay" => &[("cellSize", 1.0, 50.0), ("strength", 0.0, 100.0), ("scanlines", 0.0, 100.0)],
        "filter.render.supernova" => &[("centerX", 0.0, 1.0), ("centerY", 0.0, 1.0), ("radius", 2.0, 20_000.0), ("rays", 3.0, 200.0), ("randomness", 0.0, 100.0)],
        "filter.distort.littlePlanet" => &[("scale", 10.0, 500.0), ("rotation", -360.0, 360.0)],
        "filter.distort.panoramaView" => &[("pan", -360.0, 360.0), ("tilt", -90.0, 90.0), ("fieldOfView", 10.0, 170.0)],
        "filter.distort.lensRefraction" => &[("radius", 2.0, 20_000.0), ("refractiveIndex", 1.0, 2.5), ("centerX", 0.0, 1.0), ("centerY", 0.0, 1.0)],
        "filter.other.makeSeamless" => &[("blend", 10.0, 100.0)],
        "filter.pixelate.cubism" => &[("tileSize", 2.0, 400.0)],
        "filter.stylize.bumpMap" => &[("azimuth", -180.0, 180.0), ("elevation", 1.0, 90.0), ("depth", 0.0, 200.0), ("ambient", 0.0, 100.0)],
        "filter.stylize.normalMap" => &[("strength", 0.01, 100.0)],
        "filter.render.sparkle" => &[("threshold", 0.0, 100.0), ("length", 1.0, 400.0), ("spikes", 2.0, 16.0), ("angle", -180.0, 180.0), ("intensity", 0.0, 100.0)],
        "filter.other.stretchLevels" | "filter.other.enhanceColors" => &[],
        "filter.other.invertBrightness" | "filter.other.invertLinear" | "filter.other.extractChannel" => &[],
        _ => return None,
    };
    let bad = |msg: String| Some(Err(EngineError::BadParams { cmd: id.into(), msg }));
    for &(key, min, max) in numbers {
        if let Some(v) = p.get(key)
            && !v.as_f64().is_some_and(|n| n.is_finite() && (min..=max).contains(&n))
        {
            return bad(format!("`{key}` must be a number in {min}..{max}"));
        }
    }
    let choice = |key: &str, allowed: &str| p.get(key).filter(|v| !v.as_str().is_some_and(|x| allowed.split('|').any(|a| a == x))).map(|_| format!("`{key}` must be one of {allowed}"));
    let flags: &[&str] = match id {
        "filter.other.channelWaves" => &["red", "green", "blue"],
        "filter.other.extractChannel" => &["invert"],
        "filter.other.gaussianDifference" => &["normalize", "invert"],
        "filter.stylize.longShadow" => &["fade"],
        "filter.distort.kaleidoscope" => &["mirror"],
        "filter.stylize.normalMap" => &["flipY"],
        "filter.render.spiral" => &["clockwise"],
        _ => &[],
    };
    for key in flags {
        if p.get(*key).is_some_and(|v| !v.is_boolean()) {
            return bad(format!("`{key}` must be a boolean"));
        }
    }
    let msg = match id {
        "filter.other.extractChannel" => choice("channel", CHANNELS),
        "filter.stylize.edgeDetection" => choice("algorithm", EDGE_ALGORITHMS),
        "filter.render.noisePattern" => choice("type", "perlin|simplex|cell|value"),
        "filter.other.stretchLevels" => choice("mode", "channels|together|brightness"),
        "filter.other.dither" => choice("method", "bayer2|bayer4|bayer8|random|floydSteinberg"),
        "filter.noise.scatterPixels" => choice("mode", "random|pick|smear|spread"),
        "filter.render.sineWaves" => choice("shape", "linear|radial|cross"),
        "filter.distort.shiftRows" => choice("direction", "horizontal|vertical"),
        "filter.distort.bend" => choice("direction", "horizontal|vertical").or_else(|| choice("curve", "arc|wave")),
        "filter.distort.paperTiles" => choice("gaps", "transparent|black|white"),
        _ => None,
    };
    if let Some(msg) = msg {
        return bad(msg);
    }
    for key in ["foreground", "background", "color"] {
        if matches!(
            id,
            "filter.render.noisePattern" | "filter.render.checkerboard" | "filter.render.grid" | "filter.stylize.longShadow" | "filter.render.spiral" | "filter.render.maze" | "filter.render.sineWaves" | "filter.render.supernova"
        )
            && p.get(key).is_some_and(|v| !v.is_array() && v.as_str().is_none_or(|h| h.len() != 7 || !h.starts_with('#')))
        {
            return bad(format!("`{key}` must be #rrggbb or [r, g, b]"));
        }
    }
    for key in ["from", "to"] {
        if id == "filter.other.replaceColorRange"
            && p.get(key).is_some_and(|v| v.as_str().is_none_or(|h| h.len() != 7 || !h.starts_with('#')) && !v.is_array())
        {
            return bad(format!("`{key}` must be #rrggbb or [r, g, b]"));
        }
    }
    Some(Ok(()))
}

macro_rules! cmd {
    ($id:literal, $label:literal, [$($m:literal),*], $params:literal) => {
        CommandSpec {
            id: $id,
            label: $label,
            menu: &[$($m),*],
            shortcut: None,
            params: $params,
            enabled: crate::filters::has_filterable_layer,
            run: |s, p| crate::filters::run_filter(s, $id, p),
            journal: true,
        }
    };
}

/// The command specs of this batch.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("filter.other.invertBrightness", "Invert Brightness", ["Filter", "Other"], "{}"),
        cmd!("filter.other.invertLinear", "Invert (Linear Light)", ["Filter", "Other"], "{}"),
        cmd!(
            "filter.other.replaceColorRange",
            "Replace Color Range…",
            ["Filter", "Other"],
            r##"{"from":"#rrggbb"="#808080","to":"#rrggbb"="#ff0000","fuzziness":0..100=10}"##
        ),
        cmd!(
            "filter.other.channelWaves",
            "Channel Waves…",
            ["Filter", "Other"],
            r##"{"frequency":0..20=1,"phase":0..360=0,"red":bool=true,"green":bool=true,"blue":bool=true}"##
        ),
        cmd!(
            "filter.other.extractChannel",
            "Extract Channel…",
            ["Filter", "Other"],
            r##"{"channel":"red|green|blue|hue|saturation|brightness|lightness|luminosity|cyan|magenta|yellow|black|alpha"="red","invert":bool=false}"##
        ),
        cmd!(
            "filter.stylize.edgeDetection",
            "Edge Detection…",
            ["Filter", "Stylize"],
            r##"{"algorithm":"sobel|prewitt|roberts|gradient|differential|laplace"="sobel","amount":0.1..10=1}"##
        ),
        cmd!(
            "filter.other.gaussianDifference",
            "Gaussian Difference…",
            ["Filter", "Other"],
            r##"{"radius":0.1..100=1,"outerRadius":0.1..100=3,"normalize":bool=true,"invert":bool=false}"##
        ),
        cmd!(
            "filter.render.noisePattern",
            "Noise Pattern…",
            ["Filter", "Render"],
            r##"{"type":"perlin|simplex|cell|value"="perlin","size":1..10000=40,"detail":1..8=3,"seed":u32=0,"foreground":json,"background":json}"##
        ),
        cmd!("filter.render.checkerboard", "Checkerboard…", ["Filter", "Render"], r##"{"size":1..10000=32,"foreground":json,"background":json}"##),
        cmd!("filter.render.grid", "Grid…", ["Filter", "Render"], r##"{"spacing":2..10000=32,"lineWidth":1..1000=1,"color":color=#000000}"##),
        cmd!("filter.render.vignette", "Vignette…", ["Filter", "Render"], r##"{"amount":-100..100=-60,"size":0..100=40,"softness":0..100=60,"roundness":0..100=50}"##),
        cmd!("filter.other.stretchLevels", "Stretch Levels…", ["Filter", "Other"], r##"{"mode":"channels|together|brightness"="channels"}"##),
        cmd!("filter.other.enhanceColors", "Enhance Colors", ["Filter", "Other"], "{}"),
        cmd!(
            "filter.other.dither",
            "Dither…",
            ["Filter", "Other"],
            r##"{"method":"bayer2|bayer4|bayer8|random|floydSteinberg"="bayer4","levels":2..32=4,"seed":u32=0}"##
        ),
        cmd!(
            "filter.noise.scatterPixels",
            "Scatter Pixels…",
            ["Filter", "Noise"],
            r##"{"mode":"random|pick|smear|spread"="random","amount":0..100=20,"radius":1..200=4,"seed":u32=0}"##
        ),
        cmd!("filter.render.plasma", "Plasma…", ["Filter", "Render"], r##"{"turbulence":0.1..7=2,"saturation":0..100=80,"seed":u32=0}"##),
        cmd!("filter.render.spiral", "Spiral…", ["Filter", "Render"], r##"{"spacing":2..10000=16,"thickness":1..10000=8,"clockwise":bool=true,"foreground":json,"background":json}"##),
        cmd!("filter.render.maze", "Maze…", ["Filter", "Render"], r##"{"cellSize":4..2000=16,"wallWidth":1..1000=2,"seed":u32=0,"foreground":json,"background":json}"##),
        cmd!(
            "filter.render.sineWaves",
            "Sine Waves…",
            ["Filter", "Render"],
            r##"{"shape":"linear|radial|cross"="linear","wavelength":2..10000=32,"angle":-360..360=0,"phase":-360..360=0,"contrast":0..100=100,"foreground":json,"background":json}"##
        ),
        cmd!("filter.distort.shiftRows", "Shift Rows…", ["Filter", "Distort"], r##"{"direction":"horizontal|vertical"="horizontal","distance":0..10000=10,"seed":u32=0}"##),
        cmd!("filter.distort.bend", "Bend…", ["Filter", "Distort"], r##"{"amount":-100..100=40,"direction":"horizontal|vertical"="horizontal","curve":"arc|wave"="arc"}"##),
        cmd!("filter.distort.glassTiles", "Glass Tiles…", ["Filter", "Distort"], r##"{"tileSize":2..2000=32,"refraction":0..100=40}"##),
        cmd!(
            "filter.distort.paperTiles",
            "Paper Tiles…",
            ["Filter", "Distort"],
            r##"{"tileSize":2..2000=32,"jitter":0..2000=8,"gaps":"transparent|black|white"="transparent","seed":u32=0}"##
        ),
        cmd!("filter.distort.illusion", "Illusion…", ["Filter", "Distort"], r##"{"copies":2..32=4,"centerX":0..1=0.5,"centerY":0..1=0.5}"##),
        cmd!("filter.stylize.engrave", "Engrave…", ["Filter", "Stylize"], r##"{"spacing":2..200=6,"angle":-180..180=45,"contrast":0..100=70}"##),
        cmd!("filter.video.pixelDisplay", "Pixel Display…", ["Filter", "Video"], r##"{"cellSize":1..50=3,"strength":0..100=80,"scanlines":0..100=30}"##),
        cmd!(
            "filter.render.supernova",
            "Supernova…",
            ["Filter", "Render"],
            r##"{"centerX":0..1=0.5,"centerY":0..1=0.5,"radius":2..20000=60,"rays":3..200=40,"randomness":0..100=60,"color":color=#ffffff,"seed":u32=0}"##
        ),
        cmd!("filter.distort.littlePlanet", "Little Planet…", ["Filter", "Distort"], r##"{"scale":10..500=100,"rotation":-360..360=0}"##),
        cmd!("filter.distort.panoramaView", "Panorama View…", ["Filter", "Distort"], r##"{"pan":-360..360=0,"tilt":-90..90=0,"fieldOfView":10..170=90}"##),
        cmd!(
            "filter.distort.lensRefraction",
            "Lens Refraction…",
            ["Filter", "Distort"],
            r##"{"radius":2..20000=100,"refractiveIndex":1..2.5=1.5,"centerX":0..1=0.5,"centerY":0..1=0.5}"##
        ),
        cmd!("filter.other.makeSeamless", "Make Seamless…", ["Filter", "Other"], r##"{"blend":10..100=50}"##),
        cmd!("filter.pixelate.cubism", "Cubism…", ["Filter", "Pixelate"], r##"{"tileSize":2..400=12,"seed":u32=0}"##),
        cmd!("filter.stylize.bumpMap", "Bump Map…", ["Filter", "Stylize"], r##"{"azimuth":-180..180=135,"elevation":1..90=45,"depth":0..200=10,"ambient":0..100=20}"##),
        cmd!("filter.stylize.normalMap", "Normal Map…", ["Filter", "Stylize"], r##"{"strength":0.01..100=4,"flipY":bool=false}"##),
        cmd!("filter.render.sparkle", "Sparkle…", ["Filter", "Render"], r##"{"threshold":0..100=80,"length":1..400=30,"spikes":2..16=4,"angle":-180..180=0,"intensity":0..100=70}"##),
        cmd!("filter.distort.kaleidoscope", "Kaleidoscope…", ["Filter", "Distort"], r##"{"segments":2..64=6,"angle":-360..360=0,"centerX":0..1=0.5,"centerY":0..1=0.5,"mirror":bool=false}"##),
        cmd!("filter.stylize.longShadow", "Long Shadow…", ["Filter", "Stylize"], r##"{"angle":-360..360=45,"length":1..2000=50,"color":color=#000000,"fade":bool=true}"##),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    const IDS: [&str; 37] = [
        "filter.other.invertBrightness",
        "filter.other.invertLinear",
        "filter.other.replaceColorRange",
        "filter.other.channelWaves",
        "filter.other.extractChannel",
        "filter.stylize.edgeDetection",
        "filter.other.gaussianDifference",
        "filter.render.noisePattern",
        "filter.render.checkerboard",
        "filter.render.grid",
        "filter.render.vignette",
        "filter.distort.kaleidoscope",
        "filter.stylize.longShadow",
        "filter.other.stretchLevels",
        "filter.other.enhanceColors",
        "filter.other.dither",
        "filter.noise.scatterPixels",
        "filter.render.plasma",
        "filter.render.spiral",
        "filter.render.maze",
        "filter.render.sineWaves",
        "filter.distort.shiftRows",
        "filter.distort.bend",
        "filter.distort.glassTiles",
        "filter.distort.paperTiles",
        "filter.distort.illusion",
        "filter.stylize.engrave",
        "filter.video.pixelDisplay",
        "filter.render.supernova",
        "filter.distort.littlePlanet",
        "filter.distort.panoramaView",
        "filter.distort.lensRefraction",
        "filter.other.makeSeamless",
        "filter.pixelate.cubism",
        "filter.stylize.bumpMap",
        "filter.stylize.normalMap",
        "filter.render.sparkle",
    ];

    fn session(depth: u64) -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 24, "height": 24, "background": "white", "depth": depth})).unwrap();
        s.execute("select.rect", json!({"x": 0, "y": 0, "width": 12, "height": 24})).unwrap();
        s.execute("edit.fill", json!({"color": "#2060c0"})).unwrap();
        s.execute("select.all", json!({})).unwrap();
        s
    }

    fn pixels(s: &Session) -> Vec<f32> {
        let st = s.active().unwrap();
        let l = st.doc.layer(st.active_layer.unwrap()).unwrap();
        l.surface().unwrap().read_region(photocraft_geom::Rect::new(0, 0, 24, 24))
    }

    #[test]
    fn every_filter_runs_changes_pixels_and_undoes_at_each_depth() {
        for depth in [8, 16, 32] {
            for id in IDS {
                let mut s = session(depth);
                if id == "filter.stylize.longShadow" {
                    // A shadow needs somewhere to fall: blue on the left half of a transparent layer.
                    s = Session::new();
                    s.execute("file.new", json!({"width": 24, "height": 24, "background": "transparent", "depth": depth})).unwrap();
                    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 12, "height": 24})).unwrap();
                    s.execute("edit.fill", json!({"color": "#2060c0"})).unwrap();
                    s.execute("select.all", json!({})).unwrap();
                }
                let before = pixels(&s);
                let params = match id {
                    "filter.other.replaceColorRange" => json!({"from": "#2060c0", "to": "#ff8000", "fuzziness": 5}),
                    "filter.other.extractChannel" => json!({"channel": "blue"}),
                    _ => json!({}),
                };
                s.execute(id, params).unwrap_or_else(|e| panic!("{id} @{depth}: {e}"));
                assert_ne!(pixels(&s), before, "{id} @{depth} changed nothing");
                s.execute("edit.undo", json!({})).unwrap();
                assert_eq!(pixels(&s), before, "{id} @{depth} did not undo");
            }
        }
    }

    #[test]
    fn bad_params_are_refused_not_clamped() {
        let mut s = session(8);
        for (id, bad) in [
            ("filter.other.replaceColorRange", json!({"fuzziness": 500})),
            ("filter.other.replaceColorRange", json!({"from": "red"})),
            ("filter.other.channelWaves", json!({"phase": "x"})),
            ("filter.other.channelWaves", json!({"red": 1})),
            ("filter.other.extractChannel", json!({"channel": "magic"})),
            ("filter.stylize.edgeDetection", json!({"algorithm": "canny"})),
            ("filter.stylize.edgeDetection", json!({"amount": f64::NAN})),
            ("filter.other.gaussianDifference", json!({"radius": 0})),
        ] {
            assert!(s.execute(id, bad.clone()).is_err(), "{id} {bad}");
        }
    }

    #[test]
    fn extract_channel_makes_the_chosen_component_grey() {
        let mut s = session(8);
        s.execute("filter.other.extractChannel", json!({"channel": "blue"})).unwrap();
        let px = pixels(&s);
        // The painted half was #2060c0: blue 0xc0 / 255.
        assert!((px[0] - 192.0 / 255.0).abs() < 0.01 && (px[0] - px[1]).abs() < 1e-4 && (px[1] - px[2]).abs() < 1e-4, "{:?}", &px[..4]);
    }
}
