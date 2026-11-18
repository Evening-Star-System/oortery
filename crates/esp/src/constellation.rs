//! The Overview hero: a living sky drawn with Cairo. A deep celestial gradient, a drifting
//! twinkling starfield, soft aurora, the warm afterglow of sunset at the horizon, and the
//! Evening Star (this machine) radiant at the center with your clouds orbiting it as a
//! living constellation. Static it reads as a map of your data; animated it breathes like a
//! real sky. This is where the "awe" budget lives.

use gtk::cairo;
use std::f64::consts::TAU;

/// Jewel tones for the orbiting clouds, tuned to sit in a night sky.
const PALETTE: &[(f64, f64, f64)] = &[
    (0.52, 0.60, 1.00), // periwinkle
    (0.18, 0.85, 0.92), // cyan
    (0.68, 0.46, 1.00), // violet
    (0.26, 0.86, 0.66), // aurora green
    (1.00, 0.74, 0.38), // amber
    (1.00, 0.52, 0.66), // rose
];

/// The Evening Star itself: warm white-gold, like Venus at dusk.
const STAR_CORE: (f64, f64, f64) = (1.00, 0.96, 0.86);
const STAR_GLOW: (f64, f64, f64) = (1.00, 0.84, 0.56);

pub fn draw(cr: &cairo::Context, w: f64, h: f64, phase: f64, nodes: usize) {
    // Clip everything to the rounded hero so the sky never spills past its frame.
    rounded_rect(cr, 0.0, 0.0, w, h, 18.0);
    let _ = cr.save();
    cr.clip();

    sky(cr, w, h);
    afterglow(cr, w, h);
    aurora(
        cr,
        w,
        h,
        phase,
        &Ribbon {
            idx: 0,
            color: (0.26, 0.86, 0.66),
            yfrac: 0.30,
            alpha: 0.09,
        },
    );
    aurora(
        cr,
        w,
        h,
        phase,
        &Ribbon {
            idx: 1,
            color: (0.55, 0.42, 1.00),
            yfrac: 0.44,
            alpha: 0.07,
        },
    );
    starfield(cr, w, h, phase);
    shooting_star(cr, w, h, phase);

    let cx = w * 0.5;
    let cy = h * 0.52;
    let ring = (w.min(h * 1.9) * 0.5 - 76.0).max(64.0);

    orbit_ring(cr, cx, cy, ring);
    links(cr, cx, cy, ring, phase, nodes);
    nodes_ring(cr, cx, cy, ring, phase, nodes);
    evening_star(cr, cx, cy, phase);

    let _ = cr.restore();
}

/// Deep celestial gradient: indigo at the top, violet through the middle, near-black below.
fn sky(cr: &cairo::Context, w: f64, h: f64) {
    let g = cairo::LinearGradient::new(0.0, 0.0, 0.0, h);
    g.add_color_stop_rgba(0.0, 0.043, 0.055, 0.145, 1.0);
    g.add_color_stop_rgba(0.55, 0.086, 0.070, 0.190, 1.0);
    g.add_color_stop_rgba(1.0, 0.016, 0.020, 0.055, 1.0);
    let _ = cr.set_source(&g);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();
}

/// The warm afterglow of a just-set sun, low and centered, where the evening star hangs.
fn afterglow(cr: &cairo::Context, w: f64, h: f64) {
    let gx = w * 0.5;
    let gy = h * 1.06;
    let r = w.max(h) * 0.7;
    let g = cairo::RadialGradient::new(gx, gy, 0.0, gx, gy, r);
    g.add_color_stop_rgba(0.0, 1.00, 0.52, 0.30, 0.16);
    g.add_color_stop_rgba(0.4, 0.86, 0.36, 0.44, 0.07);
    g.add_color_stop_rgba(1.0, 0.40, 0.20, 0.55, 0.0);
    let _ = cr.set_source(&g);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();
}

/// One drifting aurora ribbon.
struct Ribbon {
    idx: u32,
    color: (f64, f64, f64),
    yfrac: f64,
    alpha: f64,
}

/// A soft, slow aurora ribbon drifting across the upper sky.
fn aurora(cr: &cairo::Context, w: f64, h: f64, phase: f64, r: &Ribbon) {
    let (c, alpha) = (r.color, r.alpha);
    let base = h * r.yfrac;
    let amp = h * 0.05;
    let thick = h * 0.16;
    let speed = 0.06 + r.idx as f64 * 0.03;
    let freq = TAU / w * (1.3 + r.idx as f64 * 0.5);

    let edge = |x: f64, off: f64| base + (x * freq + phase * speed + off).sin() * amp;

    cr.move_to(0.0, edge(0.0, 0.0));
    let mut x = 0.0;
    while x <= w {
        cr.line_to(x, edge(x, 0.0));
        x += 6.0;
    }
    let mut x = w;
    while x >= 0.0 {
        cr.line_to(x, edge(x, 1.7) + thick);
        x -= 6.0;
    }
    cr.close_path();

    let g = cairo::LinearGradient::new(0.0, base - amp, 0.0, base + thick + amp);
    g.add_color_stop_rgba(0.0, c.0, c.1, c.2, 0.0);
    g.add_color_stop_rgba(0.5, c.0, c.1, c.2, alpha);
    g.add_color_stop_rgba(1.0, c.0, c.1, c.2, 0.0);
    let _ = cr.set_source(&g);
    let _ = cr.fill();
}

