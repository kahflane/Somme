import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

function isMacDesktop() {
  return (
    typeof window !== "undefined" && "__TAURI_INTERNALS__" in window && navigator.userAgent.includes("Mac")
  );
}

// On macOS the window uses an overlay title bar, so the tab strip has to make
// room for the traffic lights except in full screen, where they are hidden.
export function useTrafficLightInset(): boolean {
  const [inset, setInset] = useState(isMacDesktop);

  useEffect(() => {
    if (!isMacDesktop()) return;
    const appWindow = getCurrentWindow();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const update = () => {
      appWindow
        .isFullscreen()
        .then((fullscreen) => {
          if (!disposed) setInset(!fullscreen);
        })
        .catch(() => undefined);
    };
    update();
    appWindow
      .onResized(update)
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return inset;
}
