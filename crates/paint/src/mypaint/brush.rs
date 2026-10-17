//! The MyPaint brush engine: turns pointer events into dabs (a port of libmypaint's
//! `mypaint-brush.c`). A [`Brush`] holds the settings (each a base value plus curves over the
//! inputs: pressure, speed, direction, tilt, stroke length, randomness…) and the state that
//! changes during a stroke; [`Brush::stroke_to`] is called once per pointer event and draws
//! dabs on a [`DabSurface`].

use super::color::{hsl_to_rgb, hsv_to_rgb, mix_colors, mod_arith, rgb_to_hsl, rgb_to_hsv, smallest_angular_difference, WGM_EPSILON};
use super::mapping::Mapping;
use super::rng::Rng;
use super::tables::{inp, set, INPUT_NAMES, N_INPUTS, N_SETTINGS, SETTINGS};

const ACTUAL_RADIUS_MIN: f32 = 0.2;
const ACTUAL_RADIUS_MAX: f32 = 1000.0;
const GRID_SIZE: f32 = 256.0;
/// The number of smudge buckets MyPaint 2.x gives a brush.
pub const N_BUCKETS: usize = 256;

/// Where dabs go. Positions are in surface pixels.
pub trait DabSurface {
    #[allow(clippy::too_many_arguments)]
    fn draw_dab(
        &mut self,
        x: f32,
        y: f32,
        radius: f32,
        color: [f32; 3],
        opaque: f32,
        hardness: f32,
        softness: f32,
        alpha_eraser: f32,
        aspect_ratio: f32,
        angle: f32,
        lock_alpha: f32,
        colorize: f32,
        posterize: f32,
        posterize_num: f32,
        paint: f32,
    ) -> bool;

    /// The average colour (straight RGB) and alpha under a dab-shaped area; used by smudge.
    fn get_color(&mut self, x: f32, y: f32, radius: f32, paint: f32) -> [f32; 4];
}

/// Smudge colour state: [SMUDGE r,g,b,a, previous sample r,g,b,a, recentness].
type Bucket = [f32; 9];
const SMUDGE_R: usize = 0;
const SMUDGE_G: usize = 1;
const SMUDGE_B: usize = 2;
const SMUDGE_A: usize = 3;
const PREV_R: usize = 4;
const PREV_A: usize = 7;
const PREV_RECENT: usize = 8;

#[derive(Clone, Debug, Default)]
struct State {
    x: f32,
    y: f32,
    pressure: f32,
    partial_dabs: f32,
    actual_radius: f32,
    actual_x: f32,
    actual_y: f32,
    norm_dx_slow: f32,
    norm_dy_slow: f32,
    norm_speed1_slow: f32,
    norm_speed2_slow: f32,
    stroke: f32,
    stroke_started: bool,
    custom_input: f32,
    actual_elliptical_dab_ratio: f32,
    actual_elliptical_dab_angle: f32,
    direction_dx: f32,
    direction_dy: f32,
    declination: f32,
    ascension: f32,
    viewzoom: f32,
    viewrotation: f32,
    direction_angle_dx: f32,
    direction_angle_dy: f32,
    flip: f32,
    gridmap_x: f32,
    gridmap_y: f32,
    declinationx: f32,
    declinationy: f32,
    dabs_per_basic_radius: f32,
    dabs_per_actual_radius: f32,
    dabs_per_second: f32,
    barrel_rotation: f32,
}

/// One pointer event.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    /// Pen tilt, -1..1 on each axis (0 for a mouse).
    pub xtilt: f32,
    pub ytilt: f32,
    /// Seconds since the previous event.
    pub dtime: f64,
    /// Canvas zoom (1 = 100 %) and rotation of the view in radians.
    pub viewzoom: f32,
    pub viewrotation: f32,
    /// Barrel rotation of the pen, 0..1 of a turn.
    pub barrel_rotation: f32,
}

#[derive(Clone, Debug)]
pub struct Brush {
    settings: Vec<Mapping>,
    settings_value: [f32; N_SETTINGS],
    state: State,
    buckets: Vec<Bucket>,
    random_input: f64,
    skip: f32,
    skip_last_x: f32,
    skip_last_y: f32,
    skipped_dtime: f32,
    rng: Rng,
    speed_mapping_gamma: [f32; 2],
    speed_mapping_m: [f32; 2],
    speed_mapping_q: [f32; 2],
    reset_requested: bool,
    stroke_total_painting_time: f64,
    stroke_current_idling_time: f64,
}

fn exp_decay(t_const: f32, t: f32) -> f32 {
    if t_const <= 0.001 {
        return 0.0;
    }
    (-t / t_const).exp()
}

impl Default for Brush {
    fn default() -> Self {
        Self::new()
    }
}

