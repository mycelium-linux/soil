# soil-ui-proto

A working vertical slice of the Lisp → HTML/CSS UI path described in
[`docs/soil-lisp-roadmap.md`](../../docs/soil-lisp-roadmap.md). It is a
**prototype host**, not the final `soil:ui` implementation.

The point is to answer one question with running code:

> If Lisp emits UI forms, stylesheets, and state transitions, what is left for
> Rust/WIT, and what is left for JavaScript?

## Run it

```sh
cargo run -p soil-ui-proto
```

Open <http://127.0.0.1:7878/>.

The left side is the Soil app. The right side is the `.soil` source, with a
dropdown for every example. Edit it and press **Apply** (or `Cmd/Ctrl + Enter`);
the server re-evaluates the file and the page patches the DOM in place.

Options:

```sh
cargo run -p soil-ui-proto -- --source path/to/view.soil
cargo run -p soil-ui-proto -- --examples path/to/examples
cargo run -p soil-ui-proto -- --render --name todo --action '{"op":"add","text":"x"}'
cargo run -p soil-ui-proto -- --port 9000
```

`--render` prints the same JSON the browser receives. It is the cheap way to
check a form without starting a server.

### Editing the host without a Rust rebuild

`web/index.html` and `web/ui.js` are embedded in the binary with `include_str!`.
For JS/HTML edits, run the dev proxy instead of rebuilding:

```sh
node crates/soil-ui-proto/web/dev-proxy.mjs
# http://127.0.0.1:7879/
```

It serves the two static files from disk and proxies `/view`, `/examples`, and
`/source` to the running Rust host. It also watches `.soil`, `ui.js`, and
`index.html`, and pushes a reload over SSE: the open page re-renders or reloads
instead of opening a new tab.

The site page mounts `examples/ui/*.soil` as live inline components in Shadow
DOM. Each component gets its own state, CSS, and event loop; the carousel and
the grid use the same mount path.

## Examples

All examples live in [`examples/ui/`](../../examples/ui) and are pure `.soil`:

| Example | What it exercises |
|---|---|
| `counter.soil` | state, actions, CSS |
| `todo.soil` | forms, keyed lists, action payloads, derived counts |
| `calculator.soil` | a numeric state machine, grid layout |
| `tictactoe.soil` | lists, indexing, immutable updates, win detection |
| `tabs.soil` | conditional rendering, active styling |
| `form.soil` | named inputs, validation, success state |

`examples/ui/prelude.soil` holds the shared action decoder and base theme. The
host loads `stdlib/prelude.soil` first, then that file.

## The contract

A Soil UI file exports four functions:

```lisp
(initial-state)          ; -> value
(update state action)    ; -> value
(view state)             ; -> UI form
(styles)                 ; -> CSS text
```

A UI form is a record tree:

```lisp
((:tag "div")
 (:attrs ((:class "app")))
 (:children (((:text "hello")))))
```

The host encodes that as JSON:

```json
{
  "tag": "div",
  "attrs": { "class": "app" },
  "children": [{ "text": "hello" }]
}
```

An action is either a plain string (`"inc"`) or a JSON object string:

```json
{"op":"add","text":"buy milk"}
{"op":"toggle","id":"3"}
```

The UI emits `data-op` plus any `data-*` fields. Forms submit their named inputs.
`ui.js` builds the JSON; `update` decodes it with `(json-decode action)`.
`examples/ui/prelude.soil` wraps that in `action-op` and `action-field`.

Rules the prototype already relies on:

- **Lisp never touches the DOM.** It returns values.
- **Attributes are keyword/value pairs.** They become HTML attributes.
- **Text is a `{text}` node.** It cannot be injected as markup.
- **CSS is one string per render.** The host owns the `<style>` element.
- **Actions are data.** `update` decides what they mean.

## What this experiment says belongs where

| Layer | Prototype | Final home |
|---|---|---|
| App markup, CSS, state transitions | `examples/ui/*.soil` | Soil spore |
| Shared UI helpers and theme | `examples/ui/prelude.soil` | Soil spore / prelude |
| Form constructors, HTML escaping, `render-html` | `stdlib/ui.soil` | Soil stdlib |
| Soil syntax highlighting | `stdlib/highlight.soil` | Soil stdlib |
| Reader, evaluator, value encoding | `soil-lang` | Rust core + eventual Soil bootstrap |
| Calling `view` / `update` / `styles` | `soil-ui-proto` | `tend run` + `soil:ui` host |
| Turning forms into DOM, patching, CSS injection | `web/ui.js` | JS implementation of `soil:ui` (WIT) |
| Event routing, focus, measurement | `web/ui.js` + `data-op` | `soil:ui` contract |
| Dev server, source reload, HTTP | `soil-ui-proto` | `tend` dev command only |

The stable boundary is **forms in, events out**. Everything else — DOM nodes,
CSSOM, the reconciler, style caching, event delegation — is host implementation.
That is why the same Lisp should run against a second `soil:ui` host (native or
a GPU scene) without changing any example.

### Shape of the WIT this implies

The prototype does not define WIT yet, but the call shape is already visible:

```wit
interface ui {
  mount: func(form: string, styles: string) -> surface-id;
  patch: func(surface: surface-id, form: string, styles: string) -> result<_, string>;
  unmount: func(surface: surface-id);
  event: func(surface: surface-id, name: string, payload: string);
}
```

The guest emits `form` and `styles`; the host owns `surface-id`, input, and
layout. `event` is how a click becomes `{"op":"inc"}`; the guest's `update`
turns that into a new state and a new form. A real contract should use the
canonical S-expression encoding (`soil:sexp`) rather than a JSON string, and
should carry a source span so errors point at the form that failed.

## What is deliberately missing

- No signal graph. State is one value and one `update` call per action.
- No grants, capabilities, or sandbox. The dev server can write the source file.
- No Wasm guest. The Lisp runs in the host process.
- No native `soil:ui` implementation. Browser only.
- No source spans or comment preservation in the emitted form.

Each of those is a reason to keep the contract narrow now: the host can grow
without touching the `.soil` files.

## Tests

```sh
cargo test -p soil-ui-proto
```

The tests pin the four-function contract, each example's initial render, and the
main action paths (todo add/toggle, calculator digits, tic-tac-toe play, form
validation).