/// Two parallax layers of stars, each twinkling on its own phase and drifting slowly.
fn starfield(cr: &cairo::Context, w: f64, h: f64, phase: f64) {
    for layer in 0..2u32 {
        let count = if layer == 0 { 70 } else { 44 };
        let drift = if layer == 0 { 2.0 } else { 5.0 };
        let base_r = if layer == 0 { 0.7 } else { 1.2 };
        for i in 0..count {
            let seed = i + layer * 1000;
            let bx = rnd(seed, 1);
            let by = rnd(seed, 2);
            let x = ((bx + phase * drift / w) % 1.0) * w;
            let y = by * h;
            let tw = 0.45
                + 0.55
                    * ((phase * (0.8 + rnd(seed, 3) * 1.6) + rnd(seed, 4) * TAU).sin() * 0.5 + 0.5);
            let r = base_r + rnd(seed, 5) * 0.9;
            // Slight warm/cool variation so the field is not flat white.
            let warm = rnd(seed, 6);
            let col = (1.0, 0.96 + warm * 0.04, 0.88 + warm * 0.12);
            cr.set_source_rgba(col.0, col.1, col.2, 0.85 * tw);
            cr.arc(x, y, r, 0.0, TAU);
            let _ = cr.fill();
        }
    }
}

/// A rare shooting star: a bright head with a fading tail, crossing every several seconds.
fn shooting_star(cr: &cairo::Context, w: f64, h: f64, phase: f64) {
    let period = 13.0;
    let t = (phase % period) / 1.1; // active for ~1.1s of each period
    if t > 1.0 {
        return;
    }
    let start_x = w * 0.12;
    let start_y = h * 0.18;
    let dx = w * 0.5;
    let dy = h * 0.22;
    let hx = start_x + dx * t;
    let hy = start_y + dy * t;
    let tail = 90.0;
    let tx = hx - dx / (dx * dx + dy * dy).sqrt() * tail;
    let ty = hy - dy / (dx * dx + dy * dy).sqrt() * tail;
    let fade = (1.0 - t).min(t * 4.0); // ease in and out

    let g = cairo::LinearGradient::new(tx, ty, hx, hy);
    g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.0);
    g.add_color_stop_rgba(1.0, 1.0, 0.98, 0.9, 0.9 * fade);
    let _ = cr.set_source(&g);
    cr.set_line_width(1.6);
    cr.move_to(tx, ty);
    cr.line_to(hx, hy);
    let _ = cr.stroke();
    cr.set_source_rgba(1.0, 1.0, 0.95, 0.9 * fade);
    cr.arc(hx, hy, 1.8, 0.0, TAU);
    let _ = cr.fill();
}

fn orbit_ring(cr: &cairo::Context, cx: f64, cy: f64, ring: f64) {
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.045);
    cr.set_line_width(1.0);
    ellipse(cr, cx, cy, ring, ring * 0.62);
    let _ = cr.stroke();
}

/// Links from the hub to each orbiting cloud, each pulsing on its own phase.
fn links(cr: &cairo::Context, cx: f64, cy: f64, ring: f64, phase: f64, nodes: usize) {
    for i in 0..nodes {
        let a = i as f64 / nodes as f64 * TAU + phase * 0.10;
        let nx = cx + a.cos() * ring;
        let ny = cy + a.sin() * ring * 0.62;
        let color = PALETTE[i % PALETTE.len()];
        let pulse = 0.28 + 0.34 * (phase * 1.3 + i as f64 * 0.9).sin().abs();

        let grad = cairo::LinearGradient::new(cx, cy, nx, ny);
        grad.add_color_stop_rgba(0.0, STAR_GLOW.0, STAR_GLOW.1, STAR_GLOW.2, 0.0);
        grad.add_color_stop_rgba(1.0, color.0, color.1, color.2, pulse);
        let _ = cr.set_source(&grad);
        cr.set_line_width(1.5);
        cr.move_to(cx, cy);
        let mx = (cx + nx) / 2.0 + (ny - cy) * 0.12;
        let my = (cy + ny) / 2.0 - (nx - cx) * 0.12;
        cr.curve_to(mx, my, mx, my, nx, ny);
        let _ = cr.stroke();
    }
}

