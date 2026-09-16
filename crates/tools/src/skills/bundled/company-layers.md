---
name: company-layers
description: Build and maintain the industry, franchise, and company layers this company works by. Use when the owner says how their company works, what it always does, what nobody may do, or a number they have decided, and when they ask to set up their company, their trade, or their brand's requirements. Also use when something the owner says contradicts a layer already on disk.
metadata:
  author: nebo-official
  version: "1.0"
---

# Company layers

A company's knowledge lives in three layers, and this is how you write them. The layers are packs on disk: one marker file and typed folders beside it. You write them with the `pack` tool, which stages what you send, validates it with the loader that reads it at runtime, and only then moves it into place, so a pack that would not load never lands.

You are writing down how a company works, in the owner's own words, so that every employee reads it once and works from it. You are not writing procedure. A pack is knowledge; a skill is procedure; the loader refuses a pack that carries a `SKILL.md` or a `skills/` folder.

## When to use

- The owner describes their company, their trade, or a brand whose requirements they work under.
- The owner states something the company always does, something nobody may do, or a number they have settled.
- The owner says something that contradicts what a layer already says.
- The owner asks what the company knows, or why an employee behaved the way it did.

If the owner just wants a task done, do the task.

## 1. Which layer, before you write anything

One test, asked before every single file: **who else is this true for?**

| True for | Layer | Marker |
|---|---|---|
| Every company in this trade | industry | `INDUSTRY.md` |
| Every franchisee of this brand | franchise | `FRANCHISE.md` |
| Only this company | company | `COMPANY.md` |

Higher wins: company over franchise over industry. A layer that says nothing falls through to the next.

Getting this wrong is the most common mistake and the most expensive one. A fact that belongs to the trade but gets written into the company layer has to be written again by every company that hires the same employees, and none of them can fix it for the others. When you are unsure, say which way you are leaning and why, and let the owner settle it. "Insurance carriers pay on an approved estimate" is the trade. "We do not take cash jobs" is the company.

The company layer is the owner's own hand, and writing it is an authority the owner gives to an employee: `layers.company.write` is a gated operation, so the owner grants it to you on your Approvals screen (Settings → this employee → Approvals). Without that grant the call reaches the owner as an approval, and in an unattended run it is refused instead. Either way the authority is yours or it is not — it does not depend on which chat you happen to be standing in. When you cannot perform it, draft what you would write and put the draft in front of the owner.

## 2. Interview, do not interrogate

No forms. No questionnaires. Ask what the company does and who it serves, then derive the rest from the answer and check your reading back.

Start with two questions and listen:

1. What does the company do?
2. Who does it do it for?

From the answer, most of a pack is already implied. Work it out and confirm, one thing at a time:

- **Words.** Which ordinary words does this company use in a particular way? Repeat a word back and ask what it means here.
- **Parties.** Who does it deal with, and how does each one behave? Customers, payers, suppliers, inspectors, the brand.
- **Rules.** What does it always do, and what does it never do? Ask for the last time it happened, not for a policy.
- **Numbers.** Which numbers has the owner already decided? Terms, windows, limits, targets.

Ask about money only where money is genuinely unset, and **never propose a number for it**. An unset money value is written as a question with no default, and until the owner answers it, the operation is proposed and never performed. That is the safe state, and it needs no guess from you.

Write down what the owner said, in the owner's words. The pack is not a summary of the conversation; the sentences the owner used are the sentences the employees will read.

## 3. The file type for each thing learned

Everything goes in a typed folder, and the frontmatter carries the structure. The body is plain prose: one short paragraph saying what it means.

**`vocabulary/`** holds a word this company uses differently.
```
---
term: Supplement
---

An addition to an approved estimate, priced at the same rates.
```

**`parties/`** holds someone the company deals with, and how they behave.
```
---
party: Property manager
entity_kind: company
---

Signs the work order and pays from the building's account, never the tenant.
```

**`rules/`** holds something the company always does. `always: true` when it holds regardless of the situation; leave it off when it only matters sometimes, and it will be recalled when it does.
```
---
rule: Deposits
always: true
---

A deposit is collected before work is scheduled.
```

