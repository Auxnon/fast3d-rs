use std::{collections::VecDeque, sync::Arc};

use super::texture_resources::RgbaImage;
use crate::{
    hle::texture_request::EncodedTextureRequest,
    profiling::Recorder,
    replacement::{
        DecodeRecipe, EncodedInput, ReplacementImage, ReplacementProvider, ReplacementStatus,
        ReplacementTarget,
    },
};

struct Decision<T> {
    v5_allowed: bool,
    target: ReplacementTarget,
    recipe: DecodeRecipe,
    input: EncodedInput,
    image: Option<(T, u32)>,
    cpu_bytes: usize,
    gpu_bytes: u64,
}

pub(super) struct Replacements<T = Arc<RgbaImage>> {
    pub provider: Arc<ReplacementProvider>,
    decisions: VecDeque<Decision<T>>,
    entry_limit: usize,
    cpu_bytes: usize,
    gpu_bytes: u64,
}

impl<T: Clone> Replacements<T> {
    pub fn new(provider: Arc<ReplacementProvider>) -> Self {
        Self {
            provider,
            decisions: VecDeque::new(),
            entry_limit: 256,
            cpu_bytes: 0,
            gpu_bytes: 0,
        }
    }

    pub fn resolve_with(
        &mut self,
        request: &EncodedTextureRequest,
        v5_allowed: bool,
        max_dimension: u32,
        profiling: &Recorder,
        upload: impl FnOnce(&ReplacementImage) -> T,
    ) -> Option<(T, u32)> {
        let existing = self.decisions.iter().position(|decision| {
            decision.v5_allowed == v5_allowed
                && decision.target.extent() == request.recipe().output
                && decision.recipe == *request.recipe()
                && input_equal(&decision.input, request.input(), profiling)
        });
        if let Some(index) = existing {
            profiling.count("replacement.decision_hits", 1);
            let decision = self.decisions.remove(index).unwrap();
            let image = decision.image.clone();
            self.decisions.push_back(decision);
            return image;
        }
        profiling.count("replacement.decision_misses", 1);
        let (target, status) = self.provider.resolve(request, v5_allowed, profiling);
        let mut gpu_bytes = 0;
        let image = match status {
            Some(ReplacementStatus::Ready(image)) => match image.scale(&target, max_dimension) {
                Ok(scale) => {
                    profiling.count("replacement.matches", 1);
                    profiling.count("replacement.upload_calls", 1);
                    profiling.count("replacement.upload_bytes", image.pixels.len() as u64);
                    gpu_bytes = image.pixels.len() as u64;
                    Some((upload(image), scale))
                }
                Err(error) => {
                    profiling.count("replacement.invalid", 1);
                    log::warn!(
                        "invalid texture replacement at revision {}: {error:?}",
                        self.provider.revision()
                    );
                    None
                }
            },
            Some(ReplacementStatus::Pending) => {
                profiling.count("replacement.pending", 1);
                None
            }
            None => {
                profiling.count("replacement.no_match", 1);
                None
            }
        };
        let input_bytes = match request.input() {
            EncodedInput::Tmem(_) => 4096,
            EncodedInput::LinearCompat { bytes, .. } => 4096 + bytes.len(),
        };
        let cpu_bytes = input_bytes + target.witness().len() + gpu_bytes as usize;
        if self.entry_limit != 0 && cpu_bytes <= 8 * 1024 * 1024 && gpu_bytes <= 64 * 1024 * 1024 {
            while self.decisions.len() >= self.entry_limit
                || self.cpu_bytes + cpu_bytes > 8 * 1024 * 1024
                || self.gpu_bytes + gpu_bytes > 64 * 1024 * 1024
            {
                let removed = self.decisions.pop_front().unwrap();
                self.cpu_bytes -= removed.cpu_bytes;
                self.gpu_bytes -= removed.gpu_bytes;
                profiling.count("replacement.evictions", 1);
            }
            self.decisions.push_back(Decision {
                v5_allowed,
                target,
                recipe: request.recipe().clone(),
                input: request.input().clone(),
                image: image.clone(),
                cpu_bytes,
                gpu_bytes,
            });
            self.cpu_bytes += cpu_bytes;
            self.gpu_bytes += gpu_bytes;
        } else {
            profiling.count("replacement.bypasses", 1);
        }
        profiling.gauge("replacement.cached_entries", self.decisions.len() as u64);
        profiling.gauge("replacement.cached_cpu_bytes", self.cpu_bytes as u64);
        profiling.gauge("replacement.cached_gpu_bytes", self.gpu_bytes);
        image
    }
}

pub(super) struct ProviderFrames<T = Arc<RgbaImage>> {
    pub active: Option<Box<Replacements<T>>>,
    queued: Option<Option<Arc<ReplacementProvider>>>,
}

impl<T> Default for ProviderFrames<T> {
    fn default() -> Self {
        Self {
            active: None,
            queued: None,
        }
    }
}

impl<T: Clone> ProviderFrames<T> {
    pub fn queue(&mut self, provider: Option<Arc<ReplacementProvider>>) {
        self.queued = Some(provider);
    }
    pub fn begin_frame(&mut self) {
        if let Some(provider) = self.queued.take() {
            self.active = provider.map(|provider| Box::new(Replacements::new(provider)));
        }
    }
}