fn nodes_ring(cr: &cairo::Context, cx: f64, cy: f64, ring: f64, phase: f64, nodes: usize) {
    for i in 0..nodes {
        let a = i as f64 / nodes as f64 * TAU + phase * 0.10;
        let nx = cx + a.cos() * ring;
        let ny = cy + a.sin() * ring * 0.62;
        let color = PALETTE[i % PALETTE.len()];
        let breath = 1.0 + 0.14 * (phase * 1.5 + i as f64).sin();
        node(cr, nx, ny, 6.5 * breath, color);
    }
}

/// The Evening Star: a radiant warm core with a soft halo and a gentle four-point glint.
fn evening_star(cr: &cairo::Context, cx: f64, cy: f64, phase: f64) {
    let pulse = 1.0 + 0.06 * (phase * 1.1).sin();
    // Broad outer glow.
    let glow = cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, 46.0 * pulse);
    glow.add_color_stop_rgba(0.0, STAR_GLOW.0, STAR_GLOW.1, STAR_GLOW.2, 0.55);
    glow.add_color_stop_rgba(1.0, STAR_GLOW.0, STAR_GLOW.1, STAR_GLOW.2, 0.0);
    let _ = cr.set_source(&glow);
    cr.arc(cx, cy, 46.0 * pulse, 0.0, TAU);
    let _ = cr.fill();

    glint(cr, cx, cy, 40.0 * pulse, 0.0, 0.5);
    glint(cr, cx, cy, 22.0 * pulse, TAU / 8.0, 0.22);

    // Core.
    node(cr, cx, cy, 13.0, STAR_GLOW);
    cr.set_source_rgba(STAR_CORE.0, STAR_CORE.1, STAR_CORE.2, 1.0);
    cr.arc(cx, cy, 5.0, 0.0, TAU);
    let _ = cr.fill();
}

/// A four-spike diffraction glint, rotated by `rot`.
fn glint(cr: &cairo::Context, x: f64, y: f64, len: f64, rot: f64, alpha: f64) {
    for k in 0..4 {
        let ang = rot + k as f64 * TAU / 4.0;
        let ex = x + ang.cos() * len;
        let ey = y + ang.sin() * len;
        let g = cairo::LinearGradient::new(x, y, ex, ey);
        g.add_color_stop_rgba(0.0, STAR_CORE.0, STAR_CORE.1, STAR_CORE.2, alpha);
        g.add_color_stop_rgba(1.0, STAR_CORE.0, STAR_CORE.1, STAR_CORE.2, 0.0);
        let _ = cr.set_source(&g);
        cr.set_line_width(1.4);
        cr.move_to(x, y);
        cr.line_to(ex, ey);
        let _ = cr.stroke();
    }
}

fn node(cr: &cairo::Context, x: f64, y: f64, r: f64, c: (f64, f64, f64)) {
    let halo = cairo::RadialGradient::new(x, y, 0.0, x, y, r * 2.8);
    halo.add_color_stop_rgba(0.0, c.0, c.1, c.2, 0.55);
    halo.add_color_stop_rgba(1.0, c.0, c.1, c.2, 0.0);
    let _ = cr.set_source(&halo);
    cr.arc(x, y, r * 2.8, 0.0, TAU);
    let _ = cr.fill();
    cr.set_source_rgba(c.0, c.1, c.2, 1.0);
    cr.arc(x, y, r, 0.0, TAU);
    let _ = cr.fill();
    // A tiny highlight so each node reads as a little world, not a flat dot.
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.5);
    cr.arc(x - r * 0.3, y - r * 0.3, r * 0.3, 0.0, TAU);
    let _ = cr.fill();
}

fn ellipse(cr: &cairo::Context, cx: f64, cy: f64, rx: f64, ry: f64) {
    cr.new_sub_path();
    let steps = 64;
    for i in 0..=steps {
        let a = i as f64 / steps as f64 * TAU;
        let x = cx + a.cos() * rx;
        let y = cy + a.sin() * ry;
        if i == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -TAU / 4.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, TAU / 4.0);
    cr.arc(x + r, y + h - r, r, TAU / 4.0, TAU / 2.0);
    cr.arc(x + r, y + r, r, TAU / 2.0, 3.0 * TAU / 4.0);
    cr.close_path();
}

/// A tiny deterministic hash to [0,1); keeps the starfield stable frame to frame without
/// any RNG. `salt` varies the stream per attribute (x, y, twinkle, size, ...).
fn rnd(i: u32, salt: u32) -> f64 {
    let mut h = i
        .wrapping_mul(747_796_405)
        .wrapping_add(2_891_336_453)
        .wrapping_add(salt.wrapping_mul(2_246_822_519));
    h ^= h >> 15;
    h = h.wrapping_mul(2_246_822_519);
    h ^= h >> 13;
    (h & 0x00ff_ffff) as f64 / 0x0100_0000 as f64
}
