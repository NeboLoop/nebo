//! Addressings — who a team post or a message between employees addressed,
//! and whether each has answered (migration 0195). The record behind the ONE
//! routing model (`server::addressing`): an employee speaks only when it is
//! addressed, and answers each addressing once; whoever asked hears the
//! answers once, together, when the last one it waits for is in.

use rusqlite::params;

use crate::{DbErrExt, Store};
use types::NeboError;

/// Where one addressing stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressingState {
    /// Addressed, not answered yet.
    Asked,
    /// Not answered yet, and everyone its employee asked in turn has
    /// answered: the answer it gives next is final and asks no one.
    Collected,
    /// Answered.
    Answered,
}

impl AddressingState {
    fn parse(s: &str) -> Self {
        match s {
            "collected" => AddressingState::Collected,
            "answered" => AddressingState::Answered,
            _ => AddressingState::Asked,
        }
    }
}

/// One addressing: `post_id` addressed `agent_id`, who answers in
/// `seat_session`.
#[derive(Debug, Clone, PartialEq)]
pub struct Addressing {
    pub post_id: String,
    pub agent_id: String,
    /// The team the post is in; empty for a message between employees.
    pub team_id: String,
    pub seat_session: String,
    /// The conversation that asked; empty for the owner in a team thread.
    pub asker_session: String,
    /// The employee that asked; empty for the owner (or the main employee).
    pub asker_agent: String,
    pub state: AddressingState,
    /// The answer's words (empty until answered, or when the turn ended
    /// without any).
    pub answer: String,
    /// The answering run's provenance classes, as JSON.
    pub provenance: String,
    /// The answer's agent-to-agent hop count.
    pub handoff_depth: u8,
}

const COLUMNS: &str = "post_id, agent_id, team_id, seat_session, asker_session, asker_agent, state, answer, provenance, handoff_depth";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Addressing> {
    Ok(Addressing {
        post_id: r.get(0)?,
        agent_id: r.get(1)?,
        team_id: r.get(2)?,
        seat_session: r.get(3)?,
        asker_session: r.get(4)?,
        asker_agent: r.get(5)?,
        state: AddressingState::parse(&r.get::<_, String>(6)?),
        answer: r.get(7)?,
        provenance: r.get(8)?,
        handoff_depth: r.get::<_, i64>(9)?.clamp(0, u8::MAX as i64) as u8,
    })
}

fn select(conn: &rusqlite::Connection, filter: &str, key: &str, what: &str) -> Result<Vec<Addressing>, NeboError> {
    let mut stmt = conn
        .prepare(&format!("SELECT {COLUMNS} FROM addressings WHERE {filter} ORDER BY created_at, rowid"))
        .db_err(what)?;
    let rows = stmt.query_map(params![key], row).db_err(what)?;
    rows.collect::<Result<Vec<_>, _>>().db_err(what)
}

impl Store {
    /// `post_id` addresses `agent_id`, who answers in `seat_session`, asked
    /// from `asker_session` by `asker_agent`. Idempotent.
    pub fn open_addressing(
        &self,
        post_id: &str,
        agent_id: &str,
        team_id: &str,
        seat_session: &str,
        asker_session: &str,
        asker_agent: &str,
        at: i64,
    ) -> Result<(), NeboError> {
        self.conn()?
            .execute(
                "INSERT INTO addressings (post_id, agent_id, team_id, seat_session, asker_session, asker_agent, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (post_id, agent_id) DO NOTHING",
                params![post_id, agent_id, team_id, seat_session, asker_session, asker_agent, at],
            )
            .db_err("open_addressing")?;
        Ok(())
    }

    /// The post never reached `agent_id` (its delivery failed): nobody waits
    /// for an answer from it.
    pub fn withdraw_addressing(&self, post_id: &str, agent_id: &str) -> Result<(), NeboError> {
        self.conn()?
            .execute(
                "DELETE FROM addressings WHERE post_id = ?1 AND agent_id = ?2 AND state != 'answered'",
                params![post_id, agent_id],
            )
            .db_err("withdraw_addressing")?;
        Ok(())
    }

    /// The addressings `seat_session` has not answered yet, oldest first.
    pub fn unanswered_in_seat(&self, seat_session: &str) -> Result<Vec<Addressing>, NeboError> {
        let conn = self.conn()?;
        select(&conn, "seat_session = ?1 AND state != 'answered'", seat_session, "unanswered_in_seat")
    }

