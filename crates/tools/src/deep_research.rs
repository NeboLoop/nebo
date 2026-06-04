//! Deterministic deep-research harness — pure-code core.
//!
//! Ported from Claude Code's `deep-research` workflow (v2.1.162). The pipeline is
//! Scope → Search → URL-dedup → Fetch/Extract → 3-vote adversarial Verify → Synthesize,
//! every stage a bounded fan-out whose sub-agent is forced through a schema-validated
//! `StructuredOutput` call (see [`crate::deep_research`] callers + `agent::structured`).
//!
//! This module holds the load-bearing *deterministic* logic — the data model, JSON-schema
//! builders, URL normalisation, fetch-budget allocation, claim ranking, the survival rule,
//! and salvage paths. It performs **no** LLM or network calls; the orchestration that drives
//! the sub-agents lives in `bot_tool.rs`. Isolating the pure logic here keeps the
//! correctness-critical pieces (dedup, survival) testable without network flakiness.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ─── Tuning constants (mirror the reference workflow `standard` depth) ───

const VOTES_PER_CLAIM: usize = 3;
const REFUTATIONS_REQUIRED: usize = 2;
const MAX_FETCH: usize = 15;
const MAX_VERIFY_CLAIMS: usize = 25;
const MAX_CLAIMS_PER_SOURCE: usize = 5;

// ─── Enums (LLM-facing; serde names match the schema enums) ───

/// How relevant a search result is to the *original* question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Relevance {
    High,
    Medium,
    Low,
}

impl Relevance {
    /// Sort rank — lower is better (high relevance first).
    pub fn rank(self) -> usize {
        match self {
            Self::High => 0,
            Self::Medium => 1,
            Self::Low => 2,
        }
    }
}

/// How central a claim is to the research question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Importance {
    Central,
    Supporting,
    Tangential,
}

impl Importance {
    pub fn rank(self) -> usize {
        match self {
            Self::Central => 0,
            Self::Supporting => 1,
            Self::Tangential => 2,
        }
    }
}

/// Trustworthiness of a source, as assessed by the extractor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceQuality {
    Primary,
    Secondary,
    Blog,
    Forum,
    Unreliable,
}

impl SourceQuality {
    pub fn rank(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Secondary => 1,
            Self::Blog => 2,
            Self::Forum => 3,
            Self::Unreliable => 4,
        }
    }
}

/// Confidence level on a verdict or a synthesised finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

// ─── Phase outputs (deserialized from the validated StructuredOutput) ───

/// One search angle the scope phase produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Angle {
    pub label: String,
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

/// Output of the Scope phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeOut {
    pub question: String,
    pub summary: String,
    pub angles: Vec<Angle>,
}

/// One web result returned by a search sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub url: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub relevance: Relevance,
}

/// Output of one Search phase sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOut {
    pub results: Vec<SearchResult>,
}

/// A search angle paired with its results — input to [`dedup_and_budget`].
#[derive(Debug, Clone)]
pub struct AngleResults {
    pub angle: String,
    pub results: Vec<SearchResult>,
}

/// A claim as extracted from one source (before id / source assignment).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimDraft {
    pub claim: String,
    pub quote: String,
    pub importance: Importance,
}

/// Output of one Fetch/Extract sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractOut {
    #[serde(rename = "sourceQuality")]
    pub source_quality: SourceQuality,
    #[serde(default, rename = "publishDate", skip_serializing_if = "Option::is_none")]
    pub publish_date: Option<String>,
    pub claims: Vec<ClaimDraft>,
}

/// A fully-assembled claim, ready for ranking + verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    /// Stable id, assigned **after** the rank+cap (`rank_claims`).
    pub id: usize,
    pub text: String,
    pub quote: String,
    pub importance: Importance,
    pub source_url: String,
    pub source_quality: SourceQuality,
    /// Relative path to the saved source file (`sources/src_<hash>.txt`).
    pub source_ref: String,
}

