//! The owner's clock. Times, schedules and reminders are read in the
//! owner's home time zone (the account's, mirrored into the owner profile)
//! when it is known, and on this computer's own clock when it is not — a
//! self-hosted bot with no account keeps working exactly as before. On a
//! Nebo Cloud bot "this computer" is a server in UTC, so the owner's zone is
//! what makes "every day at 9am" mean 9am where the owner lives.

use std::fmt;

use chrono::{
    FixedOffset, Local, MappedLocalTime, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc,
};
use chrono_tz::{Tz, TzOffset};

/// The zone the owner's wall clock reads: their IANA zone, or this
/// computer's when none is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerZone {
    Iana(Tz),
    Machine,
}

impl OwnerZone {
    /// The zone named `name` (an IANA name such as "America/Denver"); this
    /// computer's for none, an empty name, or one the zone database does
    /// not know.
    pub fn parse(name: Option<&str>) -> Self {
        name.map(str::trim)
            .filter(|n| !n.is_empty())
            .and_then(|n| n.parse::<Tz>().ok())
            .map_or(OwnerZone::Machine, OwnerZone::Iana)
    }

    /// The owner's zone as the owner profile holds it.
    pub fn of(store: &db::Store) -> Self {
        let profile = store.get_user_profile().ok().flatten();
        Self::parse(profile.as_ref().and_then(|p| p.timezone.as_deref()))
    }

    /// The IANA name, when the owner's zone is known.
    pub fn name(&self) -> Option<&'static str> {
        match self {
            OwnerZone::Iana(tz) => Some(tz.name()),
            OwnerZone::Machine => None,
        }
    }

    /// Now, on the owner's clock.
    pub fn now(&self) -> chrono::DateTime<OwnerZone> {
        Utc::now().with_timezone(self)
    }

    /// The config a recurring timer carries for `schedule`: the expression,
    /// plus the zone when the owner's is known, so a timer armed on another
    /// clock is replaced when the zone changes. On this computer's clock it
    /// is the expression alone, as it always was.
    pub fn stamp(&self, schedule: &str) -> String {
        match self.name() {
            Some(zone) => format!("{schedule} @ {zone}"),
            None => schedule.to_string(),
        }
    }
}

/// The UTC offset an [`OwnerZone`] has at an instant.
#[derive(Clone, Copy, Debug)]
pub enum OwnerOffset {
    Iana(TzOffset),
    Machine(FixedOffset),
}

impl Offset for OwnerOffset {
    fn fix(&self) -> FixedOffset {
        match self {
            OwnerOffset::Iana(o) => o.fix(),
            OwnerOffset::Machine(o) => *o,
        }
    }
}

impl fmt::Display for OwnerOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OwnerOffset::Iana(o) => o.fmt(f),
            OwnerOffset::Machine(o) => o.fmt(f),
        }
    }
}

impl TimeZone for OwnerZone {
    type Offset = OwnerOffset;

    fn from_offset(offset: &OwnerOffset) -> Self {
        match offset {
            OwnerOffset::Iana(o) => OwnerZone::Iana(Tz::from_offset(o)),
            OwnerOffset::Machine(_) => OwnerZone::Machine,
        }
    }

    fn offset_from_local_date(&self, local: &NaiveDate) -> MappedLocalTime<OwnerOffset> {
        match self {
            OwnerZone::Iana(tz) => tz.offset_from_local_date(local).map(OwnerOffset::Iana),
            OwnerZone::Machine => Local
                .offset_from_local_date(local)
                .map(OwnerOffset::Machine),
        }
    }

    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> MappedLocalTime<OwnerOffset> {
        match self {
            OwnerZone::Iana(tz) => tz.offset_from_local_datetime(local).map(OwnerOffset::Iana),
            OwnerZone::Machine => Local
                .offset_from_local_datetime(local)
                .map(OwnerOffset::Machine),
        }
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> OwnerOffset {
        match self {
            OwnerZone::Iana(tz) => OwnerOffset::Iana(tz.offset_from_utc_date(utc)),
            OwnerZone::Machine => OwnerOffset::Machine(Local.offset_from_utc_date(utc)),
        }
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> OwnerOffset {
        match self {
            OwnerZone::Iana(tz) => OwnerOffset::Iana(tz.offset_from_utc_datetime(utc)),
            OwnerZone::Machine => OwnerOffset::Machine(Local.offset_from_utc_datetime(utc)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> chrono::DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn a_known_zone_reads_its_own_clock_with_daylight_saving() {
        let denver = OwnerZone::parse(Some("America/Denver"));
        assert_eq!(denver.name(), Some("America/Denver"));
        let winter = utc("2026-01-15T16:00:00Z").with_timezone(&denver);
        assert_eq!(winter.format("%-I:%M %p %:z").to_string(), "9:00 AM -07:00");
        let summer = utc("2026-07-15T15:00:00Z").with_timezone(&denver);
        assert_eq!(summer.format("%-I:%M %p %:z").to_string(), "9:00 AM -06:00");
        // And back: 9:00 local is the same instant.
        let nine = denver
            .with_ymd_and_hms(2026, 1, 15, 9, 0, 0)
            .single()
            .unwrap();
        assert_eq!(nine.with_timezone(&Utc), utc("2026-01-15T16:00:00Z"));
    }

    #[test]
    fn no_zone_or_an_unknown_one_is_this_computers_clock() {
        for name in [
            None,
            Some(""),
            Some("  "),
            Some("Mars/Olympus_Mons"),
            Some("MDT"),
        ] {
            assert_eq!(OwnerZone::parse(name), OwnerZone::Machine, "{name:?}");
        }
        let at = utc("2026-01-15T16:00:00Z");
        assert_eq!(
            at.with_timezone(&OwnerZone::Machine).naive_local(),
            at.with_timezone(&Local).naive_local()
        );
        assert_eq!(OwnerZone::Machine.stamp("0 0 9 * * * *"), "0 0 9 * * * *");
        assert_eq!(
            OwnerZone::parse(Some("America/Denver")).stamp("0 0 9 * * * *"),
            "0 0 9 * * * * @ America/Denver"
        );
    }
}
