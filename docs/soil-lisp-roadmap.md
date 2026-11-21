# Soil Substrate Roadmap — Lisp Runtime, Spores, tend

**Status:** draft for execution · **License:** MIT OR Apache-2.0 · **Scope:** the general substrate. CAD, CAM, PCB, and WDF work are explicitly out of scope.

---

## Decisions this roadmap executes

| Decision | Consequence |
|---|---|
| Soil is a homoiconic Lisp substrate | S-expressions are the source, IR, doc format, and agent-edit format. One reader, not several. |
| Tiny Rust core, heavy capabilities in WIT | Numerics, GPU, audio, UI host, filesystem, inference live behind WIT. The core stays auditable. |
| Libraries and applications are the same artifact: **spores** | One package type, one resolver, one lockfile. "Library" vs "app" is a launch convention, not a package class. |
| Build tool is **tend** | `tend build/test/wasm/doc/add/run`. Existing `tend-core` (Canvas Lua composition) becomes one subcommand, not the tool. |
| River is merged and **deadnamed** | Code and tests move into `soil-lang`; the name does not appear in crates, WIT, docs, or commands. One `HISTORY.md` line records the merge. |
| HTML/CSS is the first UI path | Lisp emits UI forms + stylesheets + signal updates. A thin JS WIT adapter reconciles to real DOM. Canvas/GPU remains a separate WIT implementation. |
| Constrained decoding is first-class | The reader grammar and spore schemas are the decoding targets. Models emit forms, not prose-to-code guesses. |
| Self-documenting | Every spore can walk itself, preserve comments, and render an interactive docs UI from the same forms it runs. |

---

## Baseline (what exists today)

- `crates/soil-lang`: reader (`sexp.rs`), closed interpreter with `host-call` (`eval.rs`), McCarthy core, WIT binding generator, Wasm backend, `stdlib/prelude.soil`, tests including JMC-on-wasmtime. **The reader currently discards comments; values carry no source spans.** This is the first thing Phase 1 changes.
- `crates/tend-core`: a host-free composer for FungOS Canvas Lua modules. Useful, but narrower than `tend`.
- `wit/`: a large, already-wired capability surface (`soil:records`, `soil:inference`, `soil:canvas`, `soil:vfs`, `soil:spore`, etc.) with a status table and review discipline.
- UI today: Canvas Lua modules, `soil-scene` documents, CPU frames, GPU leases. No Lisp→DOM path.
- `../river`: a separate repo with an Elixir-like surface, S-expression IR, parser/eval/runtime/codegen experiments. Not yet merged.
- Tooling reality: `cargo` workspace, `unsafe_code = "forbid"`, edition 2024.

The roadmap therefore **consolidates and completes** rather than starting over.

---

## Phase summary

| Phase | Name | Duration (3 eng) | Gate |
|---|---|---|---|
| 0 | Consolidation and deadnaming | 2 wk | One repo, one language, no `river` identifiers |
| 1 | Core language | 8 wk | Comment-preserving round-trip + macros + GC + conditions |
| 2 | WIT boundary and host parity | 5 wk | Same spore runs native and `wasm32-wasip2` |
| 3 | tend and spore packaging | 5 wk | `tend` builds, tests, runs, and resolves spores |
| 4 | Signals + HTML/CSS UI | 8 wk | Editable browser UI driven only by Lisp forms |
| 5 | Constrained decoding | 5 wk | Prose → valid spore/panel, validated and repaired |
| 6 | Self-documentation | 6 wk | Soil documents itself from its own source |
| 7 | Self-hosting and WASM deployment | 8 wk | Reader/eval bootstrapped in Soil; docs spore on the web |
| 8 | Hardening and 1.0 | ongoing | Spec, fuzzing, performance budgets, stable ABI |

Phases 1–3 are mostly sequential. Phase 4 can start once Phase 1 has macros and Phase 2 has a stable value codec. Phase 5 depends on Phase 1 (grammar) and Phase 3 (schemas). Phase 6 depends on Phase 1 (spans/comments) and Phase 4 (docs UI). Phase 7 depends on 1, 3, and 6.

