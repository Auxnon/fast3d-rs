use super::*;
use crate::{
    profiling::Mode,
    replacement::{ReplacementRegistration, Representation, TextureKeyScheme},
};

fn request(byte: u8) -> EncodedTextureRequest {
    EncodedTextureRequest::from_owned_parts(
        EncodedInput::Tmem(Arc::new([byte; 4096])),
        DecodeRecipe {
            representation: Representation::Tile,
            output: [4, 2],
            logical: [4, 2],
            fmt: 2,
            siz: 1,
            base: 0,
            line: 1,
            palette: 0,
            tlut: 2,
        },
    )
}
fn provider(
    request: &EncodedTextureRequest,
    scheme: TextureKeyScheme,
    status: ReplacementStatus,
) -> Arc<ReplacementProvider> {
    Arc::new(ReplacementProvider::new(
        1,
        [ReplacementRegistration {
            target: ReplacementTarget::for_request(request, scheme, &Recorder::default()).unwrap(),
            status,
        }],
    ))
}
fn ready(value: u8) -> ReplacementStatus {
    ReplacementStatus::Ready(ReplacementImage {
        extent: [4, 2],
        pixels: vec![value; 32].into(),
        format: wgpu::TextureFormat::Rgba8Unorm,
    })
}
fn resolve(
    cache: &mut Replacements<u8>,
    request: &EncodedTextureRequest,
    stats: &Recorder,
) -> Option<(u8, u32)> {
    cache.resolve_with(request, true, 4096, stats, |image| image.pixels[0])
}

#[test]
fn frame_freeze_revision_removal_and_epoch_reset() {
    let a = request(1);
    let stats = Recorder::new(Mode::Counters);
    let mut frames = ProviderFrames::<u8>::default();
    frames.begin_frame();
    assert!(frames.active.is_none());
    assert!(stats.drain().counters.is_empty());
    frames.queue(Some(provider(&a, TextureKeyScheme::Fast3dV1, ready(17))));
    assert!(frames.active.is_none());
    frames.begin_frame();
    assert_eq!(
        resolve(frames.active.as_mut().unwrap(), &a, &stats),
        Some((17, 1))
    );
    frames.queue(Some(provider(&a, TextureKeyScheme::Fast3dV1, ready(91))));
    assert_eq!(
        resolve(frames.active.as_mut().unwrap(), &a, &stats),
        Some((17, 1))
    );
    frames.begin_frame();
    assert_eq!(
        resolve(frames.active.as_mut().unwrap(), &a, &stats),
        Some((91, 1))
    );
    assert_eq!(stats.drain().counters["replacement.provider_calls"], 2);
    frames.queue(None);
    assert!(frames.active.is_some());
    frames.begin_frame();
    assert!(frames.active.is_none());
    frames.queue(Some(provider(&a, TextureKeyScheme::Fast3dV1, ready(17))));
    frames = ProviderFrames::default();
    frames.begin_frame();
    assert!(frames.active.is_none());
}

#[test]
fn decisions_cache_hits_misses_pending_and_aliases_until_revision_change() {
    let a = request(1);
    let b = request(2);
    let stats = Recorder::new(Mode::Counters);
    for status in [ready(17), ReplacementStatus::Pending] {
        let mut cache = Replacements::new(provider(&a, TextureKeyScheme::Rt64V5, status.clone()));
        for query in [&b, &a, &b, &a] {
            resolve(&mut cache, query, &stats);
        }
        let counts = stats.drain().counters;
        assert_eq!(counts["replacement.provider_calls"], 4);
        assert_eq!(counts["replacement.v5_hashes"], 2);
        assert_eq!(counts["replacement.decision_hits"], 2);
        assert_eq!(counts["replacement.no_match"], 1);
        assert_eq!(
            counts.get("replacement.upload_calls").copied().unwrap_or(0),
            u64::from(matches!(status, ReplacementStatus::Ready(_)))
        );
    }
}

#[test]
fn eviction_reinsertion_and_full_palette_slot_invalidation() {
    let a = request(1);
    let b = request(2);
    let stats = Recorder::new(Mode::Counters);
    let mut cache = Replacements::new(provider(&a, TextureKeyScheme::Rt64V5, ready(17)));
    cache.entry_limit = 1;
    assert_eq!(resolve(&mut cache, &b, &stats), None);
    assert_eq!(resolve(&mut cache, &a, &stats), Some((17, 1)));
    assert_eq!(resolve(&mut cache, &b, &stats), None);
    assert_eq!(resolve(&mut cache, &a, &stats), Some((17, 1)));
    assert_eq!(stats.drain().counters["replacement.evictions"], 3);
    let mut bank = [1; 4096];
    bank[2058] = 99;
    let c = EncodedTextureRequest::from_owned_parts(
        EncodedInput::Tmem(Arc::new(bank)),
        a.recipe().clone(),
    );
    assert_eq!(a.key(), c.key());
    assert_eq!(resolve(&mut cache, &c, &stats), None);
    assert_eq!(stats.drain().counters["replacement.v5_hashes"], 1);
}

#[test]
fn oversized_and_zero_entry_decisions_bypass_without_losing_owned_pixels() {
    let a = request(1);
    let stats = Recorder::new(Mode::Counters);
    let mut cache = Replacements::new(provider(&a, TextureKeyScheme::Fast3dV1, ready(17)));
    cache.entry_limit = 0;
    for _ in 0..2 {
        assert_eq!(resolve(&mut cache, &a, &stats), Some((17, 1)));
    }
    let counts = stats.drain().counters;
    assert_eq!(counts["replacement.bypasses"], 2);
    assert_eq!(counts["replacement.upload_calls"], 2);
    assert!(cache.decisions.is_empty());
    let mut recipe = a.recipe().clone();
    recipe.output = [2048, 1024];
    recipe.logical = recipe.output;
    let large = EncodedTextureRequest::from_owned_parts(a.input().clone(), recipe);
    let mut cache = Replacements::new(provider(
        &large,
        TextureKeyScheme::Fast3dV1,
        ReplacementStatus::Ready(ReplacementImage {
            extent: [2048, 1024],
            pixels: vec![71; 8 * 1024 * 1024].into(),
            format: wgpu::TextureFormat::Rgba8Unorm,
        }),
    ));
    assert_eq!(resolve(&mut cache, &large, &stats), Some((71, 1)));
    assert_eq!(stats.drain().counters["replacement.bypasses"], 1);
    assert!(cache.decisions.is_empty());
}

#[test]
fn raw_tlut_on_non_ci_blocks_alias_even_with_a_warm_allowed_decision() {
    let r = DecodeRecipe {
        representation: Representation::Tile,
        output: [4, 2],
        logical: [4, 2],
        fmt: 4,
        siz: 1,
        base: 0,
        line: 1,
        palette: 0,
        tlut: 0,
    };
    let request =
        EncodedTextureRequest::from_owned_parts(EncodedInput::Tmem(Arc::new([17; 4096])), r);
    let mut cache = Replacements::new(provider(&request, TextureKeyScheme::Rt64V5, ready(71)));
    let stats = Recorder::new(Mode::Counters);
    assert_eq!(
        cache.resolve_with(&request, true, 4096, &stats, |image| image.pixels[0]),
        Some((71, 1))
    );
    stats.drain();
    assert_eq!(
        cache.resolve_with(&request, false, 4096, &stats, |image| image.pixels[0]),
        None
    );
    let counts = stats.drain().counters;
    assert_eq!(counts["replacement.provider_calls"], 1);
    assert_eq!(counts.get("replacement.v5_hashes"), None);
}
