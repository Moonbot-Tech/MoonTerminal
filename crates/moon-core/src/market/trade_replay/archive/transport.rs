//! Apply a per-read deadline on the pinned ureq transport API without limiting the whole day.

use ureq::unversioned::transport::{Buffers, ConnectionDetails, Connector, NextTimeout, Transport};

/// Wrap the default HTTPS connector after its TLS and proxy negotiation has completed.
#[derive(Debug)]
pub(super) struct ReadTimeout;

impl<In: Transport> Connector<In> for ReadTimeout {
    type Out = TimedRead<In>;

    /// Retain the default connection and attach only the read timeout policy.
    fn connect(
        &self,
        _: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(TimedRead))
    }
}

/// Delegate all transport behavior while bounding each wait for fresh input.
#[derive(Debug)]
pub(super) struct TimedRead<In>(In);

impl<In: Transport> Transport for TimedRead<In> {
    /// Preserve the connector's own buffering.
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.0.buffers()
    }

    /// Preserve ureq's write timeout and TLS transport behavior.
    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.0.transmit_output(amount, timeout)
    }

    /// Restart the sixty-second deadline for each transport read, never for the whole body.
    fn await_input(&mut self, mut timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let read = ureq::unversioned::transport::time::Duration::from_secs(60);
        if timeout.after.is_not_happening() || timeout.after > read {
            timeout.after = read;
            timeout.reason = ureq::Timeout::RecvBody;
        }
        self.0.await_input(timeout)
    }

    /// Preserve pooled-connection health checks.
    fn is_open(&mut self) -> bool {
        self.0.is_open()
    }

    /// Preserve HTTPS identity after wrapping the TLS transport.
    fn is_tls(&self) -> bool {
        self.0.is_tls()
    }
}
