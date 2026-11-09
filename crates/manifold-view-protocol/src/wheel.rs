//! A `ScrollDelta` in the wheel units the Chromium engines take: WebView2's `SendMouseInput` wheel data and CEF's
//! `send_mouse_wheel_event` both read 120 as one notch, which scrolls 100 CSS px at Windows' default of 3 lines a
//! notch, as Edge does (run `2026-11-08-wheel-distance`: 120 moved both 100 CSS px).

use crate::messages::ScrollDelta;

/// Wheel units in one notch (Windows' `WHEEL_DELTA`).
pub const NOTCH: f64 = 120.0;
/// Wheel units per CSS px: a notch is 100 CSS px.
const UNITS_PER_CSS_PX: f64 = NOTCH / 100.0;

/// Turns each step into whole wheel units, carrying the fraction to the next step so a run of small steps (a
/// high-resolution wheel, a touchpad) adds up to the distance it asked for instead of rounding each step to 0.
#[derive(Debug, Default)]
pub struct WheelUnits {
    rest_x: f64,
    rest_y: f64,
}

impl WheelUnits {
    /// `(x, y)` wheel units for `delta`; `css_scale` is the engine's physical px per CSS px, which `Pixels` divide by.
    pub fn convert(&mut self, delta: ScrollDelta, css_scale: f64) -> (i32, i32) {
        let (x, y) = match delta {
            ScrollDelta::Lines { x, y } => (x * NOTCH, y * NOTCH),
            ScrollDelta::Pixels { x, y } => {
                let per_px = UNITS_PER_CSS_PX / css_scale;
                (x * per_px, y * per_px)
            }
        };
        (take_whole(&mut self.rest_x, x), take_whole(&mut self.rest_y, y))
    }
}

/// The whole part of `rest + add`, leaving the fraction in `rest`. A total within 1e-6 of a whole number counts as it:
/// winit's notch is the OS's integer wheel delta over 120, so `7 / 120 * 120` must give 7, not 6.999….
fn take_whole(rest: &mut f64, add: f64) -> i32 {
    let total = *rest + add;
    let whole = (total + 1e-6_f64.copysign(total)).trunc();
    *rest = total - whole;
    whole as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notch_is_120_units() {
        assert_eq!(WheelUnits::default().convert(ScrollDelta::Lines { x: 0.0, y: -1.0 }, 1.25), (0, -120));
    }

    #[test]
    fn small_steps_add_up_instead_of_rounding_away() {
        let mut units = WheelUnits::default();
        // 0.004 notch each is under one unit; 250 of them are one notch.
        let sum: i32 = (0..250).map(|_| units.convert(ScrollDelta::Lines { x: 0.0, y: 0.004 }, 1.0).1).sum();
        assert_eq!(sum, 120);
    }

    #[test]
    fn a_whole_os_delta_is_whole_units() {
        // Every integer wheel delta the OS can report, as winit's notch fraction, gives back that delta at once.
        for delta in -240..=240 {
            let mut units = WheelUnits::default();
            assert_eq!(units.convert(ScrollDelta::Lines { x: 0.0, y: delta as f64 / NOTCH }, 1.0).1, delta);
        }
    }

    #[test]
    fn pixels_go_through_css_px() {
        let mut units = WheelUnits::default();
        // 125 physical px at 1.25 px per CSS px is 100 CSS px, one notch.
        assert_eq!(units.convert(ScrollDelta::Pixels { x: 125.0, y: 0.0 }, 1.25), (120, 0));
        // Signs carry their own fraction: -0.5 then -0.5 makes -1.
        let mut units = WheelUnits::default();
        // At 2.4 px per CSS px a physical px is exactly half a unit.
        let step = ScrollDelta::Pixels { x: 0.0, y: -1.0 };
        assert_eq!((units.convert(step, 2.4).1, units.convert(step, 2.4).1), (0, -1));
    }
}
