//! The rules engine: which rule decides a call, whether a call is inside
//! the employee's job, and the money a standing allow covers.
//!
//! Rules live at two scopes, company defaults and one employee's overrides.
//! A deny from either scope decides: an employee's rule never undoes a
//! company deny.
//! Otherwise, when any employee-scope rule matches a call, the employee
//! scope decides; else the company's does. Within the deciding scope ask
//! beats allow; a more specific field never outranks a broader deny.
//!
//! Within one scope a rule on the call's own tool or operation is its own
//! setting, and a rule on its family (`mcp__crm__*`) or its capability is
//! the default for everything without one: the most specific key that has
//! a rule decides, whatever the broader ones say. A rule with a field
//! (a command, folder, site or recipient) is never set aside this way, and
//! neither is a locked one.
//!
//! A shell command is judged by every command it runs (`ls && rm x` is `ls`
//! and `rm x`): denied when any is denied, asked when any asks, allowed only
//! when every one is allowed. A command that can't be read before it runs
//! (`$CMD -rf x`, `git $SUB`) and that a deny or ask rule could name is
//! asked about, since it may be the denied command in disguise; a deny
//! the known words already match still refuses it.

use std::path::{Path, PathBuf};

use tools::policy::{Cover, Subcommand};
use types::permissions::{
    Effect, Grant, MoneyLimit, Mode, Rule, RuleField, RuleKey, RuleSource, Scope, Target,
};

/// An employee's rules for one run, split by scope.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    employee: Vec<Rule>,
    company: Vec<Rule>,
    /// Folders the run itself adds to a fenced job (the chat's project).
    run_folders: Vec<PathBuf>,
}

impl RuleSet {
    /// The rules that decide for `agent_id`: the company defaults and the
    /// employee's own.
    pub fn load(store: &db::Store, agent_id: &str) -> Result<RuleSet, types::NeboError> {
        Ok(Self::split(store.permission_rules(agent_id)?, Vec::new()))
    }

    /// The rules a grant carries.
    pub fn of(grant: &Grant) -> RuleSet {
        Self::split(grant.rules.clone(), grant.run_folders.clone())
    }

    fn split(rules: Vec<Rule>, run_folders: Vec<PathBuf>) -> RuleSet {
        let (employee, company) = rules
            .into_iter()
            .partition(|r| matches!(r.scope, Scope::Employee(_)));
        RuleSet { employee, company, run_folders }
    }

    /// The rule that decides `t` and its effect, or `None` when no rule
    /// matches. For a shell command: the strictest decision among the
    /// commands it runs, and `None` unless a rule matches every one of them.
    pub fn decide(&self, t: &Target) -> Option<(&Rule, Effect)> {
        let mut strictest: Option<(&Rule, Effect)> = None;
        let mut undecided = false;
        for piece in pieces(t) {
            match self.decide_piece(t, piece.as_ref()) {
                None => undecided = true,
                Some(d) if strictest.is_none_or(|s| d.1 > s.1) => strictest = Some(d),
                Some(_) => {}
            }
        }
        match strictest {
            Some((_, Effect::Allow)) if undecided => None,
            decided => decided,
        }
    }

    /// The rule that decides one piece of `t` (see [`pieces`]) and its
    /// effect: a deny from either scope; else an ask for a piece that can't
    /// be read and that a deny rule of either scope could name; else the
    /// deciding scope's strongest rule.
    pub fn decide_piece(&self, t: &Target, piece: Option<&Subcommand>) -> Option<(&Rule, Effect)> {
        let employee = applying(&self.employee, t, piece);
        let company = applying(&self.company, t, piece);
        if let Some(deny) = employee.iter().chain(&company).find(|(_, e)| *e == Effect::Deny) {
            return Some(*deny);
        }
        // A deny that may name a command it can't read asks.
        if let Some(unread) = self.all().find(|r| r.effect == Effect::Deny && effect_on(r, t, piece) == Some(Effect::Ask)) {
            return Some((unread, Effect::Ask));
        }
        let deciding = if employee.is_empty() { company } else { employee };
        deciding.into_iter().max_by_key(|(_, e)| *e)
    }

    /// The rules of the scope that decides one piece of `t`, with what each
    /// does to it: the employee's when any of its rules applies, else the
    /// company's.
    fn deciding(&self, t: &Target, piece: Option<&Subcommand>) -> Vec<(&Rule, Effect)> {
        let employee = applying(&self.employee, t, piece);
        if employee.is_empty() { applying(&self.company, t, piece) } else { employee }
    }

