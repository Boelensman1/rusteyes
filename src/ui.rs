use crate::backend::{DisableRequest, RuntimeEvent};
use crate::config::Breaks;
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiConfig {
    break_names: Vec<String>,
    disable_presets: Vec<Duration>,
}

impl UiConfig {
    pub(crate) fn from_config(breaks: &Breaks, disable_presets: &[Duration]) -> Self {
        let mut break_names = breaks
            .types
            .iter()
            .map(|(name, break_type)| (break_type.interval, name.clone()))
            .collect::<Vec<_>>();
        break_names.sort_by(|(left_interval, left_name), (right_interval, right_name)| {
            left_interval
                .cmp(right_interval)
                .then_with(|| left_name.cmp(right_name))
        });

        Self {
            break_names: break_names
                .into_iter()
                .map(|(_interval, name)| name)
                .collect(),
            disable_presets: disable_presets.to_vec(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UiCommand {
    ShowPreBreakNotification(PreBreakNotification),
    ClearPreBreakNotification,
    ShowNotification(UiNotification),
    UpdateStatus(StatusDisplay),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StatusDisplay {
    Active(Duration),
    DisabledFor(Duration),
    DisabledUntilRestart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreBreakNotification {
    pub(crate) break_name: String,
    pub(crate) starts_after: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiNotification {
    pub(crate) summary: String,
    pub(crate) body: String,
}

#[derive(Debug)]
pub(crate) struct RuntimeUi {
    event_receiver: Option<flume::Receiver<RuntimeEvent>>,
    command_sender: Option<flume::Sender<UiCommand>>,
}

impl RuntimeUi {
    #[cfg(test)]
    pub(crate) const fn inactive() -> Self {
        Self {
            event_receiver: None,
            command_sender: None,
        }
    }

    pub(crate) fn from_handle(handle: UiHandle) -> Self {
        Self {
            event_receiver: Some(handle.event_receiver),
            command_sender: Some(handle.command_sender),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_command_sender(command_sender: flume::Sender<UiCommand>) -> Self {
        Self {
            event_receiver: None,
            command_sender: Some(command_sender),
        }
    }

    pub(crate) fn event_receiver(&self) -> Option<&flume::Receiver<RuntimeEvent>> {
        self.event_receiver.as_ref()
    }

    pub(crate) fn clear_event_receiver(&mut self) {
        self.event_receiver = None;
    }

    pub(crate) fn send_command(&self, command: UiCommand) -> Result<(), UiCommandError> {
        let Some(command_sender) = &self.command_sender else {
            return Ok(());
        };

        command_sender
            .send(command)
            .map_err(|_| UiCommandError::Stopped)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiCommandError {
    Stopped,
}

impl fmt::Display for UiCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => formatter.write_str("ui command channel closed"),
        }
    }
}

impl std::error::Error for UiCommandError {}

#[derive(Debug)]
pub(crate) struct UiHandle {
    event_receiver: flume::Receiver<RuntimeEvent>,
    command_sender: flume::Sender<UiCommand>,
}

#[derive(Debug)]
struct UiAppChannels {
    event_sender: flume::Sender<RuntimeEvent>,
    command_receiver: flume::Receiver<UiCommand>,
}

fn ui_channels() -> (UiHandle, UiAppChannels) {
    let (event_sender, event_receiver) = flume::unbounded();
    let (command_sender, command_receiver) = flume::unbounded();

    (
        UiHandle {
            event_receiver,
            command_sender,
        },
        UiAppChannels {
            event_sender,
            command_receiver,
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UiMenuAction {
    StartBreak(String),
    DisableFor(Duration),
    DisableUntilRestart,
    Enable,
    Quit,
}

impl UiMenuAction {
    fn runtime_event(&self) -> RuntimeEvent {
        match self {
            Self::StartBreak(name) => RuntimeEvent::StartManualBreak(name.clone()),
            Self::DisableFor(duration) => RuntimeEvent::Disable(DisableRequest::For(*duration)),
            Self::DisableUntilRestart => RuntimeEvent::Disable(DisableRequest::UntilRestart),
            Self::Enable => RuntimeEvent::Enable,
            Self::Quit => RuntimeEvent::Shutdown,
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod app {
    use super::{
        PreBreakNotification, RuntimeEvent, RuntimeUi, StatusDisplay, UiAppChannels, UiCommand,
        UiConfig, UiHandle, UiMenuAction, UiNotification, ui_channels,
    };
    use notify_rust::{Notification, NotificationHandle, Timeout};
    use std::collections::BTreeMap;
    use std::fmt;
    use std::io;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;
    use tao::event::{Event, StartCause};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
    use tracing::warn;
    use tray_icon::menu::{MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    const ICON_SIZE: u32 = 64;
    const ICON_BYTE_LEN: usize = 64 * 64 * 4;
    const TRAY_ICON_RGBA: &[u8; ICON_BYTE_LEN] =
        include_bytes!("../package/icons/rusteyes-tray.rgba");
    const APP_NAME: &str = "RustEyes";
    const TOOLTIP: &str = "RustEyes";
    // Sound played the first time the pre-break notification appears. The name
    // vocabularies differ per platform: Linux uses the freedesktop XDG sound
    // naming spec, macOS expects a system sound from /System/Library/Sounds.
    #[cfg(not(target_os = "macos"))]
    const PRE_BREAK_NOTIFICATION_SOUND: &str = "message";
    #[cfg(target_os = "macos")]
    const PRE_BREAK_NOTIFICATION_SOUND: &str = "Ping";
    const STATUS_MENU_ID: &str = "status";
    pub(crate) fn run<F>(config: UiConfig, start_runtime: F) -> Result<(), UiError>
    where
        F: FnOnce(UiLoopProxy, UiHandle) -> Result<JoinHandle<()>, UiError> + 'static,
    {
        let mut event_loop = EventLoopBuilder::<UiLoopEvent>::with_user_event().build();
        configure_tray_only_app(&mut event_loop);
        configure_notification_application();
        let proxy = UiLoopProxy::new(event_loop.create_proxy());
        let (ui_handle, app_channels) = ui_channels();
        let UiAppChannels {
            event_sender,
            command_receiver,
        } = app_channels;
        let runtime_thread = start_runtime(proxy.clone(), ui_handle)?;
        let _command_forwarder = spawn_command_forwarder(command_receiver, proxy.clone())?;
        let mut app = UiApp::new(config, event_sender, runtime_thread);

        event_loop.run(move |event, _target, control_flow| {
            *control_flow = ControlFlow::Wait;

            match event {
                Event::NewEvents(StartCause::Init) => {
                    if let Err(error) = app.initialize(&proxy) {
                        eprintln!("rusteyes: {error}");
                        app.request_shutdown();
                        app.join_runtime();
                        *control_flow = ControlFlow::ExitWithCode(1);
                    }
                }
                Event::UserEvent(UiLoopEvent::Menu(menu_id)) => app.handle_menu(&menu_id),
                Event::UserEvent(UiLoopEvent::Command(command)) => {
                    app.handle_command(command);
                }
                Event::UserEvent(UiLoopEvent::RuntimeStopped) => {
                    app.clear_pre_break_notification();
                    app.join_runtime();
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            }
        });
    }

    fn spawn_command_forwarder(
        command_receiver: flume::Receiver<UiCommand>,
        proxy: UiLoopProxy,
    ) -> Result<JoinHandle<()>, UiError> {
        thread::Builder::new()
            .name(String::from("rusteyes-ui-command-forwarder"))
            .spawn(move || {
                while let Ok(command) = command_receiver.recv() {
                    proxy.send(UiLoopEvent::Command(command));
                }
            })
            .map_err(|error| UiError::command_forwarder_thread(&error))
    }

    #[cfg(target_os = "macos")]
    fn configure_tray_only_app(event_loop: &mut tao::event_loop::EventLoop<UiLoopEvent>) {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};

        event_loop.set_activation_policy(ActivationPolicy::Accessory);
        event_loop.set_dock_visibility(false);
    }

    #[cfg(not(target_os = "macos"))]
    fn configure_tray_only_app(_event_loop: &mut tao::event_loop::EventLoop<UiLoopEvent>) {}

    #[cfg(target_os = "macos")]
    fn configure_notification_application() {
        match notify_rust::request_auth_blocking() {
            Ok(true) => {}
            Ok(false) => {
                warn!("macOS notification permission denied");
            }
            Err(error) => {
                warn!(%error, "failed to request macOS notification permission");
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn configure_notification_application() {}

    pub(crate) fn runtime_ui_from_handle(handle: UiHandle) -> RuntimeUi {
        RuntimeUi::from_handle(handle)
    }

    #[derive(Debug, Clone)]
    pub(crate) struct UiLoopProxy {
        proxy: EventLoopProxy<UiLoopEvent>,
    }

    impl UiLoopProxy {
        fn new(proxy: EventLoopProxy<UiLoopEvent>) -> Self {
            Self { proxy }
        }

        pub(crate) fn runtime_stopped(&self) {
            self.send(UiLoopEvent::RuntimeStopped);
        }

        fn send(&self, event: UiLoopEvent) {
            _ = self.proxy.send_event(event);
        }
    }

    #[derive(Debug)]
    enum UiLoopEvent {
        Menu(String),
        Command(UiCommand),
        RuntimeStopped,
    }

    struct UiApp {
        config: UiConfig,
        event_sender: flume::Sender<RuntimeEvent>,
        runtime_thread: Option<JoinHandle<()>>,
        tray_icon: Option<TrayIcon>,
        status_item: Option<MenuItem>,
        enable_item: Option<MenuItem>,
        pre_break_notification: Option<NotificationHandle>,
        menu_actions: BTreeMap<String, UiMenuAction>,
        initialized: bool,
    }

    impl UiApp {
        fn new(
            config: UiConfig,
            event_sender: flume::Sender<RuntimeEvent>,
            runtime_thread: JoinHandle<()>,
        ) -> Self {
            Self {
                config,
                event_sender,
                runtime_thread: Some(runtime_thread),
                tray_icon: None,
                status_item: None,
                enable_item: None,
                pre_break_notification: None,
                menu_actions: BTreeMap::new(),
                initialized: false,
            }
        }

        fn initialize(&mut self, proxy: &UiLoopProxy) -> Result<(), UiError> {
            if self.initialized {
                return Ok(());
            }

            let menu_proxy = proxy.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                menu_proxy.send(UiLoopEvent::Menu(event.id().as_ref().to_owned()));
            }));

            let built_tray_icon = build_tray_icon(&self.config, &mut self.menu_actions)?;
            self.status_item = Some(built_tray_icon.status_item);
            self.enable_item = Some(built_tray_icon.enable_item);
            self.tray_icon = Some(built_tray_icon.tray_icon);
            self.initialized = true;
            Ok(())
        }

        fn handle_menu(&self, menu_id: &str) {
            let Some(action) = self.menu_actions.get(menu_id) else {
                warn!(menu_id, "ignoring unknown tray menu action");
                return;
            };

            if self.event_sender.send(action.runtime_event()).is_err() {
                warn!(?action, "failed to send tray menu action to runtime");
            }
        }

        fn handle_command(&mut self, command: UiCommand) {
            match command {
                UiCommand::ShowPreBreakNotification(notification) => {
                    if let Err(error) =
                        show_pre_break_notification(&mut self.pre_break_notification, &notification)
                    {
                        warn!(%error, ?notification, "failed to show pre-break notification");
                    }
                }
                UiCommand::ClearPreBreakNotification => {
                    self.clear_pre_break_notification();
                }
                UiCommand::ShowNotification(notification) => {
                    if let Err(error) = show_notification(&notification) {
                        warn!(%error, ?notification, "failed to show notification");
                    }
                }
                UiCommand::UpdateStatus(status) => {
                    if let Some(item) = &self.status_item {
                        item.set_text(status_menu_text(&status));
                    }
                    if let Some(item) = &self.enable_item {
                        item.set_enabled(!matches!(status, StatusDisplay::Active(_)));
                    }
                }
            }
        }

        fn request_shutdown(&self) {
            _ = self.event_sender.send(RuntimeEvent::Shutdown);
        }

        fn clear_pre_break_notification(&mut self) {
            clear_pre_break_notification(&mut self.pre_break_notification);
        }

        fn join_runtime(&mut self) {
            if let Some(thread) = self.runtime_thread.take()
                && thread.join().is_err()
            {
                warn!("runtime thread panicked");
            }
        }
    }

    struct BuiltTrayIcon {
        tray_icon: TrayIcon,
        status_item: MenuItem,
        enable_item: MenuItem,
    }

    fn build_tray_icon(
        config: &UiConfig,
        actions: &mut BTreeMap<String, UiMenuAction>,
    ) -> Result<BuiltTrayIcon, UiError> {
        let menu = Submenu::new(APP_NAME, true);

        let status_item = MenuItem::with_id(
            MenuId::new(STATUS_MENU_ID),
            status_menu_text(&StatusDisplay::Active(Duration::ZERO)),
            false,
            None,
        );
        menu.append(&status_item)
            .map_err(|error| UiError::menu(error.to_string()))?;
        append_separator(&menu)?;

        for name in &config.break_names {
            let menu_id = start_break_menu_id(name);
            let item = MenuItem::with_id(
                MenuId::new(&menu_id),
                format!("Start {name} break"),
                true,
                None,
            );
            menu.append(&item)
                .map_err(|error| UiError::menu(error.to_string()))?;
            actions.insert(menu_id, UiMenuAction::StartBreak(name.clone()));
        }

        append_separator(&menu)?;

        for (index, duration) in config.disable_presets.iter().enumerate() {
            let menu_id = disable_for_menu_id(index);
            let item = MenuItem::with_id(
                MenuId::new(&menu_id),
                format!("Disable for {}", humantime::format_duration(*duration)),
                true,
                None,
            );
            menu.append(&item)
                .map_err(|error| UiError::menu(error.to_string()))?;
            actions.insert(menu_id, UiMenuAction::DisableFor(*duration));
        }

        let disable_until_restart_id = String::from("disable-until-restart");
        let disable_until_restart_item = MenuItem::with_id(
            MenuId::new(&disable_until_restart_id),
            "Disable until restart",
            true,
            None,
        );
        menu.append(&disable_until_restart_item)
            .map_err(|error| UiError::menu(error.to_string()))?;
        actions.insert(disable_until_restart_id, UiMenuAction::DisableUntilRestart);

        // Always present; clickable only while disabled. The app starts enabled,
        // so the item begins greyed out and is toggled via UpdateStatus.
        let enable_id = String::from("enable");
        let enable_item = MenuItem::with_id(MenuId::new(&enable_id), "Enable", false, None);
        menu.append(&enable_item)
            .map_err(|error| UiError::menu(error.to_string()))?;
        actions.insert(enable_id, UiMenuAction::Enable);

        append_separator(&menu)?;

        let quit_id = String::from("quit");
        let quit_item = MenuItem::with_id(MenuId::new(&quit_id), "Quit", true, None);
        menu.append(&quit_item)
            .map_err(|error| UiError::menu(error.to_string()))?;
        actions.insert(quit_id, UiMenuAction::Quit);

        let icon = rusteyes_icon()?;
        let builder = TrayIconBuilder::new()
            .with_tooltip(TOOLTIP)
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(true);

        // macOS menu-bar icons are monochrome template images: the system tints
        // the alpha silhouette to match the menu bar (white on dark, black on
        // light). RGB is ignored, so the existing icon's shape is reused.
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_as_template(true);

        let tray_icon = builder
            .build()
            .map_err(|error| UiError::tray_icon(error.to_string()))?;

        Ok(BuiltTrayIcon {
            tray_icon,
            status_item,
            enable_item,
        })
    }

    fn append_separator(menu: &Submenu) -> Result<(), UiError> {
        let separator = PredefinedMenuItem::separator();
        menu.append(&separator)
            .map_err(|error| UiError::menu(error.to_string()))
    }

    fn rusteyes_icon() -> Result<Icon, UiError> {
        Icon::from_rgba(TRAY_ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE)
            .map_err(|error| UiError::tray_icon(error.to_string()))
    }

    fn show_pre_break_notification(
        handle: &mut Option<NotificationHandle>,
        notification: &PreBreakNotification,
    ) -> Result<(), UiError> {
        let notification = pre_break_ui_notification(notification);

        if let Some(handle) = handle {
            handle
                .summary(&notification.summary)
                .body(&notification.body)
                .timeout(Timeout::Never);
            return handle
                .update()
                .map_err(|error| UiError::notification(error.to_string()));
        }

        *handle = Some(
            build_notification(&notification, Timeout::Never)
                .sound_name(PRE_BREAK_NOTIFICATION_SOUND)
                .show()
                .map_err(|error| UiError::notification(error.to_string()))?,
        );
        Ok(())
    }

    fn clear_pre_break_notification(handle: &mut Option<NotificationHandle>) {
        if let Some(handle) = handle.take() {
            handle.close();
        }
    }

    fn pre_break_ui_notification(notification: &PreBreakNotification) -> UiNotification {
        UiNotification {
            summary: String::from("RustEyes break soon"),
            body: format!(
                "{} break starts in {}.",
                notification.break_name,
                humantime::format_duration(notification.starts_after)
            ),
        }
    }

    fn show_notification(notification: &UiNotification) -> Result<(), UiError> {
        build_notification(notification, Timeout::Milliseconds(6_000))
            .show()
            .map(|_| ())
            .map_err(|error| UiError::notification(error.to_string()))
    }

    fn build_notification(notification: &UiNotification, timeout: Timeout) -> Notification {
        let mut notification_builder = Notification::new();
        notification_builder
            .appname(APP_NAME)
            .summary(&notification.summary)
            .body(&notification.body)
            .timeout(timeout);
        notification_builder
    }

    fn status_menu_text(status: &StatusDisplay) -> String {
        match status {
            StatusDisplay::Active(active_time) => {
                format!(
                    "Active time: {}",
                    humantime::format_duration(Duration::from_secs(active_time.as_secs()))
                )
            }
            StatusDisplay::DisabledFor(remaining) => {
                format!("Disabled for {}", humantime::format_duration(*remaining))
            }
            StatusDisplay::DisabledUntilRestart => String::from("Permanently disabled"),
        }
    }

    fn start_break_menu_id(name: &str) -> String {
        format!("start-break:{name}")
    }

    fn disable_for_menu_id(index: usize) -> String {
        format!("disable-for:{index}")
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct UiError {
        kind: UiErrorKind,
    }

    impl UiError {
        pub(crate) fn runtime_thread(error: &io::Error) -> Self {
            Self {
                kind: UiErrorKind::RuntimeThread {
                    message: error.to_string(),
                },
            }
        }

        fn command_forwarder_thread(error: &io::Error) -> Self {
            Self {
                kind: UiErrorKind::CommandForwarderThread {
                    message: error.to_string(),
                },
            }
        }

        fn menu(message: String) -> Self {
            Self {
                kind: UiErrorKind::Menu { message },
            }
        }

        fn tray_icon(message: String) -> Self {
            Self {
                kind: UiErrorKind::TrayIcon { message },
            }
        }

        fn notification(message: String) -> Self {
            Self {
                kind: UiErrorKind::Notification { message },
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum UiErrorKind {
        RuntimeThread { message: String },
        CommandForwarderThread { message: String },
        Menu { message: String },
        TrayIcon { message: String },
        Notification { message: String },
    }

    impl fmt::Display for UiError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match &self.kind {
                UiErrorKind::RuntimeThread { message } => {
                    write!(formatter, "failed to spawn runtime thread: {message}")
                }
                UiErrorKind::CommandForwarderThread { message } => {
                    write!(
                        formatter,
                        "failed to spawn UI command forwarder thread: {message}"
                    )
                }
                UiErrorKind::Menu { message } => {
                    write!(formatter, "failed to build tray menu: {message}")
                }
                UiErrorKind::TrayIcon { message } => {
                    write!(formatter, "failed to build tray icon: {message}")
                }
                UiErrorKind::Notification { message } => {
                    write!(formatter, "failed to show notification: {message}")
                }
            }
        }
    }

    impl std::error::Error for UiError {}

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn menu_ids_are_stable() {
            assert_eq!(start_break_menu_id("short"), "start-break:short");
            assert_eq!(disable_for_menu_id(2), "disable-for:2");
        }

        #[test]
        fn status_menu_text_renders_each_state() {
            assert_eq!(
                status_menu_text(&StatusDisplay::Active(Duration::from_secs(65))),
                "Active time: 1m 5s"
            );
            assert_eq!(
                status_menu_text(&StatusDisplay::DisabledFor(Duration::from_secs(65))),
                "Disabled for 1m 5s"
            );
            assert_eq!(
                status_menu_text(&StatusDisplay::DisabledUntilRestart),
                "Permanently disabled"
            );
        }

        #[test]
        fn active_status_menu_text_floors_to_whole_seconds() {
            assert_eq!(
                status_menu_text(&StatusDisplay::Active(Duration::from_nanos(999_999_999))),
                "Active time: 0s"
            );
            assert_eq!(
                status_menu_text(&StatusDisplay::Active(Duration::from_millis(8_391))),
                "Active time: 8s"
            );
        }

        #[test]
        fn tray_icon_asset_matches_declared_size() {
            assert_eq!(TRAY_ICON_RGBA.len(), ICON_BYTE_LEN);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) use app::{UiError, run, runtime_ui_from_handle};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BreakTypeConfig;
    use std::collections::BTreeMap;

    #[test]
    fn menu_actions_map_to_runtime_events() {
        assert_eq!(
            UiMenuAction::StartBreak(String::from("short")).runtime_event(),
            RuntimeEvent::StartManualBreak(String::from("short"))
        );
        assert_eq!(
            UiMenuAction::DisableFor(Duration::from_secs(30)).runtime_event(),
            RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30)))
        );
        assert_eq!(
            UiMenuAction::DisableUntilRestart.runtime_event(),
            RuntimeEvent::Disable(DisableRequest::UntilRestart)
        );
        assert_eq!(UiMenuAction::Enable.runtime_event(), RuntimeEvent::Enable);
        assert_eq!(UiMenuAction::Quit.runtime_event(), RuntimeEvent::Shutdown);
    }

    #[test]
    fn break_menu_names_are_ordered_by_shortest_cadence() {
        let mut types = BTreeMap::new();
        types.insert(
            String::from("long"),
            BreakTypeConfig {
                interval: 3,
                duration: Duration::from_mins(5),
                messages: vec![String::from("Long break")],
                autolock: true,
            },
        );
        types.insert(
            String::from("short"),
            BreakTypeConfig {
                interval: 1,
                duration: Duration::from_secs(20),
                messages: vec![String::from("Short break")],
                autolock: false,
            },
        );

        let ui_config = UiConfig::from_config(
            &Breaks {
                after_active: Duration::from_mins(20),
                reset_after_idle: None,
                reset_count_after_idle: None,
                types,
            },
            &[],
        );

        assert_eq!(ui_config.break_names, ["short", "long"]);
    }
}
