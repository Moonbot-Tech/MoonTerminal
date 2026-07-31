//! Stage `command_error_contract`: a UI command aimed at a core that does not exist must fail.
//!
//! The failure mode this guards is silent, not loud: if the session layer answers `Ok(())` for a
//! missing core, the dialog that sent the command closes as if the work happened. Only an error
//! reaching the caller keeps that from looking like success.

use crate::Backend;

use crate::firetest::Runtime;
use crate::firetest::logging::firetest_info;

impl Runtime {
    /// Send one command to a sentinel core id that cannot exist and require an error back.
    pub(in crate::firetest) fn verify_command_error_contract(
        &self,
        backend: &Backend,
    ) -> Result<(), String> {
        let missing_core = u64::MAX;
        let session_exists = backend
            .session
            .sessions()
            .iter()
            .any(|session| session.id == missing_core);
        if session_exists {
            return Err("firetest missing-core sentinel unexpectedly exists".into());
        }

        match backend.session.refresh_transfer_assets(missing_core) {
            Ok(()) => Err(
                "session command to missing core returned Ok; UI could close dialog silently"
                    .into(),
            ),
            Err(error) => {
                firetest_info(&format!(
                    "[firetest] command_error_contract missing_core={missing_core} err={error}"
                ));
                Ok(())
            }
        }
    }
}