    /// Whether one piece of `t` is allowed by an allow `pick` accepts among
    /// the rules that decide it.
    pub fn piece_allowed_by(&self, t: &Target, piece: Option<&Subcommand>, pick: impl Fn(&Rule) -> bool) -> bool {
        matches!(self.decide_piece(t, piece), Some((_, Effect::Allow)))
            && self
                .deciding(t, piece)
                .iter()
                .any(|(r, _)| r.effect == Effect::Allow && pick(r))
    }

    /// Whether `t` is allowed and, for every piece of it, an allow `pick`
    /// accepts is among the rules that decide it.
    fn allowed_by(&self, t: &Target, pick: impl Fn(&Rule) -> bool) -> bool {
        matches!(self.decide(t), Some((_, Effect::Allow)))
            && pieces(t).iter().all(|piece| self.piece_allowed_by(t, piece.as_ref(), &pick))
    }

    /// Whether `t` is inside the job: basic work (no capability), or a
    /// call the rules allow, within the job's folders.
    pub fn in_job(&self, t: &Target, input: &serde_json::Value) -> bool {
        if t.capability.is_none() {
            return true;
        }
        self.decide(t).is_some_and(|(_, e)| e == Effect::Allow) && self.outside_folders(t, input).is_none()
    }

    /// The money the allow that decides `t` covers.
    pub fn money_limit(&self, t: &Target) -> Option<&MoneyLimit> {
        match self.decide(t) {
            Some((rule, Effect::Allow)) => rule.money.as_ref(),
            _ => None,
        }
    }

    /// Whether an allow the owner wrote for this key (not the whole
    /// capability) covers `t`: what Ask mode runs without asking.
    pub fn owner_allowed(&self, t: &Target) -> bool {
        self.allowed_by(t, |r| !matches!(r.key, RuleKey::Capability(_)))
    }

    /// Whether the owner answered "Allow always" to an ask for this call:
    /// an allow written by that answer covers it.
    pub fn answered_always(&self, t: &Target) -> bool {
        self.allowed_by(t, |r| matches!(r.source, RuleSource::AllowAlways { .. }))
    }

    /// Whether the owner allowed every command of `t` that watches him or
    /// drives his apps ([`tools::policy::reach_of`]): an allow he wrote for
    /// that command himself, on the Permissions page or by answering "Allow
    /// always". An allow of the whole shell, a capability or one a package
    /// wrote never covers them, and a call whose command can't be read
    /// isn't covered.
    pub fn reach_allowed(&self, t: &Target) -> bool {
        let reaching: Vec<Subcommand> =
            pieces(t).into_iter().flatten().filter(|s| tools::policy::reach_of(s).is_some()).collect();
        !reaching.is_empty() && reaching.iter().all(|s| self.reach_piece_allowed(t, s))
    }

    /// Whether the owner allowed this one command of `t` (see
    /// [`RuleSet::reach_allowed`]).
    pub fn reach_piece_allowed(&self, t: &Target, piece: &Subcommand) -> bool {
        self.piece_allowed_by(t, Some(piece), |r| {
            matches!(r.field, Some(RuleField::CommandPrefix(_)))
                && matches!(r.source, RuleSource::Owner | RuleSource::AllowAlways { .. })
        })
    }

    /// The job's folders (see [`types::permissions::folders_of`]).
    pub fn folders(&self) -> Vec<PathBuf> {
        let rules: Vec<Rule> = self.all().cloned().collect();
        types::permissions::folders_of(&rules, &self.run_folders)
    }

    /// Why `t` falls outside the job's folders, or `None` when it is inside
    /// (or the job has none). File changes and shell working directories
    /// are fenced; reads are not.
    pub fn outside_folders(&self, t: &Target, input: &serde_json::Value) -> Option<String> {
        let folders = self.folders();
        outside(&folders, t, input)
    }

    /// Every rule, employee scope first.
    pub fn all(&self) -> impl Iterator<Item = &Rule> {
        self.employee.iter().chain(self.company.iter())
    }
}

