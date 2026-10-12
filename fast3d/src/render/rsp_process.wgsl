// rsp_process.wgsl — per-vertex RSP transform (F3DEX2 RSP-process stage). Writes pos + color + uv.
// pos: clip = mvp*v (transpose-on-upload reproduces CPU row-vector), w==0 guard, viewport fold.
// uv: (s*sc)/DIVISOR / max(tile,1) — F3DEX2 texcoord scale/divisor; the /tile
// normalize is the extra step matching hle set_vertex.
// color: diffuse lighting (light_count>0) or cn RGBA passthrough (unlit).

// Must mirror RspProcessParams (render/mod.rs) exactly — 16 bytes.
struct Params { vertex_count: u32, _pad: u32, fb_width: f32, fb_height: f32 };
// Must mirror rsp_buffers::SrcVertex (render/mod.rs) exactly — 80 bytes.
struct SrcVertex {
    pos: vec3<f32>,
    st: vec2<f32>,
    mtx_index: u32,
    viewport_index: u32,
    texcoord_index: u32,
    cn: u32,
    light_index: u32,
    light_count: u32,
    lookat_index: u32,
    texgen_mode: u32,
    fog: u32,
    modify_flags: u32,
    modify_screen: vec4<f32>,
};
struct GpuViewport { scale: vec4<f32>, trans: vec4<f32> };
struct GpuTexcoord { scale_s: f32, scale_t: f32, texgen_scale_s: f32, texgen_scale_t: f32 };
struct GpuLight { dir: vec4<f32>, col: vec4<f32> };
struct GpuLookAt { axis_s: vec4<f32>, axis_t: vec4<f32> };
struct OutVertex { pos: vec4<f32>, color: vec4<f32>, uv: vec2<f32> }; // std430 stride 48

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> src: array<SrcVertex>;
@group(0) @binding(2) var<storage, read> mvp_table: array<mat4x4<f32>>;
@group(0) @binding(3) var<storage, read> viewport_table: array<GpuViewport>;
@group(0) @binding(4) var<storage, read> texcoord_table: array<GpuTexcoord>;
@group(0) @binding(5) var<storage, read> lights: array<GpuLight>;
@group(0) @binding(6) var<storage, read> lookat: array<GpuLookAt>;
@group(0) @binding(7) var<storage, read_write> out: array<OutVertex>;
@group(0) @binding(8) var<storage, read> fog_table: array<vec2<f32>>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let vi = gid.x;
    if (vi >= params.vertex_count) { return; }

    let v = src[vi];
    let mvp = mvp_table[v.mtx_index];
    let vp = viewport_table[v.viewport_index];
    let tc = texcoord_table[v.texcoord_index];

    // Row-major upload read column-major by WGSL IS the transpose, so `mvp * pos` reproduces
    // the CPU `mul_row_vec4(pos, mvp)`.
    let clip = mvp * vec4<f32>(v.pos, 1.0);
    // --- Deferred RSP stages land here (post-transform): lighting (color) / texgen (uv override).
    var w = clip.w;
    if (w == 0.0) { w = 1e-6; } // w==0 guard before the viewport fold

    var o: OutVertex;
    o.pos = vec4<f32>(
        clip.x * (2.0 * vp.scale.x / params.fb_width)  + w * (2.0 * vp.trans.x / params.fb_width  - 1.0),
        clip.y * (2.0 * vp.scale.y / params.fb_height) + w * (1.0 - 2.0 * vp.trans.y / params.fb_height),
        clip.z * vp.scale.z + w * vp.trans.z,
        w,
    );
    // gSPModifyVertex screen overrides. Rebuild clip = ndc*w so the GPU's perspective divide
    // lands on the requested pixel/depth while keeping the shader-computed w for correct UVs.
    if ((v.modify_flags & 1u) != 0u) {
        o.pos.x = (2.0 * v.modify_screen.x / params.fb_width - 1.0) * w;
        o.pos.y = (1.0 - 2.0 * v.modify_screen.y / params.fb_height) * w;
    }
    if ((v.modify_flags & 2u) != 0u) {
        o.pos.z = v.modify_screen.z * w;
    }
    // No near clipping (.NoN microcodes): keep geometry in front of the near plane by clamping
    // depth to it instead of clipping. RT64 clamps per fragment (depth clip off); clamping per
    // vertex differs only inside triangles that cross the near plane. Far clipping is unchanged.
    if ((v.modify_flags & 4u) != 0u) {
        o.pos.z = max(o.pos.z, 0.0);
    }
    // Prefolded scale (f64-computed CPU-side): one f32 multiply per axis (spec §2 Precision).
    o.uv = vec2<f32>(v.st.x * tc.scale_s, v.st.y * tc.scale_t);

    // Hoist the s8 normal to function scope: shared by lighting AND texgen (one source).
    // Raw normals are s8 packed in bits 0..23 of cn (LSB = nx). extract_bits(cn, offset, count)
    // returns the unsigned bits; casting to i32 then back to f32 gives the signed interpretation
    // without renormalization.
    var n: vec3<f32> = vec3<f32>(0.0);
    if (v.light_count > 0u || v.texgen_mode != 0u) {
        n = vec3<f32>(
            f32(i32(extractBits(v.cn,  0u, 8u) << 24u) >> 24),
            f32(i32(extractBits(v.cn,  8u, 8u) << 24u) >> 24),
            f32(i32(extractBits(v.cn, 16u, 8u) << 24u) >> 24)) / 127.0;
    }

    // Lighting / color
    let cn = v.cn;
    let alpha = f32((cn >> 24u) & 0xffu) / 255.0;
    let lc = v.light_count;
    if lc > 0u {
        let li = v.light_index;
        // Ambient = last light entry
        let ambient = lights[li + lc - 1u];
        var c = ambient.col.rgb;
        // F3DZEX2 point lights: ambient.dir.w = 1 means the modelview rows follow the set.
        var r0 = vec3<f32>(0.0);
        var r1 = vec3<f32>(0.0);
        var r2 = vec3<f32>(0.0);
        var r3 = vec3<f32>(0.0);
        if (ambient.dir.w > 0.5) {
            let a = lights[li + lc];
            let b = lights[li + lc + 1u];
            r0 = a.dir.xyz;
            r1 = a.col.xyz;
            r2 = b.dir.xyz;
            r3 = b.col.xyz;
        }
        // Diffuse accumulation
        for (var k: u32 = 0u; k < lc - 1u; k++) {
            let l = lights[li + k];
            if (l.dir.w > 0.0) {
                // Point light (RT64's computePosLight, which matches the microcode): direction
                // and distance in model/world space, distance with the RSP's doubled Z term.
                let world = v.pos.x * r0 + v.pos.y * r1 + v.pos.z * r2 + r3;
                var d = l.dir.xyz - world;
                let dist = sqrt(d.x * d.x + d.y * d.y + 2.0 * d.z * d.z);
                if (dist > 0.0) { d = d / dist; }
                // Into object space, unnormalized (M·d), like the directional prefold.
                let local = vec3<f32>(dot(r0, d), dot(r1, d), dot(r2, d));
                let nl = clamp(dot(n, local) * 4.0, 0.0, 1.0);
                let kc = l.dir.w;
                let kl = floor(l.col.w / 256.0);
                let kq = l.col.w - kl * 256.0;
                var att = 1.0 / (0.5 + kc / 16.0 + kl * dist / 32768.0 + kq * dist * dist / 524288.0);
                att = att % (1.0 + 1e-6);
                c += nl * att * l.col.rgb;
            } else {
                let nl = max(dot(n, l.dir.xyz), 0.0);
                c += nl * l.col.rgb;
            }
        }
        o.color = vec4<f32>(min(c, vec3<f32>(1.0)), alpha);
    } else {
        // Unlit: RGBA passthrough from cn bytes
        let r = f32(cn & 0xffu) / 255.0;
        let g = f32((cn >> 8u) & 0xffu) / 255.0;
        let b = f32((cn >> 16u) & 0xffu) / 255.0;
        o.color = vec4<f32>(r, g, b, alpha);
    }

    // C2: Fog factor — write shade alpha from raw clip-Z (F3DEX2 RSP fog stage).
    // Uses `clip` (the raw mvp*pos result) and `w` (guarded clip.w), NOT the viewport-folded
    // `o.pos.z` (which folds in vp.scale.z / vp.trans.z and gives wrong depth for fog).
    // Gate PER-VERTEX (fog indices): only vertices loaded with G_FOG get the fog factor, so
    // unfogged overlay geometry (HUD / dialog box) keeps its real alpha.
    if (v.fog != 0u) {
        let fz = max(clip.z, 0.0) / w;
        let factors = fog_table[v.fog - 1u];
        let fog_alpha = clamp(fz * factors.x + factors.y, 0.0, 255.0) / 255.0;
        o.color.a = fog_alpha;
    }

    if (v.texgen_mode != 0u) {
        let lk = lookat[v.lookat_index];
        let ds = clamp(dot(n, lk.axis_s.xyz), -1.0, 1.0);
        let dt = clamp(dot(n, lk.axis_t.xyz), -1.0, 1.0);
        var gs = (ds + 1.0) * 512.0;
        var gt = (dt + 1.0) * 512.0;
        if (v.texgen_mode == 2u) {
            gs = acos(-ds) * (1024.0 / 3.141592653589793);
            gt = acos(-dt) * (1024.0 / 3.141592653589793);
        }
        o.uv = vec2<f32>(gs * tc.texgen_scale_s, gt * tc.texgen_scale_t);
    }

    out[vi] = o;
}
