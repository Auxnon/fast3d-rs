//! S2DEX2 object commands: 2D backgrounds (`gSPBgRectCopy`, `gSPBgRect1Cyc`) and object
//! texture loads (`gSPObjLoadTxtr`).
//!
//! Ported from RT64's `rt64_gbi_s2dex.cpp`, which reproduces the microcode's TMEM slicing:
//! a background bigger than TMEM is drawn as a series of texture rectangles, each after
//! loading the image rows it needs. These run as the same RDP tile/load/rectangle operations
//! the commands would issue, so materials, TMEM and framebuffer aliasing apply unchanged.
//!
//! As in RT64's fast path, a background whose image is a known framebuffer (Zelda's pause
//! screen draws the previous frame this way) is drawn as one rectangle sampling that
//! framebuffer, since fast3d keeps framebuffers on the GPU.

// Image formats and sizes (libultra G_IM_FMT_* / G_IM_SIZ_*).
const G_IM_FMT_RGBA: u8 = 0;
const G_IM_FMT_CI: u8 = 2;
#[allow(non_upper_case_globals)]
const G_IM_SIZ_16b: u8 = 2;
#[allow(non_upper_case_globals)]
const G_IM_SIZ_32b: u8 = 3;
use crate::hle::interp::{memory_try, Cmd, Ctx};
use crate::hle::mem::{MemoryError, Rdram};

const G_TX_RENDERTILE: u32 = 0;
const G_TX_LOADTILE: u32 = 7;
const G_TX_MIRROR: u32 = 1;
/// `uObjBg_t::imageLoad` values.
const G_BGLT_LOADBLOCK: u16 = 0x0033;
const G_BGLT_LOADTILE: u16 = 0xFFF4;
const G_BG_FLAG_FLIPS: u16 = 0x01;
/// `G_OBJ_RENDERMODE` flag: the microcode loads an extra row/column for bilinear filtering.
const G_OBJRM_BILERP: u32 = 0x08;

/// The fields shared by `uObjBg_t` (copy) and `uObjScaleBg_t` (1-cycle), big-endian.
#[derive(Clone, Copy, Debug, Default)]
struct Background {
    image_x: u16,
    image_w: u16,
    frame_x: i16,
    frame_w: u16,
    image_y: u16,
    image_h: u16,
    frame_y: i16,
    frame_h: u16,
    image_ptr: u32,
    image_load: u16,
    image_fmt: u8,
    image_siz: u8,
    image_pal: u16,
    image_flip: u16,
    /// `uObjBg_t`: TMEM height (copy); `uObjScaleBg_t`: horizontal scale (s5.10).
    tmem_h_or_scale_w: u16,
    /// `uObjBg_t`: (unused here); `uObjScaleBg_t`: vertical scale (s5.10).
    scale_h: u16,
    /// `uObjScaleBg_t` only.
    image_y_orig: i32,
}

fn read_u32<M: Rdram>(mem: &M, addr: u64) -> Result<u32, MemoryError> {
    Ok((u32::from(mem.read_u16(addr)?) << 16) | u32::from(mem.read_u16(addr + 2)?))
}

fn read_background<M: Rdram>(mem: &M, addr: u64) -> Result<Background, MemoryError> {
    let u16_at = |off: u64| mem.read_u16(addr + off);
    Ok(Background {
        image_x: u16_at(0)?,
        image_w: u16_at(2)?,
        frame_x: mem.read_i16(addr + 4)?,
        frame_w: u16_at(6)?,
        image_y: u16_at(8)?,
        image_h: u16_at(10)?,
        frame_y: mem.read_i16(addr + 12)?,
        frame_h: u16_at(14)?,
        image_ptr: read_u32(mem, addr + 16)?,
        image_load: u16_at(20)?,
        image_fmt: mem.read_u8(addr + 22)?,
        image_siz: mem.read_u8(addr + 23)?,
        image_pal: u16_at(24)?,
        image_flip: u16_at(26)?,
        // uObjBg_t: tmemW@28, tmemH@30; uObjScaleBg_t: scaleW@28, scaleH@30, imageYorig@32.
        tmem_h_or_scale_w: u16_at(28)?,
        scale_h: u16_at(30)?,
        image_y_orig: read_u32(mem, addr + 32)? as i32,
    })
}

/// The copy-mode `uObjBg_t` names `tmemH` the field at offset 30.
fn tmem_h(bg: &Background) -> u16 {
    bg.scale_h
}

// RDP operations, issued through the regular RDP handlers.

