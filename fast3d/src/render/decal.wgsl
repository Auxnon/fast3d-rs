// decal.wgsl — base depth-reading fragment entry (E2): combiner + depth_read.wgsl's in-shader
// tests (ZMODE_DEC occlusion + coplanar discard, or a gEXVertexZTest).
// Assembled in lib.rs AFTER combiner_prelude.wgsl + skeleton.wgsl + depth_read.wgsl (it adds the
// depth-reading entry `fs_decal` alongside skeleton's `fs_main`). Bound to the Replace/AlphaOver
// decal pipelines (decal layout g0+g1+g2), which run in the pass with NO depth attachment.

@fragment
fn fs_decal(in: VsOut) -> @location(0) vec4<f32> {
    let r = eval_combiner(in);
    clip_depth(in);
    var rgb = r.rgb;
    // C3: fog mix — applied when cyc1 blender P == CLR_FOG (3). Matches skeleton.wgsl/fs_main.
    let p1 = (combiner.blender_mux >> 14u) & 3u;
    if (p1 == 3u) {
        rgb = mix(rgb, combiner.fog_color.rgb, in.color.a);
    }
    alpha_discard(r, in.clip_position.xy);

    depth_read_tests(in);
    return vec4<f32>(rgb, r.alpha);
}