---

## Phase 0 — Consolidation and deadnaming (2 weeks)

**Goal:** end the split-brain. There is one language implementation and one name.

**Deliverables**

- Import the useful parts of the river tree into `crates/soil-lang` (history-preserving merge where practical): IR fixtures, tests, any evaluator semantics worth keeping.
- Delete or archive duplicate parser/evaluator paths. `soil-lang` is the only implementation.
- Establish the naming policy: `river` is forbidden in new crate names, WIT packages, commands, docs, and identifiers. Record the old name once in `HISTORY.md`.
- Archive the standalone repo with a README pointing at Soil and a one-line explanation.
- Add `docs/soil-language.md`: the normative surface, special forms, value model, and comment/span rules.
- Move river's Elixir-like surface decision into an explicit non-goal for this roadmap unless re-adopted later. The S-expression IR is the source of truth.
- CI: `cargo test -p soil-lang`, WIT validation, license headers, `rg -i river` guard (excluding `HISTORY.md`).

**Done when**

- A clean checkout has one Lisp implementation and no uncommitted merge branches.
- `rg -i '\briver\b'` matches only `HISTORY.md`.
- The language document is reviewed and merged.
- The existing JMC and wasm tests still pass.

**Depends on:** nothing.

---

## Phase 1 — Core language (8 weeks)

**Goal:** a small, predictable Lisp that can carry real spores, with the properties the rest of the roadmap assumes.

**Deliverables**

1. **Reader/printer with lossless syntax**
   - Add explicit `Comment` and `Span` nodes (or side tables keyed by node id). Comments attach to the following form and survive read → print.
   - Preserve source text ranges for every node; printer can emit canonical or original form.
   - Property test: `read(print(ast)) == ast` and comment/byte-position preservation over a corpus.
2. **Value model**
   - nil, bool, i64, f64, string, symbol, keyword, list, vector, map, closure, host handle, error/condition.
   - Canonical text encoding and canonical binary encoding; both versioned.
3. **Evaluator**
   - `eval`/`apply`, lexical scope, proper tail calls, `let`, `fn`, `def`, `defmacro`, quasiquote/unquote, `do`, `if`, `cond`, `match` (small, fixed patterns).
   - Macros expand in Soil; macro expansion is inspectable (`macroexpand`).
4. **Condition system (minimal)**
   - `raise`, `handler-case`-style handlers, restarts sufficient for host errors and decode failures. No full CL condition hierarchy in v1.
5. **GC**
   - Arena/slot-index representation with mark-sweep, no `unsafe`. Explicit roots for interpreter stack, globals, host handles. Deterministic collection points.
   - Budgets and metrics: allocation count, live slots, collection pause.
6. **Prelude in Soil**
   - Lists, maps, strings, JSON, basic formatting, test helpers. Nothing in the prelude may require a capability.
7. **Error reporting**
   - Span-carrying diagnostics with source excerpt; machine-readable diagnostic form for agents and editors.

**Done when**

- JMC `eval.` runs in the interpreter and on wasm.
- Reader round-trips a corpus containing comments, strings, keywords, and nested forms byte-for-byte in original mode.
- A 10k-form stress script runs under a fixed memory budget without unbounded growth.
- Macro expansion and condition handling have conformance tests.
- The language spec in `docs/soil-language.md` matches the implementation.

**Depends on:** Phase 0.

**Explicitly Rust:** reader/printer primitives, GC, evaluator core, encoding. **Explicitly Soil:** prelude, macros, test helpers. The reader is rewritten in Soil only in Phase 7.

---

## Phase 2 — WIT boundary and host parity (5 weeks)

**Goal:** one contract, two hosts. A spore must not know whether it is native or in a browser tab.

**Deliverables**

- `soil:sexp@0.1.0`: canonical value/text encoding across the boundary, with source spans optional. One implementation in Rust, one in JS, one conformance suite.
- A single host-call ABI: capability name, function name, encoded args, encoded result, typed error. Grants are checked before dispatch.
- `soil-lang` host trait implemented by:
  - native host adapters (existing capability crates),
  - `wasm32-wasip2` guest bindings generated from WIT,
  - a browser host backed by JS imports.