impl Brush {
    /// A brush with libmypaint's default settings.
    pub fn new() -> Self {
        let mut settings: Vec<Mapping> = (0..N_SETTINGS).map(|_| Mapping::new()).collect();
        for (i, m) in settings.iter_mut().enumerate() {
            m.base = SETTINGS[i].1;
        }
        settings[set::OPAQUE_MULTIPLY].set_curve(inp::PRESSURE, vec![(0.0, 0.0), (1.0, 1.0)]);
        let mut b = Brush {
            settings,
            settings_value: [0.0; N_SETTINGS],
            state: State::default(),
            buckets: vec![[0.0; 9]; N_BUCKETS],
            random_input: 0.0,
            skip: 0.0,
            skip_last_x: 0.0,
            skip_last_y: 0.0,
            skipped_dtime: 0.0,
            rng: Rng::new(1000),
            speed_mapping_gamma: [0.0; 2],
            speed_mapping_m: [0.0; 2],
            speed_mapping_q: [0.0; 2],
            reset_requested: true,
            stroke_total_painting_time: 0.0,
            stroke_current_idling_time: 0.0,
        };
        b.reset();
        b.settings_base_values_have_changed();
        b
    }

    /// Reads a `.myb` file (JSON, version 3). Settings the file does not name keep their
    /// defaults; unknown settings and inputs are ignored.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not a MyPaint brush: {e}"))?;
        let version = v.get("version").and_then(serde_json::Value::as_i64).ok_or("not a MyPaint brush: no version")?;
        if version != 3 {
            return Err(format!("unsupported MyPaint brush version {version} (3 is the one MyPaint 2 writes)"));
        }
        let settings = v.get("settings").and_then(serde_json::Value::as_object).ok_or("not a MyPaint brush: no settings")?;
        let mut b = Brush::new();
        for (name, obj) in settings {
            let Some(id) = SETTINGS.iter().position(|s| s.0 == name) else { continue };
            let Some(base) = obj.get("base_value").and_then(serde_json::Value::as_f64) else { continue };
            b.settings[id].base = base as f32;
            let Some(inputs) = obj.get("inputs").and_then(serde_json::Value::as_object) else { continue };
            for (input, points) in inputs {
                let Some(iid) = INPUT_NAMES.iter().position(|n| n == input) else { continue };
                let pts: Vec<(f32, f32)> = points
                    .as_array()
                    .map(|a| a.iter().filter_map(|p| Some((p.get(0)?.as_f64()? as f32, p.get(1)?.as_f64()? as f32))).collect())
                    .unwrap_or_default();
                if pts.len() > 64 || pts.windows(2).any(|w| w[1].0 < w[0].0) {
                    return Err(format!("setting {name}: the curve for {input} is not valid"));
                }
                b.settings[id].set_curve(iid, pts);
            }
        }
        b.settings_base_values_have_changed();
        Ok(b)
    }

    pub fn base_value(&self, setting: usize) -> f32 {
        self.settings[setting].base
    }

    pub fn set_base_value(&mut self, setting: usize, v: f32) {
        self.settings[setting].base = v;
        self.settings_base_values_have_changed();
    }

    pub fn mapping(&self, setting: usize) -> &Mapping {
        &self.settings[setting]
    }

    /// Sets the brush colour (straight RGB), as MyPaint does: the base hue, saturation and value.
    pub fn set_color(&mut self, rgb: [f32; 3]) {
        let [h, s, v] = rgb_to_hsv(rgb[0], rgb[1], rgb[2]);
        self.settings[set::COLOR_H].base = h;
        self.settings[set::COLOR_S].base = s;
        self.settings[set::COLOR_V].base = v;
    }

    /// Restarts the random numbers: the same seed and events paint the same stroke.
    pub fn seed(&mut self, seed: i64) {
        self.rng.set_seed(seed);
    }

    /// The next stroke is unrelated to the last: forget the pointer, speed and smudge colours.
    pub fn reset(&mut self) {
        self.reset_requested = true;
        self.stroke_total_painting_time = 0.0;
        self.stroke_current_idling_time = 0.0;
    }

    fn brush_reset(&mut self) {
        self.skip = 0.0;
        self.skip_last_x = 0.0;
        self.skip_last_y = 0.0;
        self.skipped_dtime = 0.0;
        self.state = State { flip: -1.0, ..Default::default() };
        self.buckets.fill([0.0; 9]);
    }

    fn base(&self, s: usize) -> f32 {
        self.settings[s].base
    }

    fn setting(&self, s: usize) -> f32 {
        self.settings_value[s]
    }

    fn settings_base_values_have_changed(&mut self) {
        for i in 0..2 {
            let gamma = (if i == 0 { self.base(set::SPEED1_GAMMA) } else { self.base(set::SPEED2_GAMMA) }).exp();
            let (fix1_x, fix1_y, fix2_x, fix2_dy) = (45.0f32, 0.5f32, 45.0f32, 0.015f32);
            let c1 = (fix1_x + gamma).ln();
            let m = fix2_dy * (fix2_x + gamma);
            let q = fix1_y - m * c1;
            self.speed_mapping_gamma[i] = gamma;
            self.speed_mapping_m[i] = m;
            self.speed_mapping_q[i] = q;
        }
    }

    fn directional_offsets(&self, base_radius: f32, brush_flip: i32) -> (f32, f32) {
        let offset_mult = self.setting(set::OFFSET_MULTIPLIER).exp();
        if !offset_mult.is_finite() {
            return (0.0, 0.0);
        }
        let (mut dx, mut dy) = (self.setting(set::OFFSET_X), self.setting(set::OFFSET_Y));
        let adj = self.setting(set::OFFSET_ANGLE_ADJ);
        let st = &self.state;
        let angle_deg = (st.direction_angle_dy.atan2(st.direction_angle_dx).to_degrees() - 90.0) % 360.0;
        let flip = brush_flip as f32;
        let offset_angle = self.setting(set::OFFSET_ANGLE);
        if offset_angle != 0.0 {
            let a = (angle_deg + adj).to_radians();
            dx += a.cos() * offset_angle;
            dy += a.sin() * offset_angle;
        }
        let view_rotation = st.viewrotation;
        let asc_off = self.setting(set::OFFSET_ANGLE_ASC);
        if asc_off != 0.0 {
            let a = (st.ascension - view_rotation + adj).to_radians();
            dx += a.cos() * asc_off;
            dy += a.sin() * asc_off;
        }
        let view_off = self.setting(set::OFFSET_ANGLE_VIEW);
        if view_off != 0.0 {
            let a = (view_rotation + adj).to_radians();
            dx += (-a).cos() * view_off;
            dy += (-a).sin() * view_off;
        }
        let dir_mirror = self.setting(set::OFFSET_ANGLE_2).max(0.0);
        if dir_mirror != 0.0 {
            let a = (angle_deg + adj * flip).to_radians();
            let f = dir_mirror * flip;
            dx += a.cos() * f;
            dy += a.sin() * f;
        }
        let asc_mirror = self.setting(set::OFFSET_ANGLE_2_ASC).max(0.0);
        if asc_mirror != 0.0 {
            let a = (st.ascension - view_rotation + adj * flip).to_radians();
            let f = flip * asc_mirror;
            dx += a.cos() * f;
            dy += a.sin() * f;
        }
        let view_mirror = self.setting(set::OFFSET_ANGLE_2_VIEW).max(0.0);
        if view_mirror != 0.0 {
            let f = flip * view_mirror;
            let a = (view_rotation + adj).to_radians();
            dx += (-a).cos() * f;
            dy += (-a).sin() * f;
        }
        let lim = 3240.0;
        let base_mul = base_radius * offset_mult;
        ((dx * base_mul).clamp(-lim, lim), (dy * base_mul).clamp(-lim, lim))
    }

    #[allow(clippy::too_many_arguments)]
    fn update_states_and_setting_values(
        &mut self,
        step_ddab: f32,
        step_dx: f32,
        step_dy: f32,
        step_dpressure: f32,
        step_declination: f32,
        step_ascension: f32,
        mut step_dtime: f32,
        step_viewzoom: f32,
        step_viewrotation: f32,
        step_declinationx: f32,
        step_declinationy: f32,
        step_barrel_rotation: f32,
    ) {
        if step_dtime <= 0.0 {
            step_dtime = 0.001;
        }
        {
            let st = &mut self.state;
            st.x += step_dx;
            st.y += step_dy;
            st.pressure += step_dpressure;
            st.declination += step_declination;
            st.ascension += step_ascension;
            st.declinationx += step_declinationx;
            st.declinationy += step_declinationy;
            st.viewzoom = step_viewzoom;
        }
        let viewrotation = mod_arith(step_viewrotation.to_degrees() + 180.0, 360.0) - 180.0;
        self.state.viewrotation = viewrotation;
        {
            let (x, y) = (self.state.actual_x, self.state.actual_y);
            let scale = self.setting(set::GRIDMAP_SCALE).exp();
            let (scale_x, scale_y) = (self.setting(set::GRIDMAP_SCALE_X), self.setting(set::GRIDMAP_SCALE_Y));
            let scaled_size = scale * GRID_SIZE;
            self.state.gridmap_x = mod_arith((x * scale_x).abs(), scaled_size) / scaled_size * GRID_SIZE;
            self.state.gridmap_y = mod_arith((y * scale_y).abs(), scaled_size) / scaled_size * GRID_SIZE;
            if x < 0.0 {
                self.state.gridmap_x = GRID_SIZE - self.state.gridmap_x;
            }
            if y < 0.0 {
                self.state.gridmap_y = GRID_SIZE - self.state.gridmap_y;
            }
        }
        let base_radius = self.base(set::RADIUS_LOGARITHMIC).exp();
        self.state.barrel_rotation += step_barrel_rotation;
        if self.state.pressure <= 0.0 {
            self.state.pressure = 0.0;
        }
        let pressure = self.state.pressure;
        {
            let lim = 0.0001;
            let threshold = self.base(set::STROKE_THRESHOLD);
            let started = self.state.stroke_started;
            if !started && pressure > threshold + lim {
                self.state.stroke_started = true;
                self.state.stroke = 0.0;
            } else if started && pressure <= threshold * 0.9 + lim {
                self.state.stroke_started = false;
            }
        }
        let zoom = self.state.viewzoom;
        let norm_dx = step_dx / step_dtime * zoom;
        let norm_dy = step_dy / step_dtime * zoom;
        let norm_speed = norm_dx.hypot(norm_dy);
        let norm_dist = (step_dx / step_dtime / base_radius).hypot(step_dy / step_dtime / base_radius) * step_dtime;

        let mut inputs = [0.0f32; N_INPUTS];
        inputs[inp::PRESSURE] = pressure * self.base(set::PRESSURE_GAIN_LOG).exp();
        inputs[inp::SPEED1] = (self.speed_mapping_gamma[0] + self.state.norm_speed1_slow).ln() * self.speed_mapping_m[0] + self.speed_mapping_q[0];
        inputs[inp::SPEED2] = (self.speed_mapping_gamma[1] + self.state.norm_speed2_slow).ln() * self.speed_mapping_m[1] + self.speed_mapping_q[1];
        inputs[inp::RANDOM] = self.random_input as f32;
        inputs[inp::STROKE] = self.state.stroke.min(1.0);
        let dir_angle = self.state.direction_dy.atan2(self.state.direction_dx);
        inputs[inp::DIRECTION] = mod_arith(dir_angle.to_degrees() + viewrotation + 180.0, 180.0);
        let dir_angle_360 = self.state.direction_angle_dy.atan2(self.state.direction_angle_dx);
        inputs[inp::DIRECTION_ANGLE] = (dir_angle_360.to_degrees() + viewrotation + 360.0) % 360.0;
        inputs[inp::TILT_DECLINATION] = self.state.declination;
        inputs[inp::TILT_ASCENSION] = mod_arith(self.state.ascension + viewrotation + 180.0, 360.0) - 180.0;
        inputs[inp::VIEWZOOM] = self.base(set::RADIUS_LOGARITHMIC) - (base_radius / zoom).ln();
        inputs[inp::ATTACK_ANGLE] = smallest_angular_difference(self.state.ascension, mod_arith(dir_angle_360.to_degrees() + 90.0, 360.0));
        inputs[inp::BRUSH_RADIUS] = self.base(set::RADIUS_LOGARITHMIC);
        inputs[inp::GRIDMAP_X] = self.state.gridmap_x.clamp(0.0, GRID_SIZE);
        inputs[inp::GRIDMAP_Y] = self.state.gridmap_y.clamp(0.0, GRID_SIZE);
        inputs[inp::TILT_DECLINATIONX] = self.state.declinationx;
        inputs[inp::TILT_DECLINATIONY] = self.state.declinationy;
        inputs[inp::CUSTOM] = self.state.custom_input;
        inputs[inp::BARREL_ROTATION] = mod_arith(self.state.barrel_rotation, 360.0);

        for i in 0..N_SETTINGS {
            self.settings_value[i] = self.settings[i].calculate(&inputs);
        }
        self.state.dabs_per_basic_radius = self.setting(set::DABS_PER_BASIC_RADIUS);
        self.state.dabs_per_actual_radius = self.setting(set::DABS_PER_ACTUAL_RADIUS);
        self.state.dabs_per_second = self.setting(set::DABS_PER_SECOND);
        {
            let fac = 1.0 - exp_decay(self.setting(set::SLOW_TRACKING_PER_DAB), step_ddab);
            self.state.actual_x += (self.state.x - self.state.actual_x) * fac;
            self.state.actual_y += (self.state.y - self.state.actual_y) * fac;
        }
        {
            let fac1 = 1.0 - exp_decay(self.setting(set::SPEED1_SLOWNESS), step_dtime);
            self.state.norm_speed1_slow += (norm_speed - self.state.norm_speed1_slow) * fac1;
            let fac2 = 1.0 - exp_decay(self.setting(set::SPEED2_SLOWNESS), step_dtime);
            self.state.norm_speed2_slow += (norm_speed - self.state.norm_speed2_slow) * fac2;
        }
        {
            let mut time_constant = (self.setting(set::OFFSET_BY_SPEED_SLOWNESS) * 0.01).exp() - 1.0;
            if time_constant < 0.002 {
                time_constant = 0.002;
            }
            let fac = 1.0 - exp_decay(time_constant, step_dtime);
            self.state.norm_dx_slow += (norm_dx - self.state.norm_dx_slow) * fac;
            self.state.norm_dy_slow += (norm_dy - self.state.norm_dy_slow) * fac;
        }
        {
            let mut dx = step_dx * self.state.viewzoom;
            let mut dy = step_dy * self.state.viewzoom;
            let step_in_dabtime = dx.hypot(dy);
            let fac = 1.0 - exp_decay((self.setting(set::DIRECTION_FILTER) * 0.5).exp() - 1.0, step_in_dabtime);
            let (dx_old, dy_old) = (self.state.direction_dx, self.state.direction_dy);
            self.state.direction_angle_dx += (dx - self.state.direction_angle_dx) * fac;
            self.state.direction_angle_dy += (dy - self.state.direction_angle_dy) * fac;
            let sq = |v: f32| v * v;
            if sq(dx_old - dx) + sq(dy_old - dy) > sq(dx_old - (-dx)) + sq(dy_old - (-dy)) {
                dx = -dx;
                dy = -dy;
            }
            self.state.direction_dx += (dx - self.state.direction_dx) * fac;
            self.state.direction_dy += (dy - self.state.direction_dy) * fac;
        }
        {
            let fac = 1.0 - exp_decay(self.setting(set::CUSTOM_INPUT_SLOWNESS), 0.1);
            self.state.custom_input += (self.setting(set::CUSTOM_INPUT) - self.state.custom_input) * fac;
        }
        {
            let frequency = (-self.setting(set::STROKE_DURATION_LOGARITHMIC)).exp();
            let stroke = (self.state.stroke + norm_dist * frequency).max(0.0);
            let wrap = 1.0 + self.setting(set::STROKE_HOLDTIME).max(0.0);
            if stroke >= wrap && wrap > 9.9 + 1.0 {
                self.state.stroke = 1.0;
            } else if stroke >= wrap {
                self.state.stroke = stroke % wrap;
            } else {
                self.state.stroke = stroke;
            }
        }
        let radius_log = self.setting(set::RADIUS_LOGARITHMIC);
        self.state.actual_radius = radius_log.exp().clamp(ACTUAL_RADIUS_MIN, ACTUAL_RADIUS_MAX);
        self.state.actual_elliptical_dab_ratio = self.setting(set::ELLIPTICAL_DAB_RATIO);
        self.state.actual_elliptical_dab_angle = mod_arith(self.setting(set::ELLIPTICAL_DAB_ANGLE) - viewrotation + 180.0, 180.0) - 180.0;
    }

    fn bucket_index(&self) -> usize {
        (self.setting(set::SMUDGE_BUCKET).round() as i64).clamp(0, N_BUCKETS as i64 - 1) as usize
    }

    /// Returns true when the dab must be skipped (the sampled canvas was too transparent).
    #[allow(clippy::too_many_arguments)]
    fn update_smudge_color<S: DabSurface>(&mut self, surface: &mut S, bucket: usize, smudge_length: f32, px: i32, py: i32, radius: f32, legacy: bool, paint_factor: f32) -> bool {
        let mut update_factor = smudge_length.max(0.01);
        let smudge_length_log = self.setting(set::SMUDGE_LENGTH_LOG);
        let recentness = self.buckets[bucket][PREV_RECENT] * update_factor;
        self.buckets[bucket][PREV_RECENT] = recentness;
        let margin = 0.0000000000000001f32;
        let (r, g, b, a);
        if recentness < (0.5 * update_factor).powf(smudge_length_log).min(1.0) + margin {
            if recentness == 0.0 {
                update_factor = 0.0;
            }
            self.buckets[bucket][PREV_RECENT] = 1.0;
            let radius_log = self.setting(set::SMUDGE_RADIUS_LOG);
            let smudge_radius = (radius * radius_log.exp()).clamp(ACTUAL_RADIUS_MIN, ACTUAL_RADIUS_MAX);
            let c = surface.get_color(px as f32, py as f32, smudge_radius, if legacy { -1.0 } else { paint_factor });
            (r, g, b, a) = (c[0], c[1], c[2], c[3]);
            let lim = self.setting(set::SMUDGE_TRANSPARENCY);
            if (lim > 0.0 && a < lim) || (lim < 0.0 && a > -lim) {
                return true;
            }
            let bk = &mut self.buckets[bucket];
            bk[PREV_R] = r;
            bk[PREV_R + 1] = g;
            bk[PREV_R + 2] = b;
            bk[PREV_A] = a;
        } else {
            let bk = &self.buckets[bucket];
            (r, g, b, a) = (bk[PREV_R], bk[PREV_R + 1], bk[PREV_R + 2], bk[PREV_A]);
        }
        let bk = &mut self.buckets[bucket];
        if legacy {
            let fac_old = update_factor;
            let fac_new = (1.0 - update_factor) * a;
            bk[SMUDGE_R] = fac_old * bk[SMUDGE_R] + fac_new * r;
            bk[SMUDGE_G] = fac_old * bk[SMUDGE_G] + fac_new * g;
            bk[SMUDGE_B] = fac_old * bk[SMUDGE_B] + fac_new * b;
            bk[SMUDGE_A] = (fac_old * bk[SMUDGE_A] + fac_new).clamp(0.0, 1.0);
        } else if a > WGM_EPSILON * 10.0 {
            let prev = [bk[SMUDGE_R], bk[SMUDGE_G], bk[SMUDGE_B], bk[SMUDGE_A]];
            let new = mix_colors(prev, [r, g, b, a], update_factor, paint_factor);
            bk[SMUDGE_R..=SMUDGE_A].copy_from_slice(&new);
        } else {
            bk[SMUDGE_A] = (bk[SMUDGE_A] + a) / 2.0;
        }
        false
    }

    fn apply_smudge(&self, bucket: usize, smudge_value: f32, legacy: bool, paint_factor: f32, color: &mut [f32; 3]) -> f32 {
        let bk = &self.buckets[bucket];
        let smudge_factor = smudge_value.min(1.0);
        let eraser_target_alpha = ((1.0 - smudge_factor) + smudge_factor * bk[SMUDGE_A]).clamp(0.0, 1.0);
        if eraser_target_alpha > 0.0 {
            if legacy {
                let col_factor = 1.0 - smudge_factor;
                for i in 0..3 {
                    color[i] = (smudge_factor * bk[i] + col_factor * color[i]) / eraser_target_alpha;
                }
            } else {
                let new = mix_colors([bk[0], bk[1], bk[2], bk[3]], [color[0], color[1], color[2], 1.0], smudge_factor, paint_factor);
                color.copy_from_slice(&new[..3]);
            }
        } else {
            *color = [1.0, 0.0, 0.0];
        }
        eraser_target_alpha
    }

    fn prepare_and_draw_dab<S: DabSurface>(&mut self, surface: &mut S, linear: bool) -> bool {
        let opaque_fac = self.setting(set::OPAQUE_MULTIPLY);
        let mut opaque = self.setting(set::OPAQUE).max(0.0);
        opaque = (opaque * opaque_fac).clamp(0.0, 1.0);
        let opaque_linearize = self.base(set::OPAQUE_LINEARIZE);
        if opaque_linearize != 0.0 {
            let mut dabs_per_pixel = (self.state.dabs_per_actual_radius + self.state.dabs_per_basic_radius) * 2.0;
            if dabs_per_pixel < 1.0 {
                dabs_per_pixel = 1.0;
            }
            dabs_per_pixel = 1.0 + opaque_linearize * (dabs_per_pixel - 1.0);
            let beta = 1.0 - opaque;
            let beta_dab = beta.powf(1.0 / dabs_per_pixel);
            opaque = 1.0 - beta_dab;
        }
        let mut x = self.state.actual_x;
        let mut y = self.state.actual_y;
        let base_radius = self.base(set::RADIUS_LOGARITHMIC).exp();
        let (ox, oy) = self.directional_offsets(base_radius, self.state.flip as i32);
        x += ox;
        y += oy;
        let view_zoom = self.state.viewzoom;
        let offset_by_speed = self.setting(set::OFFSET_BY_SPEED);
        if offset_by_speed != 0.0 {
            x += self.state.norm_dx_slow * offset_by_speed * 0.1 / view_zoom;
            y += self.state.norm_dy_slow * offset_by_speed * 0.1 / view_zoom;
        }
        let offset_by_random = self.setting(set::OFFSET_BY_RANDOM);
        if offset_by_random != 0.0 {
            let amp = offset_by_random.max(0.0);
            x += self.rng.gauss() * amp * base_radius;
            y += self.rng.gauss() * amp * base_radius;
        }
        let mut radius = self.state.actual_radius;
        let radius_by_random = self.setting(set::RADIUS_BY_RANDOM);
        if radius_by_random != 0.0 {
            let noise = self.rng.gauss() * radius_by_random;
            let radius_log = self.setting(set::RADIUS_LOGARITHMIC) + noise;
            radius = radius_log.exp().clamp(ACTUAL_RADIUS_MIN, ACTUAL_RADIUS_MAX);
            let q = self.state.actual_radius / radius;
            let alpha_correction = q * q;
            if alpha_correction <= 1.0 {
                opaque *= alpha_correction;
            }
        }
        let paint_factor = self.setting(set::PAINT_MODE);
        let paint_constant = self.settings[set::PAINT_MODE].is_constant();
        let legacy_smudge = paint_factor <= 0.0 && paint_constant;
        let mut color = hsv_to_rgb(self.base(set::COLOR_H), self.base(set::COLOR_S), self.base(set::COLOR_V));
        let smudge_length = self.setting(set::SMUDGE_LENGTH);
        if smudge_length < 1.0 && (self.setting(set::SMUDGE) != 0.0 || !self.settings[set::SMUDGE].is_constant()) {
            let bucket = self.bucket_index();
            let early = self.update_smudge_color(surface, bucket, smudge_length, (x + 0.5) as i32, (y + 0.5) as i32, radius, legacy_smudge, paint_factor);
            if early {
                return false;
            }
        }
        let mut eraser_target_alpha = 1.0;
        let smudge_value = self.setting(set::SMUDGE);
        if smudge_value > 0.0 {
            let bucket = self.bucket_index();
            eraser_target_alpha = self.apply_smudge(bucket, smudge_value, legacy_smudge, paint_factor, &mut color);
        }
        if self.setting(set::ERASER) != 0.0 {
            eraser_target_alpha *= 1.0 - self.setting(set::ERASER);
        }
        let using_hsv = self.setting(set::CHANGE_COLOR_H) != 0.0 || self.setting(set::CHANGE_COLOR_HSV_S) != 0.0 || self.setting(set::CHANGE_COLOR_V) != 0.0;
        let using_hsl = self.setting(set::CHANGE_COLOR_L) != 0.0 || self.setting(set::CHANGE_COLOR_HSL_S) != 0.0;
        let using_dynamics = using_hsv || using_hsl;
        if linear && using_dynamics {
            color = color.map(|c| c.powf(1.0 / 2.2));
        }
        if using_hsv {
            let [mut h, mut s, mut v] = rgb_to_hsv(color[0], color[1], color[2]);
            h += self.setting(set::CHANGE_COLOR_H);
            s += s * v * self.setting(set::CHANGE_COLOR_HSV_S);
            v += self.setting(set::CHANGE_COLOR_V);
            color = hsv_to_rgb(h, s, v);
        }
        if using_hsl {
            let [h, mut s, mut l] = rgb_to_hsl(color[0], color[1], color[2]);
            l += self.setting(set::CHANGE_COLOR_L);
            s += s * (1.0 - l).abs().min(l.abs()) * 2.0 * self.setting(set::CHANGE_COLOR_HSL_S);
            color = hsl_to_rgb(h, s, l);
        }
        if linear && using_dynamics {
            color = color.map(|c| c.powf(2.2));
        }
        let mut hardness = self.setting(set::HARDNESS).clamp(0.0, 1.0);
        let softness = self.setting(set::SOFTNESS).clamp(0.0, 1.0);
        let current_fadeout = radius * (1.0 - hardness);
        let min_fadeout = self.setting(set::ANTI_ALIASING);
        if current_fadeout < min_fadeout {
            let current_optical_radius = radius - (1.0 - hardness) * radius / 2.0;
            let hardness_new = (current_optical_radius - min_fadeout / 2.0) / (current_optical_radius + min_fadeout / 2.0);
            let radius_new = min_fadeout / (1.0 - hardness_new);
            hardness = hardness_new;
            radius = radius_new;
        }
        let snap = self.setting(set::SNAP_TO_PIXEL);
        if snap > 0.0 {
            let (sx, sy) = (x.floor() + 0.5, y.floor() + 0.5);
            x += (sx - x) * snap;
            y += (sy - y) * snap;
            let mut snapped_radius = (radius * 2.0).round() / 2.0;
            if snapped_radius < 0.5 {
                snapped_radius = 0.5;
            }
            if snap > 0.9999 {
                snapped_radius -= 0.0001;
            }
            radius += (snapped_radius - radius) * snap;
        }
        surface.draw_dab(
            x,
            y,
            radius,
            color,
            opaque,
            hardness,
            softness,
            eraser_target_alpha,
            self.state.actual_elliptical_dab_ratio,
            self.state.actual_elliptical_dab_angle,
            self.setting(set::LOCK_ALPHA),
            self.setting(set::COLORIZE),
            self.setting(set::POSTERIZE),
            self.setting(set::POSTERIZE_NUM),
            paint_factor,
        )
    }

    fn count_dabs_to(&mut self, x: f32, y: f32, dt: f32) -> f32 {
        let base_radius = self.base(set::RADIUS_LOGARITHMIC).exp().clamp(ACTUAL_RADIUS_MIN, ACTUAL_RADIUS_MAX);
        if self.state.actual_radius == 0.0 {
            self.state.actual_radius = base_radius;
        }
        let dx = x - self.state.x;
        let dy = y - self.state.y;
        let dist = if self.state.actual_elliptical_dab_ratio > 1.0 {
            let a = self.state.actual_elliptical_dab_angle.to_radians();
            let (cs, sn) = (a.cos(), a.sin());
            let yyr = (dy * cs - dx * sn) * self.state.actual_elliptical_dab_ratio;
            let xxr = dy * sn + dx * cs;
            (yyr * yyr + xxr * xxr).sqrt()
        } else {
            dx.hypot(dy)
        };
        let res1 = dist / self.state.actual_radius * self.state.dabs_per_actual_radius;
        let res2 = dist / base_radius * self.state.dabs_per_basic_radius;
        let res3 = dt * self.state.dabs_per_second;
        let res4 = res1 + res2 + res3;
        if res4.is_nan() || res4 < 0.0 { 0.0 } else { res4 }
    }

    /// Feeds one pointer event; draws the dabs it causes on `surface`. Returns true when the
    /// stroke should be split here (an idle pointer, or a very long stroke).
    pub fn stroke_to<S: DabSurface>(&mut self, surface: &mut S, ev: Event, linear: bool) -> bool {
        let max_dtime = 5.0f64;
        let (mut x, mut y, mut pressure) = (ev.x, ev.y, ev.pressure);
        let (mut xtilt, mut ytilt) = (ev.xtilt, ev.ytilt);
        let mut dtime = ev.dtime;
        let (mut viewzoom, mut viewrotation, mut barrel) = (ev.viewzoom, ev.viewrotation, ev.barrel_rotation);
        let (mut tilt_ascension, mut tilt_declination, mut tilt_declinationx, mut tilt_declinationy) = (0.0f32, 90.0f32, 90.0f32, 90.0f32);
        if xtilt != 0.0 || ytilt != 0.0 {
            xtilt = xtilt.clamp(-1.0, 1.0);
            ytilt = ytilt.clamp(-1.0, 1.0);
            tilt_ascension = (-f64::from(xtilt)).atan2(f64::from(ytilt)).to_degrees() as f32;
            let rad = xtilt.hypot(ytilt);
            tilt_declination = 90.0 - rad * 60.0;
            tilt_declinationx = xtilt * 60.0;
            tilt_declinationy = ytilt * 60.0;
        }
        if pressure <= 0.0 {
            pressure = 0.0;
        }
        if !x.is_finite() || !y.is_finite() || x.abs() > 1e8 || y.abs() > 1e8 {
            x = 0.0;
            y = 0.0;
            pressure = 0.0;
            viewzoom = 0.0;
            viewrotation = 0.0;
            barrel = 0.0;
        }
        if viewzoom <= 0.0 {
            viewzoom = 1.0;
        }
        if dtime <= 0.0 {
            dtime = 0.0001;
        }
        if dtime > 0.100 && pressure != 0.0 && self.state.pressure == 0.0 {
            // Tablets that report no motion without pressure: do not interpolate the pressure
            // between the two events.
            self.stroke_to(surface, Event { x, y, pressure: 0.0, xtilt: 1.0, ytilt: 0.0, dtime: dtime - 0.0001, viewzoom, viewrotation, barrel_rotation: 0.0 }, linear);
            dtime = 0.0001;
        }
        if self.skip > 0.001 {
            let dist = (self.skip_last_x - x).hypot(self.skip_last_y - y);
            self.skip_last_x = x;
            self.skip_last_y = y;
            self.skipped_dtime += dtime as f32;
            self.skip -= dist;
            dtime = f64::from(self.skipped_dtime);
            if self.skip > 0.001 && !(dtime > max_dtime || self.reset_requested) {
                return false;
            }
            self.skip = 0.0;
            self.skip_last_x = 0.0;
            self.skip_last_y = 0.0;
            self.skipped_dtime = 0.0;
        }
        {
            if self.base(set::TRACKING_NOISE) != 0.0 {
                let base_radius = self.base(set::RADIUS_LOGARITHMIC).exp();
                let noise = base_radius * self.base(set::TRACKING_NOISE);
                if noise > 0.001 {
                    self.skip = 0.5 * noise;
                    self.skip_last_x = x;
                    self.skip_last_y = y;
                    x += noise * self.rng.gauss();
                    y += noise * self.rng.gauss();
                }
            }
            let fac = 1.0 - exp_decay(self.base(set::SLOW_TRACKING), 100.0 * dtime as f32);
            x = self.state.x + (x - self.state.x) * fac;
            y = self.state.y + (y - self.state.y) * fac;
        }
        if dtime > max_dtime || self.reset_requested {
            self.reset_requested = false;
            self.brush_reset();
            self.random_input = self.rng.next();
            self.state.x = x;
            self.state.y = y;
            self.state.pressure = pressure;
            self.state.actual_x = x;
            self.state.actual_y = y;
            self.state.stroke = 1.0;
            return true;
        }
        let mut painted: Option<bool> = None;
        let mut dtime_left = dtime;
        let mut dabs_moved = self.state.partial_dabs;
        let mut dabs_todo = self.count_dabs_to(x, y, dtime as f32);
        let mut step_dtime;
        let mut step_dpressure;
        while dabs_moved + dabs_todo >= 1.0 {
            let step_ddab;
            if dabs_moved > 0.0 {
                step_ddab = 1.0 - dabs_moved;
                dabs_moved = 0.0;
            } else {
                step_ddab = 1.0;
            }
            let frac = step_ddab / dabs_todo;
            let step_dx = frac * (x - self.state.x);
            let step_dy = frac * (y - self.state.y);
            step_dpressure = frac * (pressure - self.state.pressure);
            step_dtime = (f64::from(frac) * dtime_left) as f32;
            let step_declination = frac * (tilt_declination - self.state.declination);
            let step_declinationx = frac * (tilt_declinationx - self.state.declinationx);
            let step_declinationy = frac * (tilt_declinationy - self.state.declinationy);
            let step_ascension = frac * smallest_angular_difference(self.state.ascension, tilt_ascension);
            let step_barrel = frac * smallest_angular_difference(self.state.barrel_rotation, barrel * 360.0);
            self.update_states_and_setting_values(step_ddab, step_dx, step_dy, step_dpressure, step_declination, step_ascension, step_dtime, viewzoom, viewrotation, step_declinationx, step_declinationy, step_barrel);
            self.state.flip *= -1.0;
            let painted_now = self.prepare_and_draw_dab(surface, linear);
            if painted_now {
                painted = Some(true);
            } else if painted.is_none() {
                painted = Some(false);
            }
            self.random_input = self.rng.next();
            dtime_left -= f64::from(step_dtime);
            dabs_todo = self.count_dabs_to(x, y, dtime_left as f32);
        }
        {
            let step_ddab = dabs_todo;
            let step_dx = x - self.state.x;
            let step_dy = y - self.state.y;
            step_dpressure = pressure - self.state.pressure;
            let step_declination = tilt_declination - self.state.declination;
            let step_declinationx = tilt_declinationx - self.state.declinationx;
            let step_declinationy = tilt_declinationy - self.state.declinationy;
            let step_ascension = smallest_angular_difference(self.state.ascension, tilt_ascension);
            let step_barrel = smallest_angular_difference(self.state.barrel_rotation, barrel * 360.0);
            self.update_states_and_setting_values(step_ddab, step_dx, step_dy, step_dpressure, step_declination, step_ascension, dtime_left as f32, viewzoom, viewrotation, step_declinationx, step_declinationy, step_barrel);
        }
        self.state.partial_dabs = dabs_moved + dabs_todo;
        let painted = painted.unwrap_or(!(self.stroke_current_idling_time > 0.0 || self.stroke_total_painting_time == 0.0));
        if painted {
            self.stroke_total_painting_time += dtime;
            self.stroke_current_idling_time = 0.0;
            if self.stroke_total_painting_time > 4.0 + 3.0 * f64::from(pressure) && step_dpressure >= 0.0 {
                return true;
            }
        } else {
            self.stroke_current_idling_time += dtime;
            if self.stroke_total_painting_time == 0.0 {
                if self.stroke_current_idling_time > 1.0 {
                    return true;
                }
            } else if self.stroke_total_painting_time + self.stroke_current_idling_time > 0.9 + 5.0 * f64::from(pressure) {
                return true;
            }
        }
        false
    }
}
