use rusqlite::{params, OptionalExtension};
use types::NeboError;

use crate::Store;

/// One message the mail intake took (`inbound_mail`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMailRow {
    pub id: String,
    pub source: String,
    pub reply_handle: String,
    pub sender_address: String,
    pub sender_name: String,
    /// owner | external
    pub standing: String,
    /// The employee it went to ("" = the primary).
    pub agent_id: String,
    pub employee_tag: String,
    pub session_key: String,
    pub subject: String,
    pub auto_submitted: bool,
    /// The whole normalized message, as JSON.
    pub record: String,
}

impl Store {
    /// Record a message the intake took. Returns false when the source
    /// already delivered this message (same `reply_handle`): nothing is
    /// written and the caller does nothing more with it.
    pub fn record_inbound_mail(&self, row: &InboundMailRow) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "INSERT OR IGNORE INTO inbound_mail
                   (id, source, reply_handle, sender_address, sender_name, standing, agent_id,
                    employee_tag, session_key, subject, auto_submitted, record)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    row.id,
                    row.source,
                    row.reply_handle,
                    row.sender_address,
                    row.sender_name,
                    row.standing,
                    row.agent_id,
                    row.employee_tag,
                    row.session_key,
                    row.subject,
                    row.auto_submitted as i64,
                    row.record,
                ],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(n == 1)
    }

    /// The message recorded under `id`.
    pub fn get_inbound_mail(&self, id: &str) -> Result<Option<InboundMailRow>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, source, reply_handle, sender_address, sender_name, standing, agent_id,
                    employee_tag, session_key, subject, auto_submitted, record
             FROM inbound_mail WHERE id = ?1",
            params![id],
            |r| {
                Ok(InboundMailRow {
                    id: r.get(0)?,
                    source: r.get(1)?,
                    reply_handle: r.get(2)?,
                    sender_address: r.get(3)?,
                    sender_name: r.get(4)?,
                    standing: r.get(5)?,
                    agent_id: r.get(6)?,
                    employee_tag: r.get(7)?,
                    session_key: r.get(8)?,
                    subject: r.get(9)?,
                    auto_submitted: r.get::<_, i64>(10)? != 0,
                    record: r.get(11)?,
                })
            },
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::InboundMailRow;
    use crate::Store;

    fn row(id: &str, handle: &str) -> InboundMailRow {
        InboundMailRow {
            id: id.into(),
            source: "nebo.bot".into(),
            reply_handle: handle.into(),
            sender_address: "pat@example.com".into(),
            sender_name: "Pat".into(),
            standing: "external".into(),
            agent_id: String::new(),
            employee_tag: "front-desk".into(),
            session_key: "email:thread:x".into(),
            subject: "Hello".into(),
            auto_submitted: false,
            record: "{}".into(),
        }
    }

    /// A message is recorded once per source id; a redelivery writes nothing.
    #[test]
    fn a_redelivered_message_is_recorded_once() {
        let path = std::env::temp_dir().join(format!("nebo-inbound-mail-{}.db", uuid::Uuid::new_v4()));
        let store = Store::new(&path.to_string_lossy()).expect("store");
        assert!(store.record_inbound_mail(&row("m1", "hub-1")).unwrap());
        assert!(!store.record_inbound_mail(&row("m2", "hub-1")).unwrap());
        assert_eq!(store.get_inbound_mail("m1").unwrap().unwrap().employee_tag, "front-desk");
        assert!(store.get_inbound_mail("m2").unwrap().is_none());
    }
}
