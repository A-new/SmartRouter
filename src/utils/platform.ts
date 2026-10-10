// Synchronous macOS check — the frameless title bar must decide on the first
// paint whether window controls sit on the left (native traffic lights, macOS)
// or the right (Windows/Linux), and CSS must know whether to drop the custom
// rounded shell (macOS uses native decorations, so the OS owns the corners).
export const IS_MACOS: boolean = (() => {
  try {
    const ua = (navigator as unknown as { userAgentData?: { platform?: string } }).userAgentData;
    const platform = (ua?.platform || navigator.platform || '').toLowerCase();
    return platform.includes('mac');
  } catch {
    return false;
  }
})();
