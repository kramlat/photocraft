//! Brush setting and input tables, generated from libmypaint's `brushsettings.json` (ISC licence,
//! (C) 2007-2011 Martin Renold and contributors; see `NOTICE`). Names are the keys of `.myb` files.

pub const N_SETTINGS: usize = 65;
pub const N_INPUTS: usize = 18;

/// Setting indices.
pub mod set {
    pub const OPAQUE: usize = 0;
    pub const OPAQUE_MULTIPLY: usize = 1;
    pub const OPAQUE_LINEARIZE: usize = 2;
    pub const RADIUS_LOGARITHMIC: usize = 3;
    pub const HARDNESS: usize = 4;
    pub const SOFTNESS: usize = 5;
    pub const ANTI_ALIASING: usize = 6;
    pub const DABS_PER_BASIC_RADIUS: usize = 7;
    pub const DABS_PER_ACTUAL_RADIUS: usize = 8;
    pub const DABS_PER_SECOND: usize = 9;
    pub const GRIDMAP_SCALE: usize = 10;
    pub const GRIDMAP_SCALE_X: usize = 11;
    pub const GRIDMAP_SCALE_Y: usize = 12;
    pub const RADIUS_BY_RANDOM: usize = 13;
    pub const SPEED1_SLOWNESS: usize = 14;
    pub const SPEED2_SLOWNESS: usize = 15;
    pub const SPEED1_GAMMA: usize = 16;
    pub const SPEED2_GAMMA: usize = 17;
    pub const OFFSET_BY_RANDOM: usize = 18;
    pub const OFFSET_Y: usize = 19;
    pub const OFFSET_X: usize = 20;
    pub const OFFSET_ANGLE: usize = 21;
    pub const OFFSET_ANGLE_ASC: usize = 22;
    pub const OFFSET_ANGLE_VIEW: usize = 23;
    pub const OFFSET_ANGLE_2: usize = 24;
    pub const OFFSET_ANGLE_2_ASC: usize = 25;
    pub const OFFSET_ANGLE_2_VIEW: usize = 26;
    pub const OFFSET_ANGLE_ADJ: usize = 27;
    pub const OFFSET_MULTIPLIER: usize = 28;
    pub const OFFSET_BY_SPEED: usize = 29;
    pub const OFFSET_BY_SPEED_SLOWNESS: usize = 30;
    pub const SLOW_TRACKING: usize = 31;
    pub const SLOW_TRACKING_PER_DAB: usize = 32;
    pub const TRACKING_NOISE: usize = 33;
    pub const COLOR_H: usize = 34;
    pub const COLOR_S: usize = 35;
    pub const COLOR_V: usize = 36;
    pub const RESTORE_COLOR: usize = 37;
    pub const CHANGE_COLOR_H: usize = 38;
    pub const CHANGE_COLOR_L: usize = 39;
    pub const CHANGE_COLOR_HSL_S: usize = 40;
    pub const CHANGE_COLOR_V: usize = 41;
    pub const CHANGE_COLOR_HSV_S: usize = 42;
    pub const SMUDGE: usize = 43;
    pub const PAINT_MODE: usize = 44;
    pub const SMUDGE_TRANSPARENCY: usize = 45;
    pub const SMUDGE_LENGTH: usize = 46;
    pub const SMUDGE_LENGTH_LOG: usize = 47;
    pub const SMUDGE_BUCKET: usize = 48;
    pub const SMUDGE_RADIUS_LOG: usize = 49;
    pub const ERASER: usize = 50;
    pub const STROKE_THRESHOLD: usize = 51;
    pub const STROKE_DURATION_LOGARITHMIC: usize = 52;
    pub const STROKE_HOLDTIME: usize = 53;
    pub const CUSTOM_INPUT: usize = 54;
    pub const CUSTOM_INPUT_SLOWNESS: usize = 55;
    pub const ELLIPTICAL_DAB_RATIO: usize = 56;
    pub const ELLIPTICAL_DAB_ANGLE: usize = 57;
    pub const DIRECTION_FILTER: usize = 58;
    pub const LOCK_ALPHA: usize = 59;
    pub const COLORIZE: usize = 60;
    pub const POSTERIZE: usize = 61;
    pub const POSTERIZE_NUM: usize = 62;
    pub const SNAP_TO_PIXEL: usize = 63;
    pub const PRESSURE_GAIN_LOG: usize = 64;
}
/// Input indices.
pub mod inp {
    pub const PRESSURE: usize = 0;
    pub const RANDOM: usize = 1;
    pub const STROKE: usize = 2;
    pub const DIRECTION: usize = 3;
    pub const TILT_DECLINATION: usize = 4;
    pub const TILT_ASCENSION: usize = 5;
    pub const SPEED1: usize = 6;
    pub const SPEED2: usize = 7;
    pub const CUSTOM: usize = 8;
    pub const DIRECTION_ANGLE: usize = 9;
    pub const ATTACK_ANGLE: usize = 10;
    pub const TILT_DECLINATIONX: usize = 11;
    pub const TILT_DECLINATIONY: usize = 12;
    pub const GRIDMAP_X: usize = 13;
    pub const GRIDMAP_Y: usize = 14;
    pub const VIEWZOOM: usize = 15;
    pub const BRUSH_RADIUS: usize = 16;
    pub const BARREL_ROTATION: usize = 17;
}

