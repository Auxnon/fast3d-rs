//! RT64's extended GBI (`rt64_extended_gbi.h`), as Zelda64Recomp's patches use it.

use crate::diag::DiagKind;
use crate::hle::gbi::GbiUcode;
use crate::hle::interp::interpret;
use crate::hle::mem::{GbiDataFormat, RdramImage};
use crate::hle::rsp::{SceneOp, NO_NEAR_CLIP};
use crate::hle::InterpResult;
use n64_gbi::consts::{G_LIGHTING, G_RM_OPA_SURF, G_RM_OPA_SURF2};
use n64_gbi::encode::*;

use super::dl_builder::DlBuilder;

const END: (u32, u32) = (0xdf00_0000, 0);
const EX: u32 = 0x64;
const HOOK: u32 = 0xE052_5464;
/// G_SPECIAL_1: rejects the task if it's ever executed as a command, so it proves the extra
/// slots of a multi-slot command were consumed.
const FILLER: (u32, u32) = (0xD500_0000, 0);

// Encoders mirroring the gEX* macros.
fn ex_enable() -> (u32, u32) {
    (HOOK, (1 << 28) | EX)
}
fn ex_disable() -> (u32, u32) {
    (HOOK, 2 << 28)
}
fn ex_display_list(addr: u32) -> (u32, u32) {
    (HOOK, (3 << 28) | addr)
}
fn ex_branch_list(addr: u32) -> (u32, u32) {
    (HOOK, (4 << 28) | addr)
}
fn ex(sub: u32, w1: u32) -> (u32, u32) {
    ((EX << 24) | sub, w1)
}
fn halves(a: i16, b: i16) -> u32 {
    (u32::from(a as u16) << 16) | u32::from(b as u16)
}

fn run(build: impl FnOnce(&mut DlBuilder) -> Vec<(u32, u32)>, ucode: GbiUcode) -> InterpResult {
    let mut b = DlBuilder::new();
    let commands = build(&mut b);
    b.list("main", &commands);
    let built = b.finish("main");
    interpret(
        RdramImage::new(&built.rdram),
        built.entry as u64,
        ucode,
        GbiDataFormat::Fixed,
        None,
    )
}

fn shade_triangle_setup() -> [(u32, u32); 3] {
    let shade_c = CcPass {
        a: ZERO_C,
        b: ZERO_C,
        c: ZERO_C,
        d: 4,
    };
    let shade_a = CcPass {
        a: ZERO_A,
        b: ZERO_A,
        c: ZERO_A,
        d: 4,
    };
    [
        gdp_set_cycle_type(0),
        gdp_set_render_mode(G_RM_OPA_SURF, G_RM_OPA_SURF2),
        gdp_set_combine_lerp(shade_c, shade_a, shade_c, shade_a),
    ]
}

fn white(x: i16, y: i16) -> VtxColored {
    VtxColored {
        x,
        y,
        z: 0,
        flag: 0,
        s: 0,
        t: 0,
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    }
}

#[test]
fn disabled_extended_opcode_is_unknown() {
    let r = run(|_| vec![ex(0x15, 0), END], GbiUcode::F3dzex2);
    assert!(
        r.diags
            .iter()
            .any(|d| d.kind == DiagKind::UnknownOpcode(0x64)),
        "{:?}",
        r.diags
    );
    let r = run(
        |_| vec![ex_enable(), ex(0x15, 0), ex_disable(), ex(0x16, 0), END],
        GbiUcode::F3dzex2,
    );
    assert_eq!(
        r.diags.iter().map(|d| d.kind).collect::<Vec<_>>(),
        vec![DiagKind::UnknownOpcode(0x64)]
    );
}

