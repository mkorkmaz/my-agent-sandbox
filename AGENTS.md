---
name: karpathy-guidelines
description: Behavioral guidelines to reduce common LLM coding mistakes. Use when writing, reviewing, or refactoring code to avoid overcomplication, make surgical changes, surface assumptions, and define verifiable success criteria.
license: MIT
---

# Karpathy Guidelines

Behavioral guidelines to reduce common LLM coding mistakes, derived from [Andrej Karpathy's observations](https://x.com/karpathy/status/2015883857489522876) on LLM coding pitfalls.

Use task scope and observable risk to select the required guidance and checks.

## Required reading at the start of every task

Read the [Tugboat agent guide](docs/agent-guide.html) in full before planning, editing, or running project commands.
Apply it together with this file. Use its module and test references to identify the boundaries and checks that apply to the task.

## 0. Be understandable

**When explaining complex content, make good use of visualization.**


## 1. Think Before Coding

Resolve routine, reversible implementation details from current source and project conventions. State assumptions that affect the result.

Ask only when an unresolved decision materially changes public behavior, compatibility, required platform choices, shared data, or external effects. Continue independent work while that decision is pending.

An implementation request authorizes necessary local edits and known isolated checks within its scope. Reuse explicit authority for the same action, target, and effects. Ask again only when authority is missing or those details materially change. Verify service-backed targets before execution. Local code work alone does not authorize shared-data changes, deployment, or remote synchronization.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

## 5. Object Calisthenics

1. One level of indentation per method — extract a private method when a loop or conditional would add another level.
2. Do not use `else` — handle guard and error cases with early returns.
3. Wrap primitives that carry business rules in dedicated value objects instead of passing raw strings, integers, or floats.
4. Use first-class collections — a collection class has the contained items as its only instance property and exposes collection behavior through methods.
5. Use at most one object access operator (`->` or `?->`) per statement to avoid traversing object graphs (Law of Demeter).
6. Do not abbreviate class, method, property, or variable names; use complete, intention-revealing names.
7. Keep classes small — no more than 150 lines per class file and no more than 10 class files per namespace directory.
8. Use no more than two instance properties per class.
9. Do not expose public properties or trivial getters and setters for callers to make business decisions; put that behavior on the object (Tell, don't ask).


## 6. Coding Style

- Unless otherwise specified, use `camelCase` for all names.
- Use `PascalCase` for class and namespace names.
- Use `snake_case` for database, schema, table, column, and index names.
- Use `kebab-case` for URL path parameter names, configuration values, and similar names. This rule does not apply to query parameters or request-body fields.
- Use `camelCase` for request query parameters and request-body fields.
- Use `camelCase` for response object attributes.

## 7. Defensive Programming

Defend at trust boundaries. Do not scatter redundant checks throughout trusted internal code.

### Validate External Input

- Treat HTTP input, command-line arguments, configuration, persisted data, file contents, and external-service responses as untrusted.
- Validate required fields, types, formats, ranges, sizes, and allowed values at the boundary before invoking domain logic.
- Reject invalid input explicitly; do not silently coerce, truncate, or substitute a default unless the contract requires it.
- Convert validated input into typed value objects or command objects so it cannot become invalid deeper in the application.

### Preserve Invariants

- Use precise parameter, property, and return types; avoid `mixed` and nullable types when the domain does not require them.
- Enforce domain invariants in constructors or named constructors so invalid objects cannot be created.
- Validate an operation completely before mutating the state.
- Use database transactions when multiple writes must succeed or fail as one unit.

### Fail Explicitly

- Never suppress errors with `@`, leave a `catch` block empty, or hide failure behind an unrelated fallback value.
- Catch only exceptions that can be recovered from, translated at an architectural boundary, or enriched with useful context.
- Throw specific exceptions and preserve the original exception as the previous exception when translating it.
- Use assertions for programmer invariants only, never as validation for user or external input.

### Isolate External Failures

- Set explicit timeouts for network and process calls.
- Retry only transient failures, use a bounded retry count with backoff, and retry state-changing operations only when they are idempotent.
- Validate external responses before using them; do not assume a successful status means the payload is complete or valid.
- Keep partial failure from leaving an inconsistent state; roll back or make the operation safely resumable.

### Protect Diagnostic Data

- Log enough context to diagnose a failure without logging credentials, secrets, access tokens, or unnecessary personal data.
- Return stable, safe error responses to clients; keep stack traces and internal implementation details out of public responses.

### Verify Failure Paths

- Test invalid, missing, boundary, and oversized inputs.
- Test dependency timeouts, malformed responses, transaction rollbacks, and retry exhaustion where those behaviors exist.
- Add a regression test for every fixed defect that can be reproduced automatically.

## 8. Make implicit state machines explicit

If a workflow's current state and permitted operations are determined by
interdependent boolean flags, null checks, or conditions scattered across
the code, treat it as an implicit state machine.

- Before modifying the workflow, identify its existing states, the events
  that trigger transitions, transition conditions, and the invariants
  that must hold. Design new behavior against this model.
- Represent mutually exclusive states with an enum, discriminated union,
  or an equivalent construct supported by the language. Do not store
  additional flags that can be derived from the current state.
- Route state changes through a shared transition function, reducer,
  or existing state management mechanism that enforces allowed
  transitions. Explicitly define how invalid transitions are handled.
- Where applicable, account for errors, cancellation, retries, and late
  asynchronous results, including their effects on state. Test the
  critical transitions affected by the change and the handling of
  invalid transitions.
- Keep refactoring scoped to the workflow being changed. Independent
  boolean fields do not, by themselves, indicate an implicit state
  machine. Do not introduce unnecessary abstractions or a new state
  machine dependency for simple workflows.

## 9. Language

Use ASD-STE100 Simplified Technical English for all English agent responses.

- Use English unless the user explicitly requests a different language.
- Use approved, common words. Use each word with one meaning.
- Use the same term for the same item or action. Do not use synonyms only to vary the text.
- Use active voice. Use the imperative form for instructions.
- Give only one instruction in each sentence.
- Limit procedural sentences to 20 words and descriptive sentences to 25 words when practical.
- Keep paragraphs short. Use lists for sequences, alternatives, and groups of related facts.
- Avoid idioms, slang, jargon, contractions, and ambiguous pronouns.
- Define an abbreviation or an unfamiliar technical term at its first use.
- Preserve exact code identifiers, commands, paths, protocol terms, and quoted text when accuracy requires them.