impl Claim {
    /// Assemble a claim from an extracted draft + its source context. `id` is a
    /// placeholder (0) until [`rank_claims`] assigns stable ids after the cap.
    pub fn from_draft(
        draft: ClaimDraft,
        source_url: impl Into<String>,
        source_quality: SourceQuality,
        source_ref: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            text: draft.claim,
            quote: draft.quote,
            importance: draft.importance,
            source_url: source_url.into(),
            source_quality,
            source_ref: source_ref.into(),
        }
    }
}

/// One adversarial verifier's verdict. Absence of a verdict (a [`Vote`] of `None`)
/// means the voter abstained (user-skip or sub-agent error) and is NOT a pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub refuted: bool,
    pub evidence: String,
    pub confidence: Confidence,
    #[serde(default, rename = "counterSource", skip_serializing_if = "Option::is_none")]
    pub counter_source: Option<String>,
}

/// One verifier slot. `None` = abstain (never counts as a passing vote).
pub type Vote = Option<Verdict>;

/// A synthesised finding row (one entry in the final report).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFinding {
    pub claim: String,
    pub confidence: Confidence,
    pub sources: Vec<String>,
    pub evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vote: Option<String>,
}

/// Output of the Synthesize phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportOut {
    pub summary: String,
    pub findings: Vec<ReportFinding>,
    pub caveats: String,
    #[serde(default, rename = "openQuestions", skip_serializing_if = "Option::is_none")]
    pub open_questions: Option<Vec<String>>,
}

// ─── Depth config ───

/// Per-run tuning: how many angles, results-per-angle, fetches, and claims to verify.
/// Scales the bounded fan-out so a `quick` run is ~19 calls and `standard` ~97.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub min_angles: usize,
    pub max_angles: usize,
    pub results_per_angle: usize,
    pub max_fetch: usize,
    pub max_verify_claims: usize,
    pub max_claims_per_source: usize,
    pub votes_per_claim: usize,
    pub refutations_required: usize,
}

impl Config {
    /// Resolve a depth label. Unknown labels fall back to `standard`.
    pub fn for_depth(depth: &str) -> Self {
        match depth {
            // ~19 calls: 1 scope + 3 search + 5 fetch + 3×3 verify + 1 synth.
            "quick" => Self {
                min_angles: 3,
                max_angles: 3,
                results_per_angle: 4,
                max_fetch: 5,
                max_verify_claims: 3,
                max_claims_per_source: MAX_CLAIMS_PER_SOURCE,
                votes_per_claim: VOTES_PER_CLAIM,
                refutations_required: REFUTATIONS_REQUIRED,
            },
            // ~150+ calls: 1 + 6 + 25 + 40×3 + 1.
            "deep" => Self {
                min_angles: 4,
                max_angles: 6,
                results_per_angle: 6,
                max_fetch: 25,
                max_verify_claims: 40,
                max_claims_per_source: MAX_CLAIMS_PER_SOURCE,
                votes_per_claim: VOTES_PER_CLAIM,
                refutations_required: REFUTATIONS_REQUIRED,
            },
            // ~97 calls: 1 + 5 + 15 + 25×3 + 1. The reference default.
            _ => Self::standard(),
        }
    }

