//! Just enough 3D math: vectors and rigid 3×4 transforms (OpenVR layout).

use crate::vr::Mat34;
use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V3(pub f32, pub f32, pub f32);

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3(self.0 + o.0, self.1 + o.1, self.2 + o.2)
    }
}
impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3(self.0 - o.0, self.1 - o.1, self.2 - o.2)
    }
}
impl Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, s: f32) -> V3 {
        V3(self.0 * s, self.1 * s, self.2 * s)
    }
}

impl V3 {
    pub const UP: V3 = V3(0., 1., 0.);
    pub fn dot(self, o: V3) -> f32 {
        self.0 * o.0 + self.1 * o.1 + self.2 * o.2
    }
    pub fn cross(self, o: V3) -> V3 {
        V3(self.1 * o.2 - self.2 * o.1, self.2 * o.0 - self.0 * o.2, self.0 * o.1 - self.1 * o.0)
    }
    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn norm(self) -> V3 {
        let l = self.len();
        if l > 1e-6 {
            self * (1.0 / l)
        } else {
            self
        }
    }
}

impl Mat34 {
    pub fn col(&self, c: usize) -> V3 {
        V3(self.m[0][c], self.m[1][c], self.m[2][c])
    }
    pub fn pos(&self) -> V3 {
        self.col(3)
    }
    pub fn set_pos(&mut self, p: V3) {
        self.m[0][3] = p.0;
        self.m[1][3] = p.1;
        self.m[2][3] = p.2;
    }
    /// -Z: where a controller / head points.
    pub fn forward(&self) -> V3 {
        self.col(2) * -1.0
    }
    pub fn from_axes(x: V3, y: V3, z: V3, p: V3) -> Mat34 {
        Mat34 { m: [[x.0, y.0, z.0, p.0], [x.1, y.1, z.1, p.1], [x.2, y.2, z.2, p.2]] }
    }
    pub fn dir(&self, v: V3) -> V3 {
        V3(
            self.m[0][0] * v.0 + self.m[0][1] * v.1 + self.m[0][2] * v.2,
            self.m[1][0] * v.0 + self.m[1][1] * v.1 + self.m[1][2] * v.2,
            self.m[2][0] * v.0 + self.m[2][1] * v.1 + self.m[2][2] * v.2,
        )
    }
    pub fn point(&self, v: V3) -> V3 {
        self.dir(v) + self.pos()
    }
    /// self ∘ o  (apply `o` first, then `self`).
    pub fn mul(&self, o: &Mat34) -> Mat34 {
        let x = self.dir(o.col(0));
        let y = self.dir(o.col(1));
        let z = self.dir(o.col(2));
        Mat34::from_axes(x, y, z, self.point(o.pos()))
    }
    /// Inverse of a rotation+translation.
    pub fn inv_rigid(&self) -> Mat34 {
        let (x, y, z, p) = (self.col(0), self.col(1), self.col(2), self.pos());
        // Rows of R become columns of R^T.
        let rt = Mat34::from_axes(V3(x.0, y.0, z.0), V3(x.1, y.1, z.1), V3(x.2, y.2, z.2), V3::default());
        let t = rt.dir(p) * -1.0;
        let mut r = rt;
        r.set_pos(t);
        r
    }
    /// Panel at `pos` whose front (+Z) faces `eye`, rolled so its top points
    /// along `up` as closely as possible (tilts freely, unlike `facing`).
    pub fn facing_up(pos: V3, eye: V3, up: V3) -> Mat34 {
        let z = eye - pos;
        let z = if z.len() < 1e-4 { V3(0., 0., 1.) } else { z.norm() };
        let mut x = up.cross(z);
        if x.len() < 1e-4 {
            x = V3(1., 0., 0.);
        }
        let x = x.norm();
        let y = z.cross(x);
        Mat34::from_axes(x, y, z, pos)
    }

    /// Upright panel at `pos` whose front (+Z) faces `eye`.
    pub fn facing(pos: V3, eye: V3) -> Mat34 {
        let mut z = eye - pos;
        z.1 = 0.0; // keep panels upright
        let z = if z.len() < 1e-4 { V3(0., 0., 1.) } else { z.norm() };
        let x = V3::UP.cross(z).norm();
        let y = z.cross(x);
        Mat34::from_axes(x, y, z, pos)
    }
}

/// Ray vs. a flat overlay of `w`×`h` meters centred on `tf`.
/// Returns (distance, u, v) with u,v in 0..1, v = 0 at the top edge.
/// Only the front face counts.
pub fn ray_quad(o: V3, d: V3, tf: &Mat34, w: f32, h: f32) -> Option<(f32, f32, f32)> {
    let n = tf.col(2);
    let denom = d.dot(n);
    if denom > -1e-4 {
        return None;
    }
    let p = tf.pos();
    let t = (p - o).dot(n) / denom;
    if t <= 0.0 {
        return None;
    }
    let hit = o + d * t - p;
    let (x, y) = (hit.dot(tf.col(0)), hit.dot(tf.col(1)));
    if x.abs() > w / 2.0 || y.abs() > h / 2.0 {
        return None;
    }
    Some((t, x / w + 0.5, 0.5 - y / h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Mat34, b: &Mat34) -> bool {
        a.m.iter().flatten().zip(b.m.iter().flatten()).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn inverse_roundtrip() {
        let a = Mat34::pitch_translate(37.0, [0.3, -1.2, 2.0]);
        let b = Mat34::facing(V3(1., 1.5, -2.), V3(0., 1.6, 0.));
        let c = a.mul(&b);
        assert!(close(&c.inv_rigid().mul(&c), &Mat34::identity()));
        assert!(close(&a.inv_rigid().mul(&c), &b));
    }

    #[test]
    fn facing_up_points_at_eye_with_top_up() {
        // Lying down: the eye is below the panel (panel on the ceiling side),
        // head-up is horizontal (+Z here). The panel faces down at the eye.
        let tf = Mat34::facing_up(V3(0., 2.0, 0.), V3(0., 0.5, 0.), V3(0., 0., -1.));
        assert!((tf.col(2).1 + 1.0).abs() < 1e-4, "should face straight down");
        assert!(tf.col(1).dot(V3(0., 0., -1.)) > 0.99, "top follows head-up");
        let r = ray_quad(V3(0., 0.5, 0.), V3(0., 1., 0.), &tf, 1.0, 0.5);
        assert!(r.is_some(), "looking up hits it");
    }

    #[test]
    fn ray_hits_center_and_corner() {
        let tf = Mat34::facing(V3(0., 1.5, -2.), V3(0., 1.5, 0.));
        let (t, u, v) = ray_quad(V3(0., 1.5, 0.), V3(0., 0., -1.), &tf, 1.0, 0.5).unwrap();
        assert!((t - 2.0).abs() < 1e-4 && (u - 0.5).abs() < 1e-4 && (v - 0.5).abs() < 1e-4);
        // top-left corner area: left (-x) and up (+y)
        let (_, u, v) = ray_quad(V3(-0.4, 1.7, 0.), V3(0., 0., -1.), &tf, 1.0, 0.5).unwrap();
        assert!(u < 0.2 && v < 0.2, "u={u} v={v}");
        // from behind: no hit
        assert!(ray_quad(V3(0., 1.5, -4.), V3(0., 0., 1.), &tf, 1.0, 0.5).is_none());
    }
}