/// Why `t` falls outside `folders` (empty: no fence).
pub fn outside(folders: &[PathBuf], t: &Target, input: &serde_json::Value) -> Option<String> {
    if folders.is_empty() {
        return None;
    }
    let strings: Vec<String> = folders.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    tools::safeguard::check_path_scope(&t.key, input, &strings)
}

/// What rules judge `t` by: each command a shell call runs, or the whole call
/// (`None`) for anything else.
pub fn pieces(t: &Target) -> Vec<Option<Subcommand>> {
    match &t.field {
        Some(RuleField::CommandPrefix(cmd)) => tools::policy::subcommands(cmd).into_iter().map(Some).collect(),
        _ => vec![None],
    }
}

/// Whether `rule` applies to `t`: its key names the call's rule key, tool,
/// operation or capability, and its field (if any) covers the call's — for a
/// shell call, any one of the commands it runs.
pub fn matches(rule: &Rule, t: &Target) -> bool {
    pieces(t).iter().any(|piece| effect_on(rule, t, piece.as_ref()).is_some())
}

/// How narrowly a rule's key names a call: its own tool or operation, its
/// family of tools (`mcp__crm__*`), or its capability.
pub fn key_rank(key: &RuleKey) -> u8 {
    match key {
        RuleKey::Capability(_) => 0,
        RuleKey::Tool(k) if k.ends_with('*') => 1,
        RuleKey::Tool(_) | RuleKey::Operation(_) => 2,
    }
}

/// The rules of one scope that apply to one piece of `t`, with what each
/// does to it. A rule without a field is set aside when a rule of the same
/// scope with a narrower key (and no field) also applies: the call's own
/// setting outranks its family's and its capability's. Locked rules and
/// rules with a field always apply.
fn applying<'a>(rules: &'a [Rule], t: &Target, piece: Option<&Subcommand>) -> Vec<(&'a Rule, Effect)> {
    let matched: Vec<(&Rule, Effect)> = rules.iter().filter_map(|r| effect_on(r, t, piece).map(|e| (r, e))).collect();
    let narrowest = matched.iter().filter(|(r, _)| r.field.is_none()).map(|(r, _)| key_rank(&r.key)).max();
    matched
        .into_iter()
        .filter(|(r, _)| r.field.is_some() || r.locked || Some(key_rank(&r.key)) == narrowest)
        .collect()
}

/// What `rule` does to one piece of `t` (see [`pieces`]): its effect when it
/// applies, an ask when it may apply to a command that can't be read, or
/// `None`.
fn effect_on(rule: &Rule, t: &Target, piece: Option<&Subcommand>) -> Option<Effect> {
    let key = match &rule.key {
        RuleKey::Tool(k) => match k.strip_suffix('*') {
            Some(prefix) => !prefix.is_empty() && (t.key.starts_with(prefix) || t.tool.starts_with(prefix)),
            None => *k == t.key || *k == t.tool,
        },
        RuleKey::Operation(op) => t.operation.as_deref().is_some_and(|o| {
            tools::plugin_tool::port_suffix(o) == tools::plugin_tool::port_suffix(op)
        }),
        RuleKey::Capability(c) => t.capability.as_deref() == Some(c.as_str()),
    };
    if !key {
        return None;
    }
    let cover = match &rule.field {
        None => Cover::Yes,
        Some(field) => field_covers(field, rule.effect, t, piece),
    };
    match cover {
        Cover::Yes => Some(rule.effect),
        Cover::Unread => Some(Effect::Ask),
        Cover::No => None,
    }
}

fn field_covers(field: &RuleField, effect: Effect, t: &Target, piece: Option<&Subcommand>) -> Cover {
    let yes = |b: bool| if b { Cover::Yes } else { Cover::No };
    match (field, &t.field) {
        (RuleField::CommandPrefix(prefix), Some(RuleField::CommandPrefix(_))) => {
            piece.map_or(Cover::No, |command| command.covered_by(prefix, effect == Effect::Allow))
        }
        (RuleField::Folder(folder), Some(RuleField::Folder(path))) => yes(within(path, folder)),
        (RuleField::Domain(domain), Some(RuleField::Domain(host))) => {
            let (d, h) = (domain.to_ascii_lowercase(), host.to_ascii_lowercase());
            yes(h == d || h.ends_with(&format!(".{d}")))
        }
        (RuleField::Recipient(who), Some(RuleField::Recipient(to))) => yes(who.eq_ignore_ascii_case(to)),
        (RuleField::Recipient(who), _) => yes(
            !t.effects.recipients.is_empty() && t.effects.recipients.iter().all(|r| r.eq_ignore_ascii_case(who)),
        ),
        _ => Cover::No,
    }
}

