//! The condition expression language a workflow `condition` step routes on
//! (`params.expression`, mode `expression`, the default).
//!
//! Grammar, loosest binding first:
//!
//! ```text
//! or      := and ( "||" and )*
//! and     := unary ( "&&" unary )*
//! unary   := "!" unary | primary
//! primary := "(" or ")" | operand ( ("==" | "!=" | ">=" | "<=" | ">" | "<") operand )?
//! operand := 'text' | "text" | number | true | false | null | reference
//! ```
//!
//! A reference names data, never text: `inputs.<path>`, `nodes.<id>.<path>`,
//! `item` or `item.<path>`, or a bare `<field>.<path>`, which is a field of
//! the condition's one direct parent step's output. Text is always quoted,
//! so `kind == won` compares against a field called `won`; write
//! `kind == 'won'` for the word.
//!
//! Evaluation:
//! - An operand standing alone is tested for truthiness: null, false, 0,
//!   blank text and empty lists or objects are false; everything else true.
//! - `==` and `!=` compare numerically when both sides are numbers (or text
//!   that reads as one), else by their text form (`true` equals `"true"`).
//! - `>=`, `<=`, `>`, `<` compare numbers numerically and text by character
//!   order (ISO dates sort correctly); any other pairing is an error.
//! - `&&` and `||` short-circuit, so `inputs.mode == 'auto' || nodes.x.ok`
//!   does not need `nodes.x` when the mode is auto.
//! - A reference that does not resolve is an error, never false: a missing
//!   field fails the step loudly. Test for presence with mode `exists`.
//!
//! Anything outside this grammar is a parse error naming what was wrong.
//! The same parser runs when a definition is saved, installed or published
//! and when the step runs, so a condition that saves is one the step can
//! evaluate.

use serde_json::Value;

/// A parsed condition.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Or(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Compare(Operand, CmpOp, Operand),
    Truthy(Operand),
}

/// One side of a comparison, or a value tested on its own.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Literal(Value),
    Ref(Reference),
}

/// Where a reference points.
#[derive(Debug, Clone, PartialEq)]
pub enum Reference {
    /// `inputs.<path>`, `nodes.<id>.<path>`, `item[.<path>]`: a path from
    /// the run's data root.
    Rooted(String),
    /// `<field>[.<path>]`: a field of the one direct parent step's output.
    Parent(String),
}

impl Reference {
    pub fn path(&self) -> &str {
        match self {
            Reference::Rooted(p) | Reference::Parent(p) => p,
        }
    }

