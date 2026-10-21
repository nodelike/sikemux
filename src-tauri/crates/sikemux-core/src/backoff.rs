use std::time::Duration;

/// How long to wait before trying again after failures in a row: anywhere
/// from nothing to `first` doubled for each failure but the first, at most `cap`.
pub(crate) struct Backoff {
    pub first: Duration,
    pub cap: Duration,
}

impl Backoff {
    pub(crate) fn after(&self, failures: u32) -> Duration {
        let ceiling = self
            .first
            .saturating_mul(1u32 << failures.saturating_sub(1).min(16))
            .min(self.cap);
        let fraction = (uuid::Uuid::new_v4().as_u128() % 1000) as u32;
        ceiling.mul_f64(f64::from(fraction) / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_never_passes_its_cap() {
        let backoff = Backoff {
            first: Duration::from_secs(1),
            cap: Duration::from_secs(300),
        };
        for failures in [1, 2, 10, 40, u32::MAX] {
            assert!(backoff.after(failures) <= backoff.cap);
        }
        assert!(backoff.after(1) <= backoff.first);
    }
}
