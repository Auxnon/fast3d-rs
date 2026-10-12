//! F3DZEX2 point lights: the prefolded light sets, and the GPU lighting math against a
//! reference port of RT64's `computePosLight` (which matches the microcode).

use crate::hle::gbi::GbiUcode;
use crate::hle::interp::interpret;
use crate::hle::math::{identity, Mat4};
use crate::hle::mem::{GbiDataFormat, RdramImage};
use crate::hle::Scene;
use crate::render::headless_device;
use n64_gbi::consts::rsp_f3dzex2::G_LIGHTING_POSITIONAL;
use n64_gbi::consts::{G_LIGHTING, G_MV_LIGHT};
use n64_gbi::encode::*;

use super::dl_builder::DlBuilder;
use super::render::run_compute_outputs;

const END: (u32, u32) = (0xdf00_0000, 0);

/// Scale 2, then translate by (10, 0, 0) (N64 row-vector convention).
const MODEL: Mat4 = [
    [2.0, 0.0, 0.0, 0.0],
    [0.0, 2.0, 0.0, 0.0],
    [0.0, 0.0, 2.0, 0.0],
    [10.0, 0.0, 0.0, 1.0],
];

/// A `PointLight_t`: col, kc, colc, kl, s16 pos[3], kq, pad.
fn point_light(col: [u8; 3], kc: u8, kl: u8, kq: u8, pos: [i16; 3]) -> Vec<u8> {
    let mut b = vec![col[0], col[1], col[2], kc, col[0], col[1], col[2], kl];
    for p in pos {
        b.extend(p.to_be_bytes());
    }
    b.extend([kq, 0]);
    b
}

fn lit_scene(ucode: GbiUcode, extra: &[(u32, u32)]) -> Scene {
    let mut b = DlBuilder::new();
    let light = b.bytes(8, &point_light([255, 128, 0], 8, 0, 2, [100, 0, 0]));
    let ambient = b.bytes(8, &[32, 32, 32, 0, 32, 32, 32, 0]);
    let model = b.matrix(MODEL);
    let vtx = b.vertices(&[VtxColored {
        x: 0,
        y: 0,
        z: 0,
        flag: 0,
        s: 0,
        t: 0,
        r: 127,
        g: 0,
        b: 0,
        a: 255,
    }]);
    let mut commands = vec![
        gsp_matrix(model, false, true, false),
        gsp_set_geometrymode(G_LIGHTING | G_LIGHTING_POSITIONAL),
        gsp_numlights(1),
        gsp_light(light, 1),
        gsp_dma2p(ambient, 8, G_MV_LIGHT, 3 * 24),
    ];
    commands.extend_from_slice(extra);
    commands.extend([gsp_vertex(0, 1, vtx), END]);
    let entry = b.list("main", &commands);
    let built = b.finish("main");
    let _ = entry;
    let r = interpret(
        RdramImage::new(&built.rdram),
        built.entry as u64,
        ucode,
        GbiDataFormat::Fixed,
        None,
    );
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    r.scene
}

