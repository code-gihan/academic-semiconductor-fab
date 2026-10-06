// Strategy code: JavaScript functions a run calls (priority, admit and startBatch, the core's
// strategy code hooks), written in a code editor (Monaco, www/vendor/monaco, loaded when first
// opened) that completes and checks them by their types (strategy.d.ts). The code runs only in
// the simulation workers, never on the page; code that came with a shared link runs once its
// reader allows it.
import { t } from "./i18n.js";
import { infoButton } from "./tooltip.js";

export const HOOKS = ["priority", "admit", "startBatch"];

/** Example code by name: the papers' rules written as code. */
export const EXAMPLES = {
  leastSlack: `// Least slack first: where lots rank by code, lots in CQT segments go first, the one with the
// least queue-time slack before the others; lots outside segments come after.
// "CQT dispatching: Code" ranks by it wherever the queue-time rule would.

/**
 * @param {Lot} lot
 * @param {number} now
 * @returns {number} smaller goes first
 */
function priority(lot, now) {
  // now + slack is the latest time the lot can wait until: the same at every dispatch.
  return lot.cqt ? now + lot.cqt.slack : Infinity;
}
`,
  stopping: `// Stopping ([P2] §3.2) at the steppers: a lot does not start a CQT segment while a stepper of
// the segment has 50 or more lots of CQT segments in front of it, or 85 or more in front and on
// their way (LithoTrack_FE_115: 55 and 125).

const LIMITS = { LithoTrack_FE_95: [50, 85], LithoTrack_FE_115: [55, 125] };

/**
 * @param {Lot} lot
 * @param {Segment} segment
 * @param {number} now
 * @returns {boolean | number} true starts it now, false holds it
 */
function admit(lot, segment, now) {
  return segment.groups.every((group) => {
    const limit = LIMITS[group.toolGroup];
    return !limit || (group.front < limit[0] && group.total < limit[1]);
  });
}
`,
  batches: `// Batches below their minimum size start once one of their lots has two hours of queue-time
// slack left; otherwise they wait for more lots.

/**
 * @param {Batch} batch
 * @param {number} now
 * @returns {boolean | number} true starts it now, false waits, a time asks again then
 */
function startBatch(batch, now) {
  if (batch.slack === null) return false;
  if (batch.slack <= 2 * HOUR) return true;
  // Asked again when the slack reaches two hours, unless a dispatch asks earlier.
  return Math.ceil(now + batch.slack - 2 * HOUR);
}
`,
};

/** Lines of `source`. */
export function lines(source) {
  return source.split("\n").length;
}

/**
 * The strategy code card for `code` ({source, enabled, trusted}): its source in the editor (or,
 * until the editor opens, as text), whether runs use it, examples, the hooks it defines, and the
 * allowance code from a shared link needs. `changed()` follows edits; `useRule()` sets the
 * queue-time rule to the code's priority, for the examples that rank.
 */
export function codeCard(code, { changed, ranksByCode, useRule }) {
  const use = el("input", { type: "checkbox" });
  use.checked = code.enabled;
  use.addEventListener("change", () => {
    code.enabled = use.checked;
    changed();
  });
  const examples = el("select", { "aria-label": t("code.examples") });
  examples.append(new Option(t("code.examples"), ""));
  for (const name of Object.keys(EXAMPLES)) examples.append(new Option(t(`code.example.${name}`), name));
  const hooks = el("p", { class: "hint code-hooks" });
  const host = el("div", { class: "code-host" });
  const allow = el("div", { class: "notice", role: "status" });
  const showHooks = (names) => {
    const text = names.length > 0 ? t("code.hooks", { hooks: names.join(" · ") }) : t("code.noHooks");
    const unused = names.includes("priority") && !ranksByCode();
    hooks.textContent = unused ? `${text} ${t("code.unranked")}` : text;
  };
  const drawAllow = () => {
    allow.hidden = code.trusted;
    const button = el("button", { type: "button" }, t("code.allow"));
    button.addEventListener("click", () => {
      code.trusted = true;
      drawAllow();
      changed();
    });
    allow.replaceChildren(el("p", {}, t("code.fromLink")), button);
  };
  drawAllow();
  examples.addEventListener("change", () => {
    const name = examples.value;
    examples.value = "";
    if (!name) return;
    setSource(EXAMPLES[name]);
    code.enabled = true;
    use.checked = true;
    if (name === "leastSlack" && !ranksByCode()) useRule();
    changed();
    open();
  });

  /** The source `text`, undoable in the editor (whose change takes it). */
  function setSource(text) {
    // Code the reader picks is theirs.
    code.trusted = true;
    drawAllow();
    if (editor && bound?.code === code) {
      const model = editor.getModel();
      editor.executeEdits("example", [{ range: model.getFullModelRange(), text }]);
    } else {
      code.source = text;
      preview();
    }
  }

  /** The source as text with the button that opens the editor. */
  function preview() {
    const button = el("button", { type: "button" }, t(code.source ? "code.edit" : "code.write"));
    button.addEventListener("click", open);
    host.replaceChildren(
      ...(code.source ? [el("pre", { class: "code-preview" }, el("code", {}, code.source))] : []),
      el("div", { class: "row" }, button, el("span", { class: "hint" }, t("code.size"))),
    );
    hooks.textContent = code.source ? t("code.lines", { lines: lines(code.source) }) : "";
  }

  /** Opens the editor in the card, loading it the first time. */
  async function open() {
    host.replaceChildren(el("p", { class: "hint" }, t("code.loading")));
    try {
      await makeEditor();
    } catch (error) {
      host.replaceChildren(el("p", { class: "hint" }, t("code.failed", { message: error.message })));
      return;
    }
    bind(host, { code, showHooks, changed });
  }

  if (editor) {
    bind(host, { code, showHooks, changed });
  } else {
    preview();
  }
  const heading = el("h3", {}, t("code.heading"));
  heading.append(infoButton(() => t("code.hint")));
  return el(
    "section",
    { class: "card code-card" },
    heading,
    el("p", { class: "hint" }, t("code.short")),
    allow,
    el("div", { class: "row" }, el("label", { class: "check" }, use, t("code.use")), examples),
    host,
    hooks,
  );
}