    fn standard() -> Self {
        Self {
            min_angles: 3,
            max_angles: 6,
            results_per_angle: 6,
            max_fetch: MAX_FETCH,
            max_verify_claims: MAX_VERIFY_CLAIMS,
            max_claims_per_source: MAX_CLAIMS_PER_SOURCE,
            votes_per_claim: VOTES_PER_CLAIM,
            refutations_required: REFUTATIONS_REQUIRED,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::standard()
    }
}

// ─── JSON-schema builders (one per phase; mirror the reference A.4 schemas) ───
//
// Every object closes `additionalProperties: false` so the model cannot smuggle extra
// keys past validation, and every verbatim `quote` carries `minLength: 1` so an empty
// quote (an unsupported claim) is rejected at the schema boundary.

/// Schema for the Scope phase (`min..max` angles).
pub fn scope_schema(cfg: &Config) -> Value {
    json!({
        "type": "object",
        "required": ["question", "angles", "summary"],
        "additionalProperties": false,
        "properties": {
            "question": { "type": "string" },
            "summary": { "type": "string" },
            "angles": {
                "type": "array",
                "minItems": cfg.min_angles,
                "maxItems": cfg.max_angles,
                "items": {
                    "type": "object",
                    "required": ["label", "query"],
                    "additionalProperties": false,
                    "properties": {
                        "label": { "type": "string" },
                        "query": { "type": "string" },
                        "rationale": { "type": "string" }
                    }
                }
            }
        }
    })
}

/// Schema for a Search phase sub-agent (≤ `results_per_angle` results).
pub fn search_schema(cfg: &Config) -> Value {
    json!({
        "type": "object",
        "required": ["results"],
        "additionalProperties": false,
        "properties": {
            "results": {
                "type": "array",
                "maxItems": cfg.results_per_angle,
                "items": {
                    "type": "object",
                    "required": ["url", "title", "relevance"],
                    "additionalProperties": false,
                    "properties": {
                        "url": { "type": "string" },
                        "title": { "type": "string" },
                        "snippet": { "type": "string" },
                        "relevance": { "enum": ["high", "medium", "low"] }
                    }
                }
            }
        }
    })
}

/// Schema for a Fetch/Extract sub-agent (≤ `max_claims_per_source` claims).
pub fn extract_schema(cfg: &Config) -> Value {
    json!({
        "type": "object",
        "required": ["claims", "sourceQuality"],
        "additionalProperties": false,
        "properties": {
            "sourceQuality": { "enum": ["primary", "secondary", "blog", "forum", "unreliable"] },
            "publishDate": { "type": "string" },
            "claims": {
                "type": "array",
                "maxItems": cfg.max_claims_per_source,
                "items": {
                    "type": "object",
                    "required": ["claim", "quote", "importance"],
                    "additionalProperties": false,
                    "properties": {
                        "claim": { "type": "string" },
                        "quote": { "type": "string", "minLength": 1 },
                        "importance": { "enum": ["central", "supporting", "tangential"] }
                    }
                }
            }
        }
    })
}

/// Schema for one adversarial Verify vote.
pub fn verdict_schema() -> Value {
    json!({
        "type": "object",
        "required": ["refuted", "evidence", "confidence"],
        "additionalProperties": false,
        "properties": {
            "refuted": { "type": "boolean" },
            "evidence": { "type": "string" },
            "confidence": { "enum": ["high", "medium", "low"] },
            "counterSource": { "type": "string" }
        }
    })
}

/// Schema for the Synthesize phase report.
pub fn report_schema() -> Value {
    json!({
        "type": "object",
        "required": ["summary", "findings", "caveats"],
        "additionalProperties": false,
        "properties": {
            "summary": { "type": "string" },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["claim", "confidence", "sources", "evidence"],
                    "additionalProperties": false,
                    "properties": {
                        "claim": { "type": "string" },
                        "confidence": { "enum": ["high", "medium", "low"] },
                        "sources": { "type": "array", "items": { "type": "string" } },
                        "evidence": { "type": "string" },
                        "vote": { "type": "string" }
                    }
                }
            },
            "caveats": { "type": "string" },
            "openQuestions": { "type": "array", "items": { "type": "string" } }
        }
    })
}

// ─── URL normalisation + fetch-budget allocation ───

/// Normalise a URL for dedup: lowercase, strip scheme + leading `www.` + `#fragment`,
/// and trim a trailing slash. String-based (not URL-parse based) so scheme-less inputs
/// (`x.com/#a`) collapse to the same key as their fully-qualified forms.
pub fn norm_url(u: &str) -> String {
    let mut s = u.trim().to_lowercase();
    if let Some(i) = s.find('#') {
        s.truncate(i);
    }
    for scheme in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest.to_string();
            break;
        }
    }
    if let Some(rest) = s.strip_prefix("www.") {
        s = rest.to_string();
    }
    s.trim_end_matches('/').to_string()
}

/// Why a search result did not make the fetch plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DropReason {
    /// A higher-ranked result already claimed this normalised URL.
    Duplicate { dup_of: String },
    /// The fetch budget was spent and this result was only medium/low relevance.
    Budget,
}

/// A source the harness will fetch + extract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedFetch {
    pub url: String,
    pub title: String,
    pub angle: String,
    pub relevance: Relevance,
}

