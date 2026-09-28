//! The owner's phone position, for the employees the owner shares it with:
//! the latest reading only, held in memory, never a history. The phone
//! (`PUT /phone/location`) replaces its whole recipient list on every
//! update; an empty list revokes. A reading expires ten minutes after it was
//! taken, so a phone that went quiet never reads as live. A turn of an
//! employee it is shared with hears each new reading as a `phone_location`
//! row; once nothing is shared with it any more, the conversation is told
//! the readings it heard are withdrawn (`harness::events`). When the bot has
//! a Location with coordinates (the office, Bot settings → Location), each
//! reading carries the owner's straight-line distance from it, which Nebo
//! works out (haversine) so the model never does the arithmetic; it goes
//! and comes with the reading.

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
/// The Earth's mean radius (IUGG), for the great-circle distance.
const EARTH_RADIUS_KM: f64 = 6371.0088;
const KM_PER_MILE: f64 = 1.609344;

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
        for id in &mut reading.agent_ids {
            *id = recipient(id).to_string();
        }
        entries.insert(key, (reading, taken_at + READING_LIFETIME_SECS));
        Ok(())
    }

    /// What `agent_id`'s turn is told at `now`: each good reading shared
    /// with it, with the owner's distance from the [`Office`] when there is
    /// one, or `None` when nothing is shared.
    pub fn reading_for(&self, agent_id: &str, now: i64, office: Option<Office>) -> Option<SharedPosition> {
        let agent_id = recipient(agent_id);
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
                let mut line = format!(
                    "The owner's phone is at {lat:.6}, {lon:.6}, accurate to about {accuracy:.0} m, read {} seconds ago. \
                     This is a location reading, not a route or an arrival estimate: an ETA needs a destination and \
                     routing. Location access does not authorize contacting anyone.",
                    (now - taken_at).max(0)
                );
                if let Some(office) = office {
                    line.push(' ');
                    line.push_str(&office.distance_text((lat, lon)));
                }
                Some((line, format!("{device}@{taken_at}")))
            })
            .unzip();
        (!lines.is_empty()).then(|| SharedPosition {
            text: lines.join("\n"),
            // The office is part of what was told: a moved office, or a
            // changed unit, tells the distance again.
            taken: match office {
                Some(o) => format!("{};office@{:.6},{:.6},{}", taken.join(","), o.latitude, o.longitude, o.unit.name()),
                None => taken.join(","),
            },
        })
    }
}

/// Where the owner's distance is measured from: the bot's Location (the
/// office), once it has coordinates, and the unit the owner reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Office {
    pub latitude: f64,
    pub longitude: f64,
    pub unit: DistanceUnit,
}

impl Office {
    /// The distance fact for a reading at `at`: "The owner is about 4.2
    /// miles from the office (straight line)."
    pub fn distance_text(&self, at: (f64, f64)) -> String {
        let km = haversine_km((self.latitude, self.longitude), at);
        let amount = match self.unit {
            DistanceUnit::Miles => km / KM_PER_MILE,
            DistanceUnit::Kilometres => km,
        };
        let amount = if amount < 100.0 { format!("{amount:.1}") } else { format!("{amount:.0}") };
        format!("The owner is about {amount} {} from the office (straight line).", self.unit.name())
    }
}

/// Miles or kilometres, as the owner reads distances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceUnit {
    Miles,
    Kilometres,
}

impl DistanceUnit {
    /// The unit for the owner's language setting (`user_preferences.language`,
    /// e.g. `en`, `en-GB`, `pt-BR`). A region names its country's road unit:
    /// the United States, the United Kingdom, Liberia and Myanmar use miles,
    /// every other country kilometres. With no region, English (and no
    /// setting at all) reads miles and any other language kilometres.
    pub fn for_language(language: &str) -> Self {
        const MILE_COUNTRIES: [&str; 4] = ["US", "GB", "LR", "MM"];
        let mut parts = language.split(['-', '_']);
        let lang = parts.next().unwrap_or_default().trim();
        match parts.last() {
            Some(region) if MILE_COUNTRIES.iter().any(|c| c.eq_ignore_ascii_case(region)) => Self::Miles,
            Some(_) => Self::Kilometres,
            None if lang.is_empty() || lang.eq_ignore_ascii_case("en") => Self::Miles,
            None => Self::Kilometres,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Miles => "miles",
            Self::Kilometres => "km",
        }
    }
}

