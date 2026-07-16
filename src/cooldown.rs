/// Per-IP paint cooldown. One pixel every `period` per client IP.
///
/// The check reserves the slot atomically (check-and-set under one lock) so
/// two concurrent requests from the same IP can't both pass. If the write
/// later fails, `rollback` returns the slot so the client isn't charged.
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

pub struct Cooldown {
    period: Duration,
    last_paint: Mutex<HashMap<IpAddr, Instant>>,
}

impl Cooldown {
    pub fn new(period: Duration) -> Self {
        Self {
            period,
            last_paint: Mutex::new(HashMap::new()),
        }
    }

    pub fn period(&self) -> Duration {
        self.period
    }

    /// Try to claim a paint slot for `ip`. Returns the remaining wait on refusal.
    pub fn try_claim(&self, ip: IpAddr) -> Result<(), Duration> {
        let now = Instant::now();
        let mut map = self.last_paint.lock().unwrap();
        if let Some(last) = map.get(&ip) {
            let elapsed = now.duration_since(*last);
            if elapsed < self.period {
                return Err(self.period - elapsed);
            }
        }
        map.insert(ip, now);
        // Opportunistic cleanup: drop expired entries once the map grows.
        if map.len() > 10_000 {
            map.retain(|_, last| now.duration_since(*last) < self.period);
        }
        Ok(())
    }

    /// Return a claimed slot after a failed write.
    pub fn rollback(&self, ip: IpAddr) {
        self.last_paint.lock().unwrap().remove(&ip);
    }

    /// Remaining wait for `ip`, if any — for the UI timer.
    pub fn remaining(&self, ip: IpAddr) -> Option<Duration> {
        let map = self.last_paint.lock().unwrap();
        let last = map.get(&ip)?;
        self.period
            .checked_sub(last.elapsed())
            .filter(|d| !d.is_zero())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_then_refuse_then_rollback() {
        let cd = Cooldown::new(Duration::from_secs(60));
        let ip: IpAddr = "10.0.0.1".parse().unwrap();

        assert!(cd.try_claim(ip).is_ok());
        assert!(
            cd.try_claim(ip).is_err(),
            "second claim inside period must fail"
        );
        assert!(cd.remaining(ip).is_some());

        cd.rollback(ip);
        assert!(cd.try_claim(ip).is_ok(), "rollback must free the slot");
    }

    #[test]
    fn different_ips_are_independent() {
        let cd = Cooldown::new(Duration::from_secs(60));
        assert!(cd.try_claim("10.0.0.1".parse().unwrap()).is_ok());
        assert!(cd.try_claim("10.0.0.2".parse().unwrap()).is_ok());
    }

    #[test]
    fn zero_period_never_refuses() {
        let cd = Cooldown::new(Duration::ZERO);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(cd.try_claim(ip).is_ok());
        assert!(cd.try_claim(ip).is_ok());
        assert_eq!(cd.remaining(ip), None);
    }
}
