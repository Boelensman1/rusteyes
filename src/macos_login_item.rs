use crate::config::StartupConfig;
use smappservice_rs::{AppService, ServiceManagementError, ServiceStatus, ServiceType};
use tracing::{info, warn};

pub(crate) fn apply_config(config: StartupConfig) {
    let Some(open_at_login) = config.open_at_login else {
        return;
    };

    let service = AppService::new(ServiceType::MainApp);
    let status = service.status();

    match registration_action(open_at_login, status) {
        RegistrationAction::Register => register(&service),
        RegistrationAction::Unregister => unregister(&service),
        RegistrationAction::WarnNotFound => {
            warn!("macOS login item service was not found; run RustEyes from its app bundle");
        }
        RegistrationAction::WarnRequiresApproval => {
            warn!(
                "macOS login item requires user approval in System Settings > General > Login Items"
            );
        }
        RegistrationAction::Noop => {
            info!(%status, "macOS login item already matches config");
        }
    }
}

fn register(service: &AppService) {
    match service.register() {
        Ok(()) | Err(ServiceManagementError::AlreadyRegistered) => {
            info!("enabled macOS login item");
            warn_if_requires_approval(service.status());
        }
        Err(error) => {
            warn!(%error, "failed to enable macOS login item");
        }
    }
}

fn unregister(service: &AppService) {
    match service.unregister() {
        Ok(()) | Err(ServiceManagementError::JobNotFound) => {
            info!("disabled macOS login item");
        }
        Err(error) => {
            warn!(%error, "failed to disable macOS login item");
        }
    }
}

fn warn_if_requires_approval(status: ServiceStatus) {
    if matches!(status, ServiceStatus::RequiresApproval) {
        warn!("macOS login item requires user approval in System Settings > General > Login Items");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegistrationAction {
    Register,
    Unregister,
    WarnNotFound,
    WarnRequiresApproval,
    Noop,
}

fn registration_action(open_at_login: bool, status: ServiceStatus) -> RegistrationAction {
    match (open_at_login, status) {
        (true, ServiceStatus::NotRegistered) => RegistrationAction::Register,
        (true, ServiceStatus::RequiresApproval) => RegistrationAction::WarnRequiresApproval,
        (true | false, ServiceStatus::NotFound) => RegistrationAction::WarnNotFound,
        (true, ServiceStatus::Enabled) | (false, ServiceStatus::NotRegistered) => {
            RegistrationAction::Noop
        }
        (false, ServiceStatus::Enabled | ServiceStatus::RequiresApproval) => {
            RegistrationAction::Unregister
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RegistrationAction, registration_action};
    use smappservice_rs::ServiceStatus;

    #[test]
    fn enable_registers_only_when_not_registered() {
        assert_eq!(
            registration_action(true, ServiceStatus::NotRegistered),
            RegistrationAction::Register
        );
        assert_eq!(
            registration_action(true, ServiceStatus::Enabled),
            RegistrationAction::Noop
        );
    }

    #[test]
    fn enable_warns_when_user_approval_is_required() {
        assert_eq!(
            registration_action(true, ServiceStatus::RequiresApproval),
            RegistrationAction::WarnRequiresApproval
        );
    }

    #[test]
    fn disable_unregisters_enabled_or_pending_approval_login_item() {
        assert_eq!(
            registration_action(false, ServiceStatus::Enabled),
            RegistrationAction::Unregister
        );
        assert_eq!(
            registration_action(false, ServiceStatus::RequiresApproval),
            RegistrationAction::Unregister
        );
        assert_eq!(
            registration_action(false, ServiceStatus::NotRegistered),
            RegistrationAction::Noop
        );
    }

    #[test]
    fn not_found_is_reported_for_either_desired_state() {
        assert_eq!(
            registration_action(true, ServiceStatus::NotFound),
            RegistrationAction::WarnNotFound
        );
        assert_eq!(
            registration_action(false, ServiceStatus::NotFound),
            RegistrationAction::WarnNotFound
        );
    }
}