// ---- the editor: one per page, moved into the card each time it is drawn ----

let monaco = null;
let loading = null;
let editor = null;
let editorBox = null;
/** The card the editor serves: {code, showHooks(names), changed()}. */
let bound = null;

/** Loads the editor library once and makes the editor. */
async function makeEditor() {
  loading ??= loadMonaco();
  monaco = await loading.catch((error) => {
    loading = null;
    throw error;
  });
  if (editor) return;
  editorBox = el("div", { class: "code-editor" });
  const model = monaco.editor.createModel("", "javascript", monaco.Uri.parse("file:///strategy.js"));
  editor = monaco.editor.create(editorBox, {
    model,
    theme: "strategy",
    fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--mono"),
    fontSize: 13,
    lineHeight: 20,
    minimap: { enabled: false },
    scrollBeyondLastLine: false,
    automaticLayout: true,
    tabSize: 2,
    wordWrap: "on",
    fixedOverflowWidgets: true,
  });
  model.onDidChangeContent(() => {
    if (bound.code.source === model.getValue()) return;
    bound.code.source = model.getValue();
    bound.changed();
    hooksOf(model);
  });
}

/** Puts the editor into `host` for the card `binding`. */
function bind(host, binding) {
  bound = binding;
  host.replaceChildren(editorBox);
  const model = editor.getModel();
  if (model.getValue() !== binding.code.source) model.setValue(binding.code.source);
  hooksOf(model);
}

/** The hooks the source defines, from the language service's outline of it. */
async function hooksOf(model) {
  const version = model.getVersionId();
  const worker = await (await monaco.typescript.getJavaScriptWorker())(model.uri);
  const tree = await worker.getNavigationTree(model.uri.toString());
  if (model.getVersionId() !== version) return;
  const names = new Set((tree?.childItems ?? []).map((item) => item.text));
  bound.showHooks(HOOKS.filter((name) => names.has(name)));
}

/** The editor library (AMD, from www/vendor/monaco), set up for strategy code. */
function loadMonaco() {
  const base = new URL("vendor/monaco/vs", document.baseURI).href;
  return new Promise((done, fail) => {
    const script = document.createElement("script");
    script.src = `${base}/loader.js`;
    script.onerror = () => fail(new Error(t("code.loadError")));
    script.onload = () => {
      globalThis.require.config({ paths: { vs: base } });
      globalThis.require(["vs/editor/editor.main"], () => done(globalThis.monaco), fail);
    };
    document.head.append(script);
  }).then(async (library) => {
    const defaults = library.typescript.javascriptDefaults;
    defaults.setCompilerOptions({
      target: library.typescript.ScriptTarget.ES2020,
      allowJs: true,
      checkJs: true,
      noEmit: true,
      // Workers have no DOM: the language only.
      lib: ["es2020"],
    });
    const types = await fetch(new URL("strategy.d.ts", document.baseURI)).then((response) => {
      if (!response.ok) throw new Error(t("code.loadError"));
      return response.text();
    });
    defaults.addExtraLib(types, "file:///strategy.d.ts");
    theme(library);
    matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => theme(library));
    return library;
  });
}

/** The editor's colours from the page's tokens, as the #rrggbb it reads. */
function theme(library) {
  const style = getComputedStyle(document.documentElement);
  const canvas = document.createElement("canvas").getContext("2d");
  const token = (name) => {
    canvas.fillStyle = style.getPropertyValue(`--${name}`).trim();
    return canvas.fillStyle;
  };
  const dark = matchMedia("(prefers-color-scheme: dark)").matches;
  library.editor.defineTheme("strategy", {
    base: dark ? "vs-dark" : "vs",
    inherit: true,
    rules: [],
    colors: {
      "editor.background": token("card"),
      "editor.lineHighlightBackground": token("surface"),
      "editorLineNumber.foreground": token("muted"),
      "editorGutter.background": token("card"),
    },
  });
  library.editor.setTheme("strategy");
}

/** An element with attributes and children (strings become text). */
function el(tag, attributes = {}, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, value);
  node.append(...children);
  return node;
}