/// A source that was dropped before fetching, with the reason (no silent truncation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroppedSource {
    pub url: String,
    pub title: String,
    pub angle: String,
    pub relevance: Relevance,
    #[serde(flatten)]
    pub reason: DropReason,
}

/// The outcome of dedup + budgeting: what to fetch, and what was dropped + why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchPlan {
    pub fetch: Vec<PlannedFetch>,
    pub dropped: Vec<DroppedSource>,
}

/// Dedup search results across angles and apply the fetch budget.
///
/// Angles are processed in order; within an angle, results are sorted by relevance
/// (high → low, stable for ties). The first occurrence of a normalised URL wins; later
/// duplicates are dropped as [`DropReason::Duplicate`]. Once the budget is spent, only
/// **medium/low** relevance results are dropped ([`DropReason::Budget`]) — high-relevance
/// results always pass, matching the reference (`fetchSlots <= 0 && relRank >= 1`).
pub fn dedup_and_budget(per_angle: &[AngleResults], max_fetch: usize) -> FetchPlan {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut slots: isize = max_fetch as isize;
    let mut fetch = Vec::new();
    let mut dropped = Vec::new();

    for angle in per_angle {
        let mut sorted = angle.results.clone();
        sorted.sort_by_key(|r| r.relevance.rank());

        for r in sorted {
            let key = norm_url(&r.url);
            if let Some(dup_of) = seen.get(&key) {
                dropped.push(DroppedSource {
                    url: r.url,
                    title: r.title,
                    angle: angle.angle.clone(),
                    relevance: r.relevance,
                    reason: DropReason::Duplicate {
                        dup_of: dup_of.clone(),
                    },
                });
                continue;
            }
            if slots <= 0 && r.relevance.rank() >= 1 {
                dropped.push(DroppedSource {
                    url: r.url,
                    title: r.title,
                    angle: angle.angle.clone(),
                    relevance: r.relevance,
                    reason: DropReason::Budget,
                });
                continue;
            }
            seen.insert(key, angle.angle.clone());
            slots -= 1;
            fetch.push(PlannedFetch {
                url: r.url,
                title: r.title,
                angle: angle.angle.clone(),
                relevance: r.relevance,
            });
        }
    }

    FetchPlan { fetch, dropped }
}

// ─── Claim ranking + survival ───

/// Rank claims by importance (central first) then source quality (primary first),
/// cap to `max`, and assign stable ids **after** the cap so ids index the final order.
pub fn rank_claims(mut claims: Vec<Claim>, max: usize) -> Vec<Claim> {
    claims.sort_by(|a, b| {
        a.importance
            .rank()
            .cmp(&b.importance.rank())
            .then_with(|| a.source_quality.rank().cmp(&b.source_quality.rank()))
    });
    claims.truncate(max);
    for (i, c) in claims.iter_mut().enumerate() {
        c.id = i;
    }
    claims
}

/// A claim survives verification iff it was actually adjudicated — a quorum of valid
/// (non-abstaining) votes AND fewer than `refutations_required` refuting it. Too many
/// abstentions = unverified, which must NOT pass (otherwise all-abstain → 0 refutes →
/// a false survive). Mirrors the reference exactly.
pub fn survives(votes: &[Vote], refutations_required: usize) -> bool {
    let valid = votes.iter().filter(|v| v.is_some()).count();
    let refutes = votes.iter().flatten().filter(|v| v.refuted).count();
    valid >= refutations_required && refutes < refutations_required
}

// ─── Final report + salvage paths ───

/// Source row in the final report's source list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRow {
    pub url: String,
    pub quality: SourceQuality,
    pub angle: String,
    pub claim_count: usize,
}

/// A refuted claim row (kept for transparency in the report).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefutedRow {
    pub claim: String,
    pub vote: String,
    pub source: String,
}

/// Run statistics emitted with every report (success and salvage alike).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub angles: usize,
    pub sources_fetched: usize,
    pub claims_extracted: usize,
    pub claims_verified: usize,
    pub confirmed: usize,
    pub killed: usize,
    pub after_synthesis: usize,
    pub url_dupes: usize,
    pub budget_dropped: usize,
}

