//! Media Foundation lifecycle and encoder enumeration.
//!
//! Everything COM-facing that is not the encoder itself lives here, so the
//! `unsafe` surface stays small and reviewable in one sitting.

use std::sync::OnceLock;

use tracing::{debug, warn};
use windows::core::{GUID, PWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFTransform, MFMediaType_Video, MFSTARTUP_FULL, MFStartup, MFTEnumEx,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_FRIENDLY_NAME_Attribute,
    MFT_REGISTER_TYPE_INFO, MFVideoFormat_H264, MFVideoFormat_NV12, MF_VERSION,
};
use windows::Win32::System::Com::CoTaskMemFree;

use crate::{EncoderInfo, HwCodecError, Result};

/// Wrap a `windows` error with the operation that produced it.
pub(crate) fn mf_err(operation: &'static str) -> impl Fn(windows::core::Error) -> HwCodecError {
    move |e| HwCodecError::Mf {
        operation,
        source_message: e.to_string(),
    }
}

/// Start Media Foundation exactly once for the process.
///
/// Deliberately never calls `MFShutdown`. Shutdown is process-wide, so a
/// `Drop`-based pairing here would tear Media Foundation out from under anything
/// else in the process that is still using it — including a second encoder. The
/// OS reclaims it at exit, which is the correct lifetime for a library that does
/// not own the process.
pub(crate) fn ensure_started() -> Result<()> {
    /// `Ok(())` once started; the error text if it failed, so every later caller
    /// gets the same explanation instead of a confusing second failure.
    static STARTED: OnceLock<std::result::Result<(), String>> = OnceLock::new();

    let outcome = STARTED.get_or_init(|| {
        // SAFETY: `MFStartup` takes a version constant and a flag by value and
        // touches no memory we own. `MF_VERSION` is the constant this crate was
        // compiled against, which is what the API requires. Called at most once
        // per process because `OnceLock::get_or_init` runs the closure once.
        let hr = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) };
        match hr {
            Ok(()) => {
                debug!("Media Foundation started");
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    });

    outcome
        .as_ref()
        .map_err(|msg| HwCodecError::Startup(msg.clone()))
        .map(|&()| ())
}

/// An `IMFActivate` array returned by `MFTEnumEx`, freed on drop.
///
/// `MFTEnumEx` allocates the array with `CoTaskMemAlloc` and hands over
/// ownership of both the array and a reference on each element. Getting that
/// wrong leaks a COM object per call, so the cleanup is a destructor rather than
/// something a code path has to remember.
struct ActivateArray {
    /// Base pointer from `MFTEnumEx`. Null when the count is zero.
    ptr: *mut Option<IMFActivate>,
    /// Number of valid elements.
    len: u32,
}

impl ActivateArray {
    /// The activation objects, as a slice.
    fn as_slice(&self) -> &[Option<IMFActivate>] {
        if self.ptr.is_null() || self.len == 0 {
            return &[];
        }
        // SAFETY: `MFTEnumEx` succeeded, so `ptr` points to `len` initialised
        // `Option<IMFActivate>` values in one CoTaskMem allocation, and we have
        // not handed the pointer anywhere else. The borrow is tied to `&self`,
        // so the slice cannot outlive the allocation this type owns.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len as usize) }
    }
}

impl Drop for ActivateArray {
    fn drop(&mut self) {
        if self.ptr.is_null() {
            return;
        }
        // Release each element's reference before freeing the array that holds
        // them, otherwise every enumeration leaks one COM object per encoder.
        for slot in self.as_slice() {
            drop(slot.clone());
        }
        // SAFETY: `ptr` came from `MFTEnumEx`, which allocates with
        // `CoTaskMemAlloc` and transfers ownership to us. It is freed exactly
        // once, here, and `self` is being dropped so nothing can observe the
        // dangling pointer afterwards.
        unsafe {
            CoTaskMemFree(Some(self.ptr.cast()));
        }
        self.ptr = std::ptr::null_mut();
    }
}

/// Enumerate H.264 encoder MFTs, hardware ones first.
pub(crate) fn enumerate_h264_encoders() -> Result<Vec<EncoderInfo>> {
    ensure_started()?;

    // Hardware MFTs first, then software, so the caller's "first hardware entry"
    // choice is also the system's preferred one. Asking for NV12 input keeps the
    // list to encoders that can take a normal video frame.
    let hardware =
        enumerate_with_flags(MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, true)?;
    let software = enumerate_with_flags(MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER, false)?;

    // Deduplicate by name. Drivers register the same encoder more than once —
    // Intel's Quick Sync MFT shows up twice on a single-GPU laptop, once per
    // registration variant — and a list with repeats reads like a bug. Hardware
    // entries are added first, so a name already present keeps its hardware
    // classification, which is what selection depends on.
    let mut out: Vec<EncoderInfo> = Vec::new();
    for candidate in hardware.into_iter().chain(software) {
        if !out.iter().any(|e| e.name == candidate.name) {
            out.push(candidate);
        }
    }
    Ok(out)
}

