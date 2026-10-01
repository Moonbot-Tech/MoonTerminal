//! Localized station headlines and progress; remote diagnostics stay secondary.

use moon_remote::error::StationError;
use moon_remote::hosts::HostEditError;
use moon_remote::progress::{Progress, Step};
use moon_remote::ssh::OpenError;
use moon_remote::station::access::RemovalError;
use moon_remote::station::bot::{BotReturnError, StationMayStillPoll};
use rust_i18n::t;

/// Render a typed failure with optional diagnostic detail, never raw English as the headline.
pub(crate) fn error(error: &anyhow::Error) -> String {
    let headline = if error.downcast_ref::<StationMayStillPoll>().is_some() {
        t!("station.error.may_poll")
    } else if let Some(kind) = error.downcast_ref::<BotReturnError>() {
        return match kind {
            BotReturnError::OldHelper => t!("telegram.server.return_old_helper").to_string(),
            BotReturnError::ReadFailed => t!("telegram.server.return_read_failed").to_string(),
        };
    } else if let Some(kind) = error.downcast_ref::<HostEditError>() {
        t!(match kind {
            HostEditError::Changed => "telegram.server.record_changed",
            HostEditError::NotSetUp => "telegram.server.no_known_server",
            HostEditError::SameAddress => "telegram.server.address_unchanged",
            HostEditError::AddressKnown => "telegram.server.address_in_use",
            HostEditError::NoFingerprint => "telegram.server.no_fingerprint",
        })
    } else if let Some(kind) = error.downcast_ref::<RemovalError>() {
        t!(match kind {
            RemovalError::Unsupported => "telegram.server.remove_unsupported",
            RemovalError::Unconfirmed => "telegram.server.remove_unconfirmed",
            RemovalError::LocalForgetFailed => "telegram.server.remove_local_failed",
            RemovalError::NotConfigured => "telegram.server.core_sync_refused",
        })
    } else if let Some(kind) = error.downcast_ref::<StationError>() {
        return with_detail(station_error(kind), error);
    } else if let Some(kind) = error.downcast_ref::<OpenError>() {
        match kind {
            OpenError::Refused { .. } => t!("station.error.login_refused"),
            OpenError::HostKeyChanged { .. } => t!("station.error.host_key_changed"),
            OpenError::Unreachable(_) => t!("station.error.unreachable"),
            OpenError::Other(inner) => {
                let headline = match inner.downcast_ref::<StationError>() {
                    Some(kind) => station_error(kind),
                    None => t!("station.error.failed").to_string(),
                };
                return with_detail(headline, error);
            }
        }
    } else {
        t!("station.error.failed")
    };
    with_detail(headline.to_string(), error)
}

/// Keep technical diagnostics on a separately labeled line below the action.
fn with_detail(headline: String, error: &anyhow::Error) -> String {
    format!(
        "{headline}\n{}",
        t!("station.detail", detail = format!("{error:#}"))
    )
}

/// Turn validation kinds into translated instructions, retaining only non-secret identifiers.
fn station_error(kind: &StationError) -> String {
    match kind {
        StationError::CoreMissing(uid) => t!("station.error.CoreMissing", uid = uid).to_string(),
        StationError::CoreWithoutKey(name) => {
            t!("station.error.CoreWithoutKey", name = name).to_string()
        }
        StationError::EmptyPassword => t!("station.error.EmptyPassword").to_string(),
        StationError::PasswordOneLine => t!("station.error.PasswordOneLine").to_string(),
        StationError::NoCorePicked => t!("station.error.NoCorePicked").to_string(),
        StationError::HelperTooOld => t!("station.error.HelperTooOld").to_string(),
        StationError::ServiceTooOld => t!("station.error.ServiceTooOld").to_string(),
        StationError::TerminalTooOld => t!("station.error.TerminalTooOld").to_string(),
        StationError::Timeout => t!("station.error.Timeout").to_string(),
        StationError::SystemdMissing => t!("station.error.SystemdMissing").to_string(),
        StationError::SystemdTooOld => t!("station.error.SystemdTooOld").to_string(),
        StationError::PasswordStillOffered => t!("station.error.PasswordStillOffered").to_string(),
        StationError::PasswordStillWorks => t!("station.error.PasswordStillWorks").to_string(),
        StationError::BotTokenMissing => t!("station.error.BotTokenMissing").to_string(),
        StationError::ConfigMissing => t!("station.error.ConfigMissing").to_string(),
        StationError::BotAlreadyPresent => t!("station.error.BotAlreadyPresent").to_string(),
        StationError::BotStopped => t!("station.error.BotStopped").to_string(),
        StationError::BotNotReady => t!("station.error.BotNotReady").to_string(),
        StationError::HostKeyUnconfirmed => t!("station.error.HostKeyUnconfirmed").to_string(),
        StationError::NoAptGet => t!("station.error.NoAptGet").to_string(),
        StationError::SshdNoInclude => t!("station.error.SshdNoInclude").to_string(),
    }
}

