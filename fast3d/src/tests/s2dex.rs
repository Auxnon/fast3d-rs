//! S2DEX2 backgrounds and object texture loads (ported from RT64's S2DEX).

use crate::hle::gbi::GbiUcode;
use crate::hle::interp::interpret;
use crate::hle::mem::{GbiDataFormat, RdramImage};
use crate::hle::rsp::SceneOp;
use crate::hle::InterpResult;
use n64_gbi::consts::rsp_s2dex2::*;
use n64_gbi::encode::*;

use super::dl_builder::DlBuilder;

const END: (u32, u32) = (0xdf00_0000, 0);
const CIMG: u32 = 0x0010_0000;

fn s2dex(b: &mut DlBuilder, commands: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let _ = b;
    let mut all = vec![
        gdp_set_color_image(0, 2, 320, CIMG),
        gdp_set_scissor(0, 0, 0, 1280, 960),
    ];
    all.extend_from_slice(commands);
    all.push(END);
    all
}

fn run(build: impl FnOnce(&mut DlBuilder) -> Vec<(u32, u32)>) -> InterpResult {
    let mut b = DlBuilder::new();
    let commands = build(&mut b);
    b.list("main", &commands);
    let built = b.finish("main");
    interpret(
        RdramImage::new(&built.rdram),
        built.entry as u64,
        GbiUcode::S2dex2,
        GbiDataFormat::Fixed,
        None,
    )
}

/// A `uObjBg_t`/`uObjScaleBg_t`: the shared header, then `w28`, `w30`, `imageYorig`.
#[allow(clippy::too_many_arguments)]
fn background(
    image: u32,
    (image_w, image_h): (u16, u16),
    (frame_x, frame_y, frame_w, frame_h): (i16, i16, u16, u16),
    fmt: u8,
    siz: u8,
    w28: u16,
    w30: u16,
) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [
        0u16,
        image_w,
        frame_x as u16,
        frame_w,
        0,
        image_h,
        frame_y as u16,
        frame_h,
    ] {
        b.extend(v.to_be_bytes());
    }
    b.extend(image.to_be_bytes());
    b.extend(0xFFF4u16.to_be_bytes()); // G_BGLT_LOADTILE
    b.extend([fmt, siz]);
    b.extend([0u8; 4]); // imagePal, imageFlip
    b.extend(w28.to_be_bytes());
    b.extend(w30.to_be_bytes());
    b.extend(0i32.to_be_bytes()); // imageYorig
    b.extend([0u8; 4]);
    b
}

fn tex_rects(r: &InterpResult) -> Vec<(i32, i32, i32, i32, i16, i16)> {
    r.scene
        .framebuffer_pairs
        .iter()
        .flat_map(|p| &p.ops)
        .filter_map(|op| match op {
            SceneOp::TexRect {
                rect, dsdx, dtdy, ..
            } => Some((rect.ulx, rect.uly, rect.lrx, rect.lry, *dsdx, *dtdy)),
            _ => None,
        })
        .collect()
}

fn copy_setup() -> [(u32, u32); 2] {
    let tex = CcPass {
        a: ZERO_C,
        b: ZERO_C,
        c: ZERO_C,
        d: 1,
    };
    let tex_a = CcPass {
        a: ZERO_A,
        b: ZERO_A,
        c: ZERO_A,
        d: 1,
    };
    [
        gdp_set_cycle_type(2),
        gdp_set_combine_lerp(tex, tex_a, tex, tex_a),
    ]
}

#[test]
fn bg_copy_draws_tmem_high_strips() {
    let r = run(|b| {
        let image = b.bytes(16, &vec![0x55; 320 * 240 * 2]);
        // 6 rows of 320 RGBA16 texels fill TMEM: tmemH = 6.0 (10.2).
        let bg = b.bytes(
            8,
            &background(
                image,
                (320 << 2, 240 << 2),
                (0, 0, 320 << 2, 240 << 2),
                0,
                2,
                0,
                6 << 2,
            ),
        );
        let mut commands = copy_setup().to_vec();
        commands.push(((G_BG_COPY as u32) << 24, bg));
        s2dex(b, &commands)
    });
    let rects = tex_rects(&r);
    assert_eq!(rects.len(), 40, "{:?}", r.diags);
    for (i, rect) in rects.iter().enumerate() {
        let uly = i as i32 * 24;
        assert_eq!(
            *rect,
            (0, uly, 1276, uly + 20, 4 << 10, 4 << 8),
            "strip {i}"
        );
    }
    // The render tile: 320 16-bit texels per line = 80 TMEM words.
    assert_eq!(r.rdp.tiles[0].line, 80);
    assert_eq!(r.rdp.tiles[0].siz, 2);
}

