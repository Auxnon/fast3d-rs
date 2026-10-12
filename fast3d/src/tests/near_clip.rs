//! `.NoN` microcodes (F3DZEX2) don't clip against the near plane; depth is clamped instead.

use crate::hle::gbi::GbiUcode;
use crate::hle::interp::interpret;
use crate::hle::math::identity;
use crate::hle::mem::{GbiDataFormat, RdramImage};
use crate::hle::rsp::NO_NEAR_CLIP;
use crate::hle::Scene;
use crate::render::headless_device;
use n64_gbi::encode::*;

use super::dl_builder::DlBuilder;
use super::render::run_compute_outputs;

fn vertex_flags(ucode: GbiUcode) -> Vec<u32> {
    let mut b = DlBuilder::new();
    let vtx = b.vertices(&[VtxColored {
        x: 0,
        y: 0,
        z: 0,
        flag: 0,
        s: 0,
        t: 0,
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    }]);
    b.list("main", &[gsp_vertex(0, 1, vtx), (0xdf00_0000, 0)]);
    let built = b.finish("main");
    let r = interpret(
        RdramImage::new(&built.rdram),
        built.entry as u64,
        ucode,
        GbiDataFormat::Fixed,
        None,
    );
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    r.scene.modify_flags
}

#[test]
fn only_f3dzex2_marks_vertices_no_near_clip() {
    assert_eq!(vertex_flags(GbiUcode::F3dzex2), vec![NO_NEAR_CLIP]);
    assert_eq!(vertex_flags(GbiUcode::F3dex2), vec![0]);
}

#[test]
fn no_near_clip_clamps_depth_to_the_near_plane() {
    let (device, queue, _) = headless_device();
    // z = -2 lands in front of the near plane: clip z = -2 * 0.5 + 1 * 0.5 = -0.5.
    let z = [-2.0, 0.5];
    let mut scene = Scene {
        raw_pos: z
            .iter()
            .flat_map(|&z| [[0.0, 0.0, z], [0.0, 0.0, z]])
            .collect(),
        raw_st: vec![[0.0; 2]; 4],
        cn: vec![0xffff_ffff; 4],
        mtx_index: vec![0; 4],
        viewport_index: vec![0; 4],
        texcoord_index: vec![0; 4],
        light_index: vec![0; 4],
        light_count: vec![0; 4],
        texgen_mode: vec![0; 4],
        lookat_index: vec![0; 4],
        fog: vec![0; 4],
        modify_flags: vec![0, NO_NEAR_CLIP, 0, NO_NEAR_CLIP],
        mvp_table: vec![identity()],
        viewport_table: vec![([160.0, 120.0, 0.5], [160.0, 120.0, 0.5])],
        texcoord_table: vec![[0.0; 2]],
        texgen_scale_table: vec![[0.0; 2]],
        ..Default::default()
    };
    scene.modify_screen = vec![[0.0; 4]; 4];
    let out = run_compute_outputs(&device, &queue, &scene);
    let depth: Vec<f32> = out.iter().map(|o| o.pos[2]).collect();
    // Clipped normally, clamped with NoN; vertices behind it are untouched either way.
    assert_eq!(depth, vec![-0.5, 0.0, 0.75, 0.75]);
}
