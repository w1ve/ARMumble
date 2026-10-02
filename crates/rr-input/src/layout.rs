//! Mapping monitor-relative coordinates to the virtual desktop.
//!
//! Wire coordinates are deliberately **monitor-relative**: an
//! [`rr_proto::InputEvent::MouseMove`] says "x pixels from the left edge of
//! monitor 2", not "x pixels from the left edge of the virtual desktop". That way
//! the Client never needs to know how the Station's displays are arranged, and a
//! Station that rearranges its monitors mid-session does not invalidate the
//! Client's idea of where things are.
//!
//! Injection, though, needs absolute virtual-desktop coordinates. This module is
//! that translation, and it is pure arithmetic so the whole of it is testable with
//! no display attached — which matters, because multi-monitor coordinate bugs are
//! miserable to diagnose live and trivial to assert here.
//!
//! # Clamping is a security property, not tidiness
//!
//! [`MonitorLayout::to_absolute`] clamps to the addressed monitor's rectangle. A
//! controller can only see the monitor being shared, so it must not be able to
//! drive the pointer onto one it cannot see. Without the clamp, an out-of-range
//! coordinate — whether from a bug, a resolution change mid-stream, or a
//! deliberately malformed message — would move the pointer onto an unshared
//! display and click on something invisible to the person doing it.

use rr_proto::MonitorInfo as ProtoMonitor;

/// Where one monitor sits on the virtual desktop, in physical pixels.
///
/// Origins can be negative: Windows places the primary monitor at `(0, 0)` and
/// anything to its left or above gets negative coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorPlacement {
    /// Monitor id, matching what the Station advertised.
    pub id: u32,
    /// Virtual-desktop X of the monitor's left edge.
    pub x: i32,
    /// Virtual-desktop Y of the monitor's top edge.
    pub y: i32,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

impl MonitorPlacement {
    /// Clamp a monitor-relative point into this monitor's bounds.
    ///
    /// Returns virtual-desktop coordinates. An empty monitor (zero width or
    /// height) clamps to its origin rather than producing a negative extent.
    #[must_use]
    pub fn clamp_to_absolute(&self, x: i32, y: i32) -> (i32, i32) {
        // `width - 1` because the rightmost addressable pixel is one inside the
        // edge; clamping to `width` would place the pointer on the next monitor.
        let max_x = i32::try_from(self.width.saturating_sub(1)).unwrap_or(i32::MAX);
        let max_y = i32::try_from(self.height.saturating_sub(1)).unwrap_or(i32::MAX);
        let local_x = x.clamp(0, max_x);
        let local_y = y.clamp(0, max_y);
        (
            self.x.saturating_add(local_x),
            self.y.saturating_add(local_y),
        )
    }

    /// Whether a monitor-relative point is inside this monitor without clamping.
    ///
    /// Used to tell "the controller addressed a point on this monitor" from "the
    /// point was out of range and we moved it", which is worth logging.
    #[must_use]
    pub fn contains_local(&self, x: i32, y: i32) -> bool {
        x >= 0
            && y >= 0
            && u32::try_from(x).is_ok_and(|ux| ux < self.width)
            && u32::try_from(y).is_ok_and(|uy| uy < self.height)
    }
}

/// The Station's monitor arrangement.
///
/// Supplied by the caller rather than discovered here: the Station already
/// enumerates its monitors to advertise them, and keeping this crate free of
/// platform enumeration is what makes the arithmetic testable headless.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonitorLayout {
    /// Known monitors. Small enough that a linear scan beats a map.
    monitors: Vec<MonitorPlacement>,
}

impl MonitorLayout {
    /// Build a layout from placements.
    #[must_use]
    pub fn new(monitors: Vec<MonitorPlacement>) -> Self {
        Self { monitors }
    }

    /// Build a single-monitor layout at the origin.
    ///
    /// The common case, and it keeps callers that have one display from having to
    /// invent coordinates.
    #[must_use]
    pub fn single(id: u32, width: u32, height: u32) -> Self {
        Self::new(vec![MonitorPlacement {
            id,
            x: 0,
            y: 0,
            width,
            height,
        }])
    }

    /// Monitors in this layout.
    #[must_use]
    pub fn monitors(&self) -> &[MonitorPlacement] {
        &self.monitors
    }

    /// Whether the layout knows about any monitors at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.monitors.is_empty()
    }

    /// Look up one monitor.
    #[must_use]
    pub fn get(&self, id: u32) -> Option<&MonitorPlacement> {
        self.monitors.iter().find(|m| m.id == id)
    }

    /// Translate a monitor-relative point to virtual-desktop coordinates.
    ///
    /// Returns `None` for an unknown monitor id — an event addressed to a monitor
    /// the Station does not have is refused rather than guessed at, because
    /// guessing would inject a click somewhere nobody asked for.
    ///
    /// Points outside the monitor are **clamped**, not rejected: a pointer at the
    /// edge is a normal consequence of the viewer's window being a different size,
    /// whereas dropping the event would make the pointer appear to stick.
    #[must_use]
    pub fn to_absolute(&self, monitor: u32, x: i32, y: i32) -> Option<(i32, i32)> {
        self.get(monitor).map(|m| m.clamp_to_absolute(x, y))
    }

    /// Build a layout from the monitor list a Station advertises.
    ///
    /// [`rr_proto::MonitorInfo`] carries each monitor's virtual-desktop origin in
    /// the same physical-pixel space injection uses, so this is a direct copy with
    /// no inference. The Station reads those origins from the platform when it
    /// enumerates its displays, which is the only place they are actually known.
    ///
    /// Earlier versions of the wire format had no origins, and this function laid
    /// the monitors out left to right in the order given. That is right for one
    /// monitor and for the common side-by-side pair, and wrong for stacked, offset
    /// or right-to-left arrangements — and wrong in the worst way, because the
    /// pointer lands at a believable offset on the *wrong display*. Nothing
    /// infers an arrangement any more.
    #[must_use]
    pub fn from_advertised(monitors: &[ProtoMonitor]) -> Self {
        Self::new(
            monitors
                .iter()
                .map(|m| MonitorPlacement {
                    id: m.id,
                    x: m.x,
                    y: m.y,
                    width: m.width,
                    height: m.height,
                })
                .collect(),
        )
    }
}