- Generated bindings via the existing `witgen`, with a checked-in golden output and a CI diff gate.
- Capability manifest emitted by `tend` and enforced by hosts: a spore can only call what it declares.
- A conformance spore that exercises: no capability, records, time, inference stub, denied grant, and error mapping.

**Done when**

- The conformance spore runs unmodified on native and in a browser/WASI host.
- Denied capabilities produce identical structured errors on both.
- WIT package review status updated in `wit/README.md`.
- Encoding has cross-language golden vectors.

**Depends on:** Phase 1 (stable values), existing WIT review process.

---

## Phase 3 — tend and spore packaging (5 weeks)

**Goal:** one tool to cultivate spores, and one package shape for libraries and apps.

**Deliverables**

- `tend` CLI, built on `tend-core`:
  - `tend new <spore>` — scaffold with manifest, source, test, docs.
  - `tend build` — resolve, compile/lower, produce `module.wasm` when needed.
  - `tend test` — run Soil tests plus capability conformance.
  - `tend wasm` — produce the deployable component.
  - `tend run` — launch a spore on the selected surface (native, web, canvas).
  - `tend add <spore>` — resolve, verify, lock.
  - `tend doc` — generate the spore's docs from forms.
- `spore.toml` v2:
  - `id`, `version`, `license`, `engine`, `module` (optional), `entry`, `launch`, `grants`, `docs`, `assets`.
  - **One entry convention:** a `(defspore …)` form exporting `:start` and/or named functions. A spore with exports is usable as a library; a spore with `:start` is launchable. Both may be true.
- Content-addressed lockfile and reproducible builds. Reuse the existing tar-over-IPFS distribution decision; tend does not invent a registry.
- Resolver: local path, git, and content-addressed sources. No private registry in v1.
- Migrate `tend-core`'s Lua composition into `tend compose` (kept for Canvas spores during the transition).
- Signing hooks that reuse existing `soil:creds`; no new key server.

**Done when**

- Two example spores exist: one pure library, one app that depends on it.
- `tend build && tend test && tend run` works from a clean checkout with no network after resolve.
- A build is byte-identical across two runs and two machines.
- A spore with an undeclared grant fails at launch with a clear error.

**Depends on:** Phases 1–2.

---

## Phase 4 — Signals and the HTML/CSS UI path (8 weeks)

**Goal:** editable, themeable UI in the browser and native, driven entirely by Lisp-emitted forms.

**Deliverables**

1. **Signals**
   - `(signal v)`, `(derive …)`, `(effect …)`, `(batch …)`, `(on …)`.
   - Dependency graph with glitch avoidance, batching, and deterministic flush order.
   - Effects are scheduled by the host; Lisp never calls the DOM.
2. **UI form schema**
   - Canonical forms: `(ui/element :tag div :attrs ((:class "panel")) :children (…))`, text nodes, fragments, keyed lists, slots.
   - Style forms compile to CSS text; theming via custom properties. No inline style soup unless explicitly requested.
   - Events are forms inside `on` handlers, returning signal updates or commands.
3. **WIT contract** `soil:ui@0.1.0`
   - `mount(form, styles)`, `patch(ops)`, `unmount`, input events, focus, measurement.
   - The contract is *rendered output and input*, not a DOM API.
4. **Browser WIT implementation (JS)**
   - Tree walker + keyed reconciler (~300–500 lines), style injection, event delegation, `textContent`/attribute/class updates, subtree replacement for structural changes.
   - No virtual-DOM dependency.
5. **Native implementation**
   - A DOM-like adapter over an existing retained view path (or a small HTML/CSS renderer). Start with the browser; native parity is required before the phase gate.
6. **Demo spores**
   - An inspector (tree + properties), a docs reader, and a small structural editor.
7. **Performance guardrails**
   - Bench: 10k-node tree mount, 60 Hz signal updates over 1k bound nodes, keyed list reorder.

**Done when**

