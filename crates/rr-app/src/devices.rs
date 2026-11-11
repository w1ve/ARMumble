//! The set of sound devices the engine opens.

use crate::settings::AudioSettings;

/// Device ids for one session. `None` for an A device means the system
/// default; `None` for a B device means it is not used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Devices {
    /// Input A (radio 1).
    pub input_a: Option<String>,
    /// Input B (radio 2), if used.
    pub input_b: Option<String>,
    /// Output A (radio 1).
    pub output_a: Option<String>,
    /// Output B (radio 2), if used.
    pub output_b: Option<String>,
}

impl Devices {
    /// The devices named in the settings.
    #[must_use]
    pub fn from_settings(a: &AudioSettings) -> Self {
        let set = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
        Self {
            input_a: set(&a.input_device),
            input_b: set(&a.input_device_b),
            output_a: set(&a.output_device),
            output_b: set(&a.output_device_b),
        }
    }
}
