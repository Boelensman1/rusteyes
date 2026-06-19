use super::{
    MAX_X11_TEXT_BYTES, MonitorGeometry, check_grab_status, normalize_monitor_geometries,
    output_has_monitor, overlay_window_event_mask, pointer_grab_event_mask, selected_break_message,
    x11_text_bytes,
};
use crate::scheduler::ScheduledBreak;
use std::time::Duration;
use x11rb::protocol::randr::Connection as RandrConnection;
use x11rb::protocol::xproto::{EventMask, GrabStatus};

#[test]
fn connected_outputs_with_crtcs_can_become_monitors() {
    assert!(output_has_monitor(RandrConnection::CONNECTED, 42));
}

#[test]
fn disconnected_outputs_are_ignored() {
    assert!(!output_has_monitor(RandrConnection::DISCONNECTED, 42));
}

#[test]
fn connected_outputs_without_crtcs_are_ignored() {
    assert!(!output_has_monitor(RandrConnection::CONNECTED, 0));
}

#[test]
fn monitor_geometries_are_deduplicated_and_sorted() {
    let fallback = MonitorGeometry::new(0, 0, 3200, 1080);
    let monitors = normalize_monitor_geometries(
        vec![
            MonitorGeometry::new(1920, 0, 1280, 1024),
            MonitorGeometry::new(0, 0, 1920, 1080),
            MonitorGeometry::new(0, 0, 1920, 1080),
        ],
        fallback,
    );

    assert_eq!(
        monitors,
        vec![
            MonitorGeometry::new(0, 0, 1920, 1080),
            MonitorGeometry::new(1920, 0, 1280, 1024),
        ]
    );
}

#[test]
fn monitor_geometries_fall_back_to_root_screen() {
    let fallback = MonitorGeometry::new(0, 0, 3200, 1080);
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

#[test]
fn overlay_windows_select_expose_and_grabbed_input_events() {
    let mask = u32::from(overlay_window_event_mask());

    assert_ne!(mask & u32::from(EventMask::EXPOSURE), 0);
    assert_ne!(mask & u32::from(EventMask::KEY_PRESS), 0);
    assert_ne!(mask & u32::from(EventMask::KEY_RELEASE), 0);
    assert_ne!(mask & u32::from(EventMask::BUTTON_PRESS), 0);
    assert_ne!(mask & u32::from(EventMask::BUTTON_RELEASE), 0);
    assert_ne!(mask & u32::from(EventMask::POINTER_MOTION), 0);
}

#[test]
fn pointer_grab_mask_keeps_pointer_motion_available() {
    let mask = u32::from(pointer_grab_event_mask());

    assert_ne!(mask & u32::from(EventMask::BUTTON_PRESS), 0);
    assert_ne!(mask & u32::from(EventMask::BUTTON_RELEASE), 0);
    assert_ne!(mask & u32::from(EventMask::POINTER_MOTION), 0);
}

#[test]
fn successful_grab_status_is_accepted() {
    assert_eq!(
        check_grab_status("grab overlay pointer input", GrabStatus::SUCCESS),
        Ok(())
    );
}

#[test]
fn failed_grab_status_returns_context() {
    let error = match check_grab_status("grab overlay pointer input", GrabStatus::ALREADY_GRABBED) {
        Ok(()) => panic!("expected grab status error"),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        "failed to grab overlay pointer input: ALREADY_GRABBED"
    );
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