fn input_equal(a: &EncodedInput, b: &EncodedInput, profiling: &Recorder) -> bool {
    let equal = |a: &[u8], b: &[u8]| {
        profiling.count("replacement.input_bytes_compared", a.len() as u64);
        a == b
    };
    match (a, b) {
        (EncodedInput::Tmem(a), EncodedInput::Tmem(b)) => {
            Arc::ptr_eq(a, b) || equal(&a[..], &b[..])
        }
        (
            EncodedInput::LinearCompat {
                bytes: a,
                palette_bank: pa,
            },
            EncodedInput::LinearCompat {
                bytes: b,
                palette_bank: pb,
            },
        ) => (Arc::ptr_eq(a, b) || equal(a, b)) && (Arc::ptr_eq(pa, pb) || equal(&pa[..], &pb[..])),
        _ => false,
    }
}

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct Lifetime {
    live: Vec<std::sync::Weak<RgbaImage>>,
    current: Vec<Arc<RgbaImage>>,
    submissions: Vec<(Arc<std::sync::atomic::AtomicBool>, Vec<Arc<RgbaImage>>)>,
}

impl Lifetime {
    pub fn created(&mut self, image: &Arc<RgbaImage>) {
        self.live.push(Arc::downgrade(image));
    }
    pub fn used(&mut self, image: &Arc<RgbaImage>) {
        self.current.push(image.clone());
    }
    pub fn submitted(&mut self, queue: &wgpu::Queue) {
        if self.current.is_empty() {
            return;
        }
        let complete = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback = complete.clone();
        queue.on_submitted_work_done(move || {
            callback.store(true, std::sync::atomic::Ordering::Release)
        });
        self.submissions
            .push((complete, std::mem::take(&mut self.current)));
    }
    pub fn collect(&mut self) {
        self.submissions
            .retain(|(complete, _)| !complete.load(std::sync::atomic::Ordering::Acquire));
        self.live.retain(|image| image.strong_count() != 0);
    }
    pub fn profile(&self, stats: &Recorder, cached: Option<&Replacements>) {
        let cached: std::collections::HashSet<_> = cached
            .into_iter()
            .flat_map(|cache| &cache.decisions)
            .filter_map(|decision| decision.image.as_ref().map(|(image, _)| Arc::as_ptr(image)))
            .collect();
        let mut transient_gpu = 0;
        let mut pinned_gpu = 0;
        let mut pixels = std::collections::HashSet::new();
        let mut gpu = 0;
        let mut cpu = 0;
        let mut entries = 0;
        for image in self.live.iter().filter_map(std::sync::Weak::upgrade) {
            entries += 1;
            gpu += image.source.len() as u64;
            let resident = cached.contains(&Arc::as_ptr(&image));
            if !resident {
                transient_gpu += image.source.len() as u64;
            }
            if Arc::strong_count(&image) > 1 + usize::from(resident) {
                pinned_gpu += image.source.len() as u64;
            }
            if pixels.insert(Arc::as_ptr(&image.source)) {
                cpu += image.source.len() as u64;
            }
        }
        let mut pinned = std::collections::HashSet::new();
        let bytes = self
            .current
            .iter()
            .chain(self.submissions.iter().flat_map(|(_, images)| images))
            .filter(|image| pinned.insert(Arc::as_ptr(image)))
            .map(|image| image.source.len() as u64)
            .sum();
        stats.gauge("replacement.transient_gpu_bytes", transient_gpu);
        stats.gauge("replacement.pinned_gpu_bytes", pinned_gpu);
        let overhead = self.live.capacity() * std::mem::size_of::<std::sync::Weak<RgbaImage>>()
            + self.current.capacity() * std::mem::size_of::<Arc<RgbaImage>>()
            + self.submissions.capacity()
                * std::mem::size_of::<(Arc<std::sync::atomic::AtomicBool>, Vec<Arc<RgbaImage>>)>()
            + self
                .submissions
                .iter()
                .map(|(_, images)| images.capacity() * std::mem::size_of::<Arc<RgbaImage>>())
                .sum::<usize>();
        stats.gauge(
            "replacement.lifetime_allocation_overhead_bytes",
            overhead as u64,
        );
        stats.gauge("replacement.live_entries", entries);
        stats.gauge("replacement.live_gpu_bytes", gpu);
        stats.gauge("replacement.live_pixel_cpu_bytes", cpu);
        stats.gauge("replacement.in_flight_gpu_bytes", bytes);
        stats.gauge("replacement.in_flight_entries", pinned.len() as u64);
    }
}

impl<T: Clone> Replacements<T> {
    pub fn profile(&self, stats: &Recorder) {
        stats.gauge("replacement.cached_entries", self.decisions.len() as u64);
        stats.gauge("replacement.cached_cpu_bytes", self.cpu_bytes as u64);
        stats.gauge("replacement.cached_gpu_bytes", self.gpu_bytes);
        stats.gauge(
            "replacement.cache_allocation_overhead_bytes",
            (self.decisions.capacity() * std::mem::size_of::<Decision<T>>()) as u64,
        );
        self.provider.profile(stats);
    }
}