#[test]
fn push_and_pop_restore_rdp_state() {
    let r = run(
        |_| {
            vec![
                ex_enable(),
                gdp_set_prim_color(0, 0, 0x1111_11ff),
                gdp_set_env_color(0x2222_22ff),
                gdp_set_blend_color(0x3333_33ff),
                gdp_set_fog_color(0x4444_44ff),
                gdp_set_fill_color(0x5555_5555),
                gdp_set_scissor(0, 4, 8, 400, 300),
                gdp_set_cycle_type(0),
                ex(0x27, 0), // push prim
                ex(0x1F, 0), // push env
                ex(0x21, 0), // push blend
                ex(0x23, 0), // push fog
                ex(0x25, 0), // push fill
                ex(0x17, 0), // push scissor
                ex(0x19, 0), // push othermode
                gdp_set_prim_color(0, 0, 0xaaaa_aaff),
                gdp_set_env_color(0xbbbb_bbff),
                gdp_set_blend_color(0xcccc_ccff),
                gdp_set_fog_color(0xdddd_ddff),
                gdp_set_fill_color(0xeeee_eeee),
                gdp_set_scissor(0, 0, 0, 40, 40),
                gdp_set_cycle_type(3),
                ex(0x28, 0),
                ex(0x20, 0),
                ex(0x22, 0),
                ex(0x24, 0),
                ex(0x26, 0),
                ex(0x18, 0),
                ex(0x1A, 0),
                END,
            ]
        },
        GbiUcode::F3dzex2,
    );
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    assert_eq!(r.rdp.prim, [0x11, 0x11, 0x11, 0xff]);
    assert_eq!(r.rdp.env, [0x22, 0x22, 0x22, 0xff]);
    assert_eq!(r.rdp.blend_color, [0x33, 0x33, 0x33, 0xff]);
    assert_eq!(r.rdp.fog_color, [0x44, 0x44, 0x44, 0xff]);
    assert_eq!(r.rdp.fill_color_raw, 0x5555_5555);
    assert_eq!((r.rdp.scissor.ulx, r.rdp.scissor.lrx), (1, 100));
    assert_eq!((r.rdp.other_mode_h >> 20) & 3, 0);
}

#[test]
fn push_and_pop_restore_geometry_mode_and_combine() {
    let r = run(
        |_| {
            vec![
                ex_enable(),
                gsp_set_geometrymode(G_LIGHTING),
                gdp_set_combine_lerp(
                    CcPass {
                        a: ZERO_C,
                        b: ZERO_C,
                        c: ZERO_C,
                        d: 4,
                    },
                    CcPass {
                        a: ZERO_A,
                        b: ZERO_A,
                        c: ZERO_A,
                        d: 4,
                    },
                    CcPass {
                        a: ZERO_C,
                        b: ZERO_C,
                        c: ZERO_C,
                        d: 4,
                    },
                    CcPass {
                        a: ZERO_A,
                        b: ZERO_A,
                        c: ZERO_A,
                        d: 4,
                    },
                ),
                ex(0x29, 0),
                ex(0x1B, 0),
                gsp_clear_geometrymode(G_LIGHTING),
                gdp_set_combine_lerp(
                    CcPass {
                        a: ZERO_C,
                        b: ZERO_C,
                        c: ZERO_C,
                        d: 3,
                    },
                    CcPass {
                        a: ZERO_A,
                        b: ZERO_A,
                        c: ZERO_A,
                        d: 3,
                    },
                    CcPass {
                        a: ZERO_C,
                        b: ZERO_C,
                        c: ZERO_C,
                        d: 3,
                    },
                    CcPass {
                        a: ZERO_A,
                        b: ZERO_A,
                        c: ZERO_A,
                        d: 3,
                    },
                ),
                ex(0x2A, 0),
                ex(0x1C, 0),
                END,
            ]
        },
        GbiUcode::F3dzex2,
    );
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    assert_ne!(r.geometry_mode & G_LIGHTING, 0);
    let shade = gdp_set_combine_lerp(
        CcPass {
            a: ZERO_C,
            b: ZERO_C,
            c: ZERO_C,
            d: 4,
        },
        CcPass {
            a: ZERO_A,
            b: ZERO_A,
            c: ZERO_A,
            d: 4,
        },
        CcPass {
            a: ZERO_C,
            b: ZERO_C,
            c: ZERO_C,
            d: 4,
        },
        CcPass {
            a: ZERO_A,
            b: ZERO_A,
            c: ZERO_A,
            d: 4,
        },
    );
    assert_eq!((r.rdp.combine_l, r.rdp.combine_h), shade);
}

