//! `script-fu-register`: what a script says about itself.

use photocraft_scheme::Value;

/// The kind of one script argument (`SF-*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Value,
    String,
    Adjustment,
    Color,
    Toggle,
    Image,
    Drawable,
    Other,
}

impl ParamKind {
    /// Script-Fu's numeric codes for the `SF-*` constants.
    pub const NAMES: &'static [(&'static str, ParamKind)] = &[
        ("SF-VALUE", ParamKind::Value),
        ("SF-STRING", ParamKind::String),
        ("SF-ADJUSTMENT", ParamKind::Adjustment),
        ("SF-COLOR", ParamKind::Color),
        ("SF-TOGGLE", ParamKind::Toggle),
        ("SF-IMAGE", ParamKind::Image),
        ("SF-DRAWABLE", ParamKind::Drawable),
        ("SF-LAYER", ParamKind::Drawable),
        ("SF-CHANNEL", ParamKind::Drawable),
        ("SF-FONT", ParamKind::Other),
        ("SF-PATTERN", ParamKind::Other),
        ("SF-GRADIENT", ParamKind::Other),
        ("SF-BRUSH", ParamKind::Other),
        ("SF-OPTION", ParamKind::Other),
        ("SF-ENUM", ParamKind::Other),
        ("SF-FILENAME", ParamKind::String),
        ("SF-DIRNAME", ParamKind::String),
        ("SF-TEXT", ParamKind::String),
        ("SF-PALETTE", ParamKind::Other),
    ];

    pub fn code(self) -> i64 {
        Self::NAMES.iter().position(|(_, k)| *k == self).map_or(0, |i| i as i64)
    }

    pub fn from_code(c: i64) -> ParamKind {
        usize::try_from(c).ok().and_then(|i| Self::NAMES.get(i)).map_or(ParamKind::Other, |(_, k)| *k)
    }
}

/// One argument a registered script takes.
#[derive(Debug, Clone)]
pub struct ParamSpec {
    pub kind: ParamKind,
    pub label: String,
    /// The default shown in the Run dialog, as `write` prints it.
    pub default: String,
}

/// A script announced with `script-fu-register`.
#[derive(Debug, Clone)]
pub struct Registered {
    /// The Scheme procedure that runs it.
    pub name: String,
    /// The menu entry's label (the last part of its menu path).
    pub label: String,
    /// The path it asked for with `script-fu-menu-register` (e.g. `<Image>/Filters/Render`).
    pub menu: Option<String>,
    pub description: String,
    pub params: Vec<ParamSpec>,
}

/// Builds a [`Registered`] from the arguments of `script-fu-register`:
/// `name menu-label description author copyright date image-types param…`, where each param is
/// `type label default…`.
pub fn parse_register(args: &[Value]) -> Result<Registered, String> {
    let s = |i: usize| args.get(i).and_then(Value::as_string).unwrap_or_default();
    let name = args.first().and_then(|v| v.as_string().or_else(|| v.as_sym().map(str::to_string))).ok_or("script-fu-register: missing script name")?;
    let mut params = Vec::new();
    let mut i = 7;
    while let Some(kind) = args.get(i) {
        let kind = kind.as_int().map(ParamKind::from_code).ok_or("script-fu-register: bad parameter type")?;
        let label = s(i + 1);
        let default = args.get(i + 2).map(Value::write).unwrap_or_default();
        params.push(ParamSpec { kind, label, default });
        i += 3;
    }
    // Menu labels look like "<Image>/Filters/Render/Foo…" or just "Foo…".
    let full = s(1);
    let (menu, label) = match full.rsplit_once('/') {
        Some((m, l)) => (Some(m.to_string()), l.to_string()),
        None => (None, full),
    };
    Ok(Registered { name, label, menu, description: s(2), params })
}
