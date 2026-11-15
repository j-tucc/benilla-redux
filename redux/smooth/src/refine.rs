//! Curved point-normal (PN) triangle refinement of a skinned mesh, on plain arrays. Ported from
//! Elderoth's `benilla-adapter/src/smooth/refine.rs`.
//!
//! Original vertices never move, so the silhouette, the face and the texture mapping stay the
//! model's. A split edge's new vertex sits on the cubic PN patch of its two endpoints and takes the
//! quadratic-interpolated normal. Vertices duplicated across UV seams are welded by position and
//! normal, so both sides of a seam compute the same point. An edge with one triangle (an open
//! geoset rim, a hard crease, a submesh border) takes the linear midpoint, the same point the
//! neighbouring mesh computes, so nothing cracks.
use std::collections::HashMap;

/// One skinned mesh's vertex streams and triangle list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkinMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Option<Vec<[f32; 2]>>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl SkinMesh {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// Every stream has one entry per vertex and every index is in range.
    pub fn valid(&self) -> bool {
        let n = self.positions.len();
        self.normals.len() == n
            && self.uv0.len() == n
            && self.uv1.as_ref().is_none_or(|u| u.len() == n)
            && self.colors.as_ref().is_none_or(|c| c.len() == n)
            && self.joints.len() == n
            && self.weights.len() == n
            && self.indices.len().is_multiple_of(3)
            && self.indices.iter().all(|&i| (i as usize) < n)
    }
}

/// The weld key: the position to 1e-4 yd and the normal to 1e-3.
fn weld_key(p: [f32; 3], n: [f32; 3]) -> [i32; 6] {
    let q = |v: f32, s: f32| (v * s).round() as i32;
    [
        q(p[0], 1e4),
        q(p[1], 1e4),
        q(p[2], 1e4),
        q(n[0], 1e3),
        q(n[1], 1e3),
        q(n[2], 1e3),
    ]
}

/// Each vertex's weld group: the first vertex with its key.
fn weld(m: &SkinMesh) -> Vec<u32> {
    let mut first: HashMap<[i32; 6], u32> = HashMap::new();
    (0..m.positions.len())
        .map(|i| {
            *first
                .entry(weld_key(m.positions[i], m.normals[i]))
                .or_insert(i as u32)
        })
        .collect()
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 1e-12 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        a
    }
}

/// The PN patch's edge midpoint and its quadratic normal between `(p1, n1)` and `(p2, n2)`.
/// Symmetric in its endpoints up to float order; callers pass a canonical order.
pub fn pn_midpoint(p1: [f32; 3], n1: [f32; 3], p2: [f32; 3], n2: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let e = sub(p2, p1);
    let w12 = dot(e, n1);
    let w21 = -dot(e, n2);
    let mut p = [0.; 3];
    for k in 0..3 {
        let b210 = (2. * p1[k] + p2[k] - w12 * n1[k]) / 3.;
        let b120 = (2. * p2[k] + p1[k] - w21 * n2[k]) / 3.;
        p[k] = (p1[k] + p2[k]) / 8. + 3. * (b210 + b120) / 8.;
    }
    let ee = dot(e, e);
    let ns = [n1[0] + n2[0], n1[1] + n2[1], n1[2] + n2[2]];
    let v = if ee > 1e-12 { 2. * dot(e, ns) / ee } else { 0. };
    let n110 = normalize([ns[0] - v * e[0], ns[1] - v * e[1], ns[2] - v * e[2]]);
    let n = normalize([
        0.25 * ns[0] + 0.5 * n110[0],
        0.25 * ns[1] + 0.5 * n110[1],
        0.25 * ns[2] + 0.5 * n110[2],
    ]);
    (p, n)
}

/// The skin of a point between two vertices: their weights added per bone, the four largest kept
/// and renormalised.
pub fn blend_skin(j1: [u16; 4], w1: [f32; 4], j2: [u16; 4], w2: [f32; 4]) -> ([u16; 4], [f32; 4]) {
    let mut acc: Vec<(u16, f32)> = Vec::with_capacity(8);
    for (j, w) in j1.iter().zip(w1).chain(j2.iter().zip(w2)) {
        if w <= 0. {
            continue;
        }
        match acc.iter_mut().find(|(b, _)| b == j) {
            Some(e) => e.1 += w,
            None => acc.push((*j, w)),
        }
    }
    top_four(acc, j1)
}