fn rdp<M: Rdram>(cx: &mut Ctx<M>, handler: fn(&Cmd, &mut Ctx<M>), (w0, w1): (u32, u32)) {
    handler(
        &Cmd {
            w0,
            w1,
            w1_addr: u64::from(w1),
        },
        cx,
    );
}

fn set_texture_image<M: Rdram>(cx: &mut Ctx<M>, fmt: u32, siz: u32, width: u32, addr: u32) {
    rdp(
        cx,
        crate::hle::rsp_f3dex2::set_texture_image,
        n64_gbi::encode::gdp_set_texture_image(fmt, siz, width.max(1), addr),
    );
}

#[allow(clippy::too_many_arguments)]
fn set_tile<M: Rdram>(
    cx: &mut Ctx<M>,
    tile: u32,
    fmt: u32,
    siz: u32,
    line: u32,
    tmem: u32,
    palette: u32,
    (cm, mask, shift): (u32, u32, u32),
) {
    rdp(
        cx,
        crate::hle::rdp::set_tile,
        n64_gbi::encode::gdp_set_tile(
            fmt, siz, line, tmem, tile, palette, cm, mask, shift, cm, mask, shift,
        ),
    );
}

fn set_tile_size<M: Rdram>(cx: &mut Ctx<M>, tile: u32, uls: i32, ult: i32, lrs: i32, lrt: i32) {
    rdp(
        cx,
        crate::hle::rdp::set_tile_size,
        n64_gbi::encode::gdp_set_tile_size(tile, uls as u32, ult as u32, lrs as u32, lrt as u32),
    );
}

fn load_tile<M: Rdram>(cx: &mut Ctx<M>, tile: u32, uls: i32, ult: i32, lrs: i32, lrt: i32) {
    let w0 = (u32::from(crate::hle::consts::G_LOADTILE) << 24)
        | ((uls as u32 & 0xFFF) << 12)
        | (ult as u32 & 0xFFF);
    let w1 = (tile << 24) | ((lrs as u32 & 0xFFF) << 12) | (lrt as u32 & 0xFFF);
    rdp(cx, crate::hle::rdp::load_tile, (w0, w1));
}

fn load_block<M: Rdram>(cx: &mut Ctx<M>, tile: u32, lrs: u32, dxt: u32) {
    let w0 = u32::from(crate::hle::consts::G_LOADBLOCK) << 24;
    let w1 = (tile << 24) | ((lrs & 0xFFF) << 12) | (dxt & 0xFFF);
    rdp(cx, crate::hle::rdp::load_block, (w0, w1));
}

fn load_tlut<M: Rdram>(cx: &mut Ctx<M>, tile: u32, lrs: u32) {
    let w0 = u32::from(crate::hle::consts::G_LOADTLUT) << 24;
    let w1 = (tile << 24) | ((lrs & 0xFFF) << 12);
    rdp(cx, crate::hle::rdp::load_tlut, (w0, w1));
}

#[allow(clippy::too_many_arguments)]
fn draw_tex_rect<M: Rdram>(
    cx: &mut Ctx<M>,
    ulx: i32,
    uly: i32,
    lrx: i32,
    lry: i32,
    tile: u32,
    uls: i16,
    ult: i16,
    dsdx: i16,
    dtdy: i16,
    framebuffer_image: bool,
) {
    crate::hle::interp::record_texrect(
        cx.rsp,
        cx.rdp,
        cx.scene,
        cx.rec,
        cx.diags,
        cx.dropped_runs,
        cx.pc,
        crate::hle::TexRectBounds { ulx, uly, lrx, lry },
        (tile as u8, uls, ult, dsdx, dtdy, false),
        framebuffer_image,
    );
}

/// Whether `image` (a segmented address) is the start of a known framebuffer, so the
/// background can sample it on the GPU instead of loading RDRAM through TMEM.
fn is_framebuffer<M: Rdram>(
    cx: &Ctx<M>,
    image: u32,
    fmt: u8,
    siz: u8,
    width: u32,
    height: u32,
) -> bool {
    let Ok(address) = cx.mem.resolve(u64::from(image)) else {
        return false;
    };
    let layout = crate::render::framebuffers::ImageLayout { width, fmt, siz };
    !matches!(
        cx.rec
            .framebuffers
            .targets
            .source(address, cx.rdp.color_image.addr, layout, height.max(1)),
        Ok(None)
    )
}

fn unsupported_parameters<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) {
    *cx.dropped_runs += 1;
    cx.diags.push(crate::diag::Diagnostic {
        at: cx.pc,
        kind: crate::diag::DiagKind::UnsupportedCommandParameters { opcode: c.opcode() },
    });
}

