//! Pure geometry for the mask transform: where the mask lands over a widget,
//! which on-screen handle is under the pointer, and what dragging it changes.
//! Local mask space is the widget rectangle `0..w × 0..h`; the transform is
//! `p' = centre + offset + R·K·S·(p − centre)` with S = scale, K = skew, R = rotation.

use crate::model::MaskXf;

pub type P = (f64, f64);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Handle {
    Body,
    /// 0 = top-left, then clockwise. Changes the size.
    Corner(usize),
    /// 0 = top, then clockwise. Stretches along one axis.
    Edge(usize),
    /// Same numbering as `Edge`. Skews along the edge.
    Skew(usize),
    /// Circled handle outside the box: spins the mask around its centre.
    Rotate,
}

/// `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy, Debug)]
pub struct Affine(pub f64, pub f64, pub f64, pub f64, pub f64, pub f64);

impl Affine {
    pub fn of(xf: &MaskXf, w: f64, h: f64) -> Affine {
        let (sx, sy) = (xf.size * xf.sx, xf.size * xf.sy);
        let (kx, ky) = (xf.skew_x.to_radians().tan(), xf.skew_y.to_radians().tan());
        let (a, b, c, d) = self::rotate(xf.rot, sx, ky * sx, kx * sy, sy);
        let (cx, cy) = (w / 2.0, h / 2.0);
        Affine(a, b, c, d, cx + xf.dx * w - (a * cx + c * cy), cy + xf.dy * h - (b * cx + d * cy))
    }

    pub fn apply(&self, p: P) -> P {
        (self.0 * p.0 + self.2 * p.1 + self.4, self.1 * p.0 + self.3 * p.1 + self.5)
    }

    fn lin(&self, p: P) -> P {
        (self.0 * p.0 + self.2 * p.1, self.1 * p.0 + self.3 * p.1)
    }

    /// The linear part only, inverted: turns a screen delta into a local one.
    pub fn unrotate(&self, d: P) -> P {
        let det = self.0 * self.3 - self.1 * self.2;
        if det.abs() < 1e-12 {
            return d;
        }
        ((self.3 * d.0 - self.2 * d.1) / det, (self.0 * d.1 - self.1 * d.0) / det)
    }

    pub fn invert(&self, p: P) -> Option<P> {
        let det = self.0 * self.3 - self.1 * self.2;
        if det.abs() < 1e-9 {
            return None;
        }
        let (x, y) = (p.0 - self.4, p.1 - self.5);
        Some(((self.3 * x - self.2 * y) / det, (self.0 * y - self.1 * x) / det))
    }
}

/// Rotation of the whole picture, about the centre of a `w × h` box.
pub fn spin(deg: f64, w: f64, h: f64) -> Affine {
    let (a, b, c, d) = rotate(deg, 1.0, 0.0, 0.0, 1.0);
    Affine(a, b, c, d, w / 2.0 - (a * w / 2.0 + c * h / 2.0), h / 2.0 - (b * w / 2.0 + d * h / 2.0))
}

/// `R(deg) · [[a, c], [b, d]]`, staying in the (a, b, c, d) order.
fn rotate(deg: f64, a: f64, b: f64, c: f64, d: f64) -> (f64, f64, f64, f64) {
    let (cos, sin) = (deg.to_radians().cos(), deg.to_radians().sin());
    (cos * a - sin * b, sin * a + cos * b, cos * c - sin * d, sin * c + cos * d)
}

/// Size of the box a rotated `w × h` picture needs.
pub fn bbox(deg: f64, w: f64, h: f64) -> (f64, f64) {
    let (cos, sin) = (deg.to_radians().cos().abs(), deg.to_radians().sin().abs());
    ((w * cos + h * sin).round(), (w * sin + h * cos).round())
}

fn corner(i: usize, w: f64, h: f64) -> P {
    [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)][i % 4]
}

fn edge_mid(i: usize, w: f64, h: f64) -> P {
    [(w / 2.0, 0.0), (w, h / 2.0), (w / 2.0, h), (0.0, h / 2.0)][i % 4]
}

const OUT: [P; 4] = [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)];
const SKEW_GAP: f64 = 16.0;
const ROTATE_GAP: f64 = 30.0;
const PAD: f64 = 9.0;
const HIT: f64 = 10.0;

