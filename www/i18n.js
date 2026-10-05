// Page text in the chosen language. Static elements name their text with a data-i18n key and their
// attributes with data-i18n-attr ("attribute:key", several separated by ";"); scripts call t().
// English is the default; a chosen language is kept in localStorage. Every locale file has the
// keys of locales/en.js, and a key missing from one falls back to English.
import en from "./locales/en.js";
import ko from "./locales/ko.js";

/** Languages by code, each named in itself. */
export const LANGUAGES = { en: "English", ko: "한국어" };

const MESSAGES = { en, ko };
const STORAGE_KEY = "smt2020.language";
let current = "en";
let formats = new Map();

export function language() {
  return current;
}

/** Shows the page in the saved language, or in English. */
export function initLanguage() {
  let saved = null;
  try {
    saved = localStorage.getItem(STORAGE_KEY);
  } catch {
    // Storage unavailable (blocked or private mode): the default language.
  }
  apply(Object.hasOwn(MESSAGES, saved) ? saved : "en");
}

/** Shows the page in `code` and keeps the choice. */
export function setLanguage(code) {
  apply(code);
  try {
    localStorage.setItem(STORAGE_KEY, current);
  } catch {
    // Not kept; the page still switches.
  }
}

function apply(code) {
  current = Object.hasOwn(MESSAGES, code) ? code : "en";
  formats = new Map();
  document.documentElement.lang = current;
  document.title = t("meta.title");
  document.querySelector('meta[name="description"]').content = t("meta.description");
  for (const element of document.querySelectorAll("[data-i18n]")) {
    element.textContent = t(element.dataset.i18n);
  }
  for (const element of document.querySelectorAll("[data-i18n-attr]")) {
    for (const pair of element.dataset.i18nAttr.split(";")) {
      const [attribute, key] = pair.split(":");
      element.setAttribute(attribute, t(key));
    }
  }
}

/** Text of `key` with each {name} replaced by params[name]. */
export function t(key, params) {
  const text = MESSAGES[current][key] ?? MESSAGES.en[key] ?? key;
  return params ? text.replace(/\{(\w+)\}/g, (match, name) => params[name] ?? match) : text;
}

/** `value` with `decimals` fraction digits and the grouping of the language. */
export function formatNumber(value, decimals) {
  let format = formats.get(decimals);
  if (!format) {
    format = new Intl.NumberFormat(current, {
      minimumFractionDigits: decimals,
      maximumFractionDigits: decimals,
    });
    formats.set(decimals, format);
  }
  return format.format(value);
}

/** `seconds` as m:ss, or h:mm:ss from an hour on. */
export function formatDuration(seconds) {
  const total = Math.round(seconds);
  const pad = (value) => String(value).padStart(2, "0");
  const [hours, minutes] = [Math.floor(total / 3600), Math.floor(total / 60) % 60];
  return hours ? `${hours}:${pad(minutes)}:${pad(total % 60)}` : `${minutes}:${pad(total % 60)}`;
}