#[test]
fn f3dzex2_prefolds_point_light_with_modelview_rows() {
    let scene = lit_scene(GbiUcode::F3dzex2, &[]);
    let amb = 32.0 / 255.0;
    assert_eq!(
        scene.lights_table,
        vec![
            ([100.0, 0.0, 0.0], [1.0, 128.0 / 255.0, 0.0]),
            ([0.0; 3], [amb; 3]),
            ([2.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
            ([0.0, 0.0, 2.0], [10.0, 0.0, 0.0]),
        ]
    );
    assert_eq!(
        scene.lights_params,
        vec![[8.0, 2.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]]
    );
    assert_eq!(scene.light_count, vec![2]);
}

#[test]
fn f3dex2_ignores_positional_lighting() {
    let scene = lit_scene(GbiUcode::F3dex2, &[]);
    assert_eq!(scene.lights_table.len(), 2);
    assert_eq!(scene.lights_params, vec![[0.0, 0.0], [0.0, 0.0]]);
}

#[test]
fn light_color_clearing_kc_makes_the_light_directional() {
    let [a, b] = gsp_lightcolor(1, 0x4040_4000);
    let scene = lit_scene(GbiUcode::F3dzex2, &[a, b]);
    assert_eq!(scene.lights_table.len(), 2);
    assert_eq!(scene.lights_params[0], [0.0, 0.0]);
    assert_eq!(scene.lights_table[0].1, [64.0 / 255.0; 3]);
}

/// RT64's `computePosLight` + ambient, in f64.
#[allow(clippy::too_many_arguments)]
fn reference(
    pos: [f64; 3],
    normal: [f64; 3],
    m: Mat4,
    light_pos: [f64; 3],
    (kc, kl, kq): (f64, f64, f64),
    col: [f64; 3],
    ambient: [f64; 3],
) -> [f64; 3] {
    let row = |r: usize| [m[r][0] as f64, m[r][1] as f64, m[r][2] as f64];
    let world: Vec<f64> = (0..3)
        .map(|i| pos[0] * row(0)[i] + pos[1] * row(1)[i] + pos[2] * row(2)[i] + row(3)[i])
        .collect();
    let mut d: Vec<f64> = (0..3).map(|i| light_pos[i] - world[i]).collect();
    let dist = (d[0] * d[0] + d[1] * d[1] + 2.0 * d[2] * d[2]).sqrt();
    if dist > 0.0 {
        d.iter_mut().for_each(|v| *v /= dist);
    }
    let dot = |a: [f64; 3], b: &[f64]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let local = [dot(row(0), &d), dot(row(1), &d), dot(row(2), &d)];
    let nl = (dot(normal, &local) * 4.0).clamp(0.0, 1.0);
    let mut att = 1.0 / (0.5 + kc / 16.0 + kl * dist / 32768.0 + kq * dist * dist / 524288.0);
    att %= 1.0 + 1e-6;
    [0, 1, 2].map(|i| (ambient[i] + nl * att * col[i]).min(1.0))
}

#[test]
fn gpu_point_light_matches_rt64_formula() {
    let (device, queue, _) = headless_device();
    let ambient = [0.1, 0.05, 0.0];
    let col = [0.9, 0.6, 0.3];
    // (vertex position, s8 normal, light position, (kc, kl, kq))
    type Case = ([i16; 3], [i8; 3], [i16; 3], (u8, u8, u8));
    let cases: [Case; 6] = [
        ([0, 0, 0], [127, 0, 0], [100, 0, 0], (8, 0, 2)),
        ([5, 3, -2], [0, 127, 0], [-40, 300, 20], (8, 3, 1)),
        // kc = 1: attenuation is above 1 and wraps (fmod) as on the RSP.
        ([0, 0, 0], [127, 0, 0], [60, 0, 0], (1, 0, 0)),
        // Facing away: no diffuse.
        ([0, 0, 0], [-127, 0, 0], [100, 0, 0], (8, 0, 0)),
        // Z counts twice in the distance.
        ([0, 0, 0], [0, 0, 127], [0, 0, 400], (16, 4, 4)),
        ([-7, 9, 4], [90, 90, 0], [500, 500, -10], (2, 1, 0)),
    ];
    let n = cases.len();
    let model = MODEL;
    let mut scene = Scene {
        raw_pos: cases.iter().map(|c| c.0.map(f32::from)).collect(),
        raw_st: vec![[0.0; 2]; n],
        cn: cases
            .iter()
            .map(|c| u32::from_le_bytes([c.1[0] as u8, c.1[1] as u8, c.1[2] as u8, 255]))
            .collect(),
        mtx_index: vec![0; n],
        viewport_index: vec![0; n],
        texcoord_index: vec![0; n],
        light_index: (0..n as u32).map(|i| i * 4).collect(),
        light_count: vec![2; n],
        texgen_mode: vec![0; n],
        lookat_index: vec![0; n],
        fog: vec![0; n],
        mvp_table: vec![identity()],
        viewport_table: vec![([160.0, 120.0, 0.0], [160.0, 120.0, 0.5])],
        texcoord_table: vec![[0.0; 2]],
        texgen_scale_table: vec![[0.0; 2]],
        ..Default::default()
    };
    for c in &cases {
        let (kc, kl, kq) = c.3;
        scene.lights_table.extend([
            (c.2.map(f32::from), col.map(|v: f64| v as f32)),
            ([0.0; 3], ambient.map(|v: f64| v as f32)),
            ([2.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
            ([0.0, 0.0, 2.0], [10.0, 0.0, 0.0]),
        ]);
        scene.lights_params.extend([
            [f32::from(kc), f32::from(kl) * 256.0 + f32::from(kq)],
            [1.0, 0.0],
            [0.0, 0.0],
            [0.0, 1.0],
        ]);
    }
    let out = run_compute_outputs(&device, &queue, &scene);
    for (i, c) in cases.iter().enumerate() {
        let expected = reference(
            c.0.map(f64::from),
            c.1.map(|v| f64::from(v) / 127.0),
            model,
            c.2.map(f64::from),
            (f64::from(c.3 .0), f64::from(c.3 .1), f64::from(c.3 .2)),
            col,
            ambient,
        );
        for axis in 0..3 {
            let got = f64::from(out[i].color[axis]);
            assert!(
                (got - expected[axis]).abs() < 1e-4,
                "case {i} axis {axis}: expected {expected:?}, got {:?}",
                out[i].color
            );
        }
    }
    // The wrapped-attenuation case really wraps (1 / 0.5625 > 1).
    assert!(f64::from(out[2].color[0]) < 0.9);
}
