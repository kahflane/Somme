export type PathStyle = "posix" | "windows";

export interface PathCrumb {
  label: string;
  path: string;
}

export function pathStyleOf(path: string): PathStyle {
  return /^[a-zA-Z]:[\\/]/.test(path) || path.startsWith("\\\\") ? "windows" : "posix";
}

function separator(style: PathStyle) {
  return style === "windows" ? "\\" : "/";
}

function trimEnd(path: string, style: PathStyle) {
  const root = rootOf(path, style);
  const trimmed = path.replace(/[\\/]+$/, "");
  return trimmed.length < root.length ? root : trimmed;
}

function rootOf(path: string, style: PathStyle) {
  if (style === "posix") return "/";
  const drive = /^[a-zA-Z]:[\\/]?/.exec(path);
  return drive ? `${drive[0].slice(0, 2)}\\` : "\\";
}

export function joinPath(directory: string, name: string, style: PathStyle = "posix"): string {
  const base = trimEnd(directory, style);
  const sep = separator(style);
  return base.endsWith(sep) ? `${base}${name}` : `${base}${sep}${name}`;
}

export function parentPath(path: string, style: PathStyle = "posix"): string {
  const root = rootOf(path, style);
  const trimmed = trimEnd(path, style);
  if (trimmed === root) return root;
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (index < root.length) return root;
  return trimmed.slice(0, index);
}

export function isRoot(path: string, style: PathStyle = "posix"): boolean {
  return trimEnd(path, style) === rootOf(path, style);
}

export function pathCrumbs(path: string, style: PathStyle = "posix"): PathCrumb[] {
  const root = rootOf(path, style);
  const rest = trimEnd(path, style).slice(root.length);
  const crumbs: PathCrumb[] = [];
  let current = root;
  for (const part of rest.split(/[\\/]+/).filter(Boolean)) {
    current = joinPath(current, part, style);
    crumbs.push({ label: part, path: current });
  }
  return crumbs;
}
