// Files the page gives the user.

/** Saves `text` as the file `name` of media type `type`. */
export function download(name, text, type) {
  const url = URL.createObjectURL(new Blob([text], { type }));
  Object.assign(document.createElement("a"), { href: url, download: name }).click();
  // Some browsers read the file after click() returns: release it later, not at once.
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}