/// The harness's final result. `findings` is empty on every salvage path; `confirmed`
/// carries the raw survivors only when synthesis itself failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchReport {
    pub question: String,
    pub summary: String,
    pub findings: Vec<ReportFinding>,
    pub refuted: Vec<RefutedRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<Vec<RefutedRow>>,
    pub sources: Vec<SourceRow>,
    pub stats: Stats,
}

/// Salvage: zero claims survived ranking (every source empty/failed). Report honestly
/// rather than fabricating findings.
pub fn salvage_no_claims(
    question: &str,
    sources: Vec<SourceRow>,
    stats: Stats,
) -> ResearchReport {
    let summary = format!(
        "No claims extracted. {} sources fetched, all empty/failed. {} URL dupes, {} budget-dropped.",
        stats.sources_fetched, stats.url_dupes, stats.budget_dropped
    );
    ResearchReport {
        question: question.to_string(),
        summary,
        findings: Vec::new(),
        refuted: Vec::new(),
        confirmed: None,
        sources,
        stats,
    }
}

/// Salvage: every claim was refuted by adversarial verification. Inconclusive.
pub fn salvage_all_refuted(
    question: &str,
    killed: Vec<RefutedRow>,
    sources: Vec<SourceRow>,
    stats: Stats,
) -> ResearchReport {
    let summary = format!(
        "All {} claims refuted by adversarial verification. Research inconclusive — sources may be low-quality or claims overstated.",
        killed.len()
    );
    ResearchReport {
        question: question.to_string(),
        summary,
        findings: Vec::new(),
        refuted: killed,
        confirmed: None,
        sources,
        stats,
    }
}

/// Salvage: claims survived but synthesis was skipped/failed. Return the verified
/// survivors raw rather than discarding the whole run.
pub fn salvage_synth_failed(
    question: &str,
    confirmed: Vec<RefutedRow>,
    killed: Vec<RefutedRow>,
    sources: Vec<SourceRow>,
    stats: Stats,
) -> ResearchReport {
    let summary = format!(
        "Synthesis step was skipped or failed — returning {} verified claims unmerged.",
        confirmed.len()
    );
    ResearchReport {
        question: question.to_string(),
        summary,
        findings: Vec::new(),
        refuted: killed,
        confirmed: Some(confirmed),
        sources,
        stats,
    }
}

