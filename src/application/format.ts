import type { Host, Identity } from "@/domain/types";

export const tileColors = [
  "#1c4775",
  "#d85f33",
  "#0a235b",
  "#e5aa43",
  "#2fbb73",
  "#7b5cf0",
  "#e64f57",
  "#1fa2b8",
];

export function hostUsername(host: Host, identities: Identity[]): string | null {
  const identity = identities.find((item) => item.id === host.identityId);
  return identity?.username ?? host.username;
}

export function hostDetail(host: Host, identities: Identity[]): string {
  const username = hostUsername(host, identities);
  return username ? `ssh, ${username}` : "ssh";
}

export function hostSubtitle(host: Host, identities: Identity[]): string {
  const username = hostUsername(host, identities);
  const target = host.port === 22 ? host.address : `${host.address}:${host.port}`;
  return username ? `${username}@${target}` : target;
}

export function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = size / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

export function keyTypeLabel(algorithm: string): string {
  const name = algorithm.toLowerCase();
  if (name.includes("ed25519")) return "ED25519";
  if (name.includes("ecdsa")) return "ECDSA";
  if (name.includes("rsa")) return "RSA";
  return algorithm.replace(/^ssh-/, "").toUpperCase();
}
