//! A celestial loading indicator: the evening star with clouds orbiting it, drawn with Cairo.
//! Replaces the default GTK throbber so waiting still feels on-brand. It animates only while
//! mapped (GTK pauses tick callbacks for hidden widgets), so hiding it stops the work.

use gtk::cairo;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::f64::consts::TAU;
use std::rc::Rc;

/// Jewel tones for the orbiting clouds, matching the Overview constellation.
const PALETTE: &[(f64, f64, f64)] = &[
    (0.52, 0.60, 1.00), // periwinkle
    (0.68, 0.46, 1.00), // violet
    (0.26, 0.86, 0.66), // aurora green
];

/// A small animated celestial spinner widget, centered and ready to drop into a dialog.
pub fn celestial() -> gtk::Widget {
    let area = gtk::DrawingArea::new();
    area.set_content_width(48);
    area.set_content_height(48);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);

    let phase = Rc::new(Cell::new(0.0f64));
    {
        let phase = phase.clone();
        area.set_draw_func(move |_, cr, w, h| draw(cr, w as f64, h as f64, phase.get()));
    }
    {
        let phase = phase.clone();
        let start = Cell::new(None::<i64>);
        area.add_tick_callback(move |area, clock| {
            let now = clock.frame_time();
            let t0 = start.get().unwrap_or_else(|| {
                start.set(Some(now));
                now
            });
            phase.set((now - t0) as f64 / 1_000_000.0);
            area.queue_draw();
            glib::ControlFlow::Continue
        });
    }
    area.upcast()
}

fn draw(cr: &cairo::Context, w: f64, h: f64, phase: f64) {
    let (cx, cy) = (w / 2.0, h / 2.0);
    let rx = w * 0.40;
    let ry = h * 0.28;

    // Faint orbit path.
    cr.set_source_rgba(0.62, 0.64, 1.0, 0.14);
    cr.set_line_width(1.0);
    ellipse(cr, cx, cy, rx, ry);
    let _ = cr.stroke();

    // The evening star: warm core with a soft halo and a slow four-point glint.
    let glow = cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, 11.0);
    glow.add_color_stop_rgba(0.0, 1.0, 0.90, 0.70, 0.9);
    glow.add_color_stop_rgba(1.0, 1.0, 0.84, 0.56, 0.0);
    let _ = cr.set_source(&glow);
    cr.arc(cx, cy, 11.0, 0.0, TAU);
    let _ = cr.fill();
    glint(cr, cx, cy, 12.0, phase * 0.6, 0.35);
    cr.set_source_rgba(1.0, 0.97, 0.88, 1.0);
    cr.arc(cx, cy, 3.0, 0.0, TAU);
    let _ = cr.fill();

    // Clouds orbiting the star.
    let n = PALETTE.len();
    for (i, &color) in PALETTE.iter().enumerate() {
        let a = phase * 2.0 + i as f64 / n as f64 * TAU;
        let x = cx + a.cos() * rx;
        let y = cy + a.sin() * ry;
        node(cr, x, y, 3.2, color);
    }
}

fn node(cr: &cairo::Context, x: f64, y: f64, r: f64, c: (f64, f64, f64)) {
    let halo = cairo::RadialGradient::new(x, y, 0.0, x, y, r * 2.6);
    halo.add_color_stop_rgba(0.0, c.0, c.1, c.2, 0.5);
    halo.add_color_stop_rgba(1.0, c.0, c.1, c.2, 0.0);
    let _ = cr.set_source(&halo);
    cr.arc(x, y, r * 2.6, 0.0, TAU);
    let _ = cr.fill();
    cr.set_source_rgba(c.0, c.1, c.2, 1.0);
    cr.arc(x, y, r, 0.0, TAU);
    let _ = cr.fill();
}

fn glint(cr: &cairo::Context, x: f64, y: f64, len: f64, rot: f64, alpha: f64) {
    for k in 0..4 {
        let ang = rot + k as f64 * TAU / 4.0;
        let ex = x + ang.cos() * len;
        let ey = y + ang.sin() * len;
        let g = cairo::LinearGradient::new(x, y, ex, ey);
        g.add_color_stop_rgba(0.0, 1.0, 0.97, 0.88, alpha);
        g.add_color_stop_rgba(1.0, 1.0, 0.97, 0.88, 0.0);
        let _ = cr.set_source(&g);
        cr.set_line_width(1.3);
        cr.move_to(x, y);
        cr.line_to(ex, ey);
        let _ = cr.stroke();
    }
}

fn ellipse(cr: &cairo::Context, cx: f64, cy: f64, rx: f64, ry: f64) {
    cr.new_sub_path();
    let steps = 48;
    for i in 0..=steps {
        let a = i as f64 / steps as f64 * TAU;
        let (x, y) = (cx + a.cos() * rx, cy + a.sin() * ry);
        if i == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
}