    /// Whether anything ever addressed `seat_session`: a conversation a
    /// post or a message opened, as against one the owner or a schedule
    /// started.
    pub fn seat_was_addressed(&self, seat_session: &str) -> Result<bool, NeboError> {
        self.conn()?
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM addressings WHERE seat_session = ?1)",
                params![seat_session],
                |r| r.get(0),
            )
            .db_err("seat_was_addressed")
    }

    /// `seat_session`'s answer: every addressing it has not answered is
    /// answered with it, at once. Returns the addressings this call
    /// answered — none when they were answered already, so an answer is
    /// given once.
    pub fn answer_seat(
        &self,
        seat_session: &str,
        answer: &str,
        provenance: &str,
        handoff_depth: u8,
        at: i64,
    ) -> Result<Vec<Addressing>, NeboError> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .db_err("answer_seat tx")?;
        let open = select(&tx, "seat_session = ?1 AND state != 'answered'", seat_session, "answer_seat")?;
        tx.execute(
            "UPDATE addressings SET state = 'answered', answer = ?2, provenance = ?3, handoff_depth = ?4, answered_at = ?5
             WHERE seat_session = ?1 AND state != 'answered'",
            params![seat_session, answer, provenance, handoff_depth as i64, at],
        )
        .db_err("answer_seat")?;
        tx.commit().db_err("answer_seat commit")?;
        Ok(open
            .into_iter()
            .map(|a| Addressing {
                state: AddressingState::Answered,
                answer: answer.to_string(),
                provenance: provenance.to_string(),
                handoff_depth,
                ..a
            })
            .collect())
    }

    /// Everyone `seat_session` asked in turn has answered: its next answer
    /// is final.
    pub fn collect_seat(&self, seat_session: &str) -> Result<(), NeboError> {
        self.conn()?
            .execute(
                "UPDATE addressings SET state = 'collected' WHERE seat_session = ?1 AND state = 'asked'",
                params![seat_session],
            )
            .db_err("collect_seat")?;
        Ok(())
    }

    /// How many asks from `asker_session` are still unanswered.
    pub fn open_asks(&self, asker_session: &str) -> Result<usize, NeboError> {
        let n: i64 = self
            .conn()?
            .query_row(
                "SELECT COUNT(*) FROM addressings WHERE asker_session = ?1 AND state != 'answered'",
                params![asker_session],
                |r| r.get(0),
            )
            .db_err("open_asks")?;
        Ok(n as usize)
    }

    /// The answers `asker_session` has not heard, taken once: empty while any
    /// of its asks is unanswered, and for every caller but one when two
    /// answers land together.
    pub fn take_answers(&self, asker_session: &str) -> Result<Vec<Addressing>, NeboError> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .db_err("take_answers tx")?;
        let open: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM addressings WHERE asker_session = ?1 AND state != 'answered'",
                params![asker_session],
                |r| r.get(0),
            )
            .db_err("take_answers")?;
        if open > 0 {
            return Ok(Vec::new());
        }
        let answers = select(&tx, "asker_session = ?1 AND reported = 0", asker_session, "take_answers")?;
        tx.execute(
            "UPDATE addressings SET reported = 1 WHERE asker_session = ?1 AND reported = 0",
            params![asker_session],
        )
        .db_err("take_answers")?;
        tx.commit().db_err("take_answers commit")?;
        Ok(answers)
    }
}

#[cfg(test)]
mod tests {
    use super::AddressingState;
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-addressings-{}.db", uuid::Uuid::new_v4()));
        Store::new(path.to_str().unwrap()).unwrap()
    }

    const LEAD: &str = "agent:lead:coworker:team:t1";

    fn seat(m: &str) -> String {
        format!("agent:{m}:coworker:team:t1")
    }

    /// A seat answers once; the asker hears the answers once, together, only
    /// when every ask it made is answered.
    #[test]
    fn answered_once_and_heard_together() {
        let s = store();
        for m in ["m1", "m2"] {
            s.open_addressing("p1", m, "t1", &seat(m), LEAD, "lead", 1).unwrap();
        }
        assert_eq!(s.open_asks(LEAD).unwrap(), 2);
        assert_eq!(s.answer_seat(&seat("m1"), "done", "[]", 2, 2).unwrap().len(), 1);
        assert!(s.answer_seat(&seat("m1"), "again", "[]", 2, 3).unwrap().is_empty(), "answered once");
        assert!(s.take_answers(LEAD).unwrap().is_empty(), "m2 is still out");
        assert_eq!(s.answer_seat(&seat("m2"), "", "[\"web\"]", 2, 4).unwrap().len(), 1);
        let heard = s.take_answers(LEAD).unwrap();
        assert_eq!(heard.iter().map(|a| a.answer.as_str()).collect::<Vec<_>>(), vec!["done", ""]);
        assert!(s.take_answers(LEAD).unwrap().is_empty(), "heard once");
        assert!(s.unanswered_in_seat(&seat("m1")).unwrap().is_empty());
        assert!(s.seat_was_addressed(&seat("m1")).unwrap());
        assert!(!s.seat_was_addressed("agent:m1:web").unwrap());
    }

    /// One turn that heard two posts answers both.
    #[test]
    fn one_answer_answers_everything_the_seat_heard() {
        let s = store();
        s.open_addressing("p1", "m1", "t1", &seat("m1"), "", "", 1).unwrap();
        s.open_addressing("p2", "m1", "t1", &seat("m1"), LEAD, "lead", 2).unwrap();
        let answered = s.answer_seat(&seat("m1"), "both done", "[]", 1, 3).unwrap();
        assert_eq!(answered.iter().map(|a| a.post_id.as_str()).collect::<Vec<_>>(), vec!["p1", "p2"]);
    }

    /// A delivery that failed is withdrawn; collecting moves only an open
    /// addressing.
    #[test]
    fn withdrawn_and_collected() {
        let s = store();
        s.open_addressing("p1", "m1", "", "agent:m1:coworker:main", "chat", "", 1).unwrap();
        s.withdraw_addressing("p1", "m1").unwrap();
        assert!(s.unanswered_in_seat("agent:m1:coworker:main").unwrap().is_empty());
        assert_eq!(s.open_asks("chat").unwrap(), 0);
        s.open_addressing("p2", "lead", "t1", LEAD, "", "", 1).unwrap();
        s.collect_seat(LEAD).unwrap();
        assert_eq!(s.unanswered_in_seat(LEAD).unwrap()[0].state, AddressingState::Collected);
        s.answer_seat(LEAD, "all set", "[]", 1, 2).unwrap();
        s.collect_seat(LEAD).unwrap();
        assert!(s.unanswered_in_seat(LEAD).unwrap().is_empty());
    }
}
