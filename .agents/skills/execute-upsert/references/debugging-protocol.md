---
workflow: "Debugging Protocol"
slug: "debugging-protocol"
description: "Systematic root-cause debugging for test failures, build errors, and runtime bugs. Stop-the-line rule: when something breaks, stop adding features, preserve evidence, and follow a structured process to find and fix the root cause."
use: "When tests fail after a code change, when the build breaks, when runtime behavior does not match expectations, when a bug report arrives, or when something worked before and stopped working"
role: "Debugging Specialist"
date:
  created: "2026-08-11"
  knowledge-basis: "2026-08-11"
  last-used: "2026-08-11"
tags:
  - "ai/workflow/software-dev/general/debugging-protocol"
  - "debugging"
  - "root-cause"
  - "stop-the-line"
  - "test-failure"
see-also:
  - knowledge: "software-architecture-essentials"
    relationship: "complement"
    description: "Root-cause-first practice page — diagnose before fixing, workarounds are last resort. This workflow operationalizes that practice for test failures and build errors"
  - skill: "execute-upsert"
    relationship: "complement"
    description: "Orchestrates this workflow in Phase 6 (Execute) when a dev subagent returns with test failures, before re-dispatching with feedback"
  - skill: "general/think-assist"
    relationship: "complement"
    description: "Five Whys root cause analysis technique, available as a reference in the think-assist skill"
---

# Debugging Protocol Workflow

## Goal

When something breaks, stop adding features, preserve evidence, and follow a
structured process to find and fix the root cause. Guessing wastes time. Errors
compound — a bug in step 3 that goes unfixed makes steps 4-6 wrong.

This workflow is the operationalization of the root-cause-first practice: treat
every failure as a symptom, diagnose the underlying cause before applying any
fix, and guard against recurrence.

## The Stop-the-Line Rule

When anything unexpected happens:

```
1. STOP adding features or making changes
2. PRESERVE evidence (error output, logs, repro steps)
3. DIAGNOSE using the triage checklist below
4. FIX the root cause
5. GUARD against recurrence
6. RESUME only after verification passes
```

Do not push past a failing test or broken build to work on the next feature.

## Process

### Step 1: Reproduce

Make the failure happen reliably. If it cannot be reproduced, it cannot be
fixed with confidence.

```
Can the failure be reproduced?
├── YES → Proceed to Step 2
└── NO
    ├── Gather more context (logs, environment details)
    ├── Try reproducing in a minimal environment
    └── If truly non-reproducible, document conditions and monitor
```

When a bug is non-reproducible:

- **Timing-dependent?** Add timestamps to logs around the suspected area. Try
  with artificial delays to widen race windows. Run under load or concurrency.
- **Environment-dependent?** Compare Node/browser versions, OS, environment
  variables. Check for differences in data (empty vs populated database). Try
  reproducing in CI where the environment is clean.
- **State-dependent?** Check for leaked state between tests or requests. Look
  for global variables, singletons, or shared caches. Run the failing scenario
  in isolation vs after other operations.
- **Truly random?** Add defensive logging at the suspected location. Set up an
  alert for the specific error signature. Document the conditions and revisit
  when it recurs.

Run the specific failing test in isolation first:

```bash
# Use the project's own test runner (discover it via project-detection)
# Example: pnpm test -- --grep "test name"
# Example: pytest tests/test_foo.py::test_bar
# Example: go test -run TestBar ./...
```

### Step 2: Localize

Narrow down where the failure happens:

```
Which layer is failing?
├── UI/Frontend     → Check console, DOM, network tab
├── API/Backend     → Check server logs, request/response
├── Database        → Check queries, schema, data integrity
├── Build tooling   → Check config, dependencies, environment
├── External service → Check connectivity, API changes, rate limits
└── Test itself     → Check if the test is correct (false negative)
```

Use bisection for regression bugs:

```bash
git bisect start
git bisect bad                    # Current commit is broken
git bisect good <known-good-sha> # This commit worked
# Git will checkout midpoint commits; run the test at each
git bisect run <the project's focused-test command>
```

### Step 3: Reduce

Create the minimal failing case:

- Remove unrelated code/config until only the bug remains
- Simplify the input to the smallest example that triggers the failure
- Strip the test to the bare minimum that reproduces the issue

A minimal reproduction makes the root cause obvious and prevents fixing
symptoms instead of causes.

### Step 4: Fix the Root Cause

Fix the underlying issue, not the symptom. Apply the root-cause-first
practice: treat every failure as a symptom, not as the thing to fix.

