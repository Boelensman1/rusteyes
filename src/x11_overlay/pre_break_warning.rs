//! Non-blocking pre-break warning: thin pulsing strips along every monitor's
//! edges plus a countdown card in each monitor's center, shown shortly before a
//! break in place of a desktop notification. The windows are override-redirect
//! so the window manager leaves them alone, and their input shape is empty so
//! pointer clicks fall through to whatever is underneath.

use super::pre_break_card::PreBreakCard;
use super::{MonitorGeometry, X11OverlayError, X11Screen, generate_id, query_monitors, x11};
use std::time::{Duration, Instant};
use x11rb::COPY_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ClipOrdering, ConfigureWindowAux, ConnectionExt as _,
    CreateWindowAux, StackMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

const STRIP_THICKNESS: u16 = 12;
// Pixel values assume a TrueColor root visual (0xRRGGBB), which every
// mainstream X server and Xwayland provide.
const BRIGHT_PIXEL: u32 = 0x00FF_8C00;
const DIM_PIXEL: u32 = 0x0055_2E00;
const SLOW_PULSE_INTERVAL: Duration = Duration::from_millis(500);
const FAST_PULSE_INTERVAL: Duration = Duration::from_millis(250);
/// Remaining time at which the warning switches to its fast pulse. Matches the
/// runtime's final pre-break notice.
const FAST_PULSE_LEAD: Duration = Duration::from_secs(5);
/// How long the warning stays up past its deadline when no clear or break start
/// arrives, so a lost command cannot leave it on screen.
const EXPIRY_GRACE: Duration = Duration::from_secs(5);
/// Bell volume for the warning's first appearance; 0 is the server default.
const WARNING_BELL_PERCENT: i8 = 0;

#[allow(clippy::module_name_repetitions)]
pub(crate) struct X11PreBreakWarning {
    windows: Vec<Window>,
    card: Option<PreBreakCard>,
    deadline: Instant,
    next_pulse_at: Instant,
    bright: bool,
}

impl X11PreBreakWarning {
    pub(crate) fn show(
        connection: &RustConnection,
        screen: X11Screen,
        break_name: &str,
        starts_after: Duration,
        now: Instant,
    ) -> Result<Self, X11OverlayError> {
        let mut warning = Self {
            windows: Vec::new(),
            card: None,
            deadline: now + starts_after,
            next_pulse_at: now,
            bright: false,
        };

        let result = (|| {
            let monitors = query_monitors(connection, screen);
            for monitor in &monitors {
                for strip in edge_strips(monitor, STRIP_THICKNESS) {
                    warning.create_strip(connection, screen, &strip)?;
                }
            }
            let card = warning
                .card
                .insert(PreBreakCard::new(connection, screen, break_name)?);
            for monitor in &monitors {
                card.add_window(connection, screen, monitor)?;
            }
            // Audible cue for the first appearance only; refreshes stay silent.
            x11(
                "ring pre-break warning bell",
                connection.bell(WARNING_BELL_PERCENT),
            )?;
            warning.pulse(connection, now)
        })();

        match result {
            Ok(()) => Ok(warning),
            Err(error) => {
                let _ = warning.destroy(connection);
                Err(error)
            }
        }
    }

    /// Moves the deadline for a repeated warning (the final 5-second notice).
    pub(crate) fn refresh(&mut self, starts_after: Duration, now: Instant) {
        self.deadline = now + starts_after;
        self.next_pulse_at = self.next_pulse_at.min(now + pulse_interval(starts_after));
    }

    pub(crate) fn is_expired_at(&self, now: Instant) -> bool {
        now >= self.deadline + EXPIRY_GRACE
    }

    pub(crate) fn next_pulse_delay_at(&self, now: Instant) -> Duration {
        self.next_pulse_at.saturating_duration_since(now)
    }

    pub(crate) fn pulse_if_due(
        &mut self,
        connection: &RustConnection,
        now: Instant,
    ) -> Result<(), X11OverlayError> {
        if now < self.next_pulse_at {
            return Ok(());
        }

        self.pulse(connection, now)
    }

    fn pulse(&mut self, connection: &RustConnection, now: Instant) -> Result<(), X11OverlayError> {
        self.bright = !self.bright;
        let pixel = if self.bright { BRIGHT_PIXEL } else { DIM_PIXEL };

        for &window in &self.windows {
            x11(
                "set pre-break warning color",
                connection.change_window_attributes(
                    window,
                    &ChangeWindowAttributesAux::new().background_pixel(pixel),
                ),
            )?;
            x11(
                "repaint pre-break warning",
                connection.clear_area(false, window, 0, 0, 0, 0),
            )?;
            // Keep the strips above windows mapped after the warning appeared.
            x11(
                "raise pre-break warning",
                connection.configure_window(
                    window,
                    &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
                ),
            )?;
        }
        if let Some(card) = &self.card {
            card.draw(connection, self.deadline.saturating_duration_since(now))?;
        }
        x11("flush pre-break warning", connection.flush())?;

        self.next_pulse_at = now + pulse_interval(self.deadline.saturating_duration_since(now));
        Ok(())
    }

