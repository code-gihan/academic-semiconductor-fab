// Scenarios in a link: JSON, deflated and in base64url after #share=, so that a link opens the
// page with the same dataset, settings and strategies. Nothing leaves the browser but the link.

const PREFIX = "#share=";

/** The page's link carrying `state`. */
export async function shareLink(state) {
  const bytes = await transform(new TextEncoder().encode(JSON.stringify(state)), new CompressionStream("deflate-raw"));
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  const code = btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  return `${location.origin}${location.pathname}${PREFIX}${code}`;
}

/** The state a link's hash carries, or null if it carries none. */
export async function sharedState(hash) {
  if (!hash.startsWith(PREFIX)) return null;
  const code = hash.slice(PREFIX.length).replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(code);
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  const text = new TextDecoder().decode(await transform(bytes, new DecompressionStream("deflate-raw")));
  return JSON.parse(text);
}

async function transform(bytes, stream) {
  const result = new Response(new Blob([bytes]).stream().pipeThrough(stream));
  return new Uint8Array(await result.arrayBuffer());
}