/// Build the success-path report from a synthesised [`ReportOut`].
pub fn build_report(
    question: &str,
    report: ReportOut,
    killed: Vec<RefutedRow>,
    sources: Vec<SourceRow>,
    mut stats: Stats,
) -> ResearchReport {
    stats.after_synthesis = report.findings.len();
    ResearchReport {
        question: question.to_string(),
        summary: report.summary,
        findings: report.findings,
        refuted: killed,
        confirmed: None,
        sources,
        stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(refuted: bool) -> Verdict {
        Verdict {
            refuted,
            evidence: "e".into(),
            confidence: Confidence::Medium,
            counter_source: None,
        }
    }

    /// V2 — survival table. Abstain (`None`) never counts as a pass; a claim survives
    /// only with a quorum of valid votes AND fewer than 2 refuting.
    #[test]
    fn survival_table() {
        let pass = || Some(verdict(false));
        let refute = || Some(verdict(true));
        let abstain = || None::<Verdict>;

        // 3 abstain → unverified → kill.
        assert!(!survives(&[abstain(), abstain(), abstain()], REFUTATIONS_REQUIRED));
        // 2 abstain + 1 pass → quorum not met → kill.
        assert!(!survives(&[abstain(), abstain(), pass()], REFUTATIONS_REQUIRED));
        // 1 refute (2 pass) → adjudicated, <2 refutes → survive.
        assert!(survives(&[refute(), pass(), pass()], REFUTATIONS_REQUIRED));
        // 2 refute (1 pass) → killed.
        assert!(!survives(&[refute(), refute(), pass()], REFUTATIONS_REQUIRED));
        // 3 pass → survive.
        assert!(survives(&[pass(), pass(), pass()], REFUTATIONS_REQUIRED));
        // 2 pass + 1 abstain → quorum met, 0 refutes → survive.
        assert!(survives(&[pass(), pass(), abstain()], REFUTATIONS_REQUIRED));
    }

    /// V3a — `norm_url` collapses scheme/www/trailing-slash/fragment variants to one key.
    #[test]
    fn norm_url_collapses_variants() {
        let a = norm_url("http://www.x.com/");
        let b = norm_url("https://x.com");
        let c = norm_url("x.com/#a");
        assert_eq!(a, "x.com");
        assert_eq!(a, b);
        assert_eq!(a, c);
        // Distinct paths stay distinct.
        assert_ne!(norm_url("https://x.com/a"), norm_url("https://x.com/b"));
    }

    fn result(url: &str, rel: Relevance) -> SearchResult {
        SearchResult {
            url: url.into(),
            title: url.into(),
            snippet: None,
            relevance: rel,
        }
    }

    /// V3b — dedup drops repeat URLs across angles and records the reason.
    #[test]
    fn dedup_drops_duplicate_urls() {
        let per_angle = vec![
            AngleResults {
                angle: "broad".into(),
                results: vec![result("https://x.com", Relevance::High)],
            },
            AngleResults {
                angle: "news".into(),
                results: vec![result("http://www.x.com/", Relevance::Medium)],
            },
        ];
        let plan = dedup_and_budget(&per_angle, MAX_FETCH);
        assert_eq!(plan.fetch.len(), 1);
        assert_eq!(plan.dropped.len(), 1);
        assert!(matches!(
            plan.dropped[0].reason,
            DropReason::Duplicate { .. }
        ));
    }

    /// V3c — once the budget is spent, medium/low are dropped but high still passes.
    #[test]
    fn budget_drops_med_low_keeps_high() {
        let per_angle = vec![AngleResults {
            angle: "a".into(),
            results: vec![
                result("https://a.com", Relevance::High),
                result("https://b.com", Relevance::Medium),
                // Budget = 1: a.com (high) takes the only slot; b.com (med) dropped;
                // c.com (high) still passes despite no slots left.
                result("https://c.com", Relevance::High),
            ],
        }];
        let plan = dedup_and_budget(&per_angle, 1);
        let fetched: Vec<&str> = plan.fetch.iter().map(|f| f.url.as_str()).collect();
        assert!(fetched.contains(&"https://a.com"));
        assert!(fetched.contains(&"https://c.com"));
        assert_eq!(plan.dropped.len(), 1);
        assert_eq!(plan.dropped[0].url, "https://b.com");
        assert!(matches!(plan.dropped[0].reason, DropReason::Budget));
    }

    /// Rank claims central→tangential then primary→unreliable, cap, assign ids after.
    #[test]
    fn rank_caps_and_assigns_ids() {
        let mk = |imp: Importance, q: SourceQuality, url: &str| Claim {
            id: 99,
            text: url.into(),
            quote: "q".into(),
            importance: imp,
            source_url: url.into(),
            source_quality: q,
            source_ref: "sources/x.txt".into(),
        };
        let claims = vec![
            mk(Importance::Tangential, SourceQuality::Primary, "t"),
            mk(Importance::Central, SourceQuality::Blog, "c-blog"),
            mk(Importance::Central, SourceQuality::Primary, "c-primary"),
        ];
        let ranked = rank_claims(claims, 2);
        assert_eq!(ranked.len(), 2);
        // central+primary first, central+blog second; tangential cut by the cap.
        assert_eq!(ranked[0].source_url, "c-primary");
        assert_eq!(ranked[0].id, 0);
        assert_eq!(ranked[1].source_url, "c-blog");
        assert_eq!(ranked[1].id, 1);
    }

    #[test]
    fn config_depth_scales() {
        assert_eq!(Config::for_depth("quick").max_fetch, 5);
        assert_eq!(Config::for_depth("standard").max_fetch, MAX_FETCH);
        assert_eq!(Config::for_depth("deep").max_fetch, 25);
        // Unknown → standard.
        assert_eq!(Config::for_depth("bogus").max_fetch, MAX_FETCH);
    }
}
