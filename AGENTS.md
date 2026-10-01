# Tea Repository Instructions

Scope: this independent Tea repository and all of its subdirectories.

Tea owns ticket/work-order intake, approval, evidence, persistence, sync, and
Loom run records. Keep Loom runtime, Gateway routing, and other product
implementations behind their owning project contracts.

Before development, read the README and the owning architecture, persistence,
sync, API, test, security, and release documents. When mounted inside Neuro,
`../docs/DEVELOPMENT_STANDARD.md` is the expanded common standard. This file is
self-contained so a standalone Tea checkout inherits the same mandatory rules.

## Mandatory incremental code-size standard

Effective code lines exclude blank lines, comment-only lines, and comment-only
multiline regions. A line containing code plus an inline comment still counts.
A repository language-aware checker, when present, is authoritative.

- Target about 150 effective lines; 100-250 is the preferred design range.
- 251-500 is acceptable only for one clear responsibility.
- 501-700 is a soft-limit exception. Record why another split would harm
  cohesion/readability/verification and identify protecting tests.
- A new file or new extraction result at 701-1500 is not complete; split it.
- More than 1500 is a hard violation with no waiver; split again.

The rule covers handwritten production code, tests, scripts, and styles.
Generated or immutable third-party files are excluded only by explicit policy.
Do not game the metric with comments, strings, minification, generated code,
renames, extensions, or dumping grounds named `common`, `utils`, or `helpers`.

## Legacy-debt ratchet

- Existing oversized files are grandfathered debt. Unrelated work does not have
  to split every historical file before it can finish.
- The grandfathering protects existing content, not future growth. New files,
  modules, responsibilities, types/functions, and material extensions are new
  code and must follow the limits.
- Put a new responsibility in a cohesive compliant module and leave only
  minimal wiring in an existing large file.
- A narrow correctness, security, compatibility, or wiring fix may be made in
  place without an unrelated rewrite. Avoid net growth where practical; report
  unavoidable growth and why immediate extraction would increase risk.
- A file that was at most 700 effective lines before the task must not cross
  700 because of the task.
- If the task explicitly refactors a large file, every new or fully migrated
  result must be at most 700 lines. Report unrelated debt honestly.

## Required development workflow

1. Measure affected files before design and check for stricter local policy.
2. Preserve behavior with focused tests; split by responsibility, dependency
   direction, state ownership, and resource lifetime rather than arbitrary size.
3. Add concise comments for module purpose and non-obvious invariants, trust
   boundaries, concurrency, cancellation, cleanup, and performance tradeoffs.
4. Review each new or materially changed file for input/auth/secret safety,
   transaction/sync correctness, bounded queues and payloads, connection/task/
   process cleanup, blocking work, retries, and algorithmic complexity.
5. Run focused tests, directly dependent compile/static checks, the official
   formatter, and any repository line checker.
6. Run `git diff --check` and inspect this independent repository's scoped diff.
7. For runtime or release work, run Tea's documented package/runtime gates;
   documentation-only governance changes do not require a fake release build.

Any stricter repository-local checker or CI gate overrides this common floor.
