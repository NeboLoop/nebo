//! Latest consented phone positions, never a location history. The mobile
//! device replaces its full recipient list on every update; an empty list
//! revokes access. Leases expire so an offline phone cannot imply live tracking.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneLocation {
    pub account_id: String,
    pub device_id: String,
    pub revision: i64,
    pub agent_ids: Vec<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub accuracy_metres: Option<f64>,
    pub taken_at: Option<i64>,
}
#[derive(Default)]
pub struct Locations {
    entries: HashMap<(String, String), (PhoneLocation, i64)>,
}
impl Locations {
    pub fn update(&mut self, mut fix: PhoneLocation, now: i64) -> Result<(), &'static str> {
        if fix.revision < 1
            || fix.account_id.is_empty()
            || fix.account_id.len() > 128
            || fix.device_id.is_empty()
            || fix.device_id.len() > 128
            || fix.agent_ids.len() > 64
            || fix.agent_ids.iter().any(|id| id.len() > 128)
        {
            return Err("Invalid location recipients");
        }
        self.entries.retain(|_, (_, expires)| *expires > now);
        let key = (fix.account_id.clone(), fix.device_id.clone());
        if self
            .entries
            .get(&key)
            .is_some_and(|(old, _)| old.revision >= fix.revision)
        {
            return Ok(());
        }
        if !self.entries.contains_key(&key) && self.entries.len() >= 128 {
            return Err("Too many location devices");
        }
        if fix.agent_ids.is_empty() {
            // Keep a tombstone beyond the lifetime of any earlier reading.
            fix.latitude = None;
            fix.longitude = None;
            fix.accuracy_metres = None;
            fix.taken_at = None;
            self.entries.insert(key, (fix, now + 630));
            return Ok(());
        }
        let valid = fix
            .latitude
            .is_some_and(|v| v.is_finite() && (-90.0..=90.0).contains(&v))
            && fix
                .longitude
                .is_some_and(|v| v.is_finite() && (-180.0..=180.0).contains(&v))
            && fix
                .accuracy_metres
                .is_some_and(|v| v.is_finite() && v >= 0.0)
            && fix
                .taken_at
                .is_some_and(|v| v >= now - 600 && v <= now + 30);
        if !valid {
            return Err("Invalid or expired location reading");
        }
        if !self.entries.contains_key(&key) && self.entries.len() >= 128 {
            return Err("Too many location devices");
        }
        for id in &mut fix.agent_ids {
            if id == "assistant" || id == "main" {
                id.clear();
            }
        }
        let expires = fix.taken_at.unwrap() + 600;
        self.entries.insert(key, (fix, expires));
        Ok(())
    }
    pub fn context(&mut self, agent_id: &str, now: i64) -> String {
        self.entries.retain(|_, (_, expires)| *expires > now);
        let mut output = String::new();
        for (fix, _) in self
            .entries
            .values()
            .filter(|(f, _)| f.agent_ids.iter().any(|id| id == agent_id))
        {
            output.push_str(&format!("\nPhone location shared with this employee: {:.6}, {:.6}; accuracy {:.0} metres; observed at Unix time {} ({} seconds ago). This is a location reading, not a route or arrival estimate. Use a destination and routing information for an ETA. Location access does not authorize contacting anyone.\n", fix.latitude.unwrap(), fix.longitude.unwrap(), fix.accuracy_metres.unwrap(), fix.taken_at.unwrap(), now - fix.taken_at.unwrap()));
        }
        output
    }
}
fn store() -> &'static Mutex<Locations> {
    static STORE: OnceLock<Mutex<Locations>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Locations::default()))
}
pub fn update(fix: PhoneLocation) -> Result<(), &'static str> {
    store()
        .lock()
        .map_err(|_| "Location unavailable")?
        .update(fix, chrono::Utc::now().timestamp())
}
pub fn context(agent_id: &str) -> String {
    store()
        .lock()
        .map(|mut s| s.context(agent_id, chrono::Utc::now().timestamp()))
        .unwrap_or_default()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fix() -> PhoneLocation {
        PhoneLocation {
            account_id: "owner".into(),
            device_id: "phone".into(),
            revision: 1,
            agent_ids: vec!["dispatch".into()],
            latitude: Some(40.0),
            longitude: Some(-111.0),
            accuracy_metres: Some(10.0),
            taken_at: Some(1000),
        }
    }
    #[test]
    fn consent_scope_revoke_and_expiry() {
        let mut s = Locations::default();
        s.update(fix(), 1000).unwrap();
        assert!(s.context("other", 1001).is_empty());
        assert!(s.context("dispatch", 1001).contains("40.000000"));
        let mut revoked = fix();
        revoked.agent_ids.clear();
        revoked.revision = 2;
        s.update(revoked, 1002).unwrap();
        assert!(s.context("dispatch", 1002).is_empty());
        s.update(fix(), 1003).unwrap();
        assert!(s.context("dispatch", 1003).is_empty());
        let mut newer = fix();
        newer.revision = 3;
        s.update(newer, 1004).unwrap();
        assert!(!s.context("dispatch", 1004).is_empty());
        assert!(s.context("dispatch", 1600).is_empty());
    }
    #[test]
    fn stale_invalid_and_recipient_replacement() {
        let mut s = Locations::default();
        assert!(s.update(fix(), 2000).is_err());
        let mut invalid = fix();
        invalid.latitude = Some(100.0);
        assert!(s.update(invalid, 1000).is_err());
        s.update(fix(), 1000).unwrap();
        let mut replaced = fix();
        replaced.revision = 2;
        replaced.agent_ids = vec!["other".into()];
        s.update(replaced, 1001).unwrap();
        assert!(s.context("dispatch", 1001).is_empty());
        assert!(!s.context("other", 1001).is_empty());
    }
}
