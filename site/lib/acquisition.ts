// Website intent only. Never send commands, clipboard text, meeting data, or URLs
// with query strings. Existing GA consent settings apply to these events too.
export type AcquisitionTarget = "mac" | "windows" | "desktop_cli" | "mcp" | "demo" | "agent_config";

declare global {
  interface Window {
    gtag?: (...args: unknown[]) => void;
  }
}

export function trackAcquisition(
  event: "download_intent" | "setup_intent",
  target: AcquisitionTarget,
) {
  if (typeof window === "undefined") return;
  // Analytics must never prevent copying a command or following a download.
  try {
    window.gtag?.("event", event, {
      acquisition_target: target,
      page_path: window.location.pathname,
      transport_type: "beacon",
    });
  } catch {
    // Browser extensions and blocked analytics are normal.
  }
}
