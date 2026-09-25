//! The owner's phone position, for the employees the owner shares it with:
//! the latest reading only, held in memory, never a history. The phone
//! (`PUT /phone/location`) replaces its whole recipient list on every
//! update; an empty list revokes. A reading expires ten minutes after it was
//! taken, so a phone that went quiet never reads as live. A turn of an
//! employee it is shared with hears each new reading as a `phone_location`
//! row; once nothing is shared with it any more, the conversation is told
//! the readings it heard are withdrawn (`harness::events`).

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// How long a reading stays good after the phone took it.
const READING_LIFETIME_SECS: i64 = 600;
/// How far ahead of this machine's clock a phone's reading may be dated.
const CLOCK_SKEW_SECS: i64 = 30;
/// Phones remembered at once, and recipients one phone may name.
const MAX_DEVICES: usize = 128;
const MAX_RECIPIENTS: usize = 64;
const MAX_ID_LEN: usize = 128;

/// One update from a phone, as the phone sends it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneReading {
    pub account_id: String,
    pub device_id: String,
    /// Grows with every update the phone sends; an older one is ignored.
    pub revision: i64,
    /// The employees the phone shares its position with; empty revokes.
    pub agent_ids: Vec<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub accuracy_metres: Option<f64>,
    /// When the phone took the reading, in Unix seconds.
    pub taken_at: Option<i64>,
}

/// The latest reading from each phone, until it expires.
#[derive(Default)]
pub struct PhoneLocations {
    /// (account, device) → (reading, expiry in Unix seconds).
    entries: Mutex<HashMap<(String, String), (PhoneReading, i64)>>,
}

impl PhoneLocations {
    /// Take a phone's update at `now`. An update older than the one held is
    /// ignored; a revocation is kept past any earlier reading's life, so a
    /// delayed older update can't share the position again.
    pub fn update(&self, mut reading: PhoneReading, now: i64) -> Result<(), &'static str> {
        if reading.revision < 1
            || reading.account_id.is_empty()
            || reading.account_id.len() > MAX_ID_LEN
            || reading.device_id.is_empty()
            || reading.device_id.len() > MAX_ID_LEN
            || reading.agent_ids.len() > MAX_RECIPIENTS
            || reading.agent_ids.iter().any(|id| id.len() > MAX_ID_LEN)
        {
            return Err("Invalid location recipients");
        }
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|_, (_, expires)| *expires > now);
        let key = (reading.account_id.clone(), reading.device_id.clone());
        if entries
            .get(&key)
            .is_some_and(|(held, _)| held.revision >= reading.revision)
        {
            return Ok(());
        }
        if !entries.contains_key(&key) && entries.len() >= MAX_DEVICES {
            return Err("Too many location devices");
        }
        if reading.agent_ids.is_empty() {
            reading.latitude = None;
            reading.longitude = None;
            reading.accuracy_metres = None;
            reading.taken_at = None;
            entries.insert(
                key,
                (reading, now + READING_LIFETIME_SECS + CLOCK_SKEW_SECS),
            );
            return Ok(());
        }
        let (Some(lat), Some(lon), Some(accuracy), Some(taken_at)) = (
            reading.latitude,
            reading.longitude,
            reading.accuracy_metres,
            reading.taken_at,
        ) else {
            return Err("Invalid or expired location reading");
        };
        let valid = lat.is_finite()
            && (-90.0..=90.0).contains(&lat)
            && lon.is_finite()
            && (-180.0..=180.0).contains(&lon)
            && accuracy.is_finite()
            && accuracy >= 0.0
            && taken_at >= now - READING_LIFETIME_SECS
            && taken_at <= now + CLOCK_SKEW_SECS;
        if !valid {
            return Err("Invalid or expired location reading");
        }
        // The phone names the primary employee by its role; its runs carry
        // no employee id.
        for id in &mut reading.agent_ids {
            if id == "assistant" || id == "main" {
                id.clear();
            }
        }
        entries.insert(key, (reading, taken_at + READING_LIFETIME_SECS));
        Ok(())
    }

    /// What `agent_id`'s turn is told at `now`: each good reading shared
    /// with it, or `None` when nothing is.
    pub fn reading_for(&self, agent_id: &str, now: i64) -> Option<SharedPosition> {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|_, (_, expires)| *expires > now);
        let mut shared: Vec<(&(String, String), &PhoneReading)> = entries
            .iter()
            .filter(|(_, (r, _))| r.agent_ids.iter().any(|id| id == agent_id))
            .map(|(key, (r, _))| (key, r))
            .collect();
        shared.sort_by(|a, b| a.0.cmp(b.0));
        let (lines, taken): (Vec<String>, Vec<String>) = shared
            .into_iter()
            .filter_map(|((_, device), r)| {
                let (lat, lon, accuracy, taken_at) = (r.latitude?, r.longitude?, r.accuracy_metres?, r.taken_at?);
                Some((
                    format!(
                        "The owner's phone is at {lat:.6}, {lon:.6}, accurate to about {accuracy:.0} m, read {} seconds ago. \
                         This is a location reading, not a route or an arrival estimate: an ETA needs a destination and \
                         routing. Location access does not authorize contacting anyone.",
                        (now - taken_at).max(0)
                    ),
                    format!("{device}@{taken_at}"),
                ))
            })
            .unzip();
        (!lines.is_empty()).then(|| SharedPosition {
            text: lines.join("\n"),
            taken: taken.join(","),
        })
    }
}

/// The readings a turn may be told, and which readings they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedPosition {
    /// The rows' words.
    pub text: String,
    /// Each reading's phone and the moment it was taken: the same readings
    /// give the same value, so a conversation is told each reading once.
    pub taken: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading() -> PhoneReading {
        PhoneReading {
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
    fn shared_only_with_its_employees_revoked_and_expired() {
        let l = PhoneLocations::default();
        l.update(reading(), 1000).unwrap();
        assert!(l.reading_for("other", 1001).is_none());
        assert!(
            l.reading_for("dispatch", 1001)
                .unwrap()
                .text
                .contains("40.000000, -111.000000")
        );
        let mut revoked = reading();
        revoked.agent_ids.clear();
        revoked.revision = 2;
        l.update(revoked, 1002).unwrap();
        assert!(l.reading_for("dispatch", 1002).is_none());
        l.update(reading(), 1003).unwrap();
        assert!(
            l.reading_for("dispatch", 1003).is_none(),
            "an older update never shares again"
        );
        let mut newer = reading();
        newer.revision = 3;
        l.update(newer, 1004).unwrap();
        assert!(l.reading_for("dispatch", 1004).is_some());
        assert!(l.reading_for("dispatch", 1600).is_none(), "expired");
    }

    #[test]
    fn stale_and_invalid_readings_are_refused_and_recipients_replaced() {
        let l = PhoneLocations::default();
        assert!(l.update(reading(), 2000).is_err(), "taken too long ago");
        let mut invalid = reading();
        invalid.latitude = Some(100.0);
        assert!(l.update(invalid, 1000).is_err());
        l.update(reading(), 1000).unwrap();
        let mut replaced = reading();
        replaced.revision = 2;
        replaced.agent_ids = vec!["other".into()];
        l.update(replaced, 1001).unwrap();
        assert!(l.reading_for("dispatch", 1001).is_none());
        assert!(l.reading_for("other", 1001).is_some());
    }

    #[test]
    fn the_primary_employee_is_named_by_its_role() {
        let l = PhoneLocations::default();
        let mut primary = reading();
        primary.agent_ids = vec!["assistant".into()];
        l.update(primary, 1000).unwrap();
        assert!(l.reading_for("", 1000).is_some());
    }
}
