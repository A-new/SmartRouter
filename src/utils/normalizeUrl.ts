/**
 * Strip endpoint-path cruft users commonly paste off the end of a provider
 * base URL. Provider docs and copy buttons often hand over the full endpoint
 * (`/v1/chat/completions`, `/v1/messages`); we keep only the base and let the
 * client append the resource path.
 *
 * Extracted from the inline logic that lived in ModelNexus's URL-input
 * `onChange` handlers so the one-click paste affordance reuses the EXACT same
 * normalization — typing and paste must agree, or a pasted URL silently keeps
 * a doubled path and every request 404s while manually typed ones work.
 */

/// Normalize an OpenAI-style base URL: keep `/v1`, drop a trailing
/// `/chat/completions` (and the redundant full `/v1/chat/completions` form).
export function normalizeOpenaiUrl(v: string): string {
  return v.replace(/\/chat\/completions\/?$/i, '').replace(/\/v1\/chat\/completions\/?$/i, '/v1');
}

/// Normalize an Anthropic-style base URL: drop a trailing `/v1/messages` (or
/// bare `/messages`), leaving the bare host — the client appends `/v1/messages`.
export function normalizeAnthropicUrl(v: string): string {
  return v.replace(/\/v1\/messages\/?$/i, '').replace(/\/messages\/?$/i, '');
}