- The inspector and editor run in a browser with real CSS, devtools, keyboard navigation, and no DOM access from Lisp.
- The same spores run natively through the native implementation of `soil:ui`.
- Signal updates mutate only the affected DOM nodes (measured, not asserted by inspection).
- Custom GPU viewports remain on `soil:scene`/`soil:canvas`; the UI path has no GPU dependency.

**Depends on:** Phase 1 (macros), Phase 2 (boundary + JS host), Phase 3 (spore packaging for demos).

---

## Phase 5 — Constrained decoding (5 weeks)

**Goal:** a model can generate valid Soil forms, reliably, from a schema and a prompt.

**Deliverables**

- Grammar generation:
  - reader grammar exported as a machine-readable grammar (GBNF or JSON-schema-equivalent) with a version.
  - spore forms, UI forms, and style forms each get a schema generated from the same definitions used by the reader/printer.
- A `tend schema` command that emits grammars for a spore, a UI panel, or a rewrite rule.
- Integration with `soil:inference`:
  - constrained completion request: prompt + grammar id + budget → forms.
  - validation → repair loop: parse, type/arity check, capability check, then either accept or return diagnostics to the model.
- Agent-facing edit path: generate a *patch* of forms plus preserved comments, not a whole-file rewrite. Reuse Phase 1 spans.
- Corpus + eval harness: prose → form → validate → render. Track valid-first-try rate and repair rounds.

**Done when**

- From a prose brief, a constrained model produces a UI panel that parses, validates, and renders in the browser in one shot for a fixed benchmark set.
- A model can add a form to an existing file without dropping comments or unrelated code.
- Schemas are versioned and covered by golden tests.
- Constrained decoding is optional: hand-written forms and macros take the same path.

**Depends on:** Phase 1 (grammar, spans), Phase 3 (schemas), and an inference host (existing WIT surface).

---

## Phase 6 — Self-documentation (6 weeks)

**Goal:** Soil explains itself from its own source, interactively, without a separate docs generator.

**Deliverables**

- Doc extraction from forms: `def`, `defmacro`, `defspore`, and arbitrary forms with attached docstrings/attributes. Comments preserved and rendered.
- A doc model: names, signatures, examples, cross-links, source location, capability grants, stability.
- A docs spore that:
  - walks a spore (including itself),
  - builds an index and link graph,
  - renders via the Phase 4 UI path,
  - runs examples and shows results,
  - supports search and "open in editor" locations.
- CI check: every exported form in the workspace has documentation; every example parses and evaluates.
- Output parity: the same doc model renders native and in the browser.

**Done when**

- The Soil language reference and all built-in spores are generated from source, not hand-maintained.
- The docs spore documents itself with the same mechanism.
- A missing docstring or broken example fails CI.
- Docs work offline (no hosted service).

**Depends on:** Phases 1, 3, 4.

---

## Phase 7 — Self-hosting and WASM deployment (8 weeks)

**Goal:** Soil is written in Soil where it matters, and ships as a browser-loadable component.

**Deliverables**

- **Bootstrap:** implement reader, printer, and evaluator in Soil, running on the Rust core. Conformance suite must pass on both implementations, including spans, comments, macros, and conditions.
- **Stage split:** Rust keeps GC, encoding, host dispatch, and the bootstrap evaluator; the Soil implementation becomes the default for reader/printer and macro expansion.
- **Browser deployment**
  - `tend wasm` produces a component plus a static loader (JS bridge, worker, asset manifest).
  - Spores can be fetched by content id, verified, and mounted in a page.
  - Capability grants are explicit in the page host; no ambient network/filesystem.
- **Native deployment parity**
  - The same artifact runs under the native host with the same grants and errors.
- **Hot reload**
  - Re-evaluate UI forms and styles without restarting the host; preserve state where keys allow.

**Done when**

- A clean machine can load a Soil docs spore from a static URL and use it, with no Rust toolchain.
- Reader/printer/evaluator conformance passes in both implementations.
- The same spore artifact runs natively and in the browser.
- A UI edit round-trip (model or human) is visible without a full rebuild.

