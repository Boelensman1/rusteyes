//! Center-of-screen pre-break countdown card, shown with the edge glow in place
//! of a desktop notification. Core X fonts are small and large ones are often
//! not installed, so the countdown itself is drawn as seven-segment digits from
//! filled rectangles; only the title and hint lines use a core font.

use super::layout::{remaining_time_text, x11_text_bytes};
use super::{MonitorGeometry, X11OverlayError, X11Screen, generate_id, x11};
use std::time::Duration;
use x11rb::COPY_FROM_PARENT;
use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
use x11rb::protocol::xproto::{
    Char2b, ClipOrdering, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
    Font, Gcontext, Rectangle, StackMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

const CARD_WIDTH: u16 = 480;
const CARD_HEIGHT: u16 = 230;
const CARD_BORDER: u16 = 3;
const CARD_PIXEL: u32 = 0x001A_1A1A;
const BORDER_PIXEL: u32 = 0x00FF_8C00;
const TITLE_PIXEL: u32 = 0x00FF_FFFF;
const HINT_PIXEL: u32 = 0x00B0_B0B0;
const DIGIT_PIXEL: u32 = 0x00FF_8C00;
const TITLE_BASELINE: i16 = 48;
const DIGITS_TOP: i32 = 72;
const HINT_BASELINE: i16 = 210;
const HINT_TEXT: &str = "Wrap up what you are doing";
/// Tried in order; the first font the server has wins, and `fixed` always
/// exists.
const TITLE_FONTS: [&str; 4] = [
    "-*-helvetica-bold-r-normal--24-*-*-*-*-*-iso8859-1",
    "-*-*-bold-r-normal--24-*-*-*-*-*-iso8859-1",
    "-misc-fixed-bold-r-normal--18-*-*-*-*-*-iso8859-1",
    "fixed",
];
const HINT_FONTS: [&str; 3] = [
    "-*-helvetica-medium-r-normal--17-*-*-*-*-*-iso8859-1",
    "-*-*-medium-r-normal--17-*-*-*-*-*-iso8859-1",
    "fixed",
];

const DIGIT_WIDTH: i32 = 50;
const DIGIT_HEIGHT: i32 = 100;
const SEGMENT: i32 = 12;
const DIGIT_GAP: i32 = 16;

pub(super) struct PreBreakCard {
    windows: Vec<Window>,
    card_gc: Gcontext,
    title_gc: Gcontext,
    hint_gc: Gcontext,
    digit_gc: Gcontext,
    fonts: Vec<Font>,
    title: TextLine,
    hint: TextLine,
}

struct TextLine {
    text: Vec<u8>,
    x: i16,
}

impl PreBreakCard {
    /// Creates the GCs and fonts. Windows are added per monitor with
    /// `add_window`; on error the caller must still `destroy` the card.
    pub(super) fn new(
        connection: &RustConnection,
        screen: X11Screen,
        break_name: &str,
    ) -> Result<Self, X11OverlayError> {
        let mut card = Self {
            windows: Vec::new(),
            card_gc: generate_id(connection, "generate pre-break card graphics context")?,
            title_gc: generate_id(connection, "generate pre-break title graphics context")?,
            hint_gc: generate_id(connection, "generate pre-break hint graphics context")?,
            digit_gc: generate_id(connection, "generate pre-break digit graphics context")?,
            fonts: Vec::new(),
            title: TextLine {
                text: Vec::new(),
                x: 0,
            },
            hint: TextLine {
                text: Vec::new(),
                x: 0,
            },
        };

        let title_font = open_first_font(connection, &TITLE_FONTS)?;
        card.fonts.push(title_font);
        let hint_font = open_first_font(connection, &HINT_FONTS)?;
        card.fonts.push(hint_font);

        for (gc, pixel, font) in [
            (card.card_gc, CARD_PIXEL, None),
            (card.title_gc, TITLE_PIXEL, Some(title_font)),
            (card.hint_gc, HINT_PIXEL, Some(hint_font)),
            (card.digit_gc, DIGIT_PIXEL, None),
        ] {
            let mut aux = CreateGCAux::new().foreground(pixel).background(CARD_PIXEL);
            if let Some(font) = font {
                aux = aux.font(font);
            }
            x11(
                "create pre-break card graphics context",
                connection.create_gc(gc, screen.root, &aux),
            )?;
        }

        card.title = centered_text(connection, title_font, &title_text(break_name))?;
        card.hint = centered_text(connection, hint_font, HINT_TEXT)?;
        Ok(card)
    }

    pub(super) fn add_window(
        &mut self,
        connection: &RustConnection,
        screen: X11Screen,
        monitor: &MonitorGeometry,
    ) -> Result<(), X11OverlayError> {
        let geometry = card_geometry(monitor);
        let window = generate_id(connection, "generate pre-break card window")?;

        x11(
            "create pre-break card window",
            connection.create_window(
                screen.root_depth,
                window,
                screen.root,
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
                CARD_BORDER,
                WindowClass::INPUT_OUTPUT,
                COPY_FROM_PARENT,
                &CreateWindowAux::new()
                    .background_pixel(CARD_PIXEL)
                    .border_pixel(BORDER_PIXEL)
                    .override_redirect(1_u32),
            ),
        )?;
        self.windows.push(window);

        // An empty input region makes the card click-through.
        x11(
            "make pre-break card click-through",
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
        x11("map pre-break card window", connection.map_window(window))?;
        Ok(())
    }

    /// Repaints every card with the current countdown and keeps it on top.
    /// Called on each pulse, which also repairs any exposed areas.
    pub(super) fn draw(
        &self,
        connection: &RustConnection,
        remaining: Duration,
    ) -> Result<(), X11OverlayError> {
        let digits = countdown_rectangles(&remaining_time_text(remaining));

        for &window in &self.windows {
            x11(
                "raise pre-break card",
                connection.configure_window(
                    window,
                    &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
                ),
            )?;
            x11(
                "clear pre-break card",
                connection.poly_fill_rectangle(
                    window,
                    self.card_gc,
                    &[Rectangle {
                        x: 0,
                        y: 0,
                        width: CARD_WIDTH,
                        height: CARD_HEIGHT,
                    }],
                ),
            )?;
            x11(
                "draw pre-break card title",
                connection.image_text8(
                    window,
                    self.title_gc,
                    self.title.x,
                    TITLE_BASELINE,
                    &self.title.text,
                ),
            )?;
            x11(
                "draw pre-break card countdown",
                connection.poly_fill_rectangle(window, self.digit_gc, &digits),
            )?;
            x11(
                "draw pre-break card hint",
                connection.image_text8(
                    window,
                    self.hint_gc,
                    self.hint.x,
                    HINT_BASELINE,
                    &self.hint.text,
                ),
            )?;
        }
        Ok(())
    }

    pub(super) fn destroy(self, connection: &RustConnection) -> Option<X11OverlayError> {
        let mut first_error = None;
        for window in self.windows {
            super::remember_first_error(
                &mut first_error,
                "destroy pre-break card window",
                connection.destroy_window(window),
            );
        }
        for gc in [self.card_gc, self.title_gc, self.hint_gc, self.digit_gc] {
            // GCs that were never created report BadGContext asynchronously; the
            // request itself only fails if the connection is gone.
            super::remember_first_error(
                &mut first_error,
                "free pre-break card graphics context",
                connection.free_gc(gc),
            );
        }
        for font in self.fonts {
            super::remember_first_error(
                &mut first_error,
                "close pre-break card font",
                connection.close_font(font),
            );
        }
        first_error
    }
}

fn open_first_font(connection: &RustConnection, names: &[&str]) -> Result<Font, X11OverlayError> {
    let font = generate_id(connection, "generate pre-break card font")?;
    let mut last_error = String::from("no font candidates");

    for name in names {
        match connection
            .open_font(font, name.as_bytes())
            .map_err(|error| error.to_string())
            .and_then(|cookie| cookie.check().map_err(|error| error.to_string()))
        {
            Ok(()) => return Ok(font),
            Err(error) => last_error = error,
        }
    }

    Err(X11OverlayError::new("open pre-break card font", last_error))
}

fn centered_text(
    connection: &RustConnection,
    font: Font,
    text: &str,
) -> Result<TextLine, X11OverlayError> {
    let text = x11_text_bytes(text);
    let chars = text
        .iter()
        .map(|&byte| Char2b {
            byte1: 0,
            byte2: byte,
        })
        .collect::<Vec<_>>();
    let width = x11(
        "measure pre-break card text",
        connection.query_text_extents(font, &chars),
    )?
    .reply()
    .map_err(|error| X11OverlayError::new("measure pre-break card text", error.to_string()))?
    .overall_width;

    Ok(TextLine {
        x: saturating_i16((i32::from(CARD_WIDTH) - width) / 2),
        text,
    })
}

fn title_text(break_name: &str) -> String {
    let mut chars = break_name.chars();
    let name = chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect::<String>()
    });
    format!("{name} break in")
}

