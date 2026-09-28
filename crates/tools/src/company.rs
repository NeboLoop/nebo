//! What everyone in the company is doing right now: one answer to "what is
//! everyone doing", read where each employee's work runs. A native
//! employee's live runs, duties and the questions it waits on are in this
//! Nebo; a linked employee's work is on the computer it runs on, whose host
//! says which of its conversations are working (`host/status`). The server
//! gathers it ([`crate::coworker::CoworkerRail::company_now`]);
//! `list_employees` says it for everyone and `get_employee` reads one
//! employee's current work.

/// Where an employee is right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    Working,
    /// Stopped on a question only the owner can answer.
    WaitingOnOwner,
    Idle,
    /// A linked employee whose agent is not running where it lives: paused,
    /// gone from its computer, or its computer is offline.
    NotRunning,
    /// Its computer could not be asked, so nothing is known.
    Unknown,
}

impl Doing {
    fn word(self) -> &'static str {
        match self {
            Doing::Working => "working",
            Doing::WaitingOnOwner => "waiting on you",
            Doing::Idle => "idle",
            Doing::NotRunning => "not running",
            Doing::Unknown => "unknown",
        }
    }
}

/// One piece of an employee's live work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Work {
    /// Where: `the conversation "Q3 close"`, `its duty weekday-check`.
    pub place: String,
    /// A linked employee's conversation: the id `send_message`'s
    /// `conversation` takes to speak into it.
    pub conversation: Option<String>,
    /// What it is doing there now: "Reading ledger.csv", "working on a
    /// request, a tool running".
    pub doing: String,
    /// How long, in words: "for 3m", "last update 20s ago".
    pub since: Option<String>,
    /// The question it waits on the owner for there.
    pub asks: Option<String>,
    /// Its current work read in detail (`get_employee` only): the request
    /// it works on, its recent calls, its latest words. Bounded.
    pub detail: Vec<String>,
}

/// One employee, right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmployeeNow {
    pub agent_id: String,
    pub name: String,
    pub doing: Doing,
    /// Where a linked employee runs: "this computer", or its computer's name.
    pub computer: Option<String>,
    /// Why it is not running, or why nothing is known.
    pub note: Option<String>,
    pub work: Vec<Work>,
}

/// What is asked of [`crate::coworker::CoworkerRail::company_now`].
#[derive(Debug, Clone, Default)]
pub struct CompanyQuery {
    /// The session asking: its own run is the question being asked, never
    /// reported as work.
    pub caller_session: String,
    /// The employee (by id) whose current work is read in detail.
    pub detail_for: Option<String>,
}

/// `list_employees`' account of right now: every employee, one line each,
/// its live work under it.
pub fn render_all(list: &[EmployeeNow]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let mut out = String::from("Right now:");
    for e in list {
        out.push_str(&format!("\n- {}", headline(e)));
        for w in &e.work {
            out.push_str(&format!("\n    · {}", work_line(w)));
        }
    }
    out
}

/// `get_employee`'s account of one employee's current work, in detail.
pub fn render_one(e: &EmployeeNow) -> String {
    let mut out = format!("Now: {}", headline(e));
    for w in &e.work {
        out.push_str(&format!("\n- {}", work_line(w)));
        for d in &w.detail {
            out.push_str(&format!("\n    {d}"));
        }
    }
    out
}

fn headline(e: &EmployeeNow) -> String {
    let mut line = e.name.clone();
    if let Some(computer) = &e.computer {
        line.push_str(&format!(" (linked, on {computer})"));
    }
    line.push_str(&format!(": {}", e.doing.word()));
    if let Some(note) = &e.note {
        line.push_str(&format!(" — {note}"));
    }
    line
}

fn work_line(w: &Work) -> String {
    let mut line = w.place.clone();
    if let Some(id) = &w.conversation {
        line.push_str(&format!(" (conversation: {id})"));
    }
    if !w.doing.is_empty() {
        line.push_str(&format!(": {}", w.doing));
    }
    if let Some(since) = &w.since {
        line.push_str(&format!(", {since}"));
    }
    if let Some(q) = &w.asks {
        line.push_str(&format!(" — asks you: \"{q}\""));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn employee(name: &str, doing: Doing) -> EmployeeNow {
        EmployeeNow {
            agent_id: name.to_lowercase(),
            name: name.to_owned(),
            doing,
            computer: None,
            note: None,
            work: Vec::new(),
        }
    }

    /// Every employee gets a line saying where it is; live work sits under
    /// it with where, what, since when, the conversation a linked one can be
    /// messaged in, and the question it waits on.
    #[test]
    fn everyone_is_said_with_their_live_work() {
        let mut coder = employee("Coder", Doing::Working);
        coder.computer = Some("this computer".into());
        coder.work.push(Work {
            place: "the conversation \"Fix login\"".into(),
            conversation: Some("s-1".into()),
            doing: "working on a request, a tool running".into(),
            since: Some("last update 20s ago".into()),
            ..Work::default()
        });
        let mut books = employee("Bookkeeper", Doing::WaitingOnOwner);
        books.work.push(Work {
            place: "its duty month-close".into(),
            doing: "stopped".into(),
            asks: Some("send the invoice to a new address".into()),
            ..Work::default()
        });
        let mut away = employee("Hermes", Doing::NotRunning);
        away.computer = Some("the office server".into());
        away.note = Some("its computer is offline".into());
        let text = render_all(&[coder, books, employee("Researcher", Doing::Idle), away]);
        assert_eq!(
            text,
            "Right now:\n\
             - Coder (linked, on this computer): working\n    \
             · the conversation \"Fix login\" (conversation: s-1): working on a request, a tool running, last update 20s ago\n\
             - Bookkeeper: waiting on you\n    \
             · its duty month-close: stopped — asks you: \"send the invoice to a new address\"\n\
             - Researcher: idle\n\
             - Hermes (linked, on the office server): not running — its computer is offline"
        );
        assert_eq!(render_all(&[]), "");
    }

    /// One employee's current work carries its detail lines under each
    /// piece of work.
    #[test]
    fn one_employee_is_read_in_detail() {
        let mut coder = employee("Coder", Doing::Working);
        coder.work.push(Work {
            place: "the conversation \"Fix login\"".into(),
            doing: "a tool running".into(),
            detail: vec![
                "Working on: \"find why sign-in fails\"".into(),
                "Latest words: \"Running the tests.\"".into(),
            ],
            ..Work::default()
        });
        assert_eq!(
            render_one(&coder),
            "Now: Coder: working\n\
             - the conversation \"Fix login\": a tool running\n    \
             Working on: \"find why sign-in fails\"\n    \
             Latest words: \"Running the tests.\""
        );
    }
}