/// Raw helper output is retained in the log, while the progress block receives step names.
pub(crate) fn progress(event: Progress) -> Option<String> {
    match event {
        Progress::Text(text) => Some(text),
        Progress::Diagnostic(detail) => {
            log::info!("station: {detail}");
            None
        }
        Progress::Step { step, diagnostic } => {
            log::info!("station: {diagnostic}");
            Some(
                match step {
                    Step::AddressProbe => t!("station.progress.AddressProbe"),
                    Step::AddressVerify => t!("station.progress.AddressVerify"),
                    Step::StationRemove => t!("station.progress.StationRemove"),
                    Step::InstallProbe => t!("station.progress.InstallProbe"),
                    Step::Login => t!("station.progress.Login"),
                    Step::Pin => t!("station.progress.Pin"),
                    Step::Probe => t!("station.progress.Probe"),
                    Step::Admin => t!("station.progress.Admin"),
                    Step::Helper => t!("station.progress.Helper"),
                    Step::Service => t!("station.progress.Service"),
                    Step::Harden => t!("station.progress.Harden"),
                    Step::Firewall => t!("station.progress.Firewall"),
                    Step::FirewallWillClose => {
                        t!("station.progress.FirewallWillClose", ports = diagnostic)
                    }
                    Step::FirewallPortsUnknown => t!("station.progress.FirewallPortsUnknown"),
                    Step::Extras => t!("station.progress.Extras"),
                    Step::Install => t!("station.progress.Install"),
                    Step::Download => t!("station.progress.Download"),
                    Step::TapePending => t!("station.progress.TapePending"),
                    Step::TapeWritten => t!("station.progress.TapeWritten"),
                    Step::TapeReload => t!("station.progress.TapeReload"),
                    Step::TapeApplied => t!("station.progress.TapeApplied"),
                    Step::TokenWritten => t!("station.progress.TokenWritten"),
                    Step::TokenDropped => t!("station.progress.TokenDropped"),
                    Step::ChatsDropped => t!("station.progress.ChatsDropped"),
                    Step::CoreWritten => t!("station.progress.CoreWritten"),
                    Step::CoreDropped => t!("station.progress.CoreDropped"),
                    Step::Status => t!("station.progress.Status"),
                    Step::Update => t!("station.progress.Update"),
                    Step::NoNewRelease => t!("station.progress.NoNewRelease"),
                    Step::Unversioned => t!("station.progress.Unversioned"),
                    Step::ValuationSend => t!("station.progress.ValuationSend"),
                    Step::ValuationKept => t!("station.progress.ValuationKept"),
                    Step::ValuationWritten => t!("station.progress.ValuationWritten"),
                    Step::ChatsTransferred => t!("station.progress.ChatsTransferred"),
                    Step::BotStarted => t!("station.progress.BotStarted"),
                    Step::UndoHandover => t!("station.progress.UndoHandover"),
                    Step::BotRemoved => t!("station.progress.BotRemoved"),
                    Step::BotReady => t!("station.progress.BotReady"),
                }
                .to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests;
