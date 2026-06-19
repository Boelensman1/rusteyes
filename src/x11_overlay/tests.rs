use super::{
    MAX_X11_TEXT_BYTES, MonitorGeometry, check_grab_status, lock_control_label,
    lock_control_layout, normalize_monitor_geometries, output_has_monitor, overlay_layout,
    overlay_window_event_mask, pointer_grab_event_mask, remaining_time_text,
    selected_break_message, x11_text_bytes,
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
fn remaining_time_text_formats_minutes_and_hours() {
    assert_eq!(remaining_time_text(Duration::ZERO), "0:00");
    assert_eq!(remaining_time_text(Duration::from_secs(59)), "0:59");
    assert_eq!(remaining_time_text(Duration::from_secs(60)), "1:00");
    assert_eq!(remaining_time_text(Duration::from_secs(3_661)), "1:01:01");
}

#[test]
fn remaining_time_text_rounds_subseconds_up() {
    assert_eq!(remaining_time_text(Duration::from_millis(1)), "0:01");
    assert_eq!(remaining_time_text(Duration::from_millis(1_001)), "0:02");
}

#[test]
fn overlay_layout_stacks_message_time_and_lock_control() {
    let monitor = MonitorGeometry::new(0, 0, 800, 600);
    let layout = overlay_layout(&monitor, b"Rest your eyes", Duration::from_secs(90), false);

    assert_eq!(layout.message.text, b"Rest your eyes");
    assert_eq!(layout.remaining.text, b"1:30");
    assert!(layout.message.y < layout.remaining.y);
    assert!(layout.remaining.y < layout.lock_control.bounds.y);
}

#[test]
fn lock_control_hit_testing_uses_button_bounds() {
    let monitor = MonitorGeometry::new(0, 0, 800, 600);
    let control = lock_control_layout(&monitor, false);
    let inside_x = control.bounds.x + 1;
    let inside_y = control.bounds.y + 1;

    assert!(control.bounds.contains(inside_x, inside_y));
    assert!(!control.bounds.contains(control.bounds.x - 1, inside_y));
    assert!(!control.bounds.contains(inside_x, control.bounds.y - 1));
}

#[test]
fn lock_control_label_reflects_requested_state() {
    let monitor = MonitorGeometry::new(0, 0, 800, 600);

    assert_eq!(lock_control_label(false), "Lock after break");
    assert_eq!(lock_control_label(true), "Lock after break requested");
    assert_eq!(
        lock_control_layout(&monitor, false).label.text,
        b"Lock after break"
    );
    assert_eq!(
        lock_control_layout(&monitor, true).label.text,
        b"Lock after break requested"
    );
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
