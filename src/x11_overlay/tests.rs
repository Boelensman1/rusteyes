use super::{
    MAX_X11_TEXT_BYTES, MonitorGeometry, monitor_from_output, normalize_monitor_geometries,
    selected_break_message, x11_text_bytes,
};
use crate::scheduler::ScheduledBreak;
use std::time::Duration;
use x11rb::protocol::randr::Connection as RandrConnection;

#[test]
fn connected_outputs_with_crtcs_become_monitors() {
    let monitor = monitor_from_output(
        String::from("HDMI-1"),
        RandrConnection::CONNECTED,
        42,
        MonitorGeometry::new("", 10, 20, 1920, 1080),
    );

    assert_eq!(
        monitor,
        Some(MonitorGeometry::new("HDMI-1", 10, 20, 1920, 1080))
    );
}

#[test]
fn disconnected_outputs_are_ignored() {
    let monitor = monitor_from_output(
        String::from("HDMI-1"),
        RandrConnection::DISCONNECTED,
        42,
        MonitorGeometry::new("", 10, 20, 1920, 1080),
    );

    assert_eq!(monitor, None);
}

#[test]
fn connected_outputs_without_crtcs_are_ignored() {
    let monitor = monitor_from_output(
        String::from("HDMI-1"),
        RandrConnection::CONNECTED,
        0,
        MonitorGeometry::new("", 10, 20, 1920, 1080),
    );

    assert_eq!(monitor, None);
}

#[test]
fn monitor_geometries_are_deduplicated_and_sorted() {
    let fallback = MonitorGeometry::new("screen", 0, 0, 3200, 1080);
    let monitors = normalize_monitor_geometries(
        vec![
            MonitorGeometry::new("right", 1920, 0, 1280, 1024),
            MonitorGeometry::new("left", 0, 0, 1920, 1080),
            MonitorGeometry::new("left-clone", 0, 0, 1920, 1080),
        ],
        fallback,
    );

    assert_eq!(
        monitors,
        vec![
            MonitorGeometry::new("left", 0, 0, 1920, 1080),
            MonitorGeometry::new("right", 1920, 0, 1280, 1024),
        ]
    );
}

#[test]
fn monitor_geometries_fall_back_to_root_screen() {
    let fallback = MonitorGeometry::new("screen", 0, 0, 3200, 1080);
    let monitors = normalize_monitor_geometries(Vec::new(), fallback.clone());

    assert_eq!(monitors, vec![fallback]);
}

#[test]
fn first_configured_break_message_is_selected() {
    let scheduled_break = scheduled_break(["Rest your eyes", "Look away"]);

    assert_eq!(selected_break_message(&scheduled_break), "Rest your eyes");
}

#[test]
fn overlay_text_replaces_non_ascii_and_is_bounded_for_x11() {
    let text = x11_text_bytes(&format!("{}é", "x".repeat(MAX_X11_TEXT_BYTES)));

    assert_eq!(text.len(), MAX_X11_TEXT_BYTES);
    assert!(text.iter().all(u8::is_ascii));
}

fn scheduled_break(messages: impl IntoIterator<Item = &'static str>) -> ScheduledBreak {
    ScheduledBreak {
        name: String::from("short"),
        slot: 1,
        duration: Duration::from_secs(20),
        messages: messages.into_iter().map(String::from).collect(),
        autolock: false,
    }
}
