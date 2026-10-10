// Robust clipboard copy. Prefer the async Clipboard API; fall back to a hidden
// <textarea> + execCommand('copy') when it's unavailable or rejects — which
// happens in real WebKit / WebView2 setups (insecure context, no document
// focus, or permission denied). Returns true on apparent success.
//
// No app dependencies (i18n/theme/Tauri), so it is safe to call even from the
// crash-floor ErrorBoundary.
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* async API rejected — fall through to the execCommand path */
  }
  try {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.top = '-9999px';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    const ok = document.execCommand('copy');
    ta.remove();
    return ok;
  } catch {
    return false;
  }
}