/// `G_BG_COPY` (gSPBgRectCopy): draw an unscaled background in copy mode, in TMEM-high
/// strips.
pub(crate) fn bg_copy<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) {
    let addr = memory_try!(cx, Command, cx.mem.resolve_masked(c.w1_addr));
    let bg = memory_try!(cx, Command, read_background(cx.mem, addr));
    // Only the load-tile layout is implemented (also true of RT64).
    if bg.image_load != G_BGLT_LOADTILE {
        unsupported_parameters(c, cx);
        return;
    }
    let fmt = u32::from(bg.image_fmt);
    let siz = u32::from(bg.image_siz);
    let line_shift_offset = u32::from(siz == u32::from(G_IM_SIZ_32b));
    let bg_line = u32::from(bg.image_w) >> (2 + siz - line_shift_offset);
    let dsdx: i16 = 4 << 10;
    let dsdy: i16 = 4 << 8;
    let lr_subtract = 4;
    let lrs = i32::from(bg.image_w) - lr_subtract;
    let pal = u32::from(bg.image_pal);
    set_texture_image(cx, fmt, siz, u32::from(bg.image_w) >> 2, bg.image_ptr);
    let wrap = (0, 0, 0);
    set_tile(cx, G_TX_LOADTILE, fmt, siz, bg_line, 0, pal, wrap);
    set_tile(cx, G_TX_RENDERTILE, fmt, siz, bg_line, 0, pal, wrap);

    let frame_x = i32::from(bg.frame_x);
    let frame_y = i32::from(bg.frame_y);
    let frame_lrx = frame_x + i32::from(bg.frame_w) - lr_subtract;

    // A framebuffer image: one rectangle, sampling the framebuffer.
    if is_framebuffer(
        cx,
        bg.image_ptr,
        bg.image_fmt,
        bg.image_siz,
        u32::from(bg.image_w) >> 2,
        u32::from(bg.image_h) >> 2,
    ) {
        let lrt = i32::from(bg.image_h) - lr_subtract;
        set_tile_size(cx, G_TX_RENDERTILE, 0, 0, lrs, lrt);
        let lry = frame_y + i32::from(bg.frame_h) - lr_subtract;
        draw_tex_rect(
            cx,
            frame_x,
            frame_y,
            frame_lrx,
            lry,
            G_TX_RENDERTILE,
            0,
            0,
            dsdx,
            dsdy,
            true,
        );
        return;
    }

    let division = if fmt == u32::from(G_IM_FMT_CI) { 2 } else { 1 };
    let strip = i32::from(tmem_h(&bg)) / division;
    if strip <= 0 {
        unsupported_parameters(c, cx);
        return;
    }
    // TODO (from RT64): the last strip's remaining rows aren't drawn.
    let count = i32::from(bg.image_h) / strip;
    let lrt = strip - lr_subtract;
    set_tile_size(cx, G_TX_RENDERTILE, 0, 0, lrs, lrt);
    for r in 0..count {
        let ult = r * strip;
        load_tile(cx, G_TX_LOADTILE, 0, ult, lrs, ult + lrt);
        draw_tex_rect(
            cx,
            frame_x,
            frame_y + ult,
            frame_lrx,
            frame_y + ult + lrt,
            G_TX_RENDERTILE,
            0,
            0,
            dsdx,
            dsdy,
            false,
        );
    }
}

/// `bg1CycTMEMLoadTile`: load `load_lines` image rows into TMEM.
fn bg1_load_tile<M: Rdram>(
    cx: &mut Ctx<M>,
    bg: &Background,
    image_ptr: u32,
    src_wsize: u16,
    load_lines: i16,
    tmem_sh: i16,
) {
    // (RT64: "Does it make sense that when using 32-bit images, the lrt is 0?")
    let is32 = u32::from(bg.image_siz) == u32::from(G_IM_SIZ_32b);
    let siz = if is32 { G_IM_SIZ_32b } else { G_IM_SIZ_16b };
    let lrt = if is32 {
        0
    } else {
        (i32::from(load_lines) << 2) - 1
    };
    set_texture_image(
        cx,
        u32::from(G_IM_FMT_RGBA),
        u32::from(siz),
        u32::from(src_wsize >> 1),
        image_ptr,
    );
    load_tile(cx, G_TX_LOADTILE, 0, 0, (i32::from(tmem_sh) - 1) << 4, lrt);
}

