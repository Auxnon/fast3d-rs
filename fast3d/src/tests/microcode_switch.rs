//! G_LOAD_UCODE switches microcodes mid-list, as Zelda does around 2D backgrounds:
//! F3DZEX2 -> S2DEX2 -> F3DZEX2.

use crate::diag::DiagKind;
use crate::hle::gbi::GbiUcode;
use crate::hle::interp::interpret;
use crate::hle::mem::{GbiDataFormat, RdramImage};
use crate::hle::InterpResult;
use n64_gbi::consts::{rsp_s2dex2, G_RM_OPA_SURF, G_RM_OPA_SURF2};
use n64_gbi::encode::*;

use super::dl_builder::DlBuilder;

const END: (u32, u32) = (0xdf00_0000, 0);
const F3DZEX_DATA: &str = "RSP Gfx ucode F3DZEX.NoN fifo 2.08I Yoshitaka Yasumoto/Kawasedo 1999.";
const S2DEX_DATA: &str = "RSP Gfx ucode S2DEX  fifo 2.08  Yoshitaka Yasumoto 1999.";

/// A fake data segment: padding, then the version string.
fn data_segment(b: &mut DlBuilder, version: &str) -> u32 {
    let mut bytes = vec![0u8; 0x100];
    bytes.extend_from_slice(version.as_bytes());
    bytes.extend_from_slice(&[0x0A, 0]);
    bytes.resize(0x800, 0);
    b.bytes(16, &bytes)
}

fn run(build: impl FnOnce(&mut DlBuilder) -> Vec<(u32, u32)>) -> InterpResult {
    let mut b = DlBuilder::new();
    let commands = build(&mut b);
    b.list("main", &commands);
    let built = b.finish("main");
    interpret(
        RdramImage::new(&built.rdram),
        built.entry as u64,
        GbiUcode::F3dzex2,
        GbiDataFormat::Fixed,
        None,
    )
}

/// A shade-only triangle (textureless, so it needs no texture state to draw).
fn triangle(b: &mut DlBuilder) -> [(u32, u32); 5] {
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
    let vtx = b.vertices(&[[0, 0], [100, 0], [0, 100]].map(|[x, y]| VtxColored {
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
    }));
    [
        gdp_set_cycle_type(0),
        gdp_set_render_mode(G_RM_OPA_SURF, G_RM_OPA_SURF2),
        gdp_set_combine_lerp(shade_c, shade_a, shade_c, shade_a),
        gsp_vertex(0, 3, vtx),
        gsp_1triangle(0, 1, 2),
    ]
}

#[test]
fn switches_to_s2dex2_and_back() {
    let r = run(|b| {
        let s2dex = data_segment(b, S2DEX_DATA);
        let f3dzex = data_segment(b, F3DZEX_DATA);
        let mut commands = vec![gdp_set_color_image(0, 2, 320, 0x10_0000)];
        commands.extend(gsp_load_ucode(0x1000, s2dex, 0x800));
        // S2DEX2 commands: render mode is tracked, sprites aren't drawn.
        commands.push(((rsp_s2dex2::G_OBJ_RENDERMODE as u32) << 24, 0x08));
        commands.push(((rsp_s2dex2::G_OBJ_SPRITE as u32) << 24, 0));
        commands.extend(gsp_load_ucode(0x2000, f3dzex, 0x800));
        commands.extend(triangle(b));
        commands.push(END);
        commands
    });
    assert!(!matches!(
        r.termination,
        crate::inspect::WalkTermination::Rejected
    ));
    assert!(
        r.diags.iter().any(|d| matches!(
            d.kind,
            DiagKind::UnsupportedCommand { opcode, .. } if opcode == rsp_s2dex2::G_OBJ_SPRITE
        )),
        "{:?}",
        r.diags
    );
    assert!(!r
        .diags
        .iter()
        .any(|d| matches!(d.kind, DiagKind::UnsupportedMicrocodeLoad { .. })));
    // Back in F3DZEX2, the triangle draws.
    assert_eq!(r.scene.indices.len(), 3, "{:?}", r.diags);
    assert_eq!(r.dropped_runs, 1);
}

#[test]
fn s2dex2_opcodes_mean_object_commands() {
    // 0x04 is G_SELECT_DL in S2DEX2, not G_BRANCH_W: no branch-target diagnostic.
    let r = run(|b| {
        let s2dex = data_segment(b, S2DEX_DATA);
        let mut commands = gsp_load_ucode(0x1000, s2dex, 0x800).to_vec();
        commands.push(((rsp_s2dex2::G_SELECT_DL as u32) << 24, 0));
        commands.push(END);
        commands
    });
    assert!(!r
        .diags
        .iter()
        .any(|d| matches!(d.kind, DiagKind::MissingBranchTarget)));
    assert!(r.diags.iter().any(|d| matches!(
        d.kind,
        DiagKind::UnsupportedCommand { opcode, .. } if opcode == rsp_s2dex2::G_SELECT_DL
    )));
}

#[test]
fn unknown_microcode_load_still_rejects() {
    let r = run(|b| {
        let unknown = data_segment(
            b,
            "RSP Gfx ucode L3DEX       fifo 2.08  Yoshitaka Yasumoto 1999.",
        );
        let mut commands = gsp_load_ucode(0x1000, unknown, 0x800).to_vec();
        commands.push(END);
        commands
    });
    assert!(r
        .diags
        .iter()
        .any(|d| matches!(d.kind, DiagKind::UnsupportedMicrocodeLoad { .. })));
    assert!(matches!(
        r.termination,
        crate::inspect::WalkTermination::Rejected
    ));
}
