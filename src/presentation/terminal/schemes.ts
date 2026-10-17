import type { ITheme } from "@xterm/xterm";

export interface TerminalScheme {
  id: string;
  name: string;
  dark: boolean;
  background: string;
  foreground: string;
  cursor: string;
  selection: string;
  palette: readonly [
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
  ];
}

export const terminalSchemes: TerminalScheme[] = [
  {
    id: "somme-dark",
    name: "Somme Dark",
    dark: true,
    background: "#000000",
    foreground: "#e7e9ea",
    cursor: "#1d9bf0",
    selection: "#1d9bf055",
    palette: [
      "#16181c", "#f4212e", "#00ba7c", "#ffd400", "#1d9bf0", "#f91880", "#3bc4d9", "#d9d9d9",
      "#71767b", "#ff5c66", "#33d69f", "#ffe45c", "#4db5f5", "#ff4fa3", "#6fdceb", "#ffffff",
    ],
  },
  {
    id: "somme-dim",
    name: "Somme Dim",
    dark: true,
    background: "#15202b",
    foreground: "#f7f9f9",
    cursor: "#1d9bf0",
    selection: "#1d9bf055",
    palette: [
      "#1e2732", "#f4212e", "#00ba7c", "#ffd400", "#1d9bf0", "#f91880", "#3bc4d9", "#d9d9d9",
      "#8b98a5", "#ff5c66", "#33d69f", "#ffe45c", "#4db5f5", "#ff4fa3", "#6fdceb", "#ffffff",
    ],
  },
  {
    id: "somme-light",
    name: "Somme Light",
    dark: false,
    background: "#ffffff",
    foreground: "#0f1419",
    cursor: "#1d9bf0",
    selection: "#1d9bf033",
    palette: [
      "#0f1419", "#e0245e", "#00a36c", "#b38f00", "#1a8cd8", "#c71a75", "#0f9bb3", "#cfd9de",
      "#536471", "#f4212e", "#00ba7c", "#d4a900", "#1d9bf0", "#f91880", "#1dafc9", "#eff3f4",
    ],
  },
  {
    id: "dracula",
    name: "Dracula",
    dark: true,
    background: "#282a36",
    foreground: "#f8f8f2",
    cursor: "#f8f8f2",
    selection: "#44475a",
    palette: [
      "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
      "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
    ],
  },
  {
    id: "monokai",
    name: "Monokai",
    dark: true,
    background: "#272822",
    foreground: "#f8f8f2",
    cursor: "#f8f8f0",
    selection: "#49483e",
    palette: [
      "#272822", "#f92672", "#a6e22e", "#f4bf75", "#66d9ef", "#ae81ff", "#a1efe4", "#f8f8f2",
      "#75715e", "#f92672", "#a6e22e", "#f4bf75", "#66d9ef", "#ae81ff", "#a1efe4", "#f9f8f5",
    ],
  },
  {
    id: "solarized-dark",
    name: "Solarized Dark",
    dark: true,
    background: "#002b36",
    foreground: "#839496",
    cursor: "#93a1a1",
    selection: "#073642",
    palette: [
      "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
      "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
    ],
  },
  {
    id: "solarized-light",
    name: "Solarized Light",
    dark: false,
    background: "#fdf6e3",
    foreground: "#657b83",
    cursor: "#586e75",
    selection: "#eee8d5",
    palette: [
      "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
      "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
    ],
  },
  {
    id: "nord",
    name: "Nord",
    dark: true,
    background: "#2e3440",
    foreground: "#d8dee9",
    cursor: "#d8dee9",
    selection: "#434c5e",
    palette: [
      "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
      "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4",
    ],
  },
  {
    id: "one-dark",
    name: "Atom One Dark",
    dark: true,
    background: "#282c34",
    foreground: "#abb2bf",
    cursor: "#528bff",
    selection: "#3e4451",
    palette: [
      "#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
      "#5c6370", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff",
    ],
  },
  {
    id: "one-light",
    name: "Atom One Light",
    dark: false,
    background: "#fafafa",
    foreground: "#383a42",
    cursor: "#526fff",
    selection: "#e5e5e6",
    palette: [
      "#383a42", "#e45649", "#50a14f", "#c18401", "#4078f2", "#a626a4", "#0184bc", "#a0a1a7",
      "#696c77", "#e45649", "#50a14f", "#c18401", "#4078f2", "#a626a4", "#0184bc", "#ffffff",
    ],
  },
  {
    id: "gruvbox-dark",
    name: "Gruvbox Dark",
    dark: true,
    background: "#282828",
    foreground: "#ebdbb2",
    cursor: "#ebdbb2",
    selection: "#504945",
    palette: [
      "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
      "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
    ],
  },
  {
    id: "tokyo-night",
    name: "Tokyo Night",
    dark: true,
    background: "#1a1b26",
    foreground: "#c0caf5",
    cursor: "#c0caf5",
    selection: "#33467c",
    palette: [
      "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
      "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
    ],
  },
  {
    id: "catppuccin-mocha",
    name: "Catppuccin Mocha",
    dark: true,
    background: "#1e1e2e",
    foreground: "#cdd6f4",
    cursor: "#f5e0dc",
    selection: "#585b70",
    palette: [
      "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
      "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
    ],
  },
  {
    id: "catppuccin-latte",
    name: "Catppuccin Latte",
    dark: false,
    background: "#eff1f5",
    foreground: "#4c4f69",
    cursor: "#dc8a78",
    selection: "#acb0be",
    palette: [
      "#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be",
      "#6c6f85", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#bcc0cc",
    ],
  },
  {
    id: "rose-pine",
    name: "Rosé Pine",
    dark: true,
    background: "#191724",
    foreground: "#e0def4",
    cursor: "#524f67",
    selection: "#403d52",
    palette: [
      "#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
      "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
    ],
  },
  {
    id: "night-owl",
    name: "Night Owl",
    dark: true,
    background: "#011627",
    foreground: "#d6deeb",
    cursor: "#80a4c2",
    selection: "#1d3b53",
    palette: [
      "#011627", "#ef5350", "#22da6e", "#addb67", "#82aaff", "#c792ea", "#21c7a8", "#ffffff",
      "#575656", "#ef5350", "#22da6e", "#ffeb95", "#82aaff", "#c792ea", "#7fdbca", "#ffffff",
    ],
  },
  {
    id: "material-dark",
    name: "Material Dark",
    dark: true,
    background: "#263238",
    foreground: "#eeffff",
    cursor: "#ffcc00",
    selection: "#546e7a",
    palette: [
      "#000000", "#ff5370", "#c3e88d", "#ffcb6b", "#82aaff", "#c792ea", "#89ddff", "#ffffff",
      "#546e7a", "#ff5370", "#c3e88d", "#ffcb6b", "#82aaff", "#c792ea", "#89ddff", "#ffffff",
    ],
  },
  {
    id: "github-dark",
    name: "GitHub Dark",
    dark: true,
    background: "#0d1117",
    foreground: "#c9d1d9",
    cursor: "#58a6ff",
    selection: "#264f78",
    palette: [
      "#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4",
      "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#f0f6fc",
    ],
  },
  {
    id: "homebrew",
    name: "Homebrew",
    dark: true,
    background: "#000000",
    foreground: "#00ff00",
    cursor: "#23ff18",
    selection: "#083905",
    palette: [
      "#000000", "#990000", "#00a600", "#999900", "#0000b2", "#b200b2", "#00a6b2", "#bfbfbf",
      "#666666", "#e50000", "#00d900", "#e5e500", "#0000ff", "#e500e5", "#00e5e5", "#e5e5e5",
    ],
  },
];

export function findScheme(id: string): TerminalScheme {
  return terminalSchemes.find((scheme) => scheme.id === id) ?? terminalSchemes[0];
}

export function toXtermTheme(scheme: TerminalScheme): ITheme {
  const [
    black,
    red,
    green,
    yellow,
    blue,
    magenta,
    cyan,
    white,
    brightBlack,
    brightRed,
    brightGreen,
    brightYellow,
    brightBlue,
    brightMagenta,
    brightCyan,
    brightWhite,
  ] = scheme.palette;
  return {
    background: scheme.background,
    foreground: scheme.foreground,
    cursor: scheme.cursor,
    cursorAccent: scheme.background,
    selectionBackground: scheme.selection,
    black,
    red,
    green,
    yellow,
    blue,
    magenta,
    cyan,
    white,
    brightBlack,
    brightRed,
    brightGreen,
    brightYellow,
    brightBlue,
    brightMagenta,
    brightCyan,
    brightWhite,
  };
}
