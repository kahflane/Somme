const PASTE_START = "[200~";
const PASTE_END = "[201~";

export interface LineTracker {
  feed: (data: string) => string[];
}

// Follows what the user types into a terminal and returns each line sent with
// Enter. Lines edited with arrows, tab completion or other control keys are
// dropped, because the typed text no longer matches what the shell runs.
export function createLineTracker(): LineTracker {
  let line = "";
  let dirty = false;

  const skipEscape = (data: string, start: number): number => {
    const rest = data.slice(start + 1);
    if (rest.startsWith(PASTE_START)) return start + PASTE_START.length;
    if (rest.startsWith(PASTE_END)) return start + PASTE_END.length;
    dirty = true;
    const lead = data[start + 1];
    if (lead !== "[" && lead !== "O") return start + 1;
    let index = start + 2;
    while (index < data.length) {
      const code = data.charCodeAt(index);
      if (code >= 0x40 && code <= 0x7e) return index;
      index += 1;
    }
    return index;
  };

  return {
    feed(data) {
      const done: string[] = [];
      for (let index = 0; index < data.length; index += 1) {
        const char = data[index];
        if (char === "\r" || char === "\n") {
          const command = line.trim();
          if (!dirty && command) done.push(command);
          line = "";
          dirty = false;
        } else if (char === "\x7f" || char === "\b") {
          line = line.slice(0, -1);
        } else if (char === "\x03" || char === "\x15") {
          line = "";
          dirty = false;
        } else if (char === "\x1b") {
          index = skipEscape(data, index);
        } else if (char < " ") {
          dirty = true;
        } else {
          line += char;
        }
      }
      return done;
    },
  };
}