/// `(name, default, minimum, maximum)` per setting.
pub const SETTINGS: [(&str, f32, f32, f32); N_SETTINGS] = [
    ("opaque", 1.0, 0.0, 2.0),
    ("opaque_multiply", 0.0, 0.0, 2.0),
    ("opaque_linearize", 0.9, 0.0, 2.0),
    ("radius_logarithmic", 2.0, -2.0, 6.0),
    ("hardness", 0.8, 0.0, 1.0),
    ("softness", 0.0, 0.0, 1.0),
    ("anti_aliasing", 1.0, 0.0, 5.0),
    ("dabs_per_basic_radius", 0.0, 0.0, 200.0),
    ("dabs_per_actual_radius", 2.0, 0.0, 200.0),
    ("dabs_per_second", 0.0, 0.0, 200.0),
    ("gridmap_scale", 0.0, -10.0, 10.0),
    ("gridmap_scale_x", 1.0, 0.0, 10.0),
    ("gridmap_scale_y", 1.0, 0.0, 10.0),
    ("radius_by_random", 0.0, 0.0, 1.5),
    ("speed1_slowness", 0.04, 0.0, 0.2),
    ("speed2_slowness", 0.8, 0.0, 3.0),
    ("speed1_gamma", 4.0, -8.0, 8.0),
    ("speed2_gamma", 4.0, -8.0, 8.0),
    ("offset_by_random", 0.0, 0.0, 25.0),
    ("offset_y", 0.0, -40.0, 40.0),
    ("offset_x", 0.0, -40.0, 40.0),
    ("offset_angle", 0.0, -40.0, 40.0),
    ("offset_angle_asc", 0.0, -40.0, 40.0),
    ("offset_angle_view", 0.0, -40.0, 40.0),
    ("offset_angle_2", 0.0, 0.0, 40.0),
    ("offset_angle_2_asc", 0.0, 0.0, 40.0),
    ("offset_angle_2_view", 0.0, 0.0, 40.0),
    ("offset_angle_adj", 0.0, -180.0, 180.0),
    ("offset_multiplier", 0.0, -2.0, 3.0),
    ("offset_by_speed", 0.0, -3.0, 3.0),
    ("offset_by_speed_slowness", 1.0, 0.0, 15.0),
    ("slow_tracking", 0.0, 0.0, 10.0),
    ("slow_tracking_per_dab", 0.0, 0.0, 10.0),
    ("tracking_noise", 0.0, 0.0, 12.0),
    ("color_h", 0.0, 0.0, 1.0),
    ("color_s", 0.0, -0.5, 1.5),
    ("color_v", 0.0, -0.5, 1.5),
    ("restore_color", 0.0, 0.0, 1.0),
    ("change_color_h", 0.0, -2.0, 2.0),
    ("change_color_l", 0.0, -2.0, 2.0),
    ("change_color_hsl_s", 0.0, -2.0, 2.0),
    ("change_color_v", 0.0, -2.0, 2.0),
    ("change_color_hsv_s", 0.0, -2.0, 2.0),
    ("smudge", 0.0, 0.0, 1.0),
    ("paint_mode", 1.0, 0.0, 1.0),
    ("smudge_transparency", 0.0, -1.0, 1.0),
    ("smudge_length", 0.5, 0.0, 1.0),
    ("smudge_length_log", 0.0, 0.0, 20.0),
    ("smudge_bucket", 0.0, 0.0, 255.0),
    ("smudge_radius_log", 0.0, -1.6, 1.6),
    ("eraser", 0.0, 0.0, 1.0),
    ("stroke_threshold", 0.0, 0.0, 0.5),
    ("stroke_duration_logarithmic", 4.0, -1.0, 14.0),
    ("stroke_holdtime", 0.0, 0.0, 10.0),
    ("custom_input", 0.0, -5.0, 5.0),
    ("custom_input_slowness", 0.0, 0.0, 10.0),
    ("elliptical_dab_ratio", 1.0, 1.0, 10.0),
    ("elliptical_dab_angle", 90.0, 0.0, 180.0),
    ("direction_filter", 2.0, 0.0, 10.0),
    ("lock_alpha", 0.0, 0.0, 1.0),
    ("colorize", 0.0, 0.0, 1.0),
    ("posterize", 0.0, 0.0, 1.0),
    ("posterize_num", 0.05, 0.01, 1.28),
    ("snap_to_pixel", 0.0, 0.0, 1.0),
    ("pressure_gain_log", 0.0, -1.8, 1.8),
];

pub const INPUT_NAMES: [&str; N_INPUTS] = [
    "pressure",
    "random",
    "stroke",
    "direction",
    "tilt_declination",
    "tilt_ascension",
    "speed1",
    "speed2",
    "custom",
    "direction_angle",
    "attack_angle",
    "tilt_declinationx",
    "tilt_declinationy",
    "gridmap_x",
    "gridmap_y",
    "viewzoom",
    "brush_radius",
    "barrel_rotation",
];