    /// The node id a `nodes.<id>...` reference reads, if it is one.
    pub fn node_id(&self) -> Option<&str> {
        match self {
            Reference::Rooted(p) => p.strip_prefix("nodes.").map(|rest| rest.split('.').next().unwrap_or("")),
            Reference::Parent(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Ge,
    Le,
    Gt,
    Lt,
}

impl CmpOp {
    fn symbol(self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Ge => ">=",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Lt => "<",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Cmp(CmpOp),
    Text(String),
    Word(String),
}

fn describe(t: &Token) -> String {
    match t {
        Token::LParen => "'('".into(),
        Token::RParen => "')'".into(),
        Token::And => "'&&'".into(),
        Token::Or => "'||'".into(),
        Token::Not => "'!'".into(),
        Token::Cmp(op) => format!("'{}'", op.symbol()),
        Token::Text(s) => format!("'{s}'"),
        Token::Word(w) => format!("'{w}'"),
    }
}

/// Words that read like operators in other languages. Each gets a message
/// naming the operator this language uses instead.
fn foreign_operator(word: &str) -> Option<&'static str> {
    match word {
        "and" | "AND" => Some("use && instead of 'and'"),
        "or" | "OR" => Some("use || instead of 'or'"),
        "not" | "NOT" => Some("use ! instead of 'not'"),
        "in" | "IN" => Some("'in' is not supported; compare each value with == and join them with ||"),
        "contains" => Some("'contains' is its own mode: set params.mode to \"contains\""),
        "is" => Some("use == instead of 'is'"),
        _ => None,
    }
}

fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            '&' if next == Some('&') => {
                out.push(Token::And);
                i += 2;
            }
            '|' if next == Some('|') => {
                out.push(Token::Or);
                i += 2;
            }
            '&' => return Err("a single '&' is not an operator; use &&".into()),
            '|' => return Err("a single '|' is not an operator; use ||".into()),
            '=' if next == Some('=') => {
                if chars.get(i + 2) == Some(&'=') {
                    return Err("'===' is not an operator; use ==".into());
                }
                out.push(Token::Cmp(CmpOp::Eq));
                i += 2;
            }
            '=' => return Err("a single '=' is not a comparison; use ==".into()),
            '!' if next == Some('=') => {
                out.push(Token::Cmp(CmpOp::Ne));
                i += 2;
            }
            '!' => {
                out.push(Token::Not);
                i += 1;
            }
            '>' if next == Some('=') => {
                out.push(Token::Cmp(CmpOp::Ge));
                i += 2;
            }
            '<' if next == Some('=') => {
                out.push(Token::Cmp(CmpOp::Le));
                i += 2;
            }
            '>' => {
                out.push(Token::Cmp(CmpOp::Gt));
                i += 1;
            }
            '<' => {
                out.push(Token::Cmp(CmpOp::Lt));
                i += 1;
            }
            '\'' | '"' => {
                let quote = c;
                let start = i + 1;
                let Some(len) = chars[start..].iter().position(|&ch| ch == quote) else {
                    return Err(format!("text starting at {quote} is never closed"));
                };
                out.push(Token::Text(chars[start..start + len].iter().collect()));
                i = start + len + 1;
            }
            c if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '-' | '.'))
                {
                    i += 1;
                }
                out.push(Token::Word(chars[start..i].iter().collect()));
            }
            '$' if next == Some('{') => {
                return Err("'${...}' is not part of a condition; write the path itself, e.g. nodes.step.field".into());
            }
            '{' if next == Some('{') => {
                return Err("'{{...}}' is not part of a condition; write the path itself, e.g. nodes.step.field".into());
            }
            other => return Err(format!("unexpected character '{other}'")),
        }
    }
    Ok(out)
}

/// Turn a word into a literal or a reference.
fn word_operand(word: &str) -> Result<Operand, String> {
    if let Some(hint) = foreign_operator(word) {
        return Err(hint.to_string());
    }
    match word {
        "true" => return Ok(Operand::Literal(Value::Bool(true))),
        "false" => return Ok(Operand::Literal(Value::Bool(false))),
        "null" => return Ok(Operand::Literal(Value::Null)),
        _ => {}
    }
    let first = word.chars().next().unwrap_or(' ');
    if first.is_ascii_digit() || first == '-' || first == '.' {
        return match word.parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(Operand::Literal(
                serde_json::Number::from_f64(n).map(Value::Number).unwrap_or(Value::Null),
            )),
            _ => Err(format!(
                "'{word}' is neither a number nor a name; quote it if it is text"
            )),
        };
    }
    if word.split('.').any(|seg| seg.is_empty()) {
        return Err(format!("'{word}' has an empty path segment"));
    }
    let root = word.split('.').next().unwrap_or("");
    let reference = match root {
        "inputs" | "nodes" => {
            if !word.contains('.') {
                return Err(format!("'{root}' needs a path after it, e.g. {root}.name"));
            }
            Reference::Rooted(word.to_string())
        }
        "item" => Reference::Rooted(word.to_string()),
        _ => Reference::Parent(word.to_string()),
    };
    Ok(Operand::Ref(reference))
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut left = self.and()?;
        while self.peek() == Some(&Token::Or) {
            self.pos += 1;
            let right = self.and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut left = self.unary()?;
        while self.peek() == Some(&Token::And) {
            self.pos += 1;
            let right = self.unary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Token::Not) {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn operand(&mut self) -> Result<Operand, String> {
        match self.next() {
            Some(Token::Text(s)) => Ok(Operand::Literal(Value::String(s))),
            Some(Token::Word(w)) => word_operand(&w),
            Some(other) => Err(format!("expected a value or a name, found {}", describe(&other))),
            None => Err("the expression ends where a value or a name was expected".into()),
        }
    }

    fn primary(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Token::LParen) {
            self.pos += 1;
            let inner = self.or()?;
            return match self.next() {
                Some(Token::RParen) => Ok(inner),
                Some(other) => Err(format!("expected ')', found {}", describe(&other))),
                None => Err("a '(' is never closed".into()),
            };
        }
        let left = self.operand()?;
        let op = match self.peek() {
            Some(Token::Cmp(op)) => *op,
            _ => return Ok(Expr::Truthy(left)),
        };
        self.pos += 1;
        let right = self.operand()?;
        if let Some(Token::Cmp(next)) = self.peek() {
            return Err(format!(
                "comparisons do not chain ('{}' after '{}'); join two comparisons with &&",
                next.symbol(),
                op.symbol()
            ));
        }
        Ok(Expr::Compare(left, op, right))
    }
}

