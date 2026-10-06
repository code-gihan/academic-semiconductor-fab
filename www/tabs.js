// Tabs within a view: a tablist of buttons (role "tab", aria-controls naming their panel), one
// panel shown at a time; arrow keys, Home and End move between the tabs (the ARIA tabs pattern).
// Charts in a hidden panel draw when it shows, as their boxes get a width.
import { reveal } from "./motion.js";

/** Wires the tablist `list`; returns {select(name)}, a tab's name being its data-tab. */
export function tabList(list) {
  const tabs = [...list.querySelectorAll('[role="tab"]')];

  function select(name, focus = false) {
    for (const tab of tabs) {
      const chosen = tab.dataset.tab === name;
      const panel = document.getElementById(tab.getAttribute("aria-controls"));
      const shows = chosen && panel.hidden;
      tab.setAttribute("aria-selected", String(chosen));
      tab.tabIndex = chosen ? 0 : -1;
      panel.hidden = !chosen;
      if (shows) reveal(panel.children);
      if (chosen && focus) tab.focus();
    }
  }

  list.addEventListener("click", (event) => {
    const tab = event.target.closest('[role="tab"]');
    if (tab) select(tab.dataset.tab);
  });
  list.addEventListener("keydown", (event) => {
    const index = tabs.indexOf(document.activeElement);
    const next = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: tabs.length - 1 }[event.key];
    if (index < 0 || next === undefined) return;
    event.preventDefault();
    select(tabs[(next + tabs.length) % tabs.length].dataset.tab, true);
  });
  return { select };
}
