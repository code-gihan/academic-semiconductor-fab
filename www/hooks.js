// Strategy code in the simulation workers: JavaScript source whose top level runs once with
// MINUTE, HOUR and DAY given and defines any of the hooks the core asks (priority, admit,
// startBatch). An error of a hook names its line in the source.
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
/** The functions strategy code may define. */
const HOOKS = ["priority", "admit", "startBatch"];

/** The hooks `source` defines. */
export function compile(source) {
  const hooks = `return {${HOOKS.map((name) => `${name}: typeof ${name} === "function" ? ${name} : undefined`).join(", ")}};`;
  let defined;
  try {
    // The source starts on the body's first line: its lines are the body's, after the wrapper's 2.
    defined = new Function("MINUTE", "HOUR", "DAY", `"use strict";${source}\n;${hooks}\n//# sourceURL=strategy.js`)(MINUTE, HOUR, DAY);
  } catch (error) {
    throw new Error(`strategy code: ${error.name}: ${error.message}`);
  }
  return Object.fromEntries(HOOKS.filter((name) => defined[name]).map((name) => [name, located(defined[name])]));
}

/** `hook`, whose errors say where in the source they arose. */
function located(hook) {
  return function (a, b, c) {
    try {
      return hook.call(this, a, b, c);
    } catch (error) {
      const text = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
      const line = /strategy\.js:(\d+):\d+/.exec(error?.stack ?? "")?.[1];
      throw line ? `${text} (line ${Number(line) - 2})` : text;
    }
  };
}
