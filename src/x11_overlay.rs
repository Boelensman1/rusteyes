use crate::scheduler::ScheduledBreak;
use std::collections::BTreeSet;
use std::fmt;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::randr::{
    Connection as RandrConnection, ConnectionExt as RandrConnectionExt, Crtc, Output,
};
use x11rb::protocol::xproto::{
    ConfigureWindowAux, ConnectionExt as XprotoConnectionExt, CreateGCAux, CreateWindowAux,
    CursorEnum, EventMask, Gcontext, GrabMode, GrabStatus, Rectangle, StackMode, Time, Timestamp,
    Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_FROM_PARENT, NONE};

const DEFAULT_BREAK_MESSAGE: &str = "Take a break";
const TEXT_WIDTH_PIXELS: i32 = 6;
const TEXT_MIN_X: i32 = 20;
const TEXT_MIN_Y: i32 = 30;
const MAX_X11_TEXT_BYTES: usize = 255;

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
        &self,
        connection: &RustConnection,
    ) -> Result<(), X11OverlayError> {
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
                Some(_) => {}
                None => return Ok(()),
            }
        }
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

        let (x, y) = message_position(&window.monitor, self.message.len());
        x11(
            "draw overlay message",
            connection.image_text8(window.window, self.foreground_gc, x, y, &self.message),
        )?;
        Ok(())
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
    EventMask::EXPOSURE
        | EventMask::STRUCTURE_NOTIFY
        | EventMask::KEY_PRESS
        | EventMask::KEY_RELEASE
        | EventMask::BUTTON_PRESS
        | EventMask::BUTTON_RELEASE
        | EventMask::POINTER_MOTION
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

fn x11_text_bytes(message: &str) -> Vec<u8> {
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

fn message_position(monitor: &MonitorGeometry, message_len: usize) -> (i16, i16) {
    let text_width = i32::try_from(message_len).map_or(i32::MAX, |message_len| {
        message_len.saturating_mul(TEXT_WIDTH_PIXELS)
    });
    let x = ((i32::from(monitor.width) - text_width) / 2).max(TEXT_MIN_X);
    let y = (i32::from(monitor.height) / 2).max(TEXT_MIN_Y);

    (saturating_i16(x), saturating_i16(y))
}

fn saturating_i16(value: i32) -> i16 {
    match i16::try_from(value) {
        Ok(value) => value,
        Err(_) if value.is_negative() => i16::MIN,
        Err(_) => i16::MAX,
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
