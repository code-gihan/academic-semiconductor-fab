// One-time notes: a short note on what a new kind of picture shows, just above it, until its
// button dismisses it; the dismissal is kept in this browser (local storage, where it works).
import { t } from "./i18n.js";
import { reveal } from "./motion.js";

const PREFIX = "smt2020.coach.";

/** Shows note `key` (text `coach.<key>`) before `target`, unless dismissed before or shown. */
export function coach(target, key) {
  if (dismissed(key) || target.previousElementSibling?.classList.contains("coach")) return;
  const text = document.createElement("p");
  text.dataset.i18n = `coach.${key}`;
  text.textContent = t(`coach.${key}`);
  const button = document.createElement("button");
  button.type = "button";
  button.dataset.i18n = "coach.dismiss";
  button.textContent = t("coach.dismiss");
  const note = document.createElement("div");
  note.className = "coach";
  note.setAttribute("role", "note");
  note.append(text, button);
  button.addEventListener("click", () => {
    try {
      localStorage.setItem(PREFIX + key, "1");
    } catch {
      // Not kept: the note shows again next time.
    }
    note.remove();
  });
  target.before(note);
  reveal([note]);
}

function dismissed(key) {
  try {
    return localStorage.getItem(PREFIX + key) === "1";
  } catch {
    return false;
  }
}
