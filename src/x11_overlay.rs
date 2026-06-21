use crate::scheduler::ScheduledBreak;
use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::randr::{
    Connection as RandrConnection, ConnectionExt as RandrConnectionExt, Crtc, Output,
};
use x11rb::protocol::xproto::{
    ButtonPressEvent, ConfigureWindowAux, ConnectionExt as XprotoConnectionExt, CreateGCAux,
    CreateWindowAux, CursorEnum, EventMask, Gcontext, GrabMode, GrabStatus, Rectangle, StackMode,
    Time, Timestamp, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_FROM_PARENT, NONE};

use layout::{TextLine, lock_control_contains_root_position, overlay_layout, x11_text_bytes};

const DEFAULT_BREAK_MESSAGE: &str = "Take a break";

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct X11Screen {
    root: Window,
    root_depth: u8,
    width: u16,
    height: u16,
    black_pixel: u32,
    white_pixel: u32,
}

impl X11Screen {
    pub(crate) const fn new(
        root: Window,
        root_depth: u8,
        width: u16,
        height: u16,
        black_pixel: u32,
        white_pixel: u32,
    ) -> Self {
        Self {
            root,
            root_depth,
            width,
            height,
            black_pixel,
            white_pixel,
        }
    }

    pub(crate) const fn root(self) -> Window {
        self.root
    }

    fn fallback_monitor(self) -> MonitorGeometry {
        MonitorGeometry::new(0, 0, self.width, self.height)
    }
}

#[allow(clippy::module_name_repetitions)]
pub(crate) struct X11Overlay {
    windows: Vec<OverlayWindow>,
    background_gc: Gcontext,
    foreground_gc: Gcontext,
    input_grab: InputGrab,
    message: Vec<u8>,
    remaining: Duration,
    lock_after_break: bool,
}

impl X11Overlay {
    pub(crate) fn show(
        connection: &RustConnection,
        screen: X11Screen,
        scheduled_break: &ScheduledBreak,
    ) -> Result<Self, X11OverlayError> {
        let monitors = query_monitors(connection, screen);
        let background_gc =
            generate_id(connection, "generate overlay background graphics context")?;
        let foreground_gc =
            generate_id(connection, "generate overlay foreground graphics context")?;

        x11(
            "create overlay background graphics context",
            connection.create_gc(
                background_gc,
                screen.root,
                &CreateGCAux::new()
                    .foreground(screen.black_pixel)
                    .background(screen.black_pixel),
            ),
        )?;
        x11(
            "create overlay foreground graphics context",
            connection.create_gc(
                foreground_gc,
                screen.root,
                &CreateGCAux::new()
                    .foreground(screen.white_pixel)
                    .background(screen.black_pixel),
            ),
        )?;

        let mut overlay = Self {
            windows: Vec::with_capacity(monitors.len()),
            background_gc,
            foreground_gc,
            input_grab: InputGrab::none(),
            message: x11_text_bytes(selected_break_message(scheduled_break)),
            remaining: scheduled_break.duration,
            lock_after_break: scheduled_break.autolock,
        };

        let result = (|| {
            for monitor in monitors {
                overlay.create_window(connection, screen, monitor)?;
            }

            x11("flush mapped overlay windows", connection.flush())?;
            overlay.draw(connection)?;
            overlay.raise(connection)?;
            overlay.grab_input(connection)?;
            Ok(())
        })();

        match result {
            Ok(()) => Ok(overlay),
            Err(error) => {
                let _ = overlay.destroy(connection);
                Err(error)
            }
        }
    }

