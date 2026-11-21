// SPDX-License-Identifier: MIT OR Apache-2.0
// The entire host-side UI implementation for the examples.
//
// Lisp emits a tree of {tag, attrs, children} and {text} records plus a CSS
// string. This file mounts the tree, patches it in place, injects the CSS, and
// sends named actions back to the server. It has no knowledge of any example.

const root = document.getElementById("root");
const source = document.getElementById("source");
const status = document.getElementById("status");
const apply = document.getElementById("apply");
const examples = document.getElementById("examples");
const filename = document.getElementById("filename");
const query = new URLSearchParams(location.search);
const embedMatch = location.pathname.match(/^\/embed\/([^/]+)$/);
const embedded = embedMatch ? decodeURIComponent(embedMatch[1]) : null;
const bare = query.has("bare") || embedded !== null;

let mounted = null;
let currentState = null;
let currentName = embedded;
let styleElement = null;
let hotAvailable = false;
let hotConnected = false;

function setStatus(text, isError = false) {
  status.textContent = text;
  status.classList.toggle("error", isError);
}

function mount(vnode) {
  if (vnode == null) return null;
  if (typeof vnode === "string") return document.createTextNode(vnode);
  if (Object.prototype.hasOwnProperty.call(vnode, "text")) {
    return document.createTextNode(vnode.text ?? "");
  }
  const element = document.createElement(vnode.tag || "div");
  element.__vnode = vnode;
  applyAttrs(element, vnode.attrs || {});
  for (const child of vnode.children || []) {
    const node = mount(child);
    if (node) element.appendChild(node);
  }
  return element;
}

function patch(dom, vnode) {
  if (!dom) return mount(vnode);
  if (vnode == null) {
    dom.remove();
    return null;
  }
  if (typeof vnode === "string" || Object.prototype.hasOwnProperty.call(vnode, "text")) {
    const text = typeof vnode === "string" ? vnode : (vnode.text ?? "");
    if (dom.nodeType === Node.TEXT_NODE) {
      if (dom.data !== text) dom.data = text;
      return dom;
    }
    const next = document.createTextNode(text);
    dom.replaceWith(next);
    return next;
  }
  if (dom.nodeType !== Node.ELEMENT_NODE || !dom.__vnode || dom.__vnode.tag !== vnode.tag) {
    const next = mount(vnode);
    dom.replaceWith(next);
    return next;
  }
  applyAttrs(dom, vnode.attrs || {}, dom.__vnode.attrs || {});
  reconcileChildren(dom, vnode.children || []);
  dom.__vnode = vnode;
  return dom;
}

function applyAttrs(element, next, previous = {}) {
  for (const name of Object.keys(previous)) {
    if (name === "src" && element.__live) continue;
    if (!(name in next)) element.removeAttribute(name);
  }
  for (const [name, value] of Object.entries(next)) {
    if (name === "src" && element.__live) continue;
    if (value === false || value == null) {
      element.removeAttribute(name);
      continue;
    }
    const text = String(value);
    if (element.getAttribute(name) !== text) element.setAttribute(name, text);
  }
}

function reconcileChildren(parent, children) {
  const previous = Array.from(parent.childNodes);
  const byKey = new Map();
  previous.forEach((node, index) => {
    const key = node.nodeType === Node.ELEMENT_NODE && node.getAttribute("data-key");
    byKey.set(key || `i:${index}`, node);
  });

  const keep = new Set();
  children.forEach((vnode, index) => {
    const key = vnode && vnode.attrs && vnode.attrs["data-key"];
    const existing = byKey.get(key || `i:${index}`);
    const next = existing ? patch(existing, vnode) : mount(vnode);
    if (!next) return;
    keep.add(next);
    const target = parent.childNodes[index] || null;
    if (target !== next) parent.insertBefore(next, target);
  });

  for (const node of previous) {
    if (!keep.has(node) && node.parentNode === parent) node.remove();
  }
}