**Depends on:** Phases 1, 2, 3, 6.

---

## Phase 8 — Hardening and 1.0 (ongoing)

- Freeze and version `soil:sexp`, `soil:ui`, and the spore manifest. Deprecation policy.
- Fuzz reader/printer/decoder; property tests for round-trip and reconciler correctness.
- Performance budgets: startup, mount, update, GC pause, Wasm size.
- Security review of the capability model and grant enforcement on every host.
- Language spec 1.0 with a conformance suite third parties can run.
- Published docs site generated by the docs spore.

---

## Rust vs Soil

| Stays in Rust | Moves to / lives in Soil |
|---|---|
| Reader/printer primitives, spans, GC, evaluator core, canonical encoding | Prelude, macros, most library code |
| WIT host adapters, grants, native capability implementations | Spore logic, UI forms, styles, signal graphs |
| Wasm lowering, component plumbing, JS bridge | Build descriptions, test suites, docs content |
| `tend` resolver, compiler driver, cache, reproducibility | Spore entry points, launch conventions, examples |
| Inference host, GPU, audio, filesystem, networking | Constrained-decoding schemas and repair loops |
| DOM reconciler and CSS compilation (host side) | Every UI tree and handler that produces them |

Principle: **Rust is the machine; Soil is the program.** If a feature is not on the hot path or not a capability, it should be written in Soil.

---

## Merge and deadnaming plan (Phase 0 detail)

1. Create `docs/HISTORY.md` entry: "The earlier Lisp experiments were developed under the name *river*. They were merged into `soil-lang` in <date>; the old name is retired."
2. Move code and tests; delete divergent implementations rather than keeping both.
3. Rename any surviving identifiers, fixtures, and env vars.
4. Archive the old repository; README points here.
5. CI guard prevents the name from returning outside `HISTORY.md`.
6. Do **not** preserve the old surface syntax unless a test proves it carries semantics the S-expression path lacks. The IR is the contract.

---

## Release gates

| Gate | Required |
|---|---|
| **0.1 "Core"** | Phase 1 done; `soil-lang` usable as a library; no UI promise |
| **0.2 "Hosts"** | Phase 2 done; conformance spore on native + WASI |
| **0.3 "Spores"** | Phase 3 done; `tend` usable end-to-end; two example spores |
| **0.4 "Surfaces"** | Phase 4 done; browser + native HTML/CSS UI |
| **0.5 "Generated"** | Phase 5 done; prose → valid forms with repair |
| **0.6 "Self-documenting"** | Phase 6 done; docs generated from source |
| **0.7 "Self-hosting"** | Phase 7 done; browser deployment, Soil reader/eval |
| **1.0** | Phase 8 gates; stable ABI, spec, security review |

---

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Two Lisp implementations survive the merge | Delete one in Phase 0; conformance suite is the arbiter |
| Comment/span preservation is bolted on late | Phase 1 gate; all downstream docs and agent edits depend on it |
| UI path drifts into a DOM API | `soil:ui` contract is forms + events only; JS adapter is replaceable |
| GC pauses or leaks under UI workloads | Budgets in Phase 1; benches in Phase 4; deterministic collection points |
| Constrained decoding produces plausible but invalid forms | Validation/repair loop is mandatory; benchmark first-try and repair rates |
| WASM/native divergence | Same conformance spore on both hosts; Phase 2 gate |
| tend becomes a package manager | No registry in v1; path/git/content-addressed only |
| River scope returns through the merge | Explicit non-goal; S-expression IR only; document rejected surface |

---

## Immediate next two weeks (start here)

1. Land the existing `crates/soil-lang` work on its branch (it is currently untracked/modified in this worktree).
2. Write `docs/soil-language.md` and `docs/HISTORY.md`.
3. Merge river fixtures and tests; delete duplicate evaluator paths.
4. Change `sexp.rs` to attach comments and spans; add the round-trip property test.
5. Add the `rg -i river` CI guard.
6. Open tracking issues for Phases 1–3 as marbles, one per gate.

That sequence makes Phase 0 exit criteria true and gives Phase 1 a clean base.