/// The four largest `(bone, weight)` pairs, renormalised; `fallback` if nothing carries weight.
pub fn top_four(mut acc: Vec<(u16, f32)>, fallback: [u16; 4]) -> ([u16; 4], [f32; 4]) {
    // Largest first; ties by bone so the result never depends on input order.
    acc.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    acc.truncate(4);
    let total: f32 = acc.iter().map(|e| e.1).sum();
    if total <= 0. {
        return (fallback, [1., 0., 0., 0.]);
    }
    let mut j = [0u16; 4];
    let mut w = [0f32; 4];
    for (k, (b, x)) in acc.into_iter().enumerate() {
        j[k] = b;
        w[k] = x / total;
    }
    (j, w)
}

/// Splits every edge `split` selects (by its two welded endpoints) and re-triangulates: a triangle
/// with three split edges becomes four, with two becomes three, with one becomes two. Edges with a
/// single triangle take the linear midpoint. `split` sees the edge's weld ids and whether the edge
/// is interior. Returns the refined mesh.
pub fn refine(m: &SkinMesh, split: impl Fn(u32, u32, bool, usize) -> bool) -> SkinMesh {
    let welds = weld(m);
    let tri_count = m.triangles();
    // Triangles per welded edge.
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    let ekey = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
    for t in 0..tri_count {
        for k in 0..3 {
            let a = welds[m.indices[3 * t + k] as usize];
            let b = welds[m.indices[3 * t + (k + 1) % 3] as usize];
            *uses.entry(ekey(a, b)).or_default() += 1;
        }
    }
    let mut out = m.clone();
    out.indices.clear();
    // New vertex per (unwelded) endpoint pair, so each side keeps its own UVs.
    let mut made: HashMap<(u32, u32), u32> = HashMap::new();
    for t in 0..tri_count {
        let v = [m.indices[3 * t], m.indices[3 * t + 1], m.indices[3 * t + 2]];
        let mut mid: [Option<u32>; 3] = [None; 3];
        for k in 0..3 {
            let (a, b) = (v[k], v[(k + 1) % 3]);
            let (wa, wb) = (welds[a as usize], welds[b as usize]);
            let interior = uses.get(&ekey(wa, wb)).copied().unwrap_or(0) >= 2;
            if !split(wa.min(wb), wa.max(wb), interior, t) {
                continue;
            }
            let key = if a < b { (a, b) } else { (b, a) };
            let id = *made
                .entry(key)
                .or_insert_with(|| add_midpoint(&mut out, m, &welds, a, b, interior));
            mid[k] = Some(id);
        }
        let [a, b, c] = v;
        let tris: &mut Vec<u32> = &mut out.indices;
        let mut push = |x: u32, y: u32, z: u32| tris.extend([x, y, z]);
        match mid {
            [None, None, None] => push(a, b, c),
            [Some(ab), Some(bc), Some(ca)] => {
                push(a, ab, ca);
                push(ab, b, bc);
                push(ca, bc, c);
                push(ab, bc, ca);
            }
            // One split edge: two triangles from its opposite corner.
            [Some(ab), None, None] => {
                push(a, ab, c);
                push(ab, b, c);
            }
            [None, Some(bc), None] => {
                push(b, bc, a);
                push(bc, c, a);
            }
            [None, None, Some(ca)] => {
                push(c, ca, b);
                push(ca, a, b);
            }
            // Two split edges: the corner between them, then the remaining quad as two.
            [Some(ab), Some(bc), None] => {
                push(ab, b, bc);
                push(a, ab, bc);
                push(a, bc, c);
            }
            [None, Some(bc), Some(ca)] => {
                push(bc, c, ca);
                push(b, bc, ca);
                push(b, ca, a);
            }
            [Some(ab), None, Some(ca)] => {
                push(ca, a, ab);
                push(c, ca, ab);
                push(c, ab, b);
            }
        }
    }
    out
}

