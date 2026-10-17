export interface KeyInfo {
  complete: boolean;
  encrypted: boolean;
  type: string | null;
}

const PEM = /-----BEGIN ([A-Z0-9 ]*?)PRIVATE KEY-----([\s\S]*?)-----END \1PRIVATE KEY-----/;
const PUTTY = /^PuTTY-User-Key-File-\d+:\s*(\S+)/m;

function typeFromName(name: string): string | null {
  const lower = name.toLowerCase();
  if (lower.includes("ed25519")) return "ED25519";
  if (lower.includes("ecdsa")) return "ECDSA";
  if (lower.includes("rsa")) return "RSA";
  if (lower.includes("dss") || lower.includes("dsa")) return "DSA";
  return null;
}

function decodeBase64(body: string): string {
  try {
    return atob(body.replace(/[^A-Za-z0-9+/=]/g, ""));
  } catch {
    return "";
  }
}

// Reads just enough of a pasted private key to show its type and to know
// whether it needs a passphrase. The vault does the real parsing on import.
export function readKeyInfo(text: string): KeyInfo {
  const putty = PUTTY.exec(text.trim());
  if (putty) {
    return {
      complete: /^Private-MAC:/m.test(text),
      encrypted: !/^Encryption:\s*none/m.test(text),
      type: typeFromName(putty[1]),
    };
  }
  const pem = PEM.exec(text);
  if (!pem) return { complete: false, encrypted: false, type: null };
  const [, kind, body] = pem;
  if (kind === "OPENSSH ") {
    const decoded = decodeBase64(body);
    return { complete: true, encrypted: decoded.includes("bcrypt"), type: typeFromName(decoded) };
  }
  return {
    complete: true,
    encrypted: kind === "ENCRYPTED " || body.includes("ENCRYPTED"),
    type: kind === "EC " ? "ECDSA" : typeFromName(kind),
  };
}

export function looksLikePrivateKey(text: string): boolean {
  return text.includes("PRIVATE KEY-----") || text.trim().startsWith("PuTTY-User-Key-File-");
}
