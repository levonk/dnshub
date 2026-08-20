---
workflow: "Doubt-Driven Review"
slug: "doubt-driven-review"
description: "Subject non-trivial decisions to a fresh-context adversarial review before they stand. A reviewer biased to disprove, not approve, examines the artifact before commit."
use: "When correctness matters more than speed, when working in unfamiliar code, when stakes are high (production, security-sensitive logic, irreversible operations), or when a confident output would be cheaper to verify now than to debug later"
role: "Adversarial Reviewer"
date:
  created: "2026-08-11"
  knowledge-basis: "2026-08-11"
  last-used: "2026-08-11"
tags:
  - "ai/workflow/software-dev/general/doubt-driven-review"
  - "adversarial-review"
  - "fresh-context"
  - "pre-commit"
  - "verification"
see-also:
  - skill: "code-review-guidance"
    relationship: "complement"
    description: "Post-commit balanced code review (strengths + weaknesses). Doubt-driven review is pre-commit and adversarial (issues only). They cover different phases of the same story"
  - skill: "execute-upsert"
    relationship: "complement"
    description: "Orchestrates this workflow as a pre-commit gate in Phase 6 (Execute) for non-trivial stories"
  - knowledge: "software-architecture-essentials"
    relationship: "complement"
    description: "Root-cause-first and fail-fast principles that doubt-driven review enforces at the decision level"
---

# Doubt-Driven Review Workflow

## Goal

A confident answer is not a correct one. Long sessions accumulate context that
quietly turns assumptions into "facts" without anyone noticing. This workflow
materializes a fresh-context reviewer — biased to **disprove**, not approve —
before any non-trivial output stands.

This is not a post-commit code review. Post-commit review is a verdict on a
finished artifact. This is an in-flight posture: non-trivial decisions get
cross-examined while course-correction is still cheap.

## When to Apply

A decision is **non-trivial** when at least one of these is true:

- It introduces or modifies branching logic
- It crosses a module or service boundary
- It asserts a property the type system or compiler cannot verify (thread
  safety, idempotence, ordering, invariants)
- Its correctness depends on context the future reader cannot see
- Its blast radius is irreversible (production deploy, data migration, public
  API change)

**Do NOT apply** to mechanical operations (renaming, formatting, file moves),
clear unambiguous instructions, reading or summarizing existing code, one-line
changes with obvious correctness, or when the user has explicitly asked for
speed over verification.

## Process

Copy this checklist when applying the workflow:

```
Doubt cycle:
- [ ] Step 1: CLAIM — wrote the claim + why-it-matters
- [ ] Step 2: EXTRACT — isolated artifact + contract, stripped reasoning
- [ ] Step 3: DOUBT — invoked fresh-context reviewer with adversarial prompt
- [ ] Step 4: RECONCILE — classified every finding against the artifact text
- [ ] Step 5: STOP — met stop condition (trivial findings, 3 cycles, or user override)
```

### Step 1: CLAIM — Surface what stands

Name the decision in two or three lines:

```
CLAIM: "The new caching layer is thread-safe under the
        read-heavy workload described in the spec."
WHY THIS MATTERS: a race here corrupts user data and is
                  hard to detect in QA.
```

If the claim cannot be written that compactly, it is a vibe, not a decision.
Surface it before scrutinizing it.

### Step 2: EXTRACT — Smallest reviewable unit

A fresh-context reviewer needs the **artifact** and the **contract**, not the
journey.

- **Code**: the diff or the function — not the whole file
- **Decision**: the proposal in 3-5 sentences plus the constraints it must
  satisfy
- **Assertion**: the claim plus the evidence that supposedly supports it (kept
  distinct from the Step 1 CLAIM block, which is the orchestrator's hypothesis
  under scrutiny)

Strip the reasoning. If conclusions are handed over, the reviewer returns
validation of those conclusions. The unit must be small enough that a reviewer
can hold it in mind in one read — if it is a 500-line PR, decompose first.

### Step 3: DOUBT — Invoke the fresh-context reviewer

The reviewer's prompt **must be adversarial**. Framing decides the answer.

```
Adversarial review. Find what is wrong with this artifact.
Assume the author is overconfident. Look for:
- Unstated assumptions
- Edge cases not handled
- Hidden coupling or shared state
- Ways the contract could be violated
- Existing conventions this might break
- Failure modes under unexpected input

Do NOT validate. Do NOT summarize. Find issues, or state
explicitly that you cannot find any after thorough examination.

ARTIFACT: <paste artifact>
CONTRACT: <paste contract>
```

**Pass ARTIFACT + CONTRACT only. Do NOT pass the CLAIM.** Handing the reviewer
the conclusion biases it toward agreement. The reviewer must independently
determine whether the artifact satisfies the contract.

When dispatching a subagent for this review, use the `subagent_explore` profile
(read-only) so the reviewer cannot modify the artifact while reviewing it. The
reviewer returns findings only — it does not fix issues.

### Step 4: RECONCILE — Fold findings back

The reviewer's output is data, not a verdict. The orchestrator is still the
decision-maker. Classify every finding against the artifact text:

| Finding Class | Action |
|---------------|--------|
| **Valid — must fix** | Fix the artifact before commit. Re-run the doubt cycle on the fix. |
| **Valid — defer** | Note the issue in the story file. Create a follow-up story if warranted. Proceed. |
| **Invalid — false positive** | Document why the finding does not apply. Proceed. |
| **Ambiguous** | Surface to the user with the question, the options, the recommendation, and why. Do not guess. |

### Step 5: STOP — When to stop doubting

Stop when one of these conditions is met:

- **Trivial findings only** — all findings are nits or false positives. The
  artifact stands. Proceed to commit.
- **3 cycles reached** — after three doubt cycles on the same artifact, further
  review has diminishing returns. Surface remaining findings to the user and
  let them decide.
- **User override** — the user explicitly says "stop reviewing, proceed." Honor
  the override and note it in the story file.

## Integration with execute-upsert

In execute-upsert's Phase 6 (Execute), this workflow runs between the dev
subagent's completion and the final commit — after the code review subagent
returns `CLEAN` but before the grm final commit. It applies only to stories
flagged as non-trivial:

- The story crosses a module or service boundary
- The story introduces branching logic or state management
- The story asserts a property the type system cannot verify
- The story's blast radius is irreversible (migration, public API change)

For trivial stories (single-file, obvious correctness, no branching), skip
this workflow and proceed directly from code review to commit.

## Common Rationalizations

| Rationalization | Reality |
|---|---|
| "I already ran code review" | Code review is post-commit and balanced. Doubt-driven is pre-commit and adversarial. They catch different things. |
| "This is simple, no need" | Simple decisions pass the doubt cycle in seconds. The cost of running it is trivial; the cost of skipping it on a wrong "simple" decision is not. |
| "The reviewer will just agree" | Only if the artifact is correct. If the reviewer finds nothing after thorough examination, that is evidence the artifact is sound. |
| "I do not have time for another review" | A 2-minute pre-commit review prevents a 2-hour post-deploy debugging session. The time is already spent; the question is when. |
| "I am confident this is right" | Confidence is not correctness. Long sessions accumulate context that turns assumptions into facts. Fresh context catches what accumulated context cannot. |

## Red Flags

- Skipping the doubt cycle because "it looks right"
- Passing the CLAIM to the reviewer (biases it toward agreement)
- Passing the whole file instead of the smallest reviewable unit
- Accepting the reviewer's output as a verdict instead of classifying findings
- Running more than 3 cycles on the same artifact (diminishing returns)
- Not documenting why a finding was classified as a false positive