/// Handle positions in widget space, pulled inside the widget so they stay reachable.
pub fn handles(xf: &MaskXf, w: f64, h: f64) -> Vec<(Handle, P)> {
    let t = Affine::of(xf, w, h);
    let keep = |p: P| (p.0.clamp(PAD, (w - PAD).max(PAD)), p.1.clamp(PAD, (h - PAD).max(PAD)));
    let mut out = Vec::with_capacity(12);
    for i in 0..4 {
        out.push((Handle::Corner(i), keep(t.apply(corner(i, w, h)))));
    }
    for i in 0..4 {
        out.push((Handle::Edge(i), keep(t.apply(edge_mid(i, w, h)))));
    }
    for i in 0..4 {
        let m = t.apply(edge_mid(i, w, h));
        let n = t.lin(OUT[i]);
        let len = n.0.hypot(n.1).max(1e-9);
        out.push((Handle::Skew(i), keep((m.0 + n.0 / len * SKEW_GAP, m.1 + n.1 / len * SKEW_GAP))));
    }
    // Rotate handle: past the top-left corner along the box diagonal.
    let (a, b) = (t.apply(corner(0, w, h)), t.apply(corner(2, w, h)));
    let (mut dx, mut dy) = (a.0 - b.0, a.1 - b.1);
    let len = dx.hypot(dy).max(1e-9);
    dx /= len;
    dy /= len;
    out.push((Handle::Rotate, keep((a.0 + dx * ROTATE_GAP, a.1 + dy * ROTATE_GAP))));
    out
}

pub fn hit(xf: &MaskXf, w: f64, h: f64, p: P) -> Option<Handle> {
    if let Some((hd, _)) = handles(xf, w, h).into_iter().find(|(_, q)| (q.0 - p.0).hypot(q.1 - p.1) <= HIT) {
        return Some(hd);
    }
    let (x, y) = Affine::of(xf, w, h).invert(p)?;
    (0.0..=w).contains(&x).then_some(()).filter(|_| (0.0..=h).contains(&y)).map(|_| Handle::Body)
}

/// Re-scale `x0` and shift it so the local point `anchor` stays where it was on screen.
fn rescale(x0: &MaskXf, size: f64, sx: f64, sy: f64, anchor: P, w: f64, h: f64) -> MaskXf {
    let mut n = MaskXf { size: size.clamp(0.02, 20.0), sx: sx.clamp(0.02, 20.0), sy: sy.clamp(0.02, 20.0), ..*x0 };
    let (before, after) = (Affine::of(x0, w, h).apply(anchor), Affine::of(&n, w, h).apply(anchor));
    n.dx += (before.0 - after.0) / w;
    n.dy += (before.1 - after.1) / h;
    n
}

/// The transform after dragging `handle` by `d` pixels, given it started at `x0` with the pointer at `p0`.
pub fn drag(handle: Handle, x0: &MaskXf, w: f64, h: f64, p0: P, d: P) -> MaskXf {
    let t = Affine::of(x0, w, h);
    match handle {
        Handle::Body => MaskXf { dx: x0.dx + d.0 / w, dy: x0.dy + d.1 / h, ..*x0 },
        Handle::Corner(i) => {
            let anchor = corner(i + 2, w, h);
            let a = t.apply(anchor);
            let (start, now) = ((p0.0 - a.0).hypot(p0.1 - a.1), (p0.0 + d.0 - a.0).hypot(p0.1 + d.1 - a.1));
            if start < 1.0 {
                return *x0;
            }
            rescale(x0, x0.size * now / start, x0.sx, x0.sy, anchor, w, h)
        }
        Handle::Edge(i) => {
            let horizontal = i % 2 == 1;
            let axis = t.lin(if horizontal { (1.0, 0.0) } else { (0.0, 1.0) });
            let unit = axis.0.hypot(axis.1).max(1e-9);
            let len = unit * if horizontal { w } else { h };
            let sign = if i == 1 || i == 2 { 1.0 } else { -1.0 };
            let r = (1.0 + sign * (d.0 * axis.0 + d.1 * axis.1) / unit / len).max(0.02);
            let anchor = edge_mid(i + 2, w, h);
            if horizontal { rescale(x0, x0.size, x0.sx * r, x0.sy, anchor, w, h) } else { rescale(x0, x0.size, x0.sx, x0.sy * r, anchor, w, h) }
        }
        Handle::Rotate => {
            let (cx, cy) = (w / 2.0 + x0.dx * w, h / 2.0 + x0.dy * h);
            let angle = |p: P| (p.1 - cy).atan2(p.0 - cx).to_degrees();
            MaskXf { rot: (x0.rot + angle((p0.0 + d.0, p0.1 + d.1)) - angle(p0)).rem_euclid(360.0), ..*x0 }
        }
        Handle::Skew(i) => {
            let (sx, sy) = (x0.size * x0.sx, x0.size * x0.sy);
            let mut n = *x0;
            let bend = |deg: f64, delta: f64| (deg.to_radians().tan() + delta).atan().to_degrees().clamp(-75.0, 75.0);
            match i {
                0 => n.skew_x = bend(x0.skew_x, -d.0 / (h * sy / 2.0)),
                2 => n.skew_x = bend(x0.skew_x, d.0 / (h * sy / 2.0)),
                1 => n.skew_y = bend(x0.skew_y, d.1 / (w * sx / 2.0)),
                _ => n.skew_y = bend(x0.skew_y, -d.1 / (w * sx / 2.0)),
            }
            n
        }
    }
}