/// The card's window geometry, centered on `monitor`. X adds the border
/// outside the window, so it is included when centering.
fn card_geometry(monitor: &MonitorGeometry) -> MonitorGeometry {
    let outer_width = i32::from(CARD_WIDTH) + 2 * i32::from(CARD_BORDER);
    let outer_height = i32::from(CARD_HEIGHT) + 2 * i32::from(CARD_BORDER);

    MonitorGeometry::new(
        saturating_i16(i32::from(monitor.x) + (i32::from(monitor.width) - outer_width) / 2),
        saturating_i16(i32::from(monitor.y) + (i32::from(monitor.height) - outer_height) / 2),
        CARD_WIDTH,
        CARD_HEIGHT,
    )
}

/// Seven-segment rectangles for an `m:ss` / `h:mm:ss` countdown, horizontally
/// centered in the card.
fn countdown_rectangles(text: &str) -> Vec<Rectangle> {
    let width = text
        .chars()
        .map(char_width)
        .sum::<i32>()
        .saturating_add(DIGIT_GAP * (i32::try_from(text.len()).unwrap_or(0) - 1).max(0));
    let mut x = (i32::from(CARD_WIDTH) - width) / 2;
    let mut rectangles = Vec::new();

    for character in text.chars() {
        rectangles.extend(char_rectangles(character, x, DIGITS_TOP));
        x += char_width(character) + DIGIT_GAP;
    }
    rectangles
}