    pub(crate) fn handle_pending_events(
        &mut self,
        connection: &RustConnection,
    ) -> Result<bool, X11OverlayError> {
        let mut lock_after_break_requested = false;

        loop {
            match x11("poll overlay X11 event", connection.poll_for_event())? {
                Some(Event::Expose(event)) => {
                    if let Some(window) = self
                        .windows
                        .iter()
                        .find(|window| window.window == event.window)
                    {
                        self.draw_window(connection, window)?;
                    }
                }
                Some(Event::ButtonPress(event)) => {
                    if self.handle_button_press(&event) {
                        lock_after_break_requested = true;
                        self.draw(connection)?;
                    }
                }
                Some(_) => {}
                None => return Ok(lock_after_break_requested),
            }
        }
    }

    pub(crate) fn update_remaining(
        &mut self,
        connection: &RustConnection,
        remaining: Duration,
    ) -> Result<(), X11OverlayError> {
        self.remaining = remaining;
        self.draw(connection)
    }

    pub(crate) fn request_lock_after_break(
        &mut self,
        connection: &RustConnection,
    ) -> Result<(), X11OverlayError> {
        if self.lock_after_break {
            return Ok(());
        }

        self.lock_after_break = true;
        self.draw(connection)
    }

    pub(crate) fn raise(&self, connection: &RustConnection) -> Result<(), X11OverlayError> {
        for window in &self.windows {
            x11(
                "raise overlay window",
                connection.configure_window(
                    window.window,
                    &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
                ),
            )?;
        }

        x11("flush raised overlay windows", connection.flush())
    }

    pub(crate) fn release_input(
        &mut self,
        connection: &RustConnection,
    ) -> Result<(), X11OverlayError> {
        self.input_grab.release(connection)
    }

    pub(crate) fn destroy(mut self, connection: &RustConnection) -> Result<(), X11OverlayError> {
        let mut first_error = self.input_grab.release(connection).err();

        for window in self.windows {
            remember_first_error(
                &mut first_error,
                "destroy overlay window",
                connection.destroy_window(window.window),
            );
        }
        remember_first_error(
            &mut first_error,
            "free overlay background graphics context",
            connection.free_gc(self.background_gc),
        );
        remember_first_error(
            &mut first_error,
            "free overlay foreground graphics context",
            connection.free_gc(self.foreground_gc),
        );
        remember_first_error(
            &mut first_error,
            "flush destroyed overlay resources",
            connection.flush(),
        );

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn create_window(
        &mut self,
        connection: &RustConnection,
        screen: X11Screen,
        monitor: MonitorGeometry,
    ) -> Result<(), X11OverlayError> {
        let window = generate_id(connection, "generate overlay window")?;
        let event_mask = overlay_window_event_mask();

        x11(
            "create overlay window",
            connection.create_window(
                screen.root_depth,
                window,
                screen.root,
                monitor.x,
                monitor.y,
                monitor.width,
                monitor.height,
                0,
                WindowClass::INPUT_OUTPUT,
                COPY_FROM_PARENT,
                &CreateWindowAux::new()
                    .background_pixel(screen.black_pixel)
                    .override_redirect(1_u32)
                    .event_mask(event_mask),
            ),
        )?;
        x11("map overlay window", connection.map_window(window))?;

        self.windows.push(OverlayWindow { window, monitor });
        Ok(())
    }

    fn grab_input(&mut self, connection: &RustConnection) -> Result<(), X11OverlayError> {
        let grab_window = self
            .windows
            .first()
            .map(|window| window.window)
            .ok_or_else(|| {
                X11OverlayError::new("grab overlay input", "no overlay windows".to_owned())
            })?;

        self.input_grab = InputGrab::acquire(connection, grab_window)?;
        Ok(())
    }

    fn draw(&self, connection: &RustConnection) -> Result<(), X11OverlayError> {
        for window in &self.windows {
            self.draw_window(connection, window)?;
        }

        x11("flush drawn overlay windows", connection.flush())
    }

    fn draw_window(
        &self,
        connection: &RustConnection,
        window: &OverlayWindow,
    ) -> Result<(), X11OverlayError> {
        x11(
            "fill overlay window",
            connection.poly_fill_rectangle(
                window.window,
                self.background_gc,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: window.monitor.width,
                    height: window.monitor.height,
                }],
            ),
        )?;