/// Parse a condition. Every error says what was wrong in the author's terms.
pub fn parse(expression: &str) -> Result<Expr, String> {
    let src = expression.trim();
    if src.is_empty() {
        return Err("the expression is empty".into());
    }
    let tokens = tokenize(src)?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.or()?;
    if let Some(extra) = parser.peek() {
        let hint = match extra {
            Token::Word(w) => foreign_operator(w).map(str::to_string),
            _ => None,
        };
        return Err(hint.unwrap_or_else(|| {
            format!(
                "unexpected {} after a complete condition; join conditions with && or ||, and quote text",
                describe(extra)
            )
        }));
    }
    Ok(expr)
}

/// Every reference the expression reads, in source order.
pub fn references(expr: &Expr) -> Vec<&Reference> {
    fn operand<'e>(o: &'e Operand, out: &mut Vec<&'e Reference>) {
        if let Operand::Ref(r) = o {
            out.push(r);
        }
    }
    fn walk<'e>(e: &'e Expr, out: &mut Vec<&'e Reference>) {
        match e {
            Expr::Or(a, b) | Expr::And(a, b) => {
                walk(a, out);
                walk(b, out);
            }
            Expr::Not(a) => walk(a, out),
            Expr::Compare(l, _, r) => {
                operand(l, out);
                operand(r, out);
            }
            Expr::Truthy(o) => operand(o, out),
        }
    }
    let mut out = Vec::new();
    walk(expr, &mut out);
    out
}