```
Symptom: "The user list shows duplicate entries"

Symptom fix (bad):
  → Deduplicate in the UI component: [...new Set(users)]

Root cause fix (good):
  → The API endpoint has a JOIN that produces duplicates
  → Fix the query, add a DISTINCT, or fix the data model
```

Ask "Why does this happen?" until the actual cause is reached, not just where
it manifests. The Five Whys technique (see the think-assist skill's
`references/five-whys.md`) is useful here — each answer becomes the basis for
the next "why" until a fundamental cause is found.

A workaround is acceptable only when all of these are true:
- The root cause is outside the project's control (third-party bug, upstream
  regression)
- The workaround is documented with a link to the upstream issue
- The workaround is tracked for removal when the root cause is fixed

### Step 5: Guard Against Recurrence

Write a test that catches this specific failure. The test should fail without
the fix and pass with it.

```typescript
// The bug: task titles with special characters broke the search
it('finds tasks with special characters in title', async () => {
  await createTask({ title: 'Fix "quotes" & <brackets>' });
  const results = await searchTasks('quotes');
  expect(results).toHaveLength(1);
  expect(results[0].title).toBe('Fix "quotes" & <brackets>');
});
```

This test prevents the same bug from recurring.

### Step 6: Verify End-to-End

After fixing, verify the complete scenario with the project's own commands:

```bash
# Run the specific test that was failing
<the project's focused-test command for this test>

# Run the full test suite (check for regressions)
<the project's full-test command>

# Build the project (check for type/compilation errors)
<the project's build command>
```

Do not declare the bug fixed until the specific test passes AND the full suite
passes with no regressions.

### Step 7: Resume

Only after verification passes, resume the work that was interrupted. If this
was a story in execute-upsert's Phase 6, the dev subagent returns success with
the fix commit hash and the new regression test.

## Error-Specific Patterns

### Test Failure Triage

```
Test fails after code change:
├── Did the code the test covers change?
│   └── YES → Check if the test or the code is wrong
│       ├── Test is outdated → Update the test
│       └── Code has a bug → Fix the code
├── Did unrelated code change?
│   └── YES → Likely a side effect → Check shared state, imports, globals
└── Test was already flaky?
    └── Check for timing issues, order dependence, external dependencies
```

### Build Failure Triage

```
Build fails:
├── Type error → Read the error, check the types at the cited location
├── Import error → Check the module exists, exports match, paths are correct
├── Config error → Check build config files for syntax/schema issues
├── Dependency error → Check package manifest, run install
└── Environment error → Check Node/tool version, OS compatibility
```

### Runtime Error Triage

```
Runtime error:
├── TypeError: Cannot read property 'x' of undefined
│   └── Something is null/undefined that should not be
│       → Check data flow: where does this value come from?
├── Network error / CORS
│   └── Check URLs, headers, server CORS config
├── Render error / White screen
│   └── Check error boundary, console, component tree
└── Unexpected behavior (no error)
    └── Add logging at key points, verify data at each step
```

## Integration with execute-upsert

In execute-upsert's Phase 6 (Execute), this workflow runs when a dev subagent
returns with test failures. Instead of re-dispatching with unstructured
"failure output as feedback," the orchestrator instructs the dev subagent to
follow this protocol:

1. The dev subagent receives the failure output plus a reference to this
   workflow (inlined in `references/debugging-protocol.md`).
2. The dev subagent follows Steps 1-6 (Reproduce → Localize → Reduce → Fix →
   Guard → Verify).
3. The dev subagent returns: the root cause, the fix commit hash, and the new
   regression test that guards against recurrence.
4. If the dev subagent cannot reproduce the bug (Step 1 fails), it returns
   `BLOCKED` with the conditions observed and the monitoring setup
   recommendation.

## Common Rationalizations

| Rationalization | Reality |
|---|---|
| "I know what the bug is, I will just fix it" | Right maybe 70% of the time. The other 30% costs hours. Reproduce first. |
| "The failing test is probably wrong" | Verify that assumption. If the test is wrong, fix the test. Do not just skip it. |
| "It works on my machine" | Environments differ. Check CI, check config, check dependencies. |
| "I will fix it in the next commit" | Fix it now. The next commit will introduce new bugs on top of this one. |
| "This is a flaky test, ignore it" | Flaky tests mask real bugs. Fix the flakiness or understand why it is intermittent. |
| "The workaround is faster" | Workarounds compound. The next person inherits the band-aid plus the original bug plus the hidden coupling the band-aid introduced. |

## Red Flags

- Starting to fix before reproducing the failure
- Fixing the symptom instead of the root cause
- Not writing a regression test after the fix
- Declaring the bug fixed without running the full test suite
- Pushing past a failing test to work on the next feature
- Adding a workaround without documenting it and tracking its removal