        let layout = overlay_layout(
            &window.monitor,
            &self.message,
            self.remaining,
            self.lock_after_break,
        );
        self.draw_text_line(connection, window, &layout.message)?;
        self.draw_text_line(connection, window, &layout.remaining)?;

        x11(
            "draw overlay lock-after-break control",
            connection.poly_rectangle(
                window.window,
                self.foreground_gc,
                &[Rectangle {
                    x: layout.lock_control.bounds.x,
                    y: layout.lock_control.bounds.y,
                    width: layout.lock_control.bounds.width,
                    height: layout.lock_control.bounds.height,
                }],
            ),
        )?;
        self.draw_text_line(connection, window, &layout.lock_control.label)?;
        Ok(())
    }

    fn draw_text_line(
        &self,
        connection: &RustConnection,
        window: &OverlayWindow,
        line: &TextLine,
    ) -> Result<(), X11OverlayError> {
        x11(
            "draw overlay text",
            connection.image_text8(
                window.window,
                self.foreground_gc,
                line.x,
                line.y,
                &line.text,
            ),
        )?;
        Ok(())
    }

    fn handle_button_press(&mut self, event: &ButtonPressEvent) -> bool {
        if self.lock_after_break {
            return false;
        }

        if lock_control_contains_root_position(&self.windows, event.root_x, event.root_y) {
            self.lock_after_break = true;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InputGrab {
    pointer: bool,
    keyboard: bool,
}

impl InputGrab {
    const fn none() -> Self {
        Self {
            pointer: false,
            keyboard: false,
        }
    }

    const fn pointer_only() -> Self {
        Self {
            pointer: true,
            keyboard: false,
        }
    }

    fn acquire(connection: &RustConnection, window: Window) -> Result<Self, X11OverlayError> {
        let pointer_reply = x11(
            "grab overlay pointer input",
            x11(
                "request overlay pointer input grab",
                connection.grab_pointer(
                    false,
                    window,
                    pointer_grab_event_mask(),
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    NONE,
                    CursorEnum::NONE,
                    Time::CURRENT_TIME,
                ),
            )?
            .reply(),
        )?;
        check_grab_status("grab overlay pointer input", pointer_reply.status)?;

        let mut input_grab = Self::pointer_only();
        if let Err(error) = grab_keyboard(connection, window) {
            let _ = input_grab.release(connection);
            return Err(error);
        }
        input_grab.keyboard = true;

        Ok(input_grab)
    }

    fn release(&mut self, connection: &RustConnection) -> Result<(), X11OverlayError> {
        let mut first_error = None;
        let release_keyboard = self.keyboard;
        let release_pointer = self.pointer;

        if release_keyboard {
            remember_first_error(
                &mut first_error,
                "ungrab overlay keyboard input",
                connection.ungrab_keyboard(Time::CURRENT_TIME),
            );
            self.keyboard = false;
        }

        if release_pointer {
            remember_first_error(
                &mut first_error,
                "ungrab overlay pointer input",
                connection.ungrab_pointer(Time::CURRENT_TIME),
            );
            self.pointer = false;
        }

        if release_keyboard || release_pointer {
            remember_first_error(
                &mut first_error,
                "flush released overlay input grabs",
                connection.flush(),
            );
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn grab_keyboard(connection: &RustConnection, window: Window) -> Result<(), X11OverlayError> {
    let keyboard_reply = x11(
        "grab overlay keyboard input",
        x11(
            "request overlay keyboard input grab",
            connection.grab_keyboard(
                false,
                window,
                Time::CURRENT_TIME,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            ),
        )?
        .reply(),
    )?;
    check_grab_status("grab overlay keyboard input", keyboard_reply.status)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OverlayWindow {
    window: Window,
    monitor: MonitorGeometry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MonitorGeometry {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

impl MonitorGeometry {
    const fn new(x: i16, y: i16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    const fn key(&self) -> (i16, i16, u16, u16) {
        (self.x, self.y, self.width, self.height)
    }
}

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct X11OverlayError {
    operation: &'static str,
    message: String,
}

impl X11OverlayError {
    fn new(operation: &'static str, message: String) -> Self {
        Self { operation, message }
    }
}

impl fmt::Display for X11OverlayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "failed to {}: {}", self.operation, self.message)
    }
}

impl std::error::Error for X11OverlayError {}

fn overlay_window_event_mask() -> EventMask {
    EventMask::EXPOSURE | EventMask::BUTTON_PRESS
}

fn pointer_grab_event_mask() -> EventMask {
    EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION
}

fn check_grab_status(operation: &'static str, status: GrabStatus) -> Result<(), X11OverlayError> {
    if status == GrabStatus::SUCCESS {
        Ok(())
    } else {
        Err(X11OverlayError::new(operation, format!("{status:?}")))
    }
}

fn query_monitors(connection: &RustConnection, screen: X11Screen) -> Vec<MonitorGeometry> {
    let monitors = current_screen_resources(connection, screen.root)
        .or_else(|| screen_resources(connection, screen.root))
        .map_or_else(Vec::new, |(config_timestamp, outputs)| {
            monitors_from_outputs(connection, config_timestamp, &outputs)
        });

    normalize_monitor_geometries(monitors, screen.fallback_monitor())
}

fn current_screen_resources(
    connection: &RustConnection,
    root: Window,
) -> Option<(Timestamp, Vec<Output>)> {
    connection
        .randr_get_screen_resources_current(root)
        .ok()?
        .reply()
        .ok()
        .map(|reply| (reply.config_timestamp, reply.outputs))
}

fn screen_resources(connection: &RustConnection, root: Window) -> Option<(Timestamp, Vec<Output>)> {
    connection
        .randr_get_screen_resources(root)
        .ok()?
        .reply()
        .ok()
        .map(|reply| (reply.config_timestamp, reply.outputs))
}

fn monitors_from_outputs(
    connection: &RustConnection,
    config_timestamp: Timestamp,
    outputs: &[Output],
) -> Vec<MonitorGeometry> {
    let mut monitors = Vec::new();

    for output in outputs {
        let Some(info) = connection
            .randr_get_output_info(*output, config_timestamp)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            continue;
        };
        if !output_has_monitor(info.connection, info.crtc) {
            continue;
        }

        let Some(crtc) = connection
            .randr_get_crtc_info(info.crtc, config_timestamp)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            continue;
        };

        monitors.push(MonitorGeometry::new(
            crtc.x,
            crtc.y,
            crtc.width,
            crtc.height,
        ));
    }

    monitors
}

fn output_has_monitor(connection: RandrConnection, crtc: Crtc) -> bool {
    connection == RandrConnection::CONNECTED && crtc != 0
}

fn normalize_monitor_geometries(
    monitors: Vec<MonitorGeometry>,
    fallback: MonitorGeometry,
) -> Vec<MonitorGeometry> {
    let mut seen = BTreeSet::new();
    let mut normalized = monitors
        .into_iter()
        .filter(|monitor| monitor.width > 0 && monitor.height > 0)
        .filter(|monitor| seen.insert(monitor.key()))
        .collect::<Vec<_>>();

    normalized.sort_by_key(|monitor| (monitor.x, monitor.y));

    if normalized.is_empty() {
        vec![fallback]
    } else {
        normalized
    }
}

fn selected_break_message(scheduled_break: &ScheduledBreak) -> &str {
    scheduled_break
        .messages
        .first()
        .map_or(DEFAULT_BREAK_MESSAGE, String::as_str)
}

mod layout {
    use super::{DEFAULT_BREAK_MESSAGE, MonitorGeometry, OverlayWindow};
    use std::time::Duration;

    const TEXT_WIDTH_PIXELS: i32 = 6;
    const TEXT_MIN_X: i32 = 20;
    const TEXT_MIN_Y: i32 = 30;
    const LINE_HEIGHT_PIXELS: i32 = 28;
    const CONTROL_MARGIN_TOP_PIXELS: i32 = 22;
    const CONTROL_PADDING_X_PIXELS: i32 = 14;
    const CONTROL_HEIGHT_PIXELS: u16 = 32;
    const CONTROL_TEXT_BASELINE_OFFSET_PIXELS: i32 = 21;
    const LOCK_CONTROL_LABEL: &str = "Lock after break";
    const LOCK_CONTROL_REQUESTED_LABEL: &str = "Locking after break";
    pub(super) const MAX_X11_TEXT_BYTES: usize = 255;

    pub(super) fn x11_text_bytes(message: &str) -> Vec<u8> {
        let bytes = message
            .bytes()
            .map(|byte| {
                if byte.is_ascii_graphic() || byte == b' ' {
                    byte
                } else {
                    b'?'
                }
            })
            .take(MAX_X11_TEXT_BYTES)
            .collect::<Vec<_>>();

        if bytes.is_empty() {
            DEFAULT_BREAK_MESSAGE.as_bytes().to_vec()
        } else {
            bytes
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct OverlayLayout {
        pub(super) message: TextLine,
        pub(super) remaining: TextLine,
        pub(super) lock_control: LockControlLayout,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct TextLine {
        pub(super) x: i16,
        pub(super) y: i16,
        pub(super) text: Vec<u8>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct LockControlLayout {
        pub(super) bounds: ControlBounds,
        pub(super) label: TextLine,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct ControlBounds {
        pub(super) x: i16,
        pub(super) y: i16,
        pub(super) width: u16,
        pub(super) height: u16,
    }

    impl ControlBounds {
        pub(super) fn contains(self, x: i32, y: i32) -> bool {
            let left = i32::from(self.x);
            let top = i32::from(self.y);
            let right = left.saturating_add(i32::from(self.width));
            let bottom = top.saturating_add(i32::from(self.height));

            x >= left && x < right && y >= top && y < bottom
        }
    }

    pub(super) fn lock_control_contains_root_position(
        windows: &[OverlayWindow],
        root_x: i16,
        root_y: i16,
    ) -> bool {
        windows.iter().any(|window| {
            let local_x = i32::from(root_x).saturating_sub(i32::from(window.monitor.x));
            let local_y = i32::from(root_y).saturating_sub(i32::from(window.monitor.y));

            lock_control_layout(&window.monitor, false)
                .bounds
                .contains(local_x, local_y)
        })
    }

    pub(super) fn overlay_layout(
        monitor: &MonitorGeometry,
        message: &[u8],
        remaining: Duration,
        lock_after_break: bool,
    ) -> OverlayLayout {
        let center_y = i32::from(monitor.height) / 2;
        let message_y = (center_y - LINE_HEIGHT_PIXELS).max(TEXT_MIN_Y);
        let remaining_y = center_y.max(TEXT_MIN_Y);

        OverlayLayout {
            message: centered_text_line(monitor, message.to_vec(), message_y),
            remaining: centered_text_line(
                monitor,
                x11_text_bytes(&remaining_time_text(remaining)),
                remaining_y,
            ),
            lock_control: lock_control_layout(monitor, lock_after_break),
        }
    }

    fn centered_text_line(monitor: &MonitorGeometry, text: Vec<u8>, y: i32) -> TextLine {
        TextLine {
            x: centered_text_x(monitor, text.len()),
            y: saturating_i16(y),
            text,
        }
    }

    fn centered_text_x(monitor: &MonitorGeometry, text_len: usize) -> i16 {
        let text_width = text_width_pixels(text_len);
        let x = ((i32::from(monitor.width) - text_width) / 2).max(TEXT_MIN_X);

        saturating_i16(x)
    }

    fn text_width_pixels(text_len: usize) -> i32 {
        i32::try_from(text_len).map_or(i32::MAX, |text_len| {
            text_len.saturating_mul(TEXT_WIDTH_PIXELS)
        })
    }

    pub(super) fn remaining_time_text(remaining: Duration) -> String {
        let seconds = if remaining.is_zero() {
            0
        } else {
            remaining
                .as_secs()
                .saturating_add(u64::from(remaining.subsec_nanos() > 0))
        };
        let hours = seconds / 3_600;
        let minutes = (seconds % 3_600) / 60;
        let seconds = seconds % 60;

        if hours > 0 {
            format!("{hours}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes}:{seconds:02}")
        }
    }

    pub(super) fn lock_control_label(lock_after_break: bool) -> &'static str {
        if lock_after_break {
            LOCK_CONTROL_REQUESTED_LABEL
        } else {
            LOCK_CONTROL_LABEL
        }
    }

    pub(super) fn lock_control_layout(
        monitor: &MonitorGeometry,
        lock_after_break: bool,
    ) -> LockControlLayout {
        let label = x11_text_bytes(lock_control_label(lock_after_break));
        let width = lock_control_width(monitor, label.len());
        let x = ((i32::from(monitor.width) - i32::from(width)) / 2).max(TEXT_MIN_X);
        let y = (i32::from(monitor.height) / 2).saturating_add(CONTROL_MARGIN_TOP_PIXELS);
        let bounds = ControlBounds {
            x: saturating_i16(x),
            y: saturating_i16(y),
            width,
            height: CONTROL_HEIGHT_PIXELS,
        };
        let label_x = i32::from(bounds.x).saturating_add(CONTROL_PADDING_X_PIXELS);
        let label_y = i32::from(bounds.y).saturating_add(CONTROL_TEXT_BASELINE_OFFSET_PIXELS);

        LockControlLayout {
            bounds,
            label: TextLine {
                x: saturating_i16(label_x),
                y: saturating_i16(label_y),
                text: label,
            },
        }
    }

    fn lock_control_width(monitor: &MonitorGeometry, label_len: usize) -> u16 {
        let width = text_width_pixels(label_len)
            .saturating_add(CONTROL_PADDING_X_PIXELS.saturating_mul(2))
            .max(1);
        let max_width = i32::from(monitor.width)
            .saturating_sub(TEXT_MIN_X.saturating_mul(2))
            .max(1);

        saturating_u16(width.min(max_width))
    }

    fn saturating_i16(value: i32) -> i16 {
        match i16::try_from(value) {
            Ok(value) => value,
            Err(_) if value.is_negative() => i16::MIN,
            Err(_) => i16::MAX,
        }
    }

    fn saturating_u16(value: i32) -> u16 {
        match u16::try_from(value) {
            Ok(value) => value,
            Err(_) if value.is_negative() => 0,
            Err(_) => u16::MAX,
        }
    }
}

fn generate_id(
    connection: &RustConnection,
    operation: &'static str,
) -> Result<u32, X11OverlayError> {
    x11(operation, connection.generate_id())
}

fn x11<T, E>(operation: &'static str, result: Result<T, E>) -> Result<T, X11OverlayError>
where
    E: fmt::Display,
{
    result.map_err(|error| X11OverlayError::new(operation, error.to_string()))
}

fn remember_first_error<T, E>(
    first_error: &mut Option<X11OverlayError>,
    operation: &'static str,
    result: Result<T, E>,
) where
    E: fmt::Display,
{
    if first_error.is_none() {
        if let Err(error) = result {
            *first_error = Some(X11OverlayError::new(operation, error.to_string()));
        }
    }
}

#[cfg(test)]
mod tests;