#[allow(clippy::too_many_arguments)]
fn bg1_set_and_load_tile<M: Rdram>(
    cx: &mut Ctx<M>,
    bg: &Background,
    image_ptr: u32,
    src_wsize: u16,
    load_lines: i16,
    slice_wmax: u16,
    tmem_addr: i16,
    tmem_sh: i16,
) {
    let siz = if u32::from(bg.image_siz) == u32::from(G_IM_SIZ_32b) {
        G_IM_SIZ_32b
    } else {
        G_IM_SIZ_16b
    };
    set_tile(
        cx,
        G_TX_LOADTILE,
        u32::from(G_IM_FMT_RGBA),
        u32::from(siz),
        u32::from(slice_wmax),
        tmem_addr as u32 & 0x1FF,
        0,
        (0, 0, 0),
    );
    bg1_load_tile(cx, bg, image_ptr, src_wsize, load_lines, tmem_sh);
}

/// `bg1CycTMEMLoad`: load the rows for one slice, splitting the load where the slice wraps
/// around the bottom of the image or past its right edge.
#[allow(clippy::too_many_arguments)]
fn bg1_load<M: Rdram>(
    cx: &mut Ctx<M>,
    bg: &Background,
    image_top: u32,
    image_ptr: &mut u32,
    image_remain: &mut i16,
    src_wsize: u16,
    ptr_x0: u16,
    tmem_src_lines: i16,
    slice_wmax: u16,
    draw_lines: i16,
    uses_bilerp: i16,
    flag_split: i16,
) {
    let load_lines = draw_lines.wrapping_add(uses_bilerp);
    let loadable = image_remain.wrapping_sub(flag_split);

    if loadable >= load_lines {
        // Everything at once.
        bg1_load_tile(cx, bg, *image_ptr, src_wsize, load_lines, slice_wmax as i16);
        *image_ptr = image_ptr.wrapping_add(u32::from(src_wsize) * draw_lines as i32 as u32);
        *image_remain = image_remain.wrapping_sub(draw_lines);
        return;
    }

    // Partitioned load.
    let top_seg = image_top & 0xFF00_0000;
    let mut sub_y2 = *image_remain;
    let mut sub_l2 = load_lines.wrapping_sub(sub_y2);
    let sub_d2 = draw_lines.wrapping_sub(sub_y2);
    if sub_l2 > 0 {
        let mut ptr2 = image_top.wrapping_add(u32::from(ptr_x0));
        if sub_y2 & 1 != 0 {
            ptr2 = ptr2.wrapping_sub(u32::from(src_wsize));
            ptr2 = top_seg | (ptr2 & 0x00FF_FFFF);
            sub_y2 -= 1;
            sub_l2 += 1;
        }
        let tmem = (i32::from(sub_y2) * i32::from(slice_wmax)) as i16;
        bg1_set_and_load_tile(
            cx,
            bg,
            ptr2,
            src_wsize,
            sub_l2,
            slice_wmax,
            tmem,
            slice_wmax as i16,
        );
    }

    if flag_split != 0 {
        let mut ptr1a = image_ptr.wrapping_add((i32::from(loadable) * i32::from(src_wsize)) as u32);
        let mut ptr1b = image_top;
        let mut sub_y1 = loadable;
        let sub_l1_odd = loadable & 1;
        if sub_l1_odd != 0 {
            ptr1a = ptr1a.wrapping_sub(u32::from(src_wsize));
            ptr1b = ptr1b.wrapping_sub(u32::from(src_wsize));
            ptr1b = top_seg | (ptr1b & 0x00FF_FFFF);
            sub_y1 -= 1;
        }
        let sub_l1 = sub_l1_odd + 1;
        let tmem_sh_a = ((i32::from(src_wsize) - i32::from(ptr_x0)) >> 3) as i16;
        let tmem_sh_b = (slice_wmax as i16).wrapping_sub(tmem_sh_a);
        let base = (i32::from(sub_y1) * i32::from(slice_wmax)) as i16;
        bg1_set_and_load_tile(
            cx,
            bg,
            ptr1b,
            src_wsize,
            sub_l1,
            slice_wmax,
            base.wrapping_add(tmem_sh_a),
            tmem_sh_b,
        );
        bg1_set_and_load_tile(
            cx, bg, ptr1a, src_wsize, sub_l1, slice_wmax, base, tmem_sh_a,
        );
    }

    if loadable > 0 {
        bg1_set_and_load_tile(
            cx,
            bg,
            *image_ptr,
            src_wsize,
            loadable,
            slice_wmax,
            0,
            slice_wmax as i16,
        );
    } else {
        let siz = if u32::from(bg.image_siz) == u32::from(G_IM_SIZ_32b) {
            G_IM_SIZ_32b
        } else {
            G_IM_SIZ_16b
        };
        set_tile(
            cx,
            G_TX_LOADTILE,
            u32::from(G_IM_FMT_RGBA),
            u32::from(siz),
            u32::from(slice_wmax),
            0,
            0,
            (0, 0, 0),
        );
    }

    *image_remain = image_remain.wrapping_sub(draw_lines);
    if *image_remain > 0 {
        *image_ptr = image_ptr.wrapping_add((i32::from(src_wsize) * i32::from(draw_lines)) as u32);
    } else {
        *image_remain = tmem_src_lines.wrapping_sub(sub_d2);
        *image_ptr = image_top
            .wrapping_add((i32::from(sub_d2) * i32::from(src_wsize)) as u32)
            .wrapping_add(u32::from(ptr_x0));
    }
}

