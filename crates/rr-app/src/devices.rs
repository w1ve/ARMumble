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
    /// The devices named in the settings. Second devices only apply to a
    /// two-radio station that has one sound card per radio.
    #[must_use]
    pub fn from_settings(a: &AudioSettings) -> Self {
        let set = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
        let two_cards = a.so2r_enabled && a.so2r_role.eq_ignore_ascii_case("station") && !a.so2r_single_device;
        Self {
            input_a: set(&a.input_device),
            input_b: if two_cards { set(&a.input_device_b) } else { None },
            output_a: set(&a.output_device),
            output_b: if two_cards { set(&a.output_device_b) } else { None },
        }
    }
}
