//! A completion becomes a notification to the parent: heard at its next step
//! when it is busy, or the input of a new turn when it is idle. One format
//! for every path.

use super::{Completion, CompletionStatus};

/// The notification text for `c`.
pub fn render_notification(c: &Completion) -> String {
    let status = match &c.status {
        CompletionStatus::Done => "done".to_string(),
        CompletionStatus::Partial { why } => format!("partial ({why})"),
        CompletionStatus::Failed { error } => format!("failed ({error})"),
        CompletionStatus::Stopped => "stopped".to_string(),
    };
    format!(
        "[Notification: not a message from the owner]\nhelper {} \"{}\": {status}\n{}",
        c.task_id, c.description, c.result
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completion(status: CompletionStatus) -> Completion {
        Completion {
            task_id: "h7".into(),
            description: "read the logs".into(),
            status,
            result: "Three errors, all from the nightly job.".into(),
            usage: ai::UsageInfo::default(),
        }
    }

    #[test]
    fn one_format_for_every_status() {
        assert_eq!(
            render_notification(&completion(CompletionStatus::Done)),
            "[Notification: not a message from the owner]\nhelper h7 \"read the logs\": done\nThree errors, all from the nightly job."
        );
        let second_line = |s: CompletionStatus| {
            render_notification(&completion(s))
                .lines()
                .nth(1)
                .unwrap()
                .to_string()
        };
        assert_eq!(
            second_line(CompletionStatus::Partial {
                why: "budget".into()
            }),
            "helper h7 \"read the logs\": partial (budget)"
        );
        assert_eq!(
            second_line(CompletionStatus::Failed {
                error: "timeout".into()
            }),
            "helper h7 \"read the logs\": failed (timeout)"
        );
        assert_eq!(
            second_line(CompletionStatus::Stopped),
            "helper h7 \"read the logs\": stopped"
        );
    }
}
