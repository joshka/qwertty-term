//! Wrapper for handling samplers.
//!
//! Port of `src/renderer/metal/Sampler.zig` (commit `2da015cd6`).

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLDevice, MTLSamplerAddressMode, MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerState,
};

use super::MetalError;
use crate::gpu::{SamplerAddressMode, SamplerFilter, SamplerOptions};

/// A Metal sampler state.
pub struct Sampler {
    /// The underlying MTLSamplerState object.
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
}

impl Sampler {
    /// Initialize a sampler. Port of `Sampler.init`.
    pub(super) fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        opts: SamplerOptions,
    ) -> Result<Self, MetalError> {
        // Create our descriptor and set the properties.
        let desc = MTLSamplerDescriptor::new();
        desc.setMinFilter(filter_bits(opts.min_filter));
        desc.setMagFilter(filter_bits(opts.mag_filter));
        desc.setSAddressMode(address_mode_bits(opts.s_address_mode));
        desc.setTAddressMode(address_mode_bits(opts.t_address_mode));

        // Create the sampler state.
        let sampler = device
            .newSamplerStateWithDescriptor(&desc)
            .ok_or(MetalError::MetalFailed)?;

        Ok(Self { sampler })
    }

    /// The underlying MTLSamplerState.
    pub fn sampler(&self) -> &ProtocolObject<dyn MTLSamplerState> {
        &self.sampler
    }
}

impl std::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sampler").finish_non_exhaustive()
    }
}

fn filter_bits(filter: SamplerFilter) -> MTLSamplerMinMagFilter {
    match filter {
        SamplerFilter::Nearest => MTLSamplerMinMagFilter::Nearest,
        SamplerFilter::Linear => MTLSamplerMinMagFilter::Linear,
    }
}

fn address_mode_bits(mode: SamplerAddressMode) -> MTLSamplerAddressMode {
    match mode {
        SamplerAddressMode::ClampToEdge => MTLSamplerAddressMode::ClampToEdge,
        SamplerAddressMode::Repeat => MTLSamplerAddressMode::Repeat,
        SamplerAddressMode::MirrorRepeat => MTLSamplerAddressMode::MirrorRepeat,
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_metal;
    use crate::gpu::{GpuBackend, SamplerAddressMode, SamplerFilter, SamplerOptions};

    #[test]
    fn sampler_creation() {
        let Some(metal) = test_metal() else { return };
        // The upstream call site's configuration ("match Shadertoy
        // behaviors"): linear/linear + clamp-to-edge.
        let sampler = metal
            .new_sampler(SamplerOptions {
                min_filter: SamplerFilter::Linear,
                mag_filter: SamplerFilter::Linear,
                s_address_mode: SamplerAddressMode::ClampToEdge,
                t_address_mode: SamplerAddressMode::ClampToEdge,
            })
            .expect("sampler");
        let _ = sampler.sampler();
    }
}