/// Appends the vertex between `a` and `b`, its position and normal from the welded endpoints in
/// weld order so both sides of a seam agree bit for bit.
fn add_midpoint(
    out: &mut SkinMesh,
    m: &SkinMesh,
    welds: &[u32],
    a: u32,
    b: u32,
    curved: bool,
) -> u32 {
    let (wa, wb) = (welds[a as usize] as usize, welds[b as usize] as usize);
    let (lo, hi) = if wa <= wb { (wa, wb) } else { (wb, wa) };
    let (p, n) = if curved {
        pn_midpoint(
            m.positions[lo],
            m.normals[lo],
            m.positions[hi],
            m.normals[hi],
        )
    } else {
        let (p1, p2) = (m.positions[lo], m.positions[hi]);
        let (n1, n2) = (m.normals[lo], m.normals[hi]);
        (
            [
                (p1[0] + p2[0]) * 0.5,
                (p1[1] + p2[1]) * 0.5,
                (p1[2] + p2[2]) * 0.5,
            ],
            normalize([n1[0] + n2[0], n1[1] + n2[1], n1[2] + n2[2]]),
        )
    };
    let (a, b) = (a as usize, b as usize);
    let avg2 = |x: [f32; 2], y: [f32; 2]| [(x[0] + y[0]) * 0.5, (x[1] + y[1]) * 0.5];
    out.positions.push(p);
    out.normals.push(n);
    out.uv0.push(avg2(m.uv0[a], m.uv0[b]));
    if let (Some(dst), Some(src)) = (out.uv1.as_mut(), m.uv1.as_ref()) {
        dst.push(avg2(src[a], src[b]));
    }
    if let (Some(dst), Some(src)) = (out.colors.as_mut(), m.colors.as_ref()) {
        let (x, y) = (src[a], src[b]);
        dst.push(std::array::from_fn(|k| (x[k] + y[k]) * 0.5));
    }
    // The skin from the welded pair too, so a seam's two copies bend alike.
    let (j, w) = blend_skin(m.joints[lo], m.weights[lo], m.joints[hi], m.weights[hi]);
    out.joints.push(j);
    out.weights.push(w);
    (out.positions.len() - 1) as u32
}

/// One level of uniform PN refinement: every triangle becomes four.
pub fn subdivide(m: &SkinMesh) -> SkinMesh {
    refine(m, |_, _, _, _| true)
}

