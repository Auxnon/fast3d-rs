//! G_FILLRECT outside fill mode: in 1- and 2-cycle modes the rectangle goes through the
//! combiner and blender (Majora's Mask fades the screen with a translucent primitive color),
//! and its lower-right edge is exclusive.

use crate::render::workload::TargetId;
use crate::render::{headless_device_forced_fallback, SceneRenderer};
use crate::tests::common::pixels_from_render;
use crate::tests::dl_builder::DlBuilder;
use crate::ClearPolicy;
use n64_gbi::{consts::*, encode::*};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const CIMG: u32 = 0x0010_0000;
/// Translucent surface: `G_RM_XLU_SURF` without the coverage and Z bits.
const XLU: u32 = IM_RD | FORCE_BL | gbl_c1(CLR_IN, A_IN, CLR_MEM, B_1MA);
const XLU2: u32 = gbl_c2(CLR_IN, A_IN, CLR_MEM, B_1MA);

/// White, then a black rectangle at half alpha over (32, 32)-(288, 208), filled in `cycle`.
fn render(cycle: u32) -> Vec<u8> {
    let prim = CcPass {
        a: ZERO_C,
        b: ZERO_C,
        c: ZERO_C,
        d: 3,
    };
    let prim_alpha = CcPass {
        a: ZERO_A,
        b: ZERO_A,
        c: ZERO_A,
        d: 3,
    };
    let mut b = DlBuilder::new();
    b.list(
        "main",
        &[
            gdp_set_color_image(0, 2, 320, CIMG),
            gdp_set_scissor(0, 0, 0, 1280, 960),
            gdp_set_cycle_type(3),
            gdp_set_fill_color(0xffff_ffff),
            gdp_fill_rectangle(0, 0, 319 * 4, 239 * 4),
            gdp_pipe_sync(),
            gdp_set_cycle_type(cycle),
            gdp_set_render_mode(XLU, XLU2),
            gdp_set_combine_lerp(prim, prim_alpha, prim, prim_alpha),
            gdp_set_prim_color(0, 0, 0x0000_0080),
            gdp_fill_rectangle(32 * 4, 32 * 4, 288 * 4, 208 * 4),
            gdp_pipe_sync(),
            (0xe900_0000, 0),
            gsp_enddl(),
        ],
    );
    let built = b.finish("main");
    let result = crate::hle::interpret_rdram(&built.rdram, built.entry);
    assert!(result.diags.is_empty(), "{:?}", result.diags);
    let (device, queue) = headless_device_forced_fallback();
    let mut renderer = SceneRenderer::new(&device, FORMAT, 320, 240, false);
    renderer.render_into_store(&device, &queue, &result.scene, ClearPolicy::PerFrame);
    pixels_from_render(&device, &queue, 320, 240, FORMAT, |view| {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        renderer.scanout(
            &device,
            &mut encoder,
            view,
            TargetId::Guest(CIMG.into()),
            None,
        );
        queue.submit(Some(encoder.finish()));
    })
}

fn pixel(pixels: &[u8], x: usize, y: usize) -> [u8; 4] {
    pixels.as_chunks::<4>().0[y * 320 + x]
}

#[test]
fn one_and_two_cycle_fill_rectangles_blend_the_primitive_color() {
    for cycle in [0, 1] {
        let pixels = render(cycle);
        let white = pixel(&pixels, 4, 4);
        assert_eq!(&white[..3], &[255; 3], "cycle {cycle}");
        // Half of white, give or take the 5-bit framebuffer.
        let inside = pixel(&pixels, 160, 120);
        for channel in &inside[..3] {
            assert!((115..=140).contains(channel), "cycle {cycle}: {inside:?}");
        }
        // The lower-right edge is exclusive outside fill mode.
        assert_eq!(pixel(&pixels, 287, 207), inside, "cycle {cycle}");
        assert_eq!(pixel(&pixels, 288, 120), white, "cycle {cycle}");
        assert_eq!(pixel(&pixels, 160, 208), white, "cycle {cycle}");
    }
}