/// Row-major 4×4 colour matrix and offset for hue / contrast / brightness; `None` when neutral.
pub fn color(xf: &MaskXf) -> Option<([[f32; 4]; 4], [f32; 4])> {
    if xf.hue == 0.0 && xf.contrast == 1.0 && xf.brightness == 0.0 {
        return None;
    }
    let (c, s) = (xf.hue.to_radians().cos() as f32, xf.hue.to_radians().sin() as f32);
    let hue = [
        [0.213 + c * 0.787 - s * 0.213, 0.715 - c * 0.715 - s * 0.715, 0.072 - c * 0.072 + s * 0.928],
        [0.213 - c * 0.213 + s * 0.143, 0.715 + c * 0.285 + s * 0.140, 0.072 - c * 0.072 - s * 0.283],
        [0.213 - c * 0.213 - s * 0.787, 0.715 - c * 0.715 + s * 0.715, 0.072 + c * 0.928 + s * 0.072],
    ];
    let k = xf.contrast as f32;
    let mut m = [[0.0; 4]; 4];
    for r in 0..3 {
        for q in 0..3 {
            m[r][q] = hue[r][q] * k;
        }
    }
    m[3][3] = 1.0;
    let o = 0.5 * (1.0 - k) + xf.brightness as f32;
    Some((m, [o, o, o, 0.0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: P, b: P) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn identity_and_inverse() {
        let xf = MaskXf::default();
        assert!(close(Affine::of(&xf, 200.0, 100.0).apply((30.0, 40.0)), (30.0, 40.0)));
        let xf = MaskXf { dx: 0.1, size: 1.5, sx: 0.8, skew_x: 20.0, ..xf };
        let t = Affine::of(&xf, 200.0, 100.0);
        assert!(close(t.invert(t.apply((30.0, 40.0))).unwrap(), (30.0, 40.0)));
    }

    #[test]
    fn corner_drag_keeps_opposite_corner_fixed() {
        let (w, h) = (200.0, 100.0);
        let xf = MaskXf::default();
        let n = drag(Handle::Corner(2), &xf, w, h, (w, h), (100.0, 50.0));
        assert!((n.size - 1.5).abs() < 1e-9);
        assert!(close(Affine::of(&n, w, h).apply((0.0, 0.0)), (0.0, 0.0)));
    }

    #[test]
    fn edge_drag_stretches_one_axis_only() {
        let (w, h) = (200.0, 100.0);
        let n = drag(Handle::Edge(1), &MaskXf::default(), w, h, (w, 50.0), (100.0, 30.0));
        assert!((n.sx - 1.5).abs() < 1e-9 && n.sy == 1.0 && n.size == 1.0);
        assert!(close(Affine::of(&n, w, h).apply((0.0, 40.0)), (0.0, 40.0)));
    }

    #[test]
    fn mask_rotation_spins_about_the_centre() {
        let (w, h) = (200.0, 100.0);
        let xf = MaskXf { rot: 90.0, ..MaskXf::default() };
        let t = Affine::of(&xf, w, h);
        assert!(close(t.apply((w / 2.0, h / 2.0)), (w / 2.0, h / 2.0)));
        // The top edge mid point ends up right of centre after a quarter turn.
        assert!(close(t.apply((w / 2.0, 0.0)), (w / 2.0 + h / 2.0, h / 2.0)));
        // Start straight above the centre and drag a quarter turn to the right.
        let n = drag(Handle::Rotate, &MaskXf::default(), w, h, (w / 2.0, -30.0), (80.0, 80.0));
        assert!((n.rot - 90.0).abs() < 1e-6, "rot was {}", n.rot);
    }

    #[test]
    fn picture_rotation_grows_the_box() {
        assert_eq!(bbox(0.0, 100.0, 50.0), (100.0, 50.0));
        assert_eq!(bbox(90.0, 100.0, 50.0), (50.0, 100.0));
        // A point right of the centre ends up below it after a quarter turn.
        assert!(close(spin(90.0, 50.0, 100.0).apply((50.0, 50.0)), (25.0, 75.0)));
        assert!(close(spin(180.0, 50.0, 100.0).apply((50.0, 50.0)), (0.0, 50.0)));
    }

    #[test]
    fn hit_prefers_handles_then_body() {
        let xf = MaskXf::default();
        assert_eq!(hit(&xf, 200.0, 100.0, (192.0, 92.0)), Some(Handle::Corner(2)));
        assert_eq!(hit(&xf, 200.0, 100.0, (100.0, 50.0)), Some(Handle::Body));
        assert_eq!(hit(&MaskXf { size: 0.3, ..xf }, 200.0, 100.0, (5.0, 5.0)), None);
    }
}