/// `G_BG_1CYC` (gSPBgRect1Cyc): draw a scaled background in 1-cycle mode, slice by slice.
pub(crate) fn bg_1cyc<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) {
    let addr = memory_try!(cx, Command, cx.mem.resolve_masked(c.w1_addr));
    let bg = memory_try!(cx, Command, read_background(cx.mem, addr));
    // The retail microcode treats load-block like load-tile (per RT64).
    if bg.image_load != G_BGLT_LOADTILE && bg.image_load != G_BGLT_LOADBLOCK {
        unsupported_parameters(c, cx);
        return;
    }

    // The scale is at least 1.
    let scale_w = i32::from(bg.tmem_h_or_scale_w.max(1));
    let scale_h = i32::from(bg.scale_h.max(1));

    // Clamp the frame to the image's area at this scale.
    let mut frame_wmax = (i32::from(bg.image_w) << 10) / scale_w;
    let mut frame_hmax = (i32::from(bg.image_h) << 10) / scale_h;
    frame_wmax = (frame_wmax - 1) & !0x3;
    frame_hmax = (frame_hmax - 1) & !0x3;
    let mut frame_w = bg.frame_w as i16;
    let mut frame_h = bg.frame_h as i16;
    frame_wmax = (i32::from(bg.frame_w) - frame_wmax).max(0);
    frame_hmax = (i32::from(bg.frame_h) - frame_hmax).max(0);
    frame_w = frame_w.wrapping_sub(frame_wmax as i16);
    frame_h = frame_h.wrapping_sub(frame_hmax as i16);

    let mut frame_x0 = bg.frame_x;
    let mut frame_y0 = bg.frame_y;
    let flip_s = bg.image_flip & G_BG_FLAG_FLIPS != 0;
    if flip_s {
        frame_x0 = frame_x0.wrapping_add(frame_wmax as i16);
    }

    // Cut away whatever is outside the scissor (10.2).
    let scissor = cx.rdp.scissor;
    let (sx0, sy0, sx1, sy1) = (
        scissor.ulx * 4,
        scissor.uly * 4,
        scissor.lrx * 4,
        scissor.lry * 4,
    );
    let pix_x0 = (sx0 - i32::from(frame_x0)).max(0) as i16;
    let pix_y0 = (sy0 - i32::from(frame_y0)).max(0) as i16;
    let pix_x1 = (i32::from(frame_w) - sx1 + i32::from(frame_x0)).max(0) as i16;
    let pix_y1 = (i32::from(frame_h) - sy1 + i32::from(frame_y0)).max(0) as i16;
    frame_w = frame_w.wrapping_sub(pix_x0.wrapping_add(pix_x1));
    frame_h = frame_h.wrapping_sub(pix_y0.wrapping_add(pix_y1));
    frame_x0 = frame_x0.wrapping_add(pix_x0);
    frame_y0 = frame_y0.wrapping_add(pix_y0);
    if frame_w <= 0 || frame_h <= 0 {
        return;
    }

    let frame_x1 = frame_x0.wrapping_add(frame_w);
    let mut frame_ptr_y0 = frame_y0 >> 2;
    let mut frame_remain = frame_h >> 2;
    let image_src_w = (bg.image_w as i16).wrapping_shl(3);
    let image_src_h = (bg.image_h as i16).wrapping_shl(3);

    // Bilinear filtering needs one more row and column. (RT64 optionally ignores the flag
    // when the RDP isn't filtering; that's an enhancement, so it's kept here.)
    let uses_bilerp: i16 = i16::from(cx.rsp.obj_render_mode & G_OBJRM_BILERP != 0);

    let image_w = ((i32::from(frame_w) * scale_w) >> 7) as i16;
    let image_slice_w = image_w.wrapping_add(uses_bilerp * 32);
    let mut image_x0 = if flip_s {
        (bg.image_x as i16).wrapping_add(((i32::from(pix_x1) * scale_w) >> 7) as i16)
    } else {
        (bg.image_x as i16).wrapping_add(((i32::from(pix_x0) * scale_w) >> 7) as i16)
    };
    let mut image_y0 =
        (bg.image_y as i16).wrapping_add(((i32::from(pix_y0) * scale_h) >> 7) as i16);
    let mut image_y_orig = bg.image_y_orig;

    // Scroll down a row while the left edge is past the image's width, and wrap the top.
    if image_src_w > 0 {
        while image_x0 >= image_src_w {
            image_x0 -= image_src_w;
            image_y0 = image_y0.wrapping_add(32);
            image_y_orig += 32;
        }
    }
    if image_src_h > 0 {
        while image_y0 >= image_src_h {
            image_y0 -= image_src_h;
            image_y_orig -= i32::from(image_src_h);
        }
    }

    // TMEM loads split where the slice crosses the image's right edge.
    let flag_split = i16::from(image_x0.wrapping_add(image_slice_w) >= image_src_w);
    let tmem_src_lines = image_src_h >> 5;

    // TMEM usable for the image (half for CI, whose palette takes the upper half).
    let mut tmem_size: i16 = if bg.image_fmt == G_IM_FMT_CI {
        256
    } else {
        512
    };
    let tmem_shift = (0x200 >> u32::from(bg.image_siz).min(9)) as i16;
    let tmem_mask = tmem_shift - 1;
    let image_slice_wmax: i32 = if u32::from(bg.image_siz) == u32::from(G_IM_SIZ_32b) {
        tmem_size = 480;
        0x2800
    } else {
        let w = (i32::from(bg.frame_w) * scale_w) >> 7;
        (w + (i32::from(uses_bilerp) << 5)).min(i32::from(image_src_w))
    };

    // How many lines fit in TMEM at once.
    let slice_wmax = ((image_slice_wmax + i32::from(tmem_mask)) / i32::from(tmem_shift) + 1) as u16;
    let tmem_slice_lines = tmem_size / slice_wmax as i16;
    let image_slice_lines = tmem_slice_lines - uses_bilerp;
    let frame_slice_lines = (i32::from(image_slice_lines) << 20) / scale_h;
    if frame_slice_lines <= 0 || tmem_src_lines <= 0 {
        unsupported_parameters(c, cx);
        return;
    }

    // The first line, from the image coordinates.
    let mut image_ly_offset = (i32::from(image_y0) - image_y_orig) << 5;
    if image_ly_offset < 0 {
        image_ly_offset -= scale_h - 1;
    }
    let frame_ly_offset = (image_ly_offset / scale_h) << 10;
    let image_num_slice = if frame_ly_offset >= 0 {
        frame_ly_offset / frame_slice_lines
    } else {
        (frame_ly_offset - frame_slice_lines + 1) / frame_slice_lines
    };

    // How much of the first rectangle is hidden above the frame.
    let mut frame_l_slice_l0 = frame_slice_lines * image_num_slice;
    let frame_ly_slice = frame_l_slice_l0 & !0x3FF;
    let frame_l_hidden = frame_ly_offset - frame_ly_slice;
    let mut image_l_hidden = (frame_l_hidden >> 10) * scale_h;
    frame_l_slice_l0 = (frame_l_slice_l0 & 0x3FF) + frame_slice_lines - frame_l_hidden;

    let mut image_t = ((image_l_hidden >> 5) & 0x1F) as i16;
    image_l_hidden >>= 10;
    let image_i_slice_l0 = (i32::from(image_slice_lines) - image_l_hidden) as i16;
    let mut image_iy0 = (i32::from(image_slice_lines) * image_num_slice
        + (image_y_orig & !0x1F) / 32
        + image_l_hidden) as i16;
    let image_h_lowered = (bg.image_h >> 2) as i16;
    if image_iy0 < 0 {
        image_iy0 = image_iy0.wrapping_add(image_h_lowered);
    }
    if image_iy0 >= image_h_lowered {
        image_iy0 = image_iy0.wrapping_sub(image_h_lowered);
    }

    let image_address = bg.image_ptr;
    let src_wsize = ((i32::from(image_src_w) / i32::from(tmem_shift)) << 3) as u16;
    let ptr_x0 = ((i32::from(image_x0) / i32::from(tmem_shift)) << 3) as u16;
    let mut image_ptr = image_address
        .wrapping_add((i32::from(src_wsize) * i32::from(image_iy0)) as u32)
        .wrapping_add(u32::from(ptr_x0));
    let mut image_s = (image_x0 & tmem_mask) as u16 as i16;
    if flip_s {
        image_s = image_s.wrapping_add(image_w).wrapping_neg();
    }

    // Tiles that stay the same for the whole image.
    let load_siz = if u32::from(bg.image_siz) == u32::from(G_IM_SIZ_32b) {
        G_IM_SIZ_32b
    } else {
        G_IM_SIZ_16b
    };
    set_tile(
        cx,
        G_TX_LOADTILE,
        u32::from(G_IM_FMT_RGBA),
        u32::from(load_siz),
        u32::from(slice_wmax),
        0,
        0,
        (0, 0, 0),
    );
    set_tile(
        cx,
        G_TX_RENDERTILE,
        u32::from(bg.image_fmt),
        u32::from(bg.image_siz),
        u32::from(slice_wmax),
        0,
        u32::from(bg.image_pal),
        (G_TX_MIRROR, 0xF, 0),
    );

    // A framebuffer image: one rectangle over the whole frame, sampling the framebuffer.
    if is_framebuffer(
        cx,
        image_address,
        bg.image_fmt,
        bg.image_siz,
        u32::from(bg.image_w) >> 2,
        u32::from(bg.image_h) >> 2,
    ) {
        let uls = i32::from(image_s);
        let ult = i32::from(image_t);
        set_texture_image(
            cx,
            u32::from(G_IM_FMT_RGBA),
            u32::from(load_siz),
            u32::from(src_wsize >> 1),
            image_ptr,
        );
        // The tile covers the image's whole pixels: a fractional size (Majora's Mask uses
        // 320.25 x 240.25) would reach a row and column past the framebuffer.
        set_tile_size(
            cx,
            G_TX_RENDERTILE,
            uls,
            ult,
            uls + (i32::from(bg.image_w) & !3) - 4,
            ult + (i32::from(bg.image_h) & !3) - 4,
        );
        let uly = i32::from(frame_y0) & !0x3;
        draw_tex_rect(
            cx,
            i32::from(frame_x0),
            uly,
            i32::from(frame_x1),
            uly + i32::from(frame_h),
            G_TX_RENDERTILE,
            image_s,
            image_t,
            scale_w as i16,
            scale_h as i16,
            true,
        );
        return;
    }

    set_tile_size(cx, G_TX_RENDERTILE, 0, 0, 0, 0);

    let mut image_remain = tmem_src_lines.wrapping_sub(image_iy0);
    let mut image_slice_h = image_i_slice_l0;
    let mut frame_slice_count = frame_l_slice_l0;
    // Guard against malformed parameters looping forever.
    let mut guard = 0u32;
    while frame_remain > 0 {
        guard += 1;
        if guard > 4096 {
            unsupported_parameters(c, cx);
            return;
        }
        let mut frame_slice_h = (frame_slice_count >> 10) as i16;
        if frame_slice_h <= 0 {
            image_remain = image_remain.wrapping_sub(image_slice_h);
            if image_remain > 0 {
                image_ptr = image_ptr
                    .wrapping_add((i32::from(src_wsize) * i32::from(image_slice_h)) as u32);
            } else {
                image_ptr = image_address
                    .wrapping_sub((i32::from(image_remain) * i32::from(src_wsize)) as u32)
                    .wrapping_add(u32::from(ptr_x0));
                image_remain = image_remain.wrapping_add(tmem_src_lines);
            }
        } else {
            frame_slice_count &= 0x3FF;
            frame_remain -= frame_slice_h;

            // The final slice.
            if frame_remain < 0 {
                frame_slice_h += frame_remain;
                image_slice_h = image_slice_h
                    .wrapping_add(((i32::from(frame_remain) * scale_h) >> 10) as i16)
                    .wrapping_add(1);
                image_slice_h = image_slice_h.min(image_slice_lines);
            }

            bg1_load(
                cx,
                &bg,
                image_address,
                &mut image_ptr,
                &mut image_remain,
                src_wsize,
                ptr_x0,
                tmem_src_lines,
                slice_wmax,
                image_slice_h,
                uses_bilerp,
                flag_split,
            );

            let frame_ptr_y1 = frame_ptr_y0 + frame_slice_h;
            draw_tex_rect(
                cx,
                i32::from(frame_x0),
                i32::from(frame_ptr_y0) << 2,
                i32::from(frame_x1),
                i32::from(frame_ptr_y1) << 2,
                G_TX_RENDERTILE,
                image_s,
                image_t,
                scale_w as i16,
                scale_h as i16,
                false,
            );
            frame_ptr_y0 = frame_ptr_y1;
        }
        frame_slice_count += frame_slice_lines;
        image_slice_h = image_slice_lines;
        image_t = 0;
    }
}