/// Run one `MFTEnumEx` pass and read the friendly names out.
fn enumerate_with_flags(flags: MFT_ENUM_FLAG, hardware: bool) -> Result<Vec<EncoderInfo>> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };

    let mut ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut len: u32 = 0;

    // SAFETY: the two type-info structs are stack locals that outlive the call,
    // and are only read by it. `ptr` and `len` are writable locals of exactly the
    // types the signature requires; on success Media Foundation stores an owned
    // allocation in `ptr` and its length in `len`, and `ActivateArray` below
    // takes ownership of both so the allocation is freed exactly once.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            Some(&raw const input),
            Some(&raw const output),
            &raw mut ptr,
            &raw mut len,
        )
    }
    .map_err(mf_err("MFTEnumEx"))?;

    let array = ActivateArray { ptr, len };
    let mut out = Vec::with_capacity(len as usize);
    for slot in array.as_slice() {
        let Some(activate) = slot.as_ref() else {
            continue;
        };
        let name = friendly_name(activate).unwrap_or_else(|| "<unnamed MFT>".to_owned());
        debug!(name = %name, hardware, "found H.264 encoder MFT");
        out.push(EncoderInfo { name, hardware });
    }
    Ok(out)
}

/// Activate the first hardware H.264 encoder MFT that will instantiate.
///
/// Enumeration order is the system's preference, and activation can fail even for
/// an enumerated MFT (a busy or disabled adapter, for example), so this walks the
/// list rather than trusting the first entry.
pub(crate) fn activate_hardware_h264() -> Result<(IMFTransform, EncoderInfo)> {
    ensure_started()?;

    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };

    let mut ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut len: u32 = 0;

    // SAFETY: as in `enumerate_with_flags` — the type-info locals outlive the
    // call and are only read; `ptr`/`len` are writable locals of the required
    // types; `ActivateArray` takes ownership of the returned allocation so it is
    // freed exactly once.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&raw const input),
            Some(&raw const output),
            &raw mut ptr,
            &raw mut len,
        )
    }
    .map_err(mf_err("MFTEnumEx"))?;

    let array = ActivateArray { ptr, len };
    for slot in array.as_slice() {
        let Some(activate) = slot.as_ref() else {
            continue;
        };
        let name = friendly_name(activate).unwrap_or_else(|| "<unnamed MFT>".to_owned());

        // SAFETY: `activate` is a live `IMFActivate` borrowed from the array we
        // own, and `ActivateObject` only reads it. The returned interface is
        // reference-counted by `windows`, so ownership is handled for us.
        match unsafe { activate.ActivateObject::<IMFTransform>() } {
            Ok(transform) => {
                debug!(name = %name, "activated hardware H.264 encoder");
                return Ok((transform, EncoderInfo { name, hardware: true }));
            }
            Err(e) => {
                // Not fatal: another adapter may still work.
                warn!(name = %name, error = %e, "hardware MFT failed to activate, trying next");
            }
        }
    }
    Err(HwCodecError::NoEncoder("hardware H.264"))
}

/// Read an MFT's friendly name, or `None` if the driver does not supply one.
fn friendly_name(activate: &IMFActivate) -> Option<String> {
    let mut raw = PWSTR::null();
    let mut chars: u32 = 0;

    // SAFETY: `GetAllocatedString` writes a CoTaskMem-allocated wide string to
    // `raw` and its length to `chars`, both writable locals. The attribute GUID
    // is read-only. On failure it writes nothing, which is why `raw` is only read
    // inside the `Ok` arm.
    let result = unsafe {
        activate.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute as *const GUID, &raw mut raw, &raw mut chars)
    };
    if result.is_err() || raw.is_null() {
        return None;
    }

    // SAFETY: `raw` is a non-null, NUL-terminated wide string that Media
    // Foundation just allocated for us; `to_string` only reads up to the
    // terminator.
    let name = unsafe { raw.to_string() }.ok();

    // SAFETY: the string was allocated by Media Foundation with CoTaskMemAlloc
    // and ownership passed to us. Freed exactly once, after the copy above, and
    // `raw` is not used again.
    unsafe {
        CoTaskMemFree(Some(raw.as_ptr().cast()));
    }
    name
}