/// A second level on the triangles `region` marks: their interior edges split (open rims never,
/// so a neighbouring mesh still meets them), and the neighbours that share a split edge close
/// with two or three triangles, so no T-junction opens.
pub fn refine_region(m: &SkinMesh, region: &[bool]) -> SkinMesh {
    let welds = weld(m);
    let mut marked: std::collections::HashSet<(u32, u32)> = Default::default();
    for (t, _) in region.iter().enumerate().filter(|(_, r)| **r) {
        for k in 0..3 {
            let a = welds[m.indices[3 * t + k] as usize];
            let b = welds[m.indices[3 * t + (k + 1) % 3] as usize];
            marked.insert((a.min(b), a.max(b)));
        }
    }
    refine(m, |a, b, interior, _| interior && marked.contains(&(a, b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An octahedron with unit normals at its corners, every face split along a UV seam on the
    /// +x/-x meridian: the vertices there are duplicated with different UVs.
    fn octahedron() -> SkinMesh {
        let corners: [[f32; 3]; 6] = [
            [1., 0., 0.],
            [-1., 0., 0.],
            [0., 1., 0.],
            [0., -1., 0.],
            [0., 0., 1.],
            [0., 0., -1.],
        ];
        let faces = [
            [0, 2, 4],
            [2, 1, 4],
            [1, 3, 4],
            [3, 0, 4],
            [2, 0, 5],
            [1, 2, 5],
            [3, 1, 5],
            [0, 3, 5],
        ];
        let mut m = SkinMesh {
            uv1: None,
            colors: None,
            ..Default::default()
        };
        // Each face gets its own three vertices: every edge is a "seam" the weld must close.
        for (f, face) in faces.iter().enumerate() {
            for (k, &c) in face.iter().enumerate() {
                m.positions.push(corners[c]);
                m.normals.push(corners[c]);
                m.uv0.push([f as f32, k as f32]);
                m.joints.push([c as u16, 7, 0, 0]);
                m.weights.push([0.75, 0.25, 0., 0.]);
                m.indices.push(m.positions.len() as u32 - 1);
            }
        }
        m
    }

    fn welded_edges(m: &SkinMesh) -> HashMap<(u32, u32), u32> {
        let w = weld(m);
        let mut uses = HashMap::new();
        for t in 0..m.triangles() {
            for k in 0..3 {
                let a = w[m.indices[3 * t + k] as usize];
                let b = w[m.indices[3 * t + (k + 1) % 3] as usize];
                *uses.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        uses
    }

    #[test]
    fn subdivision_keeps_the_original_vertices_and_quadruples() {
        let m = octahedron();
        let s = subdivide(&m);
        assert!(s.valid());
        assert_eq!(s.triangles(), 4 * m.triangles());
        assert_eq!(&s.positions[..m.positions.len()], &m.positions[..]);
        assert_eq!(&s.uv0[..m.uv0.len()], &m.uv0[..]);
    }

    #[test]
    fn midpoints_bulge_toward_the_normals_and_seams_stay_closed() {
        let s = subdivide(&octahedron());
        // On a sphere-normalled octahedron the PN midpoint of an edge lies outside the flat edge.
        for p in &s.positions[48..] {
            let r = dot(*p, *p).sqrt();
            assert!(r > 0.75 && r < 1.05, "curved midpoint radius {r}");
        }
        // Welded, every edge still has exactly two triangles: the closed surface has no crack.
        assert!(welded_edges(&s).values().all(|&n| n == 2));
        // Twice refined it stays closed.
        assert!(welded_edges(&subdivide(&s)).values().all(|&n| n == 2));
    }

    #[test]
    fn open_rims_take_the_linear_midpoint() {
        // One lone triangle: all three edges are rims.
        let mut m = octahedron();
        m.indices.truncate(3);
        let s = subdivide(&m);
        let (a, b) = (m.positions[0], m.positions[1]);
        let mid = [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2., (a[2] + b[2]) / 2.];
        assert!(s
            .positions
            .iter()
            .any(|p| sub(*p, mid).iter().all(|d| d.abs() < 1e-6)));
    }

    #[test]
    fn weights_stay_the_top_four_and_normalised() {
        let (j, w) = blend_skin(
            [1, 2, 3, 4],
            [0.4, 0.3, 0.2, 0.1],
            [5, 1, 6, 0],
            [0.5, 0.3, 0.2, 0.],
        );
        assert_eq!(j, [1, 5, 2, 3]);
        assert!((w.iter().sum::<f32>() - 1.).abs() < 1e-6);
        // Kept: 1 (0.7), 5 (0.5), 2 (0.3), 3 (0.2, ties with 6 and wins on its id).
        assert!((w[0] - 0.7 / 1.7).abs() < 1e-6);
        for (jj, ww) in subdivide(&octahedron())
            .joints
            .iter()
            .zip(subdivide(&octahedron()).weights)
        {
            assert!((ww.iter().sum::<f32>() - 1.).abs() < 1e-5, "{jj:?} {ww:?}");
        }
    }

    #[test]
    fn a_refined_region_closes_its_neighbours_without_t_junctions() {
        let s = subdivide(&octahedron());
        let mut region = vec![false; s.triangles()];
        region[0] = true;
        region[5] = true;
        let r = refine_region(&s, &region);
        assert!(r.valid());
        assert!(r.triangles() > s.triangles() + 6);
        // A T-junction would leave an edge with one triangle on a closed surface.
        assert!(welded_edges(&r).values().all(|&n| n == 2));
    }
}