/// `readS2DStruct`: DMA an object struct into the persistent S2DEX struct buffer.
fn read_struct<M: Rdram>(cx: &mut Ctx<M>, c: &Cmd) -> Result<(), MemoryError> {
    let addr = cx.mem.resolve_masked(c.w1_addr)?;
    // The microcode truncates the size to a byte.
    let size = ((c.w0 & 0xFF_FFFF) + 1) as usize & 0xFF;
    for i in 0..size {
        cx.rsp.s2d_struct[i] = cx.mem.read_u8(addr + i as u64)?;
    }
    Ok(())
}

/// `doLoadTxtr`: load a texture described by the `uObjTxtr` at the start of the struct
/// buffer, unless its status already matches (`sid`/`flag`/`mask` caching).
fn load_txtr<M: Rdram>(cx: &mut Ctx<M>) {
    let b = cx.rsp.s2d_struct;
    let be32 = |off: usize| u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]);
    let be16 = |off: usize| u16::from_be_bytes([b[off], b[off + 1]]);
    let kind = be32(0);
    let image = be32(4);
    let tmem = u32::from(be16(8));
    let val1 = u32::from(be16(10));
    let val2 = u32::from(be16(12));
    let sid = (be16(14) as u8) as usize;
    let flag = be32(16);
    let mask = be32(20);

    // sid is 0, 4, 8 or 12.
    let slot = (sid / 4).min(3);
    let status = cx.rsp.s2d_statuses[slot];
    if status & mask == flag {
        return;
    }
    cx.rsp.s2d_statuses[slot] = (status & !mask) | (flag & mask);

    let tsize = val1;
    set_texture_image(
        cx,
        u32::from(G_IM_FMT_RGBA),
        u32::from(G_IM_SIZ_16b),
        tsize + 1,
        image,
    );
    // Combine the fields as the RSP does, then decode them as the RDP would.
    let type_mask = ((kind >> 16) as u8 as i8) as i32 as u32;
    let part1 = ((tsize + 1) & type_mask) << 7;
    let part2 = u32::from((kind >> 8) as u8) << 16;
    let settile_w0 = tmem | part1 | part2;
    set_tile(
        cx,
        G_TX_LOADTILE,
        (settile_w0 >> 21) & 0x7,
        (settile_w0 >> 19) & 0x3,
        (settile_w0 >> 9) & 0x1FF,
        settile_w0 & 0x1FF,
        0,
        (0, 0, 0),
    );
    let load_w1 = ((tsize << 14) | val2) & 0x00FF_FFFF;
    let lrs = (load_w1 >> 12) & 0xFFF;
    let low = load_w1 & 0xFFF;
    // The RDP ignores the command id's top two bits.
    match kind & 0x3F {
        x if x == u32::from(crate::hle::consts::G_LOADBLOCK) & 0x3F => {
            load_block(cx, G_TX_LOADTILE, lrs, low)
        }
        x if x == u32::from(crate::hle::consts::G_LOADTILE) & 0x3F => {
            load_tile(cx, G_TX_LOADTILE, 0, 0, lrs as i32, low as i32)
        }
        x if x == u32::from(crate::hle::consts::G_LOADTLUT) & 0x3F => {
            load_tlut(cx, G_TX_LOADTILE, lrs)
        }
        _ => cx.diags.push(crate::diag::Diagnostic {
            at: cx.pc,
            kind: crate::diag::DiagKind::UnsupportedCommandParameters {
                opcode: crate::hle::consts::rsp_s2dex2::G_OBJ_LOADTXTR,
            },
        }),
    }
}

/// `G_OBJ_LOADTXTR` (gSPObjLoadTxtr).
pub(crate) fn obj_load_txtr<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) {
    memory_try!(cx, Texture, read_struct(cx, c));
    load_txtr(cx);
}

/// `G_OBJ_LDTX_SPRITE`/`RECT`/`RECT_R`: the texture load is done; the sprite or rectangle
/// draw isn't implemented (nor in RT64).
pub(crate) fn obj_load_txtr_and_draw<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) {
    memory_try!(cx, Texture, read_struct(cx, c));
    load_txtr(cx);
    *cx.dropped_runs += 1;
    cx.diags.push(crate::diag::Diagnostic {
        at: cx.pc,
        kind: crate::diag::DiagKind::UnsupportedCommand {
            opcode: c.opcode(),
            w0: c.w0,
            w1: c.w1_addr,
        },
    });
}
