# Typed Decisions

For a judgment (is this lead hot, which category, how urgent), ask a typed
decision instead of a model call. The page uses `decide`; the app's employee
has the `decide` tool with the same request. Offer it when the app makes
choices: a game's opponent, a triage, a score.

```js
const { answers } = await NeboAppSDK.decide({
  state: lead,
  questions: {
    tier: { type: "choice", instructions: "How warm is this lead, by `status` and `last_contact`?",
            criteria: { hot: "ready to buy now", warm: "interested", cold: "no interest", other: "can't tell" } },
    fit:  { type: "score", instructions: "How well does `company` fit our customers?", criteria: ["poor", "fair", "good", "great"] },
    reply:{ type: "noul", instructions: "`message` asks us for a reply." },
  },
});
```

```
decide(state: <records from app_data>, questions: { "tier": { "type": "choice", "instructions": "...", "criteria": { ... } } })
```

The whole question lives in `instructions`, naming the state's fields in
backticks. Include an escape option (`other`). Counting, dates and
thresholds stay in code.