fn within(path: &Path, folder: &Path) -> bool {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    abs(path).starts_with(abs(folder))
}

/// The mode an employee runs in: its own, else the company default, else
/// Automatic.
pub fn mode_of(store: &db::Store, agent_id: &str) -> Result<Mode, types::NeboError> {
    if !agent_id.is_empty()
        && let Some(mode) = store.permission_mode(&Scope::Employee(agent_id.to_string()))?
    {
        return Ok(mode);
    }
    Ok(store.permission_mode(&Scope::Company)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::permissions::{CallEffects, RuleSource};

    fn rule(scope: Scope, key: RuleKey, field: Option<RuleField>, effect: Effect) -> Rule {
        Rule {
            id: format!("{}-{}-{:?}", key.value(), effect.as_str(), field),
            scope,
            key,
            field,
            effect,
            money: None,
            source: RuleSource::Owner,
            locked: false,
            created_at: 0,
        }
    }

    fn target(key: &str, capability: Option<&str>, field: Option<RuleField>) -> Target {
        Target {
            tool: "os".into(),
            key: key.into(),
            operation: None,
            capability: capability.map(str::to_string),
            field,
            subject: None,
            read_only: false,
            effects: CallEffects::unknown(),
        }
    }

    fn set(rules: Vec<Rule>) -> RuleSet {
        RuleSet::split(rules, Vec::new())
    }

    #[test]
    fn deny_beats_ask_beats_allow() {
        let emp = || Scope::Employee("a".into());
        let shell = || RuleKey::Capability("shell".into());
        let t = target("run_command", Some("shell"), Some(RuleField::CommandPrefix("git status".into())));
        let allow = rule(emp(), shell(), None, Effect::Allow);
        let ask = rule(emp(), RuleKey::Tool("run_command".into()), None, Effect::Ask);
        let deny = rule(
            emp(),
            RuleKey::Tool("run_command".into()),
            Some(RuleField::CommandPrefix("git".into())),
            Effect::Deny,
        );
        assert_eq!(set(vec![allow.clone()]).decide(&t).map(|d| d.1), Some(Effect::Allow));
        assert_eq!(set(vec![allow.clone(), ask.clone()]).decide(&t).map(|d| d.1), Some(Effect::Ask));
        assert_eq!(set(vec![allow, ask, deny]).decide(&t).map(|d| d.1), Some(Effect::Deny));
        // A narrower allow never outranks a broader deny.
        let broad_deny = rule(emp(), shell(), None, Effect::Deny);
        let narrow_allow = rule(
            emp(),
            RuleKey::Tool("run_command".into()),
            Some(RuleField::CommandPrefix("git status".into())),
            Effect::Allow,
        );
        assert_eq!(set(vec![broad_deny, narrow_allow]).decide(&t).map(|d| d.1), Some(Effect::Deny));
    }

    /// A shell command is judged by every command it runs: denied when any
    /// is denied, asked when any asks, allowed only when all are allowed.
    /// Separators inside quotes are not separators.
    #[test]
    fn a_compound_command_is_judged_by_every_command_it_runs() {
        let emp = || Scope::Employee("a".into());
        let cmd = |prefix: &str, effect| {
            rule(emp(), RuleKey::Tool("run_command".into()), Some(RuleField::CommandPrefix(prefix.into())), effect)
        };
        let rules = set(vec![
            cmd("rm", Effect::Deny),
            cmd("git push", Effect::Ask),
            cmd("ls", Effect::Allow),
            cmd("echo", Effect::Allow),
            cmd("cat", Effect::Allow),
        ]);
        let run = |c: &str| target("run_command", Some("shell"), Some(RuleField::CommandPrefix(c.into())));
        let cases: &[(&str, Option<Effect>)] = &[
            ("rm -rf x", Some(Effect::Deny)),
            ("ls && rm -rf x", Some(Effect::Deny)),
            ("ls || rm -rf x", Some(Effect::Deny)),
            ("ls; rm -rf x", Some(Effect::Deny)),
            ("ls | rm -rf x", Some(Effect::Deny)),
            ("ls\nrm -rf x", Some(Effect::Deny)),
            ("(cd /tmp; rm -rf x)", Some(Effect::Deny)),
            ("echo $(rm -rf x)", Some(Effect::Deny)),
            ("echo \"$(rm -rf x)\"", Some(Effect::Deny)),
            ("echo `rm -rf x`", Some(Effect::Deny)),
            ("ls > $(rm -rf x)", Some(Effect::Deny)),
            ("rm -rf x > /dev/null 2>&1", Some(Effect::Deny)),
            ("bash -c 'rm -rf x'", Some(Effect::Deny)),
            ("sh -c \"ls && rm -rf x\"", Some(Effect::Deny)),
            ("eval 'rm -rf x'", Some(Effect::Deny)),
            ("FOO=1 rm -rf x", Some(Effect::Deny)),
            ("env FOO=1 rm -rf x", Some(Effect::Deny)),
            ("nohup rm -rf x", Some(Effect::Deny)),
            ("timeout 5 rm -rf x", Some(Effect::Deny)),
            ("'r'm -rf x", Some(Effect::Deny)),
            ("\\rm -rf x", Some(Effect::Deny)),
            ("git push && rm -rf x", Some(Effect::Deny)),
            // What can't be read may be the denied command: it is asked
            // about rather than guessed at.
            // A deny its known words already match still refuses it.
            ("$CMD -rf x", Some(Effect::Ask)),
            ("ls &&", Some(Effect::Ask)),
            ("rm -rf $DIR", Some(Effect::Deny)),
            ("ls && $CMD", Some(Effect::Ask)),
            ("git push origin main", Some(Effect::Ask)),
            ("ls && git push origin main", Some(Effect::Ask)),
            ("ls; git push", Some(Effect::Ask)),
            ("echo $(git push)", Some(Effect::Ask)),
            ("bash -c 'git push'", Some(Effect::Ask)),
            ("FOO=1 git push", Some(Effect::Ask)),
            ("git $SUB origin", Some(Effect::Ask)),
            ("ls -la", Some(Effect::Allow)),
            ("ls && echo hi", Some(Effect::Allow)),
            ("ls | cat", Some(Effect::Allow)),
            ("ls; echo done\ncat f", Some(Effect::Allow)),
            ("echo \"a && rm -rf b\"", Some(Effect::Allow)),
            ("echo 'x; rm -rf y | git push'", Some(Effect::Allow)),
            ("ls > out.txt", Some(Effect::Allow)),
            // Allowed only when every command is: one no rule covers leaves
            // the call to the mode.
            ("ls && whoami", None),
            ("echo $(whoami)", None),
            ("bash -c 'ls'", None),
            // A variable prefix can change what a command runs, so it never
            // meets an allow.
            ("PATH=/tmp ls", None),
        ];
        for (c, want) in cases {
            assert_eq!(rules.decide(&run(c)).map(|d| d.1), *want, "{c:?}");
        }
        assert!(rules.owner_allowed(&run("ls && echo hi")));
        assert!(!rules.owner_allowed(&run("ls && whoami")));
        assert!(!rules.owner_allowed(&run("ls && rm -rf x")));
        // The job's capability allow covers every command; a command deny
        // still wins inside it.
        let job = set(vec![
            rule(Scope::Company, RuleKey::Capability("shell".into()), None, Effect::Allow),
            rule(Scope::Company, RuleKey::Tool("run_command".into()), Some(RuleField::CommandPrefix("rm".into())), Effect::Deny),
        ]);
        assert_eq!(job.decide(&run("ls && whoami")).map(|d| d.1), Some(Effect::Allow));
        assert_eq!(job.decide(&run("ls && rm -rf x")).map(|d| d.1), Some(Effect::Deny));
        assert!(!job.in_job(&run("ls && rm -rf x"), &serde_json::json!({})));
    }

    /// A command that can't be read and that a deny of either scope could
    /// name is asked about, even where the job's allow covers every command
    /// (it may be the denied command in disguise). A command the
    /// deny doesn't name, with nothing unread, runs.
    #[test]
    fn an_unreadable_command_a_deny_could_name_asks() {
        let run = |c: &str| target("run_command", Some("shell"), Some(RuleField::CommandPrefix(c.into())));
        let rules = set(vec![
            rule(Scope::Employee("a".into()), RuleKey::Capability("shell".into()), None, Effect::Allow),
            rule(Scope::Company, RuleKey::Tool("run_command".into()), Some(RuleField::CommandPrefix("rm".into())), Effect::Deny),
        ]);
        let decide = |c: &str| rules.decide(&run(c)).map(|(r, e)| (r.effect, e));
        assert_eq!(decide("$CMD -rf x"), Some((Effect::Deny, Effect::Ask)), "the rule that could apply is named");
        assert_eq!(decide("ls && eval \"$X\""), Some((Effect::Deny, Effect::Ask)));
        assert_eq!(decide("rm -rf $DIR"), Some((Effect::Deny, Effect::Deny)));
        assert_eq!(decide("ls $DIR"), Some((Effect::Allow, Effect::Allow)));
        // With no deny or ask that could apply, an unreadable command is the
        // job's, as before.
        let job = set(vec![rule(Scope::Company, RuleKey::Capability("shell".into()), None, Effect::Allow)]);
        assert_eq!(job.decide(&run("$CMD -rf x")).map(|d| d.1), Some(Effect::Allow));
    }

    /// A deny from either scope decides: an employee's allow never undoes
    /// a company deny. An employee's own rule still narrows a company
    /// allow, and its allow still covers a company ask.
    #[test]
    fn a_company_deny_beats_an_employee_allow() {
        let t = target("fetch_url", Some("web"), None);
        let web = || RuleKey::Capability("web".into());
        let company = |e| rule(Scope::Company, web(), None, e);
        let employee = |e| rule(Scope::Employee("a".into()), web(), None, e);
        let decide = |rules: Vec<Rule>| set(rules).decide(&t).map(|(r, e)| (r.scope.clone(), e));
        assert_eq!(decide(vec![company(Effect::Deny)]), Some((Scope::Company, Effect::Deny)));
        assert_eq!(decide(vec![company(Effect::Deny), employee(Effect::Allow)]), Some((Scope::Company, Effect::Deny)));
        assert_eq!(decide(vec![company(Effect::Deny), employee(Effect::Ask)]), Some((Scope::Company, Effect::Deny)));
        let rules = set(vec![company(Effect::Deny), employee(Effect::Allow)]);
        assert!(!rules.in_job(&t, &serde_json::json!({})), "a company deny keeps it outside the job");
        assert!(!rules.owner_allowed(&t));
        // The employee's own narrows a company allow, and covers its ask.
        let own = Scope::Employee("a".into());
        assert_eq!(decide(vec![company(Effect::Allow), employee(Effect::Deny)]), Some((own.clone(), Effect::Deny)));
        assert_eq!(decide(vec![company(Effect::Allow), employee(Effect::Ask)]), Some((own.clone(), Effect::Ask)));
        assert_eq!(decide(vec![company(Effect::Ask), employee(Effect::Allow)]), Some((own, Effect::Allow)));
    }

    #[test]
    fn capability_rule_is_the_job() {
        let t = target("run_command", Some("shell"), None);
        let none = set(vec![]);
        assert!(!none.in_job(&t, &serde_json::json!({})), "a capability no rule grants is outside the job");
        let job = set(vec![rule(Scope::Company, RuleKey::Capability("shell".into()), None, Effect::Allow)]);
        assert!(job.in_job(&t, &serde_json::json!({})));
        // Basic work (memory, tasks, delegation) is always inside the job.
        assert!(none.in_job(&target("recall", None, None), &serde_json::json!({})));
    }

    #[test]
    fn folder_outside_every_rule_is_outside_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("inside");
        let file_allow = rule(Scope::Company, RuleKey::Capability("file".into()), None, Effect::Allow);
        let folder = rule(
            Scope::Employee("a".into()),
            RuleKey::Capability("file".into()),
            Some(RuleField::Folder(inside.clone())),
            Effect::Allow,
        );
        let rules = set(vec![file_allow, folder]);
        let write = |p: &Path| {
            let input = serde_json::json!({"path": p.to_string_lossy(), "content": "x"});
            (target("write_file", Some("file"), Some(RuleField::Folder(p.to_path_buf()))), input)
        };
        let (t, input) = write(&inside.join("a.txt"));
        assert!(rules.in_job(&t, &input));
        let (t, input) = write(&dir.path().join("outside.txt"));
        assert!(!rules.in_job(&t, &input), "a write outside every folder rule is outside the job");
        // Reads are not fenced.
        let read_input = serde_json::json!({"path": dir.path().join("outside.txt").to_string_lossy()});
        let read = target("read_file", Some("file"), Some(RuleField::Folder(dir.path().join("outside.txt"))));
        assert!(rules.in_job(&read, &read_input));
    }

    /// A tool's or an operation's own rule outranks its family's and its
    /// capability's in the same scope, whichever is stricter; a company deny
    /// still binds an employee, and a locked rule or one with a field is
    /// never set aside.
    #[test]
    fn a_calls_own_rule_outranks_its_default() {
        let co = || Scope::Company;
        let emp = || Scope::Employee("a".into());
        let tool = |s: Scope, k: &str, e: Effect| rule(s, RuleKey::Tool(k.into()), None, e);
        let effect = |rules: Vec<Rule>, t: &Target| set(rules).decide(t).map(|d| d.1);
        let mut lookup = target("mcp__crm__lookup", None, None);
        lookup.tool = "mcp__crm__lookup".into();

        // The server's default decides a tool without its own rule.
        assert_eq!(effect(vec![tool(co(), "mcp__crm__*", Effect::Ask)], &lookup), Some(Effect::Ask));
        // The tool's own rule decides it, looser or stricter.
        for (default, own) in [(Effect::Ask, Effect::Allow), (Effect::Deny, Effect::Allow), (Effect::Allow, Effect::Deny)] {
            let rules = vec![tool(co(), "mcp__crm__*", default), tool(co(), "mcp__crm__lookup", own)];
            assert_eq!(effect(rules, &lookup), Some(own), "{default:?} default, {own:?} own");
        }
        // An operation's own rule outranks its capability's.
        let mut pay = target("ledger.billpayment.create", Some("ledger"), None);
        pay.operation = Some("accounting.ap.ledger.billpayment.create".into());
        let cap = |s: Scope, e: Effect| rule(s, RuleKey::Capability("ledger".into()), None, e);
        let op = |s: Scope, e: Effect| rule(s, RuleKey::Operation("ledger.billpayment.create".into()), None, e);
        assert_eq!(effect(vec![cap(emp(), Effect::Deny), op(emp(), Effect::Allow)], &pay), Some(Effect::Allow));
        assert_eq!(effect(vec![cap(emp(), Effect::Allow), op(emp(), Effect::Ask)], &pay), Some(Effect::Ask));
        // Across scopes a deny still binds: the company's default, set aside
        // only by the company's own tool rule.
        let company_off = tool(co(), "mcp__crm__*", Effect::Deny);
        assert_eq!(effect(vec![company_off.clone(), tool(emp(), "mcp__crm__lookup", Effect::Allow)], &lookup), Some(Effect::Deny));
        assert_eq!(effect(vec![company_off, tool(co(), "mcp__crm__lookup", Effect::Allow)], &lookup), Some(Effect::Allow));
        // A locked rule is never set aside.
        let law = Rule { locked: true, ..cap(emp(), Effect::Ask) };
        assert_eq!(effect(vec![law, op(emp(), Effect::Allow)], &pay), Some(Effect::Ask));
        // A rule with a field never outranks a broader deny.
        let shell = target("run_command", Some("shell"), Some(RuleField::CommandPrefix("git status".into())));
        let prefix = rule(emp(), RuleKey::Tool("run_command".into()), Some(RuleField::CommandPrefix("git".into())), Effect::Allow);
        let off = rule(emp(), RuleKey::Capability("shell".into()), None, Effect::Deny);
        assert_eq!(effect(vec![off, prefix], &shell), Some(Effect::Deny));
    }

    #[test]
    fn a_family_key_and_an_operation_match_their_calls() {
        let mut t = target("mcp__acme__search", None, None);
        t.tool = "mcp__acme__search".into();
        let family = rule(Scope::Company, RuleKey::Tool("mcp__acme__*".into()), None, Effect::Ask);
        assert!(matches(&family, &t));
        assert!(!matches(&rule(Scope::Company, RuleKey::Tool("mcp__other__*".into()), None, Effect::Ask), &t));
        let mut op = target("plugin__ledger", None, None);
        op.operation = Some("accounting.ap.ledger.billpayment.create".into());
        assert!(matches(
            &rule(Scope::Company, RuleKey::Operation("ledger.billpayment.create".into()), None, Effect::Ask),
            &op
        ));
    }
}