**`laws/`** holds something no employee may ever do. A law names the operations it holds with `ceiling: ["capability.resource.action"]`. **Two kinds, and the difference is load-bearing:**

- **`reserved_to: owner`** is the owner's own hand. It reaches the owner as a decision; the General Manager can never grant it and no standing grant covers it. Reserve what the owner would want to decide themselves: money above the bounds, signing, equity, hiring an employee whose work touches money, changing this layer.
- **No `reserved_to`** is Blocked outright, for every employee including the owner's own. Nobody performs it and nobody grants it. Block what should never happen at all.

```
---
law: Spend above the company bounds
ceiling: ["spend.above_company_bounds"]
freshness: required
reserved_to: owner
---

Any money operation above the unattended bounds in `standards/` is the owner's. The General Manager frames it, recommends, and waits.
```

**`standards/`** holds a number or a setting already decided. A semantic dotted `id` and a `value`. The id is durable: employees look values up by it, so name it for its subject.
```
---
id: cleaning.scheduling.lead_time_days
scope: company
value: 2
---

How many days ahead a job is scheduled once the deposit clears.
```

**`standards/` without a `value`** is a number not decided yet. The loader reads a standard with an `id` and no `value` as a question: a `label` written as the question the owner will answer, and a `missing` line saying what the company does until it is answered. When it is money, `money: true` and no default at all.
```
---
id: cleaning.support.refund_limit
scope: company
label: "What may an employee refund without asking?"
money: true
missing: "Every refund is proposed to the owner and none is issued."
---

Unset on purpose. A money question never carries a default.
```

### The six ids the runtime itself reads

These six are read by Nebo, not by an employee, so they are spelled exactly the same in every company. They are the company's unattended bounds and they are the General Manager's ceiling: it never grants an employee more than they allow. Write them in the company layer, as ordinary standards with values, and never invent new ids under `company.`.

| id | what it bounds |
|---|---|
| `company.unattended.spend_per_day_cents` | every unattended money operation, every employee, one day |
| `company.unattended.spend_per_counterparty_day_cents` | one counterparty, one day |
| `company.unattended.spend_per_operation_cents` | one unattended money operation |
| `company.unattended.irreversible_per_day` | irreversible operations in a day, counted once each whatever their size |
| `company.unattended.grant_freshness_secs` | how current a money grant must be provable to be |
| `company.owner.pages` | when the owner is paged, in which timezone, on which channels |

Everything else the company names in a namespace of its own subject: `cleaning.support.first_response_hours`, not `company.support.first_response_hours`.

**Never a `SKILL.md`, and never a `skills/` folder.** The loader refuses the whole pack, and it is right to: a pack is knowledge and a skill is procedure.

### The other two folders

`workflows/` holds how a piece of work runs, in prose. `reference/` holds long material that stays out of the employees' context and is fetched when asked for. Only these seven folders exist: `vocabulary`, `parties`, `rules`, `laws`, `standards`, `workflows`, `reference`.

## 4. Writing the pack

One call. The marker's body is the whole layer in a few paragraphs, written for a new hire, opening with the sentence that says what the company is for; pass that same sentence as `purpose` in `frontmatter`, because the runtime reads it there.