#[test]
fn bg_copy_of_a_framebuffer_draws_one_rectangle() {
    let r = run(|b| {
        let bg = b.bytes(
            8,
            &background(
                CIMG,
                (320 << 2, 240 << 2),
                (0, 0, 320 << 2, 240 << 2),
                0,
                2,
                0,
                6 << 2,
            ),
        );
        let mut commands = vec![
            // Draw into the image first, so it's a known framebuffer, then target another.
            gdp_set_cycle_type(3),
            gdp_set_fill_color(0x0001_0001),
            gdp_fill_rectangle(0, 0, 1276, 956),
            gdp_set_color_image(0, 2, 320, 0x0020_0000),
        ];
        commands.extend(copy_setup());
        commands.push(((G_BG_COPY as u32) << 24, bg));
        s2dex(b, &commands)
    });
    let rects = tex_rects(&r);
    assert_eq!(
        rects,
        vec![(0, 0, 1276, 956, 4 << 10, 4 << 8)],
        "{:?}",
        r.diags
    );
    let sources: Vec<_> = r
        .scene
        .framebuffer_pairs
        .iter()
        .flat_map(|p| &p.ops)
        .filter_map(|op| match op {
            SceneOp::TexRect { fb_source, .. } => Some(*fb_source),
            _ => None,
        })
        .collect();
    assert_eq!(sources, vec![Some(u64::from(CIMG))]);
}

#[test]
fn bg_1cyc_unscaled_tiles_the_frame_in_tmem_slices() {
    let r = run(|b| {
        let image = b.bytes(16, &vec![0x33; 320 * 240 * 2]);
        // scaleW = scaleH = 1.0 (s5.10).
        let bg = b.bytes(
            8,
            &background(
                image,
                (320 << 2, 240 << 2),
                (0, 0, 320 << 2, 240 << 2),
                0,
                2,
                1 << 10,
                1 << 10,
            ),
        );
        let tex = CcPass {
            a: ZERO_C,
            b: ZERO_C,
            c: ZERO_C,
            d: 1,
        };
        let tex_a = CcPass {
            a: ZERO_A,
            b: ZERO_A,
            c: ZERO_A,
            d: 1,
        };
        let commands = [
            gdp_set_cycle_type(0),
            gdp_set_render_mode(
                n64_gbi::consts::G_RM_OPA_SURF,
                n64_gbi::consts::G_RM_OPA_SURF2,
            ),
            gdp_set_combine_lerp(tex, tex_a, tex, tex_a),
            ((G_BG_1CYC as u32) << 24, bg),
        ];
        s2dex(b, &commands)
    });
    // 81 TMEM words per line (320 texels + one), 512 / 81 = 6 lines per slice. The frame is
    // clamped to ((image << 10) / scale - 1) & !3 = 1276 x 956 (10.2), i.e. 239 lines: 39
    // six-line slices and a final five-line one.
    let rects = tex_rects(&r);
    assert_eq!(rects.len(), 40, "{:?}", r.diags);
    for (i, rect) in rects.iter().enumerate() {
        let uly = i as i32 * 24;
        let lry = (uly + 24).min(956);
        assert_eq!(*rect, (0, uly, 1276, lry, 1 << 10, 1 << 10), "slice {i}");
    }
    assert_eq!(r.rdp.tiles[0].line, 81);
}

/// A `uObjTxtrBlock_t`.
fn txtr_block(image: u32, tsize: u16, tline: u16, sid: u16, flag: u32, mask: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(0x0000_1033u32.to_be_bytes()); // G_OBJLT_TXTRBLOCK
    b.extend(image.to_be_bytes());
    for v in [0u16, tsize, tline, sid] {
        b.extend(v.to_be_bytes());
    }
    b.extend(flag.to_be_bytes());
    b.extend(mask.to_be_bytes());
    b
}

#[test]
fn obj_load_txtr_loads_a_block_and_caches_by_status() {
    // Loads `first`, then `second` with the given sid; returns (first, second, result).
    let run_loads = |second_sid: u16| {
        let (mut first, mut second) = (0, 0);
        let r = run(|b| {
            first = b.bytes(16, &vec![0x11; 32 * 32 * 2]);
            second = b.bytes(16, &vec![0x22; 32 * 32 * 2]);
            // 32x32 RGBA16: tsize = 2048 / 8 - 1, tline = dxt for 64-byte lines.
            let a = b.bytes(
                8,
                &txtr_block(first, 255, 2048 / 64, 0, 0x1234, 0xFFFF_FFFF),
            );
            let c = b.bytes(
                8,
                &txtr_block(second, 255, 2048 / 64, second_sid, 0x1234, 0xFFFF_FFFF),
            );
            let load = |addr| ((G_OBJ_LOADTXTR as u32) << 24 | 23, addr);
            vec![load(a), load(c), END]
        });
        assert!(r.diags.is_empty(), "{:?}", r.diags);
        (u64::from(first), u64::from(second), r)
    };

    // Same sid and flag: the status matches, so the second load is skipped.
    let (first, _, r) = run_loads(0);
    assert_eq!(r.rdp.tex_image.3, first);
    // tex_image keeps the width field (width - 1) of the 256-texel load.
    assert_eq!(r.rdp.tex_image.2, 255);
    assert_eq!((r.rdp.tiles[7].fmt, r.rdp.tiles[7].siz), (0, 2));
    assert!(r.rdp.texture_loaded);

    // Another sid has its own status, so it loads.
    let (_, second, r) = run_loads(4);
    assert_eq!(r.rdp.tex_image.3, second);
}