#[test]
fn push_and_pop_restore_viewport_and_projection() {
    let r = run(
        |b| {
            let first = b.viewport(Vp {
                vscale: [640, 480, 511, 0],
                vtrans: [640, 480, 511, 0],
            });
            let second = b.viewport(Vp {
                vscale: [320, 240, 511, 0],
                vtrans: [320, 240, 511, 0],
            });
            let scale2 = b.matrix(n64_gbi::gu::gu_scale(2.0, 2.0, 2.0));
            let scale3 = b.matrix(n64_gbi::gu::gu_scale(3.0, 3.0, 3.0));
            let vtx = b.vertices(&[white(1, 1)]);
            vec![
                ex_enable(),
                gsp_viewport(first),
                gsp_matrix(scale2, true, true, false),
                ex(0x15, 0),
                ex(0x1D, 0),
                gsp_viewport(second),
                gsp_matrix(scale3, true, true, false),
                ex(0x16, 0),
                ex(0x1E, 0),
                gsp_vertex(0, 1, vtx),
                END,
            ]
        },
        GbiUcode::F3dzex2,
    );
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    let scene = &r.scene;
    assert_eq!(
        scene.viewport_table[scene.viewport_index[0] as usize].0[0],
        160.0
    );
    assert_eq!(scene.mvp_table[scene.mtx_index[0] as usize][0][0], 2.0);
}

#[test]
fn ex_vertex_loads_24_byte_vertices() {
    let r = run(
        |b| {
            // VertexEX: a Vtx plus the previous frame's s16 position (and padding).
            let mut bytes = Vec::new();
            for v in [white(10, 20), white(30, 40), white(50, 60)] {
                bytes.extend(v.to_bytes());
                bytes.extend([0xAA; 8]);
            }
            let addr = b.bytes(16, &bytes);
            let mut commands = vec![ex_enable(), gdp_set_color_image(0, 2, 320, 0x10_0000)];
            commands.extend(shade_triangle_setup());
            commands.extend([
                ex(0x14, (3 << 8) | 4),
                (0, addr),
                gsp_1triangle(4, 5, 6),
                END,
            ]);
            commands
        },
        GbiUcode::F3dzex2,
    );
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    assert_eq!(
        r.scene.raw_pos,
        vec![[10.0, 20.0, 0.0], [30.0, 40.0, 0.0], [50.0, 60.0, 0.0]]
    );
    assert_eq!(r.scene.indices.len(), 3);
}

#[test]
fn ex_fill_and_texture_rectangles_draw() {
    let r = run(
        |_| {
            vec![
                ex_enable(),
                gdp_set_color_image(0, 2, 320, 0x10_0000),
                gdp_set_cycle_type(3),
                gdp_set_fill_color(0x0001_0001),
                // gEXFillRectangle with signed coordinates (offscreen left).
                ex(0x03, 0),
                (halves(-8, 4), halves(400, 80)),
                END,
            ]
        },
        GbiUcode::F3dzex2,
    );
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    let ops: Vec<_> = r
        .scene
        .framebuffer_pairs
        .iter()
        .flat_map(|p| &p.ops)
        .collect();
    assert!(ops.iter().any(|op| matches!(
        op,
        SceneOp::FillRect { rect, .. } if (rect.ulx, rect.uly, rect.lrx, rect.lry) == (-2, 1, 100, 20)
    )), "{ops:?}");

    let r = run(
        |_| {
            let prim_c = CcPass {
                a: ZERO_C,
                b: ZERO_C,
                c: ZERO_C,
                d: 3,
            };
            let prim_a = CcPass {
                a: ZERO_A,
                b: ZERO_A,
                c: ZERO_A,
                d: 3,
            };
            vec![
                ex_enable(),
                gdp_set_color_image(0, 2, 320, 0x10_0000),
                gdp_set_cycle_type(0),
                gdp_set_render_mode(G_RM_OPA_SURF, G_RM_OPA_SURF2),
                gdp_set_combine_lerp(prim_c, prim_a, prim_c, prim_a),
                // gEXTextureRectangle(lorigin, rorigin, ulx, uly, lrx, lry, tile, s, t, dsdx, dtdy)
                ex(0x02, 1),
                (halves(-40, 8), halves(200, 120)),
                (halves(32, 64), halves(1024, -1024)),
                END,
            ]
        },
        GbiUcode::F3dzex2,
    );
    let ops: Vec<_> = r
        .scene
        .framebuffer_pairs
        .iter()
        .flat_map(|p| &p.ops)
        .collect();
    assert!(
        ops.iter().any(|op| matches!(
            op,
            SceneOp::TexRect { rect, tile: 1, uls: 32, ult: 64, dsdx: 1024, dtdy: -1024, .. }
                if (rect.ulx, rect.uly, rect.lrx, rect.lry) == (-40, 8, 200, 120)
        )),
        "{ops:?} {:?}",
        r.diags
    );
}