```
pack {
  "action": "create",
  "slug": "bright-carpet",
  "layer": "company",
  "name": "Bright Carpet Care",
  "capabilities": ["mail", "calendar", "billing"],
  "frontmatter": { "purpose": "Keep apartment buildings' carpets clean on a schedule the manager never has to chase." },
  "body": "# Bright Carpet Care\n\nKeep apartment buildings' carpets clean on a schedule the manager never has to chase.\n\nWe clean for property managers, not tenants. The manager books, the building pays, and the tenant is the person whose door we knock on.",
  "folders": {
    "vocabulary": [
      { "name": "turn", "frontmatter": { "term": "Turn" }, "body": "A unit between tenants. A turn is scheduled around the lease date and cannot move." }
    ],
    "parties": [
      { "name": "property-manager", "frontmatter": { "party": "Property manager", "entity_kind": "company" }, "body": "Books the work and pays from the building's account. Never bill a tenant." }
    ],
    "rules": [
      { "name": "tenant-notice", "frontmatter": { "rule": "Tenant notice", "always": true }, "body": "A tenant is given 24 hours' notice before we enter, in writing, through the manager." }
    ],
    "laws": [
      { "name": "spend-above-bounds", "frontmatter": { "law": "Spend above the company bounds", "ceiling": ["spend.above_company_bounds"], "freshness": "required", "reserved_to": "owner" }, "body": "Any money operation above the bounds in standards/ is the owner's." }
    ],
    "standards": [
      { "name": "unattended_spend_per_day", "frontmatter": { "id": "company.unattended.spend_per_day_cents", "scope": "company", "value": 50000, "money": true }, "body": "Everything the workforce may spend unattended in one day." },
      { "name": "notice_hours", "frontmatter": { "id": "cleaning.scheduling.tenant_notice_hours", "scope": "company", "value": 24 }, "body": "Hours of notice a tenant gets before entry." },
      { "name": "refund_limit", "frontmatter": { "id": "cleaning.support.refund_limit", "scope": "company", "label": "What may an employee refund without asking?", "money": true, "missing": "Every refund is proposed to the owner and none is issued." }, "body": "Unset on purpose. A money question never carries a default." }
    ]
  }
}
```

That started from one sentence about what the company does. Everything else came out of the two questions and was confirmed with the owner before it was written.

Then read it back with `pack show` and check it says what the owner said.

## 5. Tell the owner what happens next

The write is not the end of it, and the owner should not be surprised by what follows.

**Say first that nothing has reached anyone yet.** A write to a layer is parked, not applied. The owner decides when the workforce learns it, in Settings under Layers, where they can read the change as a diff and then apply it. Until they do, every employee is working from what it knew before, and that is deliberate: an owner mid-edit should not send the whole company reading three times over.

Then say what happens when they apply it, in a line or two:

- Every employee reads the change once, decides what matters to its own job, and rewrites its own rules from it. It does not consult the pack again while working; it works from what it wrote down.
- Laws become locked entries in every employee's policy: blocked ones nobody performs, owner-reserved ones nobody grants.
- Standards fill in the questions employees were holding open, wherever the ids match.
- Questions with no value stay open, and what the `missing` line says is what the company does until the owner answers.

Then tell them what you wrote and which employees will read it. Name them.

## 6. Maintaining, not just creating

**When the owner contradicts the layer, the layer is wrong.** Do not argue with the owner and do not keep a second version in memory. Edit the file, say which file changed, and let the employees read it. The owner's files are the interface and a save is an assertion.

`pack create` writes the whole pack: it replaces the directory it names. To change one thing, either send the whole pack again with that one file changed, or write the single file in place under `<data_dir>/packs/<slug>/<folder>/<name>.md` with the `os` tool. Either way the watcher sees the write and the employees read it. Use `pack show` first so you are editing what is actually there, not what you remember writing.

**When something in the company layer turns out to be true for the whole trade, move it up.** Take it out of the company pack and put it in the industry pack, keeping the same id for a standard so nothing that reads it breaks. An industry pack that other companies will work by is published, which means review; a company pack is only ever this company's. Tell the owner that is what moving it up means before you do it.

Removing the company layer is the owner's too, the same way writing it is: `layers.company.remove` is its own grant, and an employee that may write the layer does not automatically get to delete it.

## Before you call it done

- [ ] Every file passed the "who else is this true for" test, out loud, before it was written.
- [ ] The owner's own words are in the bodies, not your summary of them.
- [ ] No number was proposed for money the owner had not already decided.
- [ ] Every money value that is unset is a question with a `missing` line and no default.
- [ ] The six `company.` ids are spelled exactly as listed.
- [ ] The pack carries no `SKILL.md` and no `skills/` folder.
- [ ] `pack show` reads back the way the owner described their company.
- [ ] The owner has been told what changed and which employees read it.