function renderTree(tree) {
  if (!mounted) {
    mounted = mount(tree);
    root.replaceChildren(mounted);
    return;
  }
  mounted = patch(mounted, tree);
  if (!mounted) root.replaceChildren();
  else if (mounted.parentNode !== root) root.replaceChildren(mounted);
}

function applyCss(css) {
  if (!styleElement) {
    styleElement = document.createElement("style");
    styleElement.id = "soil-css";
    document.head.appendChild(styleElement);
  }
  if (styleElement.textContent !== css) styleElement.textContent = css;
}

const SOIL_TOKENS = /(;[^\n]*)|("(?:\\.|[^"\\])*")|(:[^\s()";]+)|(#t|#f|\bnil\b)|(-?\b\d+(?:\.\d+)?\b)|(\b(?:defun|defmacro|defspore|let|if|cond|and|or|not|quote|lambda|fn|do|begin|match)\b)|([()])|([^\s()";]+)/g;

function escapeHtml(text) {
  return text.replace(/[&<>]/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[char]);
}

function highlightSoil(code) {
  let out = "";
  let last = 0;
  let match;
  while ((match = SOIL_TOKENS.exec(code))) {
    if (match.index > last) out += escapeHtml(code.slice(last, match.index));
    const token = match[0];
    let className = "tok-symbol";
    if (match[1]) className = "tok-comment";
    else if (match[2]) className = "tok-string";
    else if (match[3]) className = "tok-keyword";
    else if (match[4]) className = "tok-literal";
    else if (match[5]) className = "tok-number";
    else if (match[6]) className = "tok-special";
    else if (match[7]) className = "tok-paren";
    else if (code.slice(0, match.index).trimEnd().endsWith("(")) className = "tok-fn";
    out += `<span class="${className}">${escapeHtml(token)}</span>`;
    last = match.index + token.length;
  }
  return out + escapeHtml(code.slice(last));
}

function highlightCode(scope) {
  for (const block of scope.querySelectorAll("pre.code")) {
    const code = block.textContent;
    if (code) block.innerHTML = highlightSoil(code);
  }
}

function sourceUrl() {
  if (!currentName) return "/source";
  return `/source?name=${encodeURIComponent(currentName)}`;
}

function viewUrl(action) {
  const params = new URLSearchParams();
  if (currentName) params.set("name", currentName);
  if (currentState !== null) params.set("state", JSON.stringify(currentState));
  if (action) params.set("action", action);
  return `/view?${params.toString()}`;
}

async function probeHot() {
  try {
    const response = await fetch("/__hot", { cache: "no-store" });
    hotAvailable = response.ok;
  } catch {
    hotAvailable = false;
  }
  return hotAvailable;
}

function componentCss(css, chrome = false) {
  let out =
    css.replace(/:root/g, ":host") +
    "\n:host { display: block; height: 100%; }\n" +
    ".component-body { height: 100%; }\n" +
    ".component-body .app { min-height: 100% !important; }\n";
  if (!chrome) {
    out +=
      ".component-body .app { background: transparent !important; padding: 0 !important; }\n" +
      ".component-body .topbar { display: none !important; }\n" +
      ".component-body .stage { margin: 0 !important; max-width: none !important; }\n" +
      ".component-body .card { border: 0 !important; border-radius: 0 !important; box-shadow: none !important; background: transparent !important; padding: 20px !important; max-width: none !important; }\n";
  }
  return out;
}

async function renderComponent(component, action) {
  const params = new URLSearchParams({ name: component.name, action });
  if (component.state !== null) params.set("state", JSON.stringify(component.state));
  const response = await fetch(`/view?${params.toString()}`, { cache: "no-store" });
  const data = await response.json();
  if (!response.ok) {
    component.host.title = data.error || "render failed";
    return;
  }
  component.state = data.state;
  component.style.textContent = componentCss(data.css, component.chrome);
  component.tree = patch(component.tree, data.tree);
  if (!component.tree) component.body.replaceChildren();
  else if (component.tree.parentNode !== component.body) {
    component.body.replaceChildren(component.tree);
  }
  highlightCode(component.body);
}

async function mountComponent(figure, slug, options = undefined) {
  const response = await fetch(`/view?name=${encodeURIComponent(slug)}`, {
    cache: "no-store",
  });
  const data = await response.json();
  if (!response.ok) return;

  const host = document.createElement("div");
  host.className = "component-live";
  const shadow = host.attachShadow({ mode: "open" });
  const chrome = options?.chrome === true;
  const style = document.createElement("style");
  style.textContent = componentCss(data.css, chrome);
  const body = document.createElement("div");
  body.className = "component-body";
  const tree = mount(data.tree);
  if (tree) body.appendChild(tree);
  shadow.append(style, body);

  const component = { name: slug, state: data.state, style, body, tree, host, chrome };

  shadow.addEventListener("click", (event) => {
    const target =
      event.target instanceof Element ? event.target.closest("[data-op]") : null;
    if (!target) return;
    if (target.tagName === "FORM") return;
    if (target.tagName === "BUTTON" && target.type === "submit" && target.form) return;
    event.preventDefault();
    const action = actionFromElement(target);
    if (action) {
      renderComponent(component, action).catch((error) => {
        component.host.title = String(error);
      });
    }
  });

  shadow.addEventListener("submit", (event) => {
    const form = event.target instanceof HTMLFormElement ? event.target : null;
    if (!form || !form.dataset.op) return;
    event.preventDefault();
    const payload = {
      op: form.dataset.op,
      ...Object.fromEntries(new FormData(form)),
    };
    renderComponent(component, JSON.stringify(payload))
      .then(() => {
        if (form.isConnected) form.reset();
      })
      .catch((error) => {
        component.host.title = String(error);
      });
  });

  figure.querySelector("iframe")?.remove();
  figure.appendChild(host);
  highlightCode(body);
}

async function mountComponents() {
  if (!hotAvailable) return;
  for (const figure of document.querySelectorAll("figure[data-soil-component]")) {
    if (figure.querySelector(":scope > .component-live")) continue;
    await mountComponent(figure, figure.dataset.soilComponent);
  }
}
async function mountCarousel() {
  if (!hotAvailable) return;
  const frame = document.querySelector("[data-soil-carousel]");
  if (!frame) return;
  const dots = [...document.querySelectorAll("[data-carousel-slug]")];
  if (!dots.length) return;

  const show = async (index) => {
    const dot = dots[(index + dots.length) % dots.length];
    if (!dot) return;
    frame.querySelector(":scope > .component-live")?.remove();
    await mountComponent(frame, dot.dataset.carouselSlug, { chrome: true });
    const caption = frame.querySelector("figcaption");
    if (caption) caption.textContent = dot.dataset.carouselTitle ?? dot.dataset.carouselSlug;
    dots.forEach((item, at) =>
      item.classList.toggle("active", item === dot),
    );
    frame.__carouselIndex = dots.indexOf(dot);
  };

  if (!frame.__carousel) {
    frame.__carousel = true;
    const active = dots.findIndex((dot) => dot.classList.contains("active"));
    frame.__carouselIndex = active < 0 ? 0 : active;
    document.querySelector("[data-carousel=prev]")?.addEventListener("click", () => {
      show(frame.__carouselIndex - 1).catch((error) => {
        frame.title = String(error);
      });
    });
    document.querySelector("[data-carousel=next]")?.addEventListener("click", () => {
      show(frame.__carouselIndex + 1).catch((error) => {
        frame.title = String(error);
      });
    });
    dots.forEach((dot, index) => {
      dot.addEventListener("click", () => {
        show(index).catch((error) => {
          frame.title = String(error);
        });
      });
    });
    await show(frame.__carouselIndex);
  }
}

async function connectHotReload() {
  if (!hotAvailable && !(await probeHot())) return;
  if (hotConnected) return;
  hotConnected = true;
  const events = new EventSource("/__reload");
  events.addEventListener("source", () => {
    loadSource()
      .then(render)
      .catch((error) => setStatus(String(error), true));
  });
  events.addEventListener("reload", () => location.reload());
}

function actionFromElement(element) {
  if (!element.dataset || !element.dataset.op) return null;
  const payload = { op: element.dataset.op };
  for (const [key, value] of Object.entries(element.dataset)) {
    if (key !== "op") payload[key] = value;
  }
  return JSON.stringify(payload);
}

async function render(action = "") {
  if (!currentName && !bare) return;
  const response = await fetch(viewUrl(action), { cache: "no-store" });
  const data = await response.json();
  if (!response.ok) {
    setStatus(data.error || "render failed", true);
    return;
  }
  currentState = data.state;
  applyCss(data.css);
  renderTree(data.tree);
  highlightCode(root);
  mountComponents()
    .then(mountCarousel)
    .catch((error) => setStatus(String(error), true));
  setStatus(`state ${JSON.stringify(currentState)}`);
}

async function loadSource() {
  const response = await fetch(sourceUrl(), { cache: "no-store" });
  if (!response.ok) {
    setStatus("could not load source", true);
    return;
  }
  source.value = await response.text();
}

async function applySource() {
  setStatus("applying…");
  const response = await fetch(sourceUrl(), {
    method: "POST",
    body: source.value,
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
  if (!response.ok) {
    const data = await response.json().catch(() => null);
    setStatus((data && data.error) || "apply failed", true);
    return;
  }
  await render();
}

async function loadExamples() {
  const response = await fetch("/examples", { cache: "no-store" });
  if (!response.ok) {
    setStatus("could not load examples", true);
    return;
  }
  const list = await response.json();
  examples.replaceChildren();
  for (const example of list) {
    const option = document.createElement("option");
    option.value = example.name;
    option.textContent = example.title;
    examples.appendChild(option);
  }
}

async function selectExample(name) {
  currentName = name;
  currentState = null;
  mounted = null;
  root.replaceChildren();
  filename.textContent = `${name}.soil`;
  await loadSource();
  await render();
}

function runAction(action) {
  render(action).catch((error) => setStatus(String(error), true));
}

root.addEventListener("click", (event) => {
  const target = event.target instanceof Element ? event.target.closest("[data-op]") : null;
  if (!target) return;
  if (target.tagName === "FORM") return;
  if (target.tagName === "BUTTON" && target.type === "submit" && target.form) return;
  event.preventDefault();
  const action = actionFromElement(target);
  if (action) runAction(action);
});

root.addEventListener("submit", (event) => {
  const form = event.target instanceof HTMLFormElement ? event.target : null;
  if (!form || !form.dataset.op) return;
  event.preventDefault();
  const payload = {
    op: form.dataset.op,
    ...Object.fromEntries(new FormData(form)),
  };
  render(JSON.stringify(payload))
    .then(() => {
      if (form.isConnected) form.reset();
    })
    .catch((error) => setStatus(String(error), true));
});

examples.addEventListener("change", () => {
  selectExample(examples.value).catch((error) => setStatus(String(error), true));
});

apply.addEventListener("click", () => {
  applySource().catch((error) => setStatus(String(error), true));
});

source.addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
    event.preventDefault();
    applySource().catch((error) => setStatus(String(error), true));
  }
});

(async () => {
  try {
    await probeHot();
    if (embedded) {
      document.body.classList.add("bare");
      filename.textContent = `${embedded}.soil`;
      window.addEventListener("message", (event) => {
        if (event.data?.type === "soil-source") {
          loadSource()
            .then(render)
            .catch((error) => setStatus(String(error), true));
        } else if (event.data?.type === "soil-reload") {
          location.reload();
        }
      });
      await loadSource();
      await render();
      return;
    }
    if (bare) {
      document.body.classList.add("bare");
      await render();
      await connectHotReload();
      return;
    }
    await loadExamples();
    const initial = examples.value || "counter";
    await selectExample(initial);
    await connectHotReload();
  } catch (error) {
    setStatus(String(error), true);
  }
})();
