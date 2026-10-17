import { notifyError } from "@/application/errors";
import { useTabs } from "@/application/stores/tabs";
import type { Id } from "@/domain/types";
import { api } from "@/infrastructure/tauri/api";

function writeAll(sessionIds: Id[], data: string) {
  sessionIds.forEach((sessionId) => {
    api.sessions.write(sessionId, data).catch(notifyError);
  });
}

export function sendTerminalInput(ownSessionId: Id, data: string) {
  const { broadcast, sessions } = useTabs.getState();
  if (broadcast) writeAll(Object.values(sessions), data);
  else writeAll([ownSessionId], data);
}

// The focused terminal, or the last one used when a vault page is in front.
export function activeSessionId(): Id | null {
  const { activeId, lastTerminalId, sessions } = useTabs.getState();
  return sessions[activeId] ?? (lastTerminalId ? sessions[lastTerminalId] : undefined) ?? null;
}

export function runSnippet(script: string, target: "active" | "all"): number {
  const normalized = `${script.replace(/\s+$/, "").replace(/\r?\n/g, "\r")}\r`;
  if (target === "active") {
    const sessionId = activeSessionId();
    if (!sessionId) return 0;
    writeAll([sessionId], normalized);
    return 1;
  }
  const sessionIds = Object.values(useTabs.getState().sessions);
  writeAll(sessionIds, normalized);
  return sessionIds.length;
}
