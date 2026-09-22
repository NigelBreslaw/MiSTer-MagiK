// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Preparation-only curved cylinder projection. Runtime uses coordinate maps.
use std::f32::consts::TAU;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Sample {
    pub u: u16,
    pub v: u16,
    pub shade: u8,
}
#[derive(Clone, Copy, Default)]
struct V(f32, f32, f32);
impl V {
    fn add(self, b: Self) -> Self {
        Self(self.0 + b.0, self.1 + b.1, self.2 + b.2)
    }
    fn sub(self, b: Self) -> Self {
        Self(self.0 - b.0, self.1 - b.1, self.2 - b.2)
    }
    fn scale(self, t: f32) -> Self {
        Self(self.0 * t, self.1 * t, self.2 * t)
    }
    fn dot(self, b: Self) -> f32 {
        self.0 * b.0 + self.1 * b.1 + self.2 * b.2
    }
    fn cross(self, b: Self) -> Self {
        Self(
            self.1 * b.2 - self.2 * b.1,
            self.2 * b.0 - self.0 * b.2,
            self.0 * b.1 - self.1 * b.0,
        )
    }
    fn unit(self) -> Self {
        self.scale(1.0 / self.dot(self).sqrt())
    }
}
fn centre(t: f32) -> V {
    V(4.0 * t.sin(), 2.3 * (t * 2.0).sin(), 7.5 * t)
}
fn axes(t: f32) -> (V, V, V) {
    let forward = V(4.0 * t.cos(), 4.6 * (t * 2.0).cos(), 7.5).unit();
    let right = V(0.0, 1.0, 0.0).cross(forward).unit();
    (right, forward.cross(right), forward)
}
#[derive(Clone, Copy, Default)]
struct Vertex {
    p: V,
    u: f32,
    v: f32,
}
impl Vertex {
    fn lerp(self, b: Self, t: f32) -> Self {
        Self {
            p: self.p.add(b.p.sub(self.p).scale(t)),
            u: self.u + (b.u - self.u) * t,
            v: self.v + (b.v - self.v) * t,
        }
    }
}
pub(super) fn prepare(w: usize, h: usize, phases: usize) -> Vec<Vec<Sample>> {
    let mut views = Vec::with_capacity(phases);
    for phase in 0..phases {
        let time = phase as f32 * TAU / phases as f32;
        let camera = centre(time);
        let (cx, cy, cz) = axes(time);
        let roll = 0.30 * time.sin();
        let rx = cx.scale(roll.cos()).add(cy.scale(roll.sin()));
        let ry = cy.scale(roll.cos()).sub(cx.scale(roll.sin()));
        const SIDES: usize = 32;
        const RINGS: usize = 52;
        let mut vertices = Vec::with_capacity((SIDES + 1) * RINGS);
        for ring in 0..RINGS {
            let at = time - 0.14 + ring as f32 * 0.06;
            let c = centre(at);
            let (x, y, _) = axes(at);
            for side in 0..=SIDES {
                let angle = side as f32 * TAU / SIDES as f32;
                let world = c
                    .add(x.scale(angle.cos() * 1.8))
                    .add(y.scale(angle.sin() * 1.8));
                let relative = world.sub(camera);
                vertices.push(Vertex {
                    p: V(relative.dot(rx), relative.dot(ry), relative.dot(cz)),
                    u: side as f32 * 256.0 / SIDES as f32,
                    v: at * 2304.0 / TAU,
                });
            }
        }
        let mut map = vec![Sample::default(); w * h];
        let mut depth = vec![f32::INFINITY; w * h];
        for ring in 0..RINGS - 1 {
            for side in 0..SIDES {
                let a = vertices[ring * (SIDES + 1) + side];
                let b = vertices[ring * (SIDES + 1) + side + 1];
                let c = vertices[(ring + 1) * (SIDES + 1) + side];
                let d = vertices[(ring + 1) * (SIDES + 1) + side + 1];
                clip([a, c, b], w, h, &mut map, &mut depth);
                clip([b, c, d], w, h, &mut map, &mut depth);
            }
        }
        views.push(map);
    }
    views
}
fn clip(triangle: [Vertex; 3], w: usize, h: usize, map: &mut [Sample], depth: &mut [f32]) {
    const NEAR: f32 = 0.12;
    let mut polygon = [Vertex::default(); 4];
    let mut count = 0;
    let mut previous = triangle[2];
    for current in triangle {
        if (previous.p.2 >= NEAR) != (current.p.2 >= NEAR) {
            polygon[count] = previous.lerp(
                current,
                (NEAR - previous.p.2) / (current.p.2 - previous.p.2),
            );
            count += 1;
        }
        if current.p.2 >= NEAR {
            polygon[count] = current;
            count += 1;
        }
        previous = current;
    }
    for i in 1..count.saturating_sub(1) {
        raster([polygon[0], polygon[i], polygon[i + 1]], w, h, map, depth);
    }
}
fn raster(triangle: [Vertex; 3], w: usize, h: usize, map: &mut [Sample], depth: &mut [f32]) {
    let focal = h as f32 * 0.92;
    let p = triangle.map(|v| {
        let z = 1.0 / v.p.2;
        [
            w as f32 * 0.5 + v.p.0 * focal * z,
            h as f32 * 0.5 - v.p.1 * focal * z,
            z,
            v.u * z,
            v.v * z,
        ]
    });
    let edge = |a: [f32; 5], b: [f32; 5], x: f32, y: f32| {
        (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
    };
    let area = edge(p[0], p[1], p[2][0], p[2][1]);
    if area.abs() < 0.0001 {
        return;
    }
    let x0 = p
        .iter()
        .map(|p| p[0])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let x1 = (p
        .iter()
        .map(|p| p[0])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .max(0.0) as usize)
        .min(w);
    let y0 = p
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let y1 = (p
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .max(0.0) as usize)
        .min(h);
    for y in y0..y1 {
        for x in x0..x1 {
            let a = edge(p[1], p[2], x as f32 + 0.5, y as f32 + 0.5) / area;
            let b = edge(p[2], p[0], x as f32 + 0.5, y as f32 + 0.5) / area;
            let c = 1.0 - a - b;
            if a < -0.00001 || b < -0.00001 || c < -0.00001 {
                continue;
            }
            let inverse = a * p[0][2] + b * p[1][2] + c * p[2][2];
            if inverse <= 0.0 {
                continue;
            }
            let z = 1.0 / inverse;
            let offset = y * w + x;
            if z >= depth[offset] {
                continue;
            }
            depth[offset] = z;
            let u = (a * p[0][3] + b * p[1][3] + c * p[2][3]) * z;
            let v = (a * p[0][4] + b * p[1][4] + c * p[2][4]) * z;
            let fog = (1.0 - z / 12.0).clamp(0.0, 1.0);
            map[offset] = Sample {
                u: (u * 256.0) as i32 as u16,
                v: (v * 256.0) as i32 as u16,
                shade: (fog * fog * 63.0) as u8,
            };
        }
    }
}