const fn char_width(character: char) -> i32 {
    if character == ':' {
        SEGMENT
    } else {
        DIGIT_WIDTH
    }
}

/// Segments a-g, clockwise from the top with g in the middle.
const fn digit_segments(digit: u32) -> u8 {
    match digit {
        0 => 0b011_1111,
        1 => 0b000_0110,
        2 => 0b101_1011,
        3 => 0b100_1111,
        4 => 0b110_0110,
        5 => 0b110_1101,
        6 => 0b111_1101,
        7 => 0b000_0111,
        8 => 0b111_1111,
        9 => 0b110_1111,
        _ => 0,
    }
}

fn char_rectangles(character: char, x: i32, y: i32) -> Vec<Rectangle> {
    if character == ':' {
        return [DIGIT_HEIGHT / 3, DIGIT_HEIGHT * 2 / 3]
            .into_iter()
            .map(|dot_y| rectangle(x, y + dot_y - SEGMENT / 2, SEGMENT, SEGMENT))
            .collect();
    }

    let Some(digit) = character.to_digit(10) else {
        return Vec::new();
    };
    // Vertical segments overlap the horizontal ones so corners are solid.
    let vertical = DIGIT_HEIGHT / 2 + SEGMENT / 2;
    let middle = DIGIT_HEIGHT / 2 - SEGMENT / 2;
    let right = x + DIGIT_WIDTH - SEGMENT;
    let segments = [
        (x, y, DIGIT_WIDTH, SEGMENT),                          // a: top
        (right, y, SEGMENT, vertical),                         // b: top right
        (right, y + middle, SEGMENT, vertical),                // c: bottom right
        (x, y + DIGIT_HEIGHT - SEGMENT, DIGIT_WIDTH, SEGMENT), // d: bottom
        (x, y + middle, SEGMENT, vertical),                    // e: bottom left
        (x, y, SEGMENT, vertical),                             // f: top left
        (x, y + middle, DIGIT_WIDTH, SEGMENT),                 // g: middle
    ];
    let lit = digit_segments(digit);

    segments
        .into_iter()
        .enumerate()
        .filter(|&(index, _)| lit & (1 << index) != 0)
        .map(|(_, (x, y, width, height))| rectangle(x, y, width, height))
        .collect()
}

fn rectangle(x: i32, y: i32, width: i32, height: i32) -> Rectangle {
    Rectangle {
        x: saturating_i16(x),
        y: saturating_i16(y),
        width: u16::try_from(width).unwrap_or(0),
        height: u16::try_from(height).unwrap_or(0),
    }
}

fn saturating_i16(value: i32) -> i16 {
    i16::try_from(value).unwrap_or(if value < 0 { i16::MIN } else { i16::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_is_centered_on_each_monitor() {
        assert_eq!(
            card_geometry(&MonitorGeometry::new(1920, 0, 1280, 1024)),
            MonitorGeometry::new(2317, 394, CARD_WIDTH, CARD_HEIGHT)
        );
    }

    #[test]
    fn title_capitalizes_break_name() {
        assert_eq!(title_text("short"), "Short break in");
        assert_eq!(title_text(""), " break in");
    }

    #[test]
    fn digits_light_the_expected_segments() {
        assert_eq!(char_rectangles('8', 0, 0).len(), 7);
        assert_eq!(char_rectangles('1', 0, 0).len(), 2);
        assert_eq!(char_rectangles(':', 0, 0).len(), 2);
        assert!(char_rectangles('x', 0, 0).is_empty());
    }

    #[test]
    fn countdown_is_centered_in_the_card() {
        let rectangles = countdown_rectangles("0:25");
        let left = rectangles.iter().map(|r| i32::from(r.x)).min();
        let right = rectangles
            .iter()
            .map(|r| i32::from(r.x) + i32::from(r.width))
            .max();
        let width = 3 * DIGIT_WIDTH + SEGMENT + 3 * DIGIT_GAP;

        assert_eq!(left, Some((i32::from(CARD_WIDTH) - width) / 2));
        assert_eq!(right, Some(i32::from(CARD_WIDTH) / 2 + width / 2));
    }
}
