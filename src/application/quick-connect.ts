export interface SshTarget {
  username: string | null;
  address: string;
  port: number;
}

const TARGET = /^(?:ssh\s+)?(?:-p\s*(\d+)\s+)?(?:([^@\s]+)@)?([^\s@:]+)(?::(\d+))?(?:\s+-p\s*(\d+))?$/;

// Reads "host", "user@host", "user@host:2222" or "ssh -p 2222 user@host".
export function parseSshTarget(text: string): SshTarget | null {
  const match = TARGET.exec(text.trim());
  if (!match) return null;
  const [, portBefore, username, address, portInline, portAfter] = match;
  const port = Number(portBefore ?? portInline ?? portAfter ?? 22);
  if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
  return { username: username ?? null, address, port };
}