#[test]
fn hook_display_list_calls_and_branch_list_jumps() {
    let r = run(
        |b| {
            let child = b.list("child", &[gdp_set_env_color(0x1234_56ff), END]);
            let tail = b.list("tail", &[gdp_set_prim_color(0, 0, 0x6543_21ff), END]);
            vec![
                ex_display_list(child),
                ex_branch_list(tail),
                gdp_set_env_color(0),
                END,
            ]
        },
        GbiUcode::F3dzex2,
    );
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    assert_eq!(r.rdp.env, [0x12, 0x34, 0x56, 0xff]);
    assert_eq!(r.rdp.prim, [0x65, 0x43, 0x21, 0xff]);
}

#[test]
fn vertex_z_test_spans_drop_triangles() {
    let r = run(
        |b| {
            let vtx = b.vertices(&[white(0, 0), white(100, 0), white(0, 100)]);
            let mut commands = vec![ex_enable(), gdp_set_color_image(0, 2, 320, 0x10_0000)];
            commands.extend(shade_triangle_setup());
            commands.extend([
                gsp_vertex(0, 3, vtx),
                ex(0x0A, 0),
                gsp_1triangle(0, 1, 2),
                gsp_1triangle(0, 2, 1),
                ex(0x0B, 0),
                gsp_1triangle(0, 1, 2),
                END,
            ]);
            commands
        },
        GbiUcode::F3dzex2,
    );
    assert_eq!(r.scene.indices.len(), 3);
    assert_eq!(r.dropped_runs, 2);
    assert_eq!(
        r.diags.iter().map(|d| d.kind).collect::<Vec<_>>(),
        vec![DiagKind::UnsupportedExtendedCommand { command: 0x0A }]
    );
}

#[test]
fn near_clipping_toggle_and_force_branch() {
    let r = run(
        |b| {
            let vtx = b.vertices(&[white(0, 0)]);
            let target = b.list("target", &[gdp_set_env_color(0x0101_01ff), END]);
            let [half, branch] = gsp_branch_less_w(target, 1, 0);
            vec![
                ex_enable(),
                gsp_vertex(0, 1, vtx),
                ex(0x2D, 1), // near clipping on
                gsp_vertex(1, 1, vtx),
                ex(0x11, 1), // force branch
                half,
                branch,
                END,
            ]
        },
        GbiUcode::F3dzex2,
    );
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    assert_eq!(r.scene.modify_flags, vec![NO_NEAR_CLIP, 0]);
    // W = 1 isn't below 0, so only the force made the branch.
    assert_eq!(r.rdp.env, [1, 1, 1, 0xff]);
}

#[test]
fn interpolation_and_alignment_commands_are_accepted() {
    let r = run(
        |_| {
            vec![
                ex_enable(),
                ex(0x0C, 0x1234), // gEXMatrixGroup (2 slots)
                FILLER,
                ex(0x0D, 1), // gEXPopMatrixGroup
                ex(0x06, 0), // gEXSetRectAlign (2 slots)
                FILLER,
                ex(0x08, 0), // gEXSetScissorAlign (3 slots)
                FILLER,
                FILLER,
                ex(0x09, 30),
                ex(0x0E, 1),
                ex(0x2B, 512),
                ex(0x2C, 1),
                END,
            ]
        },
        GbiUcode::F3dzex2,
    );
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    let r = run(|_| vec![ex_enable(), ex(0x50, 0), END], GbiUcode::F3dzex2);
    assert_eq!(
        r.diags.iter().map(|d| d.kind).collect::<Vec<_>>(),
        vec![DiagKind::UnsupportedExtendedCommand { command: 0x50 }]
    );
}
