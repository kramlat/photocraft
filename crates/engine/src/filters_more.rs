//! Further `filter.*` commands in Photoshop's vocabulary: Invert Brightness, Invert (Linear
//! Light), Replace Color Range, Channel Waves, Extract Channel, Edge Detection and Gaussian
//! Difference. They run through [`crate::filters::run_filter`] like every other filter, so
//! selections, smart filters, history and `filter.lastFilter` behave the same.

use photocraft_algo::{ChannelSource, EdgeAlgorithm, FilterParams};
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
        _ => return crate::gallery_cmds::params_for(id, p),
    })
}

/// Bounds and choices of this module's params; `None` for ids it doesn't own.
pub(crate) fn validate(id: &str, p: &Value) -> Option<Result<()>> {
    let numbers: &[(&str, f64, f64)] = match id {
        "filter.other.replaceColorRange" => &[("fuzziness", 0.0, 100.0)],
        "filter.other.channelWaves" => &[("frequency", 0.0, 20.0), ("phase", 0.0, 360.0)],
        "filter.stylize.edgeDetection" => &[("amount", 0.1, 10.0)],
        "filter.other.gaussianDifference" => &[("radius", 0.1, 100.0), ("outerRadius", 0.1, 100.0)],
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
        _ => None,
    };
    if let Some(msg) = msg {
        return bad(msg);
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
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Session;

    const IDS: [&str; 7] = [
        "filter.other.invertBrightness",
        "filter.other.invertLinear",
        "filter.other.replaceColorRange",
        "filter.other.channelWaves",
        "filter.other.extractChannel",
        "filter.stylize.edgeDetection",
        "filter.other.gaussianDifference",
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