    fn create_strip(
        &mut self,
        connection: &RustConnection,
        screen: X11Screen,
        strip: &MonitorGeometry,
    ) -> Result<(), X11OverlayError> {
        let window = generate_id(connection, "generate pre-break warning window")?;

        x11(
            "create pre-break warning window",
            connection.create_window(
                screen.root_depth,
                window,
                screen.root,
                strip.x,
                strip.y,
                strip.width,
                strip.height,
                0,
                WindowClass::INPUT_OUTPUT,
                COPY_FROM_PARENT,
                &CreateWindowAux::new()
                    .background_pixel(DIM_PIXEL)
                    .override_redirect(1_u32),
            ),
        )?;
        self.windows.push(window);

        // An empty input region makes the strip click-through.
        x11(
            "make pre-break warning click-through",
            connection.shape_rectangles(
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                window,
                0,
                0,
                &[],
            ),
        )?;
        x11(
            "map pre-break warning window",
            connection.map_window(window),
        )?;
        Ok(())
    }

    pub(crate) fn destroy(self, connection: &RustConnection) -> Result<(), X11OverlayError> {
        let mut first_error = self.card.and_then(|card| card.destroy(connection));
        for window in self.windows {
            super::remember_first_error(
                &mut first_error,
                "destroy pre-break warning window",
                connection.destroy_window(window),
            );
        }
        super::remember_first_error(
            &mut first_error,
            "flush destroyed pre-break warning",
            connection.flush(),
        );

        first_error.map_or(Ok(()), Err)
    }
}

fn pulse_interval(remaining: Duration) -> Duration {
    if remaining > FAST_PULSE_LEAD {
        SLOW_PULSE_INTERVAL
    } else {
        FAST_PULSE_INTERVAL
    }
}

/// Top, bottom, left and right strips inside `monitor`. The side strips sit
/// between the top and bottom ones so corners are not drawn twice. Strips that
/// would be empty on a degenerate monitor are dropped, since X rejects
/// zero-sized windows.
fn edge_strips(monitor: &MonitorGeometry, thickness: u16) -> Vec<MonitorGeometry> {
    let x = i32::from(monitor.x);
    let y = i32::from(monitor.y);
    let width = i32::from(monitor.width);
    let height = i32::from(monitor.height);
    let vertical = i32::from(thickness).min(height / 2);
    let horizontal = i32::from(thickness).min(width / 2);
    let side_height = height - 2 * vertical;

    [
        (x, y, width, vertical),
        (x, y + height - vertical, width, vertical),
        (x, y + vertical, horizontal, side_height),
        (
            x + width - horizontal,
            y + vertical,
            horizontal,
            side_height,
        ),
    ]
    .into_iter()
    .filter(|&(_, _, width, height)| width > 0 && height > 0)
    .map(|(x, y, width, height)| {
        MonitorGeometry::new(
            saturating_i16(x),
            saturating_i16(y),
            saturating_u16(width),
            saturating_u16(height),
        )
    })
    .collect()
}

fn saturating_i16(value: i32) -> i16 {
    i16::try_from(value).unwrap_or(if value < 0 { i16::MIN } else { i16::MAX })
}

fn saturating_u16(value: i32) -> u16 {
    u16::try_from(value).unwrap_or(if value < 0 { 0 } else { u16::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_strips_frame_the_monitor_without_overlapping_corners() {
        let strips = edge_strips(&MonitorGeometry::new(1920, 0, 1280, 1024), 12);

        assert_eq!(
            strips,
            vec![
                MonitorGeometry::new(1920, 0, 1280, 12),
                MonitorGeometry::new(1920, 1012, 1280, 12),
                MonitorGeometry::new(1920, 12, 12, 1000),
                MonitorGeometry::new(3188, 12, 12, 1000),
            ]
        );
    }

    #[test]
    fn edge_strips_drop_empty_strips_on_tiny_monitors() {
        let strips = edge_strips(&MonitorGeometry::new(0, 0, 20, 10), 12);

        assert_eq!(
            strips,
            vec![
                MonitorGeometry::new(0, 0, 20, 5),
                MonitorGeometry::new(0, 5, 20, 5),
            ]
        );
    }

    #[test]
    fn pulse_speeds_up_for_the_final_seconds() {
        assert_eq!(pulse_interval(Duration::from_secs(30)), SLOW_PULSE_INTERVAL);
        assert_eq!(
            pulse_interval(Duration::from_millis(5_001)),
            SLOW_PULSE_INTERVAL
        );
        assert_eq!(pulse_interval(FAST_PULSE_LEAD), FAST_PULSE_INTERVAL);
        assert_eq!(pulse_interval(Duration::ZERO), FAST_PULSE_INTERVAL);
    }
}