/// Truthiness: null, false, 0, blank text and empty lists or objects are
/// false; everything else is true.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Evaluate a parsed condition. `resolve` returns a reference's value, or
/// an error saying why it has none; that error fails the evaluation.
pub fn eval(
    expr: &Expr,
    resolve: &dyn Fn(&Reference) -> Result<Value, String>,
) -> Result<bool, String> {
    let value = |o: &Operand| -> Result<Value, String> {
        match o {
            Operand::Literal(v) => Ok(v.clone()),
            Operand::Ref(r) => resolve(r),
        }
    };
    match expr {
        Expr::Or(a, b) => Ok(eval(a, resolve)? || eval(b, resolve)?),
        Expr::And(a, b) => Ok(eval(a, resolve)? && eval(b, resolve)?),
        Expr::Not(a) => Ok(!eval(a, resolve)?),
        Expr::Truthy(o) => Ok(truthy(&value(o)?)),
        Expr::Compare(l, op, r) => {
            let (lv, rv) = (value(l)?, value(r)?);
            if let (Some(a), Some(b)) = (as_number(&lv), as_number(&rv)) {
                return Ok(match op {
                    CmpOp::Eq => a == b,
                    CmpOp::Ne => a != b,
                    CmpOp::Ge => a >= b,
                    CmpOp::Le => a <= b,
                    CmpOp::Gt => a > b,
                    CmpOp::Lt => a < b,
                });
            }
            match op {
                CmpOp::Eq => Ok(as_text(&lv) == as_text(&rv)),
                CmpOp::Ne => Ok(as_text(&lv) != as_text(&rv)),
                _ => match (&lv, &rv) {
                    (Value::String(a), Value::String(b)) => Ok(match op {
                        CmpOp::Ge => a >= b,
                        CmpOp::Le => a <= b,
                        CmpOp::Gt => a > b,
                        _ => a < b,
                    }),
                    _ => Err(format!(
                        "'{}' compares {} with {}; ordering needs two numbers or two pieces of text",
                        op.symbol(),
                        lv,
                        rv
                    )),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Resolve against a fixed root, with `parent` standing in for the one
    /// direct parent's output.
    fn run(expr: &str, root: &Value, parent: &Value) -> Result<bool, String> {
        let parsed = parse(expr)?;
        eval(&parsed, &|r| {
            let (base, path) = match r {
                Reference::Rooted(p) => (root, p.as_str()),
                Reference::Parent(p) => (parent, p.as_str()),
            };
            let mut cur = base;
            for seg in path.split('.') {
                cur = match cur {
                    Value::Object(m) => m.get(seg),
                    Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i)),
                    _ => None,
                }
                .ok_or_else(|| format!("'{path}' did not resolve"))?;
            }
            Ok(cur.clone())
        })
    }

    fn root() -> Value {
        json!({
            "inputs": {"posting_policy": "review", "deposit_minimum_job_cents": 50000, "mode": "auto"},
            "nodes": {
                "assemble": {"stage": "booking", "totalCents": 120000},
                "verify": {"verified": true, "calledNumberFromFileBeforeRequest": false},
                "triage": {"changesAmount": false, "kind": "typo", "count": "7"},
                "checklist": {"blockers": 0, "note": ""}
            },
            "item": {"name": "alpha"}
        })
    }

    #[test]
    fn and_or_not_parentheses() {
        let r = root();
        let p = json!({});
        assert!(run("nodes.assemble.stage == 'booking' && nodes.assemble.totalCents >= inputs.deposit_minimum_job_cents", &r, &p).unwrap());
        assert!(!run("nodes.verify.verified == true && nodes.verify.calledNumberFromFileBeforeRequest == true", &r, &p).unwrap());
        assert!(run("nodes.verify.verified == true || nodes.verify.calledNumberFromFileBeforeRequest == true", &r, &p).unwrap());
        assert!(run("!nodes.verify.calledNumberFromFileBeforeRequest", &r, &p).unwrap());
        assert!(!run("!(nodes.verify.verified && nodes.checklist.blockers == 0)", &r, &p).unwrap());
        // && binds tighter than ||.
        assert!(run("nodes.verify.verified || nodes.checklist.blockers == 1 && false", &r, &p).unwrap());
        assert!(!run("(nodes.verify.verified || nodes.checklist.blockers == 1) && false", &r, &p).unwrap());
        assert!(run("nodes.triage.changesAmount == false && nodes.triage.kind != 'disagreement'", &r, &p).unwrap());
    }

    #[test]
    fn comparisons() {
        let r = root();
        let p = json!({});
        // Numeric text compares as a number.
        assert!(run("nodes.triage.count > 5", &r, &p).unwrap());
        assert!(run("nodes.triage.count == 7.0", &r, &p).unwrap());
        assert!(run("nodes.assemble.totalCents != -1", &r, &p).unwrap());
        // Text form equality: a boolean equals its spelling.
        assert!(run("nodes.verify.verified == 'true'", &r, &p).unwrap());
        assert!(run("nodes.assemble.stage == \"booking\"", &r, &p).unwrap());
        // Text orders by characters (ISO dates sort).
        assert!(run("'2026-10-08' > '2026-09-30'", &r, &p).unwrap());
        // Ordering a boolean is an error, not a guess.
        assert!(run("nodes.verify.verified > 1", &r, &p).is_err());
    }

    #[test]
    fn bare_names_read_the_parent() {
        let r = root();
        let parent = json!({"lane": "mechanical", "expansion_signal": true, "value_evidence_present": true, "no_open_escalation": false});
        assert!(run("lane == 'mechanical'", &r, &parent).unwrap());
        assert!(!run("expansion_signal && value_evidence_present && no_open_escalation", &r, &parent).unwrap());
        assert!(run("expansion_signal && value_evidence_present && !no_open_escalation", &r, &parent).unwrap());
        // An unquoted word is a name, never text: `== mechanical` reads a
        // field called mechanical, which the parent does not have.
        let err = run("lane == mechanical", &r, &parent).unwrap_err();
        assert!(err.contains("mechanical"), "{err}");
    }

    #[test]
    fn truthiness() {
        let r = root();
        let p = json!({"empty": [], "zero": 0, "blank": " ", "some": [1]});
        assert!(!run("empty", &r, &p).unwrap());
        assert!(!run("zero", &r, &p).unwrap());
        assert!(!run("blank", &r, &p).unwrap());
        assert!(run("some", &r, &p).unwrap());
        assert!(run("item.name", &r, &p).unwrap());
        assert!(!run("nodes.checklist.note", &r, &p).unwrap());
    }

    #[test]
    fn unresolved_is_an_error_not_false() {
        let r = root();
        let p = json!({});
        assert!(run("nodes.triage.missing == true", &r, &p).is_err());
        assert!(run("!nodes.triage.missing", &r, &p).is_err());
        assert!(run("missing_field", &r, &p).is_err());
        // Short-circuit: the right side is never read when the left decides.
        assert!(run("inputs.mode == 'auto' || nodes.nope.x", &r, &p).unwrap());
        assert!(!run("inputs.mode == 'manual' && nodes.nope.x", &r, &p).unwrap());
    }

    #[test]
    fn parse_errors_are_loud_and_specific() {
        let cases = [
            ("", "empty"),
            ("a & b", "&&"),
            ("a | b", "||"),
            ("a = 1", "=="),
            ("a === 1", "=="),
            ("a and b", "&&"),
            ("a or b", "||"),
            ("not a", "!"),
            ("reasonCode in savable_reasons", "'in' is not supported"),
            ("subject contains urgent", "mode"),
            ("(a && b", "never closed"),
            ("a && b)", "unexpected"),
            ("a == 'x", "never closed"),
            ("1 < a < 3", "do not chain"),
            ("a ==", "ends"),
            ("&& a", "expected a value"),
            ("${nodes.a.b} > 1", "path itself"),
            ("{{nodes.a.b}} > 1", "path itself"),
            ("nodes > 1", "needs a path"),
            ("nodes..a", "empty path segment"),
            ("3d > 1", "neither a number"),
            ("inputs.subject == URGENT: server down", "unexpected character ':'"),
            ("a == b c", "unexpected 'c'"),
        ];
        for (expr, needle) in cases {
            let err = parse(expr).expect_err(expr);
            assert!(err.contains(needle), "{expr:?}: {err:?} should mention {needle:?}");
        }
    }

    #[test]
    fn references_are_listed() {
        let e = parse("nodes.a.x == 1 && (lane || !inputs.y) && item.z != 'q'").unwrap();
        let refs: Vec<String> = references(&e).iter().map(|r| r.path().to_string()).collect();
        assert_eq!(refs, ["nodes.a.x", "lane", "inputs.y", "item.z"]);
        assert_eq!(references(&e)[0].node_id(), Some("a"));
        assert_eq!(references(&e)[1], &Reference::Parent("lane".into()));
    }
}
