//! Runtime-compile smoke test for the embedded MSL (chunk R3 requirement):
//! `newLibraryWithSource:options:error:` on [`super::SOURCE`] must succeed,
//! and all 5 ported function names ([`super::PORTED_FUNCTION_NAMES`]) must
//! resolve via `newFunctionWithName:`.
//!
//! This test compiles the library itself using `objc2-metal` directly — it
//! does not depend on, or coordinate with, chunk R2's frame/pipeline work
//! (which owns `metal/mod.rs`'s `Metal` context type; this module is
//! read-only with respect to that file and constructs its own throwaway
//! device via `MTLCreateSystemDefaultDevice`, matching plan decision 6:
//! "runtime `newLibraryWithSource` first").
//!
//! Skips gracefully (prints `SKIP:` and returns) when no Metal device is
//! available, matching the pattern chunk R1 established
//! (`metal::test_metal`) for CI machines without a GPU.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary};

/// Grab a throwaway system-default Metal device for this test only. Chunk
/// R1's `metal::test_metal()` additionally filters out headless GPUs
/// (relevant when *rendering*), but this test only compiles a shader
/// library — any device, including a headless one, can do that — so it
/// deliberately doesn't reach into R1/R2's `metal` module.
fn any_metal_device() -> Option<Retained<ProtocolObject<dyn MTLDevice>>> {
    MTLCreateSystemDefaultDevice()
}

#[test]
fn embedded_msl_compiles_and_exposes_ported_functions() {
    let Some(device) = any_metal_device() else {
        eprintln!("SKIP: no Metal device available; skipping shader-compile smoke test");
        return;
    };

    let source = NSString::from_str(super::SOURCE);
    let library = match device.newLibraryWithSource_options_error(&source, None) {
        Ok(library) => library,
        Err(err) => panic!(
            "newLibraryWithSource:options:error: failed to compile ghostty.metal: {}",
            err.localizedDescription()
        ),
    };

    for name in super::PORTED_FUNCTION_NAMES {
        let fn_name = NSString::from_str(name);
        assert!(
            library.newFunctionWithName(&fn_name).is_some(),
            "compiled library is missing expected function `{name}`"
        );
    }
}
