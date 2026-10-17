import { api } from "@/infrastructure/tauri/api";

export type FontSource = "bundled" | "system" | "default";

export interface TerminalFont {
  id: string;
  label: string;
  family: string;
  source: FontSource;
}

const SYSTEM_PREFIX = "system:";

const fallbackFamilies = ["SF Mono", "Menlo", "Monaco", "Consolas", "Courier New"];
const emojiFamilies = ["Apple Color Emoji", "Segoe UI Emoji", "Noto Color Emoji"];

// Most specific first: a symbols-only font, then patched Nerd Fonts.
const iconFontPatterns = [
  /^Symbols Nerd Font Mono$/i,
  /^Symbols Nerd Font$/i,
  /Nerd Font Mono$/i,
  /Nerd Font$/i,
  /Nerd Font Propo$/i,
  / NF$/,
  /Powerline/i,
];

export const terminalFonts: TerminalFont[] = [
  { id: "jetbrains-mono", label: "JetBrains Mono", family: "JetBrains Mono", source: "bundled" },
  { id: "fira-code", label: "Fira Code", family: "Fira Code", source: "bundled" },
  { id: "fira-mono", label: "Fira Mono", family: "Fira Mono", source: "bundled" },
  { id: "cascadia-code", label: "Cascadia Code", family: "Cascadia Code", source: "bundled" },
  { id: "source-code-pro", label: "Source Code Pro", family: "Source Code Pro", source: "bundled" },
  { id: "ibm-plex-mono", label: "IBM Plex Mono", family: "IBM Plex Mono", source: "bundled" },
  { id: "roboto-mono", label: "Roboto Mono", family: "Roboto Mono", source: "bundled" },
  { id: "ubuntu-mono", label: "Ubuntu Mono", family: "Ubuntu Mono", source: "bundled" },
  { id: "inconsolata", label: "Inconsolata", family: "Inconsolata", source: "bundled" },
  { id: "dejavu-mono", label: "DejaVu Sans Mono", family: "DejaVu Mono", source: "bundled" },
  { id: "anonymous-pro", label: "Anonymous Pro", family: "Anonymous Pro", source: "bundled" },
  { id: "system", label: "System monospace", family: "SF Mono", source: "default" },
];

export function systemFontId(family: string): string {
  return `${SYSTEM_PREFIX}${family}`;
}

export function findFont(id: string): TerminalFont {
  if (id.startsWith(SYSTEM_PREFIX)) {
    const family = id.slice(SYSTEM_PREFIX.length);
    return { id, label: family, family, source: "system" };
  }
  return terminalFonts.find((font) => font.id === id) ?? terminalFonts[0];
}

export function pickIconFont(families: string[]): string | null {
  for (const pattern of iconFontPatterns) {
    const match = families.find((family) => pattern.test(family));
    if (match) return match;
  }
  return null;
}

function quote(family: string) {
  return `"${family.replace(/["\\]/g, "")}"`;
}

export function fontStack(font: TerminalFont, iconFont: string | null = null): string {
  const families = [
    ...(font.source === "default" ? [] : [font.family]),
    ...(iconFont && iconFont !== font.family ? [iconFont] : []),
    ...fallbackFamilies,
    ...emojiFamilies,
  ];
  return `${families.map(quote).join(", ")}, monospace`;
}

const registered = new Map<string, Promise<void>>();

// Installed fonts are read by the app and handed to the web view directly, so
// they work even where the web view hides user-installed fonts. If a face can't
// be loaded this way the web view still looks the family up by name.
function registerFace(family: string, weight: number, italic: boolean): Promise<void> {
  const key = `${family}|${weight}|${italic}`;
  let pending = registered.get(key);
  if (!pending) {
    pending = api.fonts
      .data(family, weight, italic)
      .then(async (data) => {
        const face = new FontFace(family, data, {
          weight: String(weight),
          style: italic ? "italic" : "normal",
        });
        await face.load();
        document.fonts.add(face);
      })
      .catch(() => undefined);
    registered.set(key, pending);
  }
  return pending;
}

function registerFamily(family: string, withBold: boolean): Promise<unknown> {
  const faces = [registerFace(family, 400, false)];
  if (withBold) faces.push(registerFace(family, 700, false), registerFace(family, 400, true));
  return Promise.all(faces);
}

export async function loadFont(
  font: TerminalFont,
  size: number,
  iconFont: string | null = null,
): Promise<void> {
  if (typeof document === "undefined" || !document.fonts) return;
  const pending: Promise<unknown>[] = [];
  if (font.source === "system") pending.push(registerFamily(font.family, true));
  if (iconFont && iconFont !== font.family) pending.push(registerFamily(iconFont, false));
  await Promise.all(pending);
  if (font.source === "default") return;
  await Promise.all([
    document.fonts.load(`400 ${size}px ${quote(font.family)}`),
    document.fonts.load(`700 ${size}px ${quote(font.family)}`),
  ]).catch(() => undefined);
}
