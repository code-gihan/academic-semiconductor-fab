// Views of the page, one shown at a time. The location hash names it (#setup, #run, #analysis,
// #python), so links, bookmarks and the back button switch views; any other hash is an anchor
// within the shown view. Reacts to hashchange only.
const VIEWS = ["setup", "run", "analysis", "python"];
let shown = null;
let onShow = () => {};

/** Shows the view of the hash, and calls `callback(view)` whenever another view is shown. */
export function initViews(callback) {
  onShow = callback;
  addEventListener("hashchange", show);
  show();
}

/** Shows `view`, through the hash so that history and links agree. */
export function go(view) {
  if (location.hash.slice(1) === view) {
    show();
  } else {
    location.hash = view;
  }
}

function show() {
  const hash = location.hash.slice(1);
  if (!VIEWS.includes(hash) && shown) return;
  const view = VIEWS.includes(hash) ? hash : "setup";
  for (const section of document.querySelectorAll("main > .view")) {
    section.hidden = section.dataset.view !== view;
  }
  for (const link of document.querySelectorAll(".tabs a")) {
    if (link.dataset.view === view) {
      link.setAttribute("aria-current", "page");
    } else {
      link.removeAttribute("aria-current");
    }
  }
  if (view !== shown) {
    shown = view;
    scrollTo(0, 0);
    onShow(view);
  }
}