/// The great-circle distance between two (latitude, longitude) points in
/// degrees, in kilometres (the haversine formula).
pub fn haversine_km(from: (f64, f64), to: (f64, f64)) -> f64 {
    let (lat1, lat2) = (from.0.to_radians(), to.0.to_radians());
    let d_lat = lat2 - lat1;
    let d_lon = (to.1 - from.1).to_radians();
    let a = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * a.sqrt().asin()
}

/// The employee a reading is shared with, as a turn names it. The phone
/// names the primary employee by its role and a voice call by its row id
/// (`assistant`); a text run of the primary carries no employee id. All
/// three are the one employee.
fn recipient(agent_id: &str) -> &str {
    if agent_id == "assistant" || agent_id == "main" { "" } else { agent_id }
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
        assert!(l.reading_for("other", 1001, None).is_none());
        assert!(
            l.reading_for("dispatch", 1001, None)
                .unwrap()
                .text
                .contains("40.000000, -111.000000")
        );
        let mut revoked = reading();
        revoked.agent_ids.clear();
        revoked.revision = 2;
        l.update(revoked, 1002).unwrap();
        assert!(l.reading_for("dispatch", 1002, None).is_none());
        l.update(reading(), 1003).unwrap();
        assert!(
            l.reading_for("dispatch", 1003, None).is_none(),
            "an older update never shares again"
        );
        let mut newer = reading();
        newer.revision = 3;
        l.update(newer, 1004).unwrap();
        assert!(l.reading_for("dispatch", 1004, None).is_some());
        assert!(l.reading_for("dispatch", 1600, None).is_none(), "expired");
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
        assert!(l.reading_for("dispatch", 1001, None).is_none());
        assert!(l.reading_for("other", 1001, None).is_some());
    }

    #[test]
    fn the_primary_employee_is_named_by_its_role() {
        let l = PhoneLocations::default();
        let mut primary = reading();
        primary.agent_ids = vec!["assistant".into()];
        l.update(primary, 1000).unwrap();
        assert!(l.reading_for("", 1000, None).is_some());
    }

    /// The haversine distance against a known pair: JFK to LAX is 3,974 km
    /// (2,470 miles) along the great circle.
    #[test]
    fn haversine_matches_a_known_pair() {
        let (jfk, lax) = ((40.6413, -73.7781), (33.9416, -118.4085));
        let km = haversine_km(jfk, lax);
        assert!((km - 3974.3).abs() < 1.0, "{km}");
        assert!((haversine_km(lax, jfk) - km).abs() < 1e-9, "the same both ways");
        assert_eq!(haversine_km(jfk, jfk), 0.0);
        let office = Office { latitude: jfk.0, longitude: jfk.1, unit: DistanceUnit::Miles };
        assert_eq!(office.distance_text(lax), "The owner is about 2470 miles from the office (straight line).");
        let office = Office { unit: DistanceUnit::Kilometres, ..office };
        assert_eq!(office.distance_text(lax), "The owner is about 3974 km from the office (straight line).");
    }

    #[test]
    fn the_unit_follows_the_owners_language() {
        for (language, unit) in [
            ("", DistanceUnit::Miles),
            ("en", DistanceUnit::Miles),
            ("en-US", DistanceUnit::Miles),
            ("en-GB", DistanceUnit::Miles),
            ("en_AU", DistanceUnit::Kilometres),
            ("de", DistanceUnit::Kilometres),
            ("pt-BR", DistanceUnit::Kilometres),
            ("zh-Hant-TW", DistanceUnit::Kilometres),
        ] {
            assert_eq!(DistanceUnit::for_language(language), unit, "{language}");
        }
    }

    /// With an office the reading carries the distance, and the office is
    /// part of what was told, so moving it tells the distance again; with
    /// none there is no distance.
    #[test]
    fn a_reading_carries_the_distance_from_the_office() {
        let l = PhoneLocations::default();
        l.update(reading(), 1000).unwrap();
        let office = Office { latitude: 40.1, longitude: -111.0, unit: DistanceUnit::Miles };
        let with = l.reading_for("dispatch", 1001, Some(office)).unwrap();
        assert!(with.text.ends_with("The owner is about 6.9 miles from the office (straight line)."), "{}", with.text);
        let without = l.reading_for("dispatch", 1001, None).unwrap();
        assert!(!without.text.contains("from the office"), "{}", without.text);
        assert_ne!(with.taken, without.taken);
        let moved = l.reading_for("dispatch", 1001, Some(Office { latitude: 40.2, ..office })).unwrap();
        assert_ne!(moved.taken, with.taken, "a moved office is told again");
        assert!(l.reading_for("other", 1001, Some(office)).is_none(), "no reading shared, no distance");
    }
}
