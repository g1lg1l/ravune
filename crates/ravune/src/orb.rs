//! The AI's state as a Thinking Orb (Jakub Antalik's thinking-orbs, MIT; ported from Velo):
//! honestly 3D dots, rotated, depth-shaded and z-sorted, pure math over (size, t), drawn as
//! square pixels, Ravune's way. Three of its nine states,
//! ported from its engine with its tuned 64-px presets and checked against its golden frames:
//! breathing (waiting), working (thinking), searching (reading the project, finding sounds),
//! listening (rendering and listening to the song), composing (editing with operators).

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use eframe::egui::{Color32, Painter, Pos2, Rect, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    /// Nothing asked: a ring slowly morphing.
    Breathing,
    /// A reply on its way: particles on tilted orbits.
    Working,
    /// Commands running: an undulating sash.
    Composing,
    /// Reading the project or the library: a globe with a meridian sweeping it.
    Searching,
    /// Listening to a render: a sphere of rings in waves.
    Listening,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Dot {
    x: f64,
    y: f64,
    z: f64,
    r: f64,
    /// Ink: 0 the darkest, mirrored on dark themes.
    white: f64,
    a: f64,
}

/// thinking-orbs' resolved 64-px presets: the clock's speed and the mode's options.
struct Ribbon {
    lanes: f64,
    segs: f64,
    ghost_n: f64,
    face_on: bool,
    r_base: f64,
    r_depth: f64,
    band_mul: f64,
    wob_mul: f64,
}

const COMPOSING: Ribbon = Ribbon { lanes: 3.0, segs: 44.0, ghost_n: 38.0, face_on: false, r_base: 0.935, r_depth: 1.445, band_mul: 3.9, wob_mul: 1.0 };
const BREATHING: Ribbon = Ribbon { lanes: 3.0, segs: 44.0, ghost_n: 0.0, face_on: true, r_base: 1.0516, r_depth: 1.6252, band_mul: 3.627, wob_mul: 0.368 };
const R_MIN: f64 = 0.3;

impl State {
    fn speed(self) -> f64 {
        match self {
            State::Breathing => 3.24,
            State::Working => 1.885,
            State::Composing => 2.34,
            State::Searching => 2.015,
            State::Listening => 4.388,
        }
    }
}

/// Deterministic hash in [0, 1).
fn hash(a: f64, b: f64) -> f64 {
    let h = (a * 12.9898 + b * 78.233).sin() * 43758.5453;
    h - h.floor()
}

/// Stable directions on a unit sphere (Fibonacci lattice).
fn fib_dir(i: f64, n: f64) -> [f64; 3] {
    let golden = PI * (3.0 - 5f64.sqrt());
    let y = 1.0 - (2.0 * (i + 0.5)) / n;
    let rad = (1.0 - y * y).sqrt();
    let a = i * golden;
    [rad * a.cos(), y, rad * a.sin()]
}

/// Spin, tilt, orthographic projection, `scale` points to a unit.
fn projector(yaw: f64, tilt: f64, cx: f64, cy: f64, scale: f64) -> impl Fn(f64, f64, f64) -> [f64; 3] {
    let (st, ct, sy, cyw) = (tilt.sin(), tilt.cos(), yaw.sin(), yaw.cos());
    move |x, y, z| {
        let x1 = x * cyw + z * sy;
        let z1 = -x * sy + z * cyw;
        let y1 = y * ct - z1 * st;
        let z2 = y * st + z1 * ct;
        [cx + x1 * scale, cy - y1 * scale, z2]
    }
}

/// Shortest signed angle from `b` to `a`, in (-π, π].
fn angle_delta(a: f64, b: f64) -> f64 {
    (a - b).sin().atan2((a - b).cos())
}

/// Dot radii were tuned for a 300-pt orb; small ones shrink less.
fn radius_scale(size: f64) -> f64 {
    (size / 300.0).powf(0.6)
}

/// Drops what can't be seen, floors the radii, sorts far to near (stable, like JS).
fn finalize(dots: Vec<Dot>) -> Vec<Dot> {
    let mut visible: Vec<Dot> = dots.into_iter().filter(|d| d.a >= 0.02).map(|d| Dot { r: d.r.max(R_MIN), ..d }).collect();
    // JS's `a.z - b.z`: -0 and 0 are equal (total_cmp would order them)
    visible.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
    visible
}

fn orbits(size: f64, t: f64) -> Vec<Dot> {
    let (cx, cy, big_r) = (size / 2.0, size / 2.0, size / 2.0 * 0.82);
    let pt = projector(t * 0.12, 0.3, cx, cy, 1.0);
    let rs = radius_scale(size);
    let mut dots = Vec::new();
    for orb in 0..12 {
        let orb = orb as f64;
        let (h1, h2, h3) = (hash(orb, 1.7), hash(orb, 5.2), hash(orb, 8.9));
        let ro = big_r * (0.45 + 0.52 * h1);
        let th = h1 * TAU;
        let phi = (2.0 * h2 - 1.0).acos();
        let (nx, ny, nz) = (phi.sin() * th.cos(), phi.cos(), phi.sin() * th.sin());
        let (mut ux, mut uy, uz) = (-ny, nx, 0.0);
        let ul = (ux * ux + uy * uy).sqrt().max(1e-6);
        ux /= ul;
        uy /= ul;
        let (vx, vy, vz) = (ny * uz - nz * uy, nz * ux - nx * uz, nx * uy - ny * ux);
        let speed = (0.25 + 0.55 * h3) * if h3 > 0.5 { 1.0 } else { -1.0 };
        let on = |a: f64| pt((ux * a.cos() + vx * a.sin()) * ro, (uy * a.cos() + vy * a.sin()) * ro, (uz * a.cos() + vz * a.sin()) * ro);
        // the ghost path
        for k in 0..40 {
            let [x, y, z] = on(k as f64 / 40.0 * TAU);
            let depth = (z / ro + 1.0) / 2.0;
            dots.push(Dot { x, y, z, r: 0.9 * rs, white: 0.72, a: 0.5 * (0.4 + 0.6 * depth) });
        }
        // the particles doing the work
        for m in 0..3 {
            let [x, y, z] = on(t * speed + m as f64 / 3.0 * TAU + h2 * 6.0);
            let depth = (z / ro + 1.0) / 2.0;
            dots.push(Dot { x, y, z, r: (1.2 + 1.6 * depth) * rs, white: 0.3 - 0.22 * depth, a: 1.0 });
        }
    }
    finalize(dots)
}

/// A sash of parallel strands on a great circle (composing), or face on, a ring whose radius
/// undulates (breathing). The presets freeze the tumble (spin 0).
fn ribbon(size: f64, t: f64, o: &Ribbon) -> Vec<Dot> {
    let (cx, cy, big_r) = (size / 2.0, size / 2.0, size / 2.0 * 0.78);
    let cam_tilt = 0.3;
    let pt = projector(0.0, cam_tilt, cx, cy, 1.0);
    let rs = radius_scale(size);
    let mut dots = Vec::new();
    for i in 0..o.ghost_n as usize {
        let d = fib_dir(i as f64, o.ghost_n);
        let [x, y, z] = pt(d[0] * big_r, d[1] * big_r, d[2] * big_r);
        let depth = (z / big_r + 1.0) / 2.0;
        dots.push(Dot { x, y, z, r: 0.8 * rs, white: 0.78, a: 0.1 + 0.22 * depth });
    }
    let ta: f64 = if o.face_on { -cam_tilt } else { 0.55 };
    let (ux, uy, uz) = (1.0, 0.0, 0.0);
    let (vx, vy, vz) = (-uz * ta.sin(), ta.cos(), ux * ta.sin());
    let (nx, ny, nz) = (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx);
    let wob_amp = 0.23 * o.wob_mul;
    let base_r = if o.face_on { big_r / (1.0 + 0.85 * wob_amp) } else { big_r };
    let lanes = (o.lanes * o.band_mul).round().max(1.0) as usize;
    let half = (lanes as f64 - 1.0) / 2.0;
    for w in 0..lanes {
        let wf = w as f64;
        let lane_off = (wf - half) * 0.075;
        let edge = (wf - half).abs() / half.max(1.0);
        for k in 0..o.segs as usize {
            let a = k as f64 / o.segs * TAU;
            let wob = (0.16 * (a * 3.0 - t * 1.7 + wf * 0.22).sin() + 0.07 * (a * 5.0 + t * 1.1).sin()) * o.wob_mul;
            let (radial, off) = if o.face_on { (1.0 + wob, lane_off) } else { (1.0, lane_off + wob) };
            let x = ux * a.cos() + vx * a.sin() + nx * off;
            let y = uy * a.cos() + vy * a.sin() + ny * off;
            let z = uz * a.cos() + vz * a.sin() + nz * off;
            let l = (x * x + y * y + z * z).sqrt();
            let rr = base_r * radial;
            let [px, py, zr] = pt(x / l * rr, y / l * rr, z / l * rr);
            let depth = (zr / big_r + 1.0) / 2.0;
            dots.push(Dot { x: px, y: py, z: zr, r: (o.r_base + o.r_depth * depth) * (1.0 - 0.25 * edge) * rs, white: 0.52 - 0.44 * depth + 0.18 * edge, a: 0.4 + 0.6 * depth });
        }
    }
    finalize(dots)
}

/// A lat/long lattice of `rings` rings: each ring's dots around it, `density` at the equator.
fn lattice(rings: usize, density: f64) -> impl Iterator<Item = (usize, f64, f64, f64)> {
    (0..=rings).flat_map(move |ri| {
        let lat = -FRAC_PI_2 + ri as f64 / rings as f64 * PI;
        let (cos_lat, sin_lat) = (lat.cos(), lat.sin());
        let count = ((cos_lat.abs() * density).round() as usize).max(1);
        (0..count).map(move |lj| (ri, lj as f64 / count as f64 * TAU, cos_lat, sin_lat))
    })
}

/// A spinning globe with a meridian sweeping it, read as a ripple in the dots' size.
fn globe(size: f64, t: f64) -> Vec<Dot> {
    // the 64-px preset: 11 rings, 29 dots at the equator, radii ×1.15, a fast scan, dim elsewhere
    let (r_base, r_depth, scan_mul, dim_base) = (0.69, 1.955, 4.08, 0.45);
    let spin = 0.5;
    let radius = size / 2.0 * 0.82;
    let pt = projector(t * spin, 0.4 + 0.06 * (t * 0.35).sin(), size / 2.0, size / 2.0, radius);
    let scan = t * (spin + (1.7 - spin) * scan_mul);
    let rs = radius_scale(size);
    let dots = lattice(11, 29.0)
        .map(|(_, lon, cos_lat, sin_lat)| {
            let [x, y, z] = pt(cos_lat * lon.cos(), sin_lat, cos_lat * lon.sin());
            let depth = (z + 1.0) / 2.0;
            let d = angle_delta(lon + t * spin, scan);
            let boost = (-(d * d) / 0.18).exp() * z.max(0.0);
            Dot { x, y, z, r: (r_base + r_depth * depth + boost) * rs, white: 0.62 - 0.54 * depth, a: dim_base + (1.0 - dim_base) * boost.min(1.0) }
        })
        .collect();
    finalize(dots)
}

/// A sphere of rings swelling in two waves at different tempi.
fn wave(size: f64, t: f64) -> Vec<Dot> {
    let big_r = size / 2.0 * 0.874;
    let pt = projector(t * 0.18, 0.38, size / 2.0, size / 2.0, 1.0);
    let rs = radius_scale(size);
    let dots = lattice(9, 23.0)
        .map(|(ri, lon, cos_lat, sin_lat)| {
            let ri = ri as f64;
            let w = 0.62 * (t * 2.1 - ri * 0.52).sin() + 0.38 * (t * 1.27 + ri * 0.83).sin();
            let rr = big_r * (0.88 + 0.105 * w);
            let [x, y, z] = pt(cos_lat * lon.cos() * rr, sin_lat * rr, cos_lat * lon.sin() * rr);
            let depth = (z / big_r + 1.0) / 2.0;
            let crest = w.max(0.0);
            Dot { x, y, z, r: (0.6 + 1.7 * depth) * (1.0 + 0.4 * crest) * rs, white: 0.66 - 0.56 * depth - 0.1 * crest, a: 1.0 }
        })
        .collect();
    finalize(dots)
}

fn frame(state: State, size: f64, t: f64) -> Vec<Dot> {
    match state {
        State::Breathing => ribbon(size, t, &BREATHING),
        State::Working => orbits(size, t),
        State::Composing => ribbon(size, t, &COMPOSING),
        State::Searching => globe(size, t),
        State::Listening => wave(size, t),
    }
}

/// Paints the orb `size` points wide around `center`, `seconds` into its clock. `ink` is the
/// theme's text color: near dots reach it, far ones fade into the background.
pub fn paint(painter: &Painter, center: Pos2, size: f32, state: State, seconds: f64, ink: Color32) {
    let origin = center - eframe::egui::Vec2::splat(size / 2.0);
    for d in frame(state, size as f64, seconds * state.speed()) {
        // thinking-orbs mirrors ink on dark themes so near dots read bright; here the near
        // end is the theme's text color, whichever theme
        let strength = (1.0 - d.white.clamp(0.0, 1.0)) * d.a;
        // a square of the dot's width, not a disc: Ravune draws in pixels
        let at = pos2(origin.x + d.x as f32, origin.y + d.y as f32);
        painter.rect_filled(Rect::from_center_size(at, vec2(2.0, 2.0) * d.r as f32 * 0.9), 0, ink.gamma_multiply(strength as f32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Our frames against thinking-orbs' own (every 9th dot of its golden 64-px frames).
    #[test]
    fn matches_the_reference_frames() {
        let golden: serde_json::Value = serde_json::from_str(include_str!("orb/golden.json")).unwrap();
        let tolerance = golden["tolerance"].as_f64().unwrap() * 10.0;
        for case in golden["cases"].as_array().unwrap() {
            let state = match case["state"].as_str().unwrap() {
                "working" => State::Working,
                "composing" => State::Composing,
                "searching" => State::Searching,
                "listening" => State::Listening,
                _ => State::Breathing,
            };
            let t = case["t"].as_f64().unwrap();
            let dots = frame(state, 64.0, t);
            assert_eq!(dots.len() as u64, case["count"].as_u64().unwrap(), "{state:?} at {t}: dot count");
            let close = |d: &Dot, want: &[serde_json::Value]| [d.x, d.y, d.z, d.r, d.white, d.a].iter().zip(want).all(|(g, w)| (g - w.as_f64().unwrap()).abs() < tolerance);
            for (i, want) in case["dots"].as_object().unwrap() {
                let want = want.as_array().unwrap();
                // dots at the same depth (a face-on ring is all z = 0) sort by the last bit of
                // sin(), which libm and V8 round differently: any order of them draws the same
                let tied = want[2].as_f64().unwrap().abs() < 1e-6;
                let found = close(&dots[i.parse::<usize>().unwrap()], want) || tied && dots.iter().any(|d| close(d, want));
                assert!(found, "{state:?} at {t}, dot {i}: {:?} vs {want:?}", dots[i.parse::<usize>().unwrap()]);
            }
        }
    }
}
