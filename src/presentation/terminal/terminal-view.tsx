"use client";

import { ActionIcon, Button, Group, Loader, Paper, Text, TextInput } from "@mantine/core";
import { IconArrowDown, IconArrowUp, IconPlugConnected, IconX } from "@tabler/icons-react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { FitAddon } from "@xterm/addon-fit";
import type { SearchAddon } from "@xterm/addon-search";
import type { IDisposable, ITerminalOptions, Terminal } from "@xterm/xterm";
import { useEffect, useMemo, useRef, useState } from "react";
import { errorMessage, notifyError } from "@/application/errors";
import { hostUsername } from "@/application/format";
import { useHosts, useIdentities } from "@/application/hooks/entities";
import { createLineTracker } from "@/application/shell-history";
import { useConnectionLogs } from "@/application/stores/connection-logs";
import { useShellHistory } from "@/application/stores/shell-history";
import { useTabs, type WorkTab } from "@/application/stores/tabs";
import { useTerminalSettings, type TerminalSettings } from "@/application/stores/terminal-settings";
import { sendTerminalInput } from "@/application/terminal-io";
import type { Id } from "@/domain/types";
import { Channel, api, events } from "@/infrastructure/tauri/api";
import classes from "@/presentation/terminal/terminal.module.css";
import { findFont, fontStack, loadFont } from "@/presentation/terminal/fonts";
import { useIconFont } from "@/presentation/terminal/use-icon-font";
import { findScheme, toXtermTheme } from "@/presentation/terminal/schemes";

interface Props {
  tab: WorkTab;
  active: boolean;
}

type SessionState =
  { status: "connecting" } | { status: "open" } | { status: "closed"; reason: string | null };

function toBytes(data: unknown): Uint8Array | string {
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (Array.isArray(data)) return Uint8Array.from(data as number[]);
  if (typeof data === "string") return data;
  return new Uint8Array();
}

function terminalOptions(settings: TerminalSettings, iconFont: string | null): ITerminalOptions {
  return {
    fontFamily: fontStack(findFont(settings.fontId), iconFont),
    fontSize: settings.fontSize,
    lineHeight: settings.lineHeight,
    letterSpacing: settings.letterSpacing,
    cursorStyle: settings.cursorStyle,
    cursorBlink: settings.cursorBlink,
    scrollback: settings.scrollback,
    drawBoldTextInBrightColors: settings.brightBold,
    macOptionIsMeta: settings.optionAsMeta,
    theme: toXtermTheme(findScheme(settings.schemeId)),
  };
}

function useSettingsSnapshot(): TerminalSettings {
  const schemeId = useTerminalSettings((state) => state.schemeId);
  const fontId = useTerminalSettings((state) => state.fontId);
  const fontSize = useTerminalSettings((state) => state.fontSize);
  const lineHeight = useTerminalSettings((state) => state.lineHeight);
  const letterSpacing = useTerminalSettings((state) => state.letterSpacing);
  const cursorStyle = useTerminalSettings((state) => state.cursorStyle);
  const cursorBlink = useTerminalSettings((state) => state.cursorBlink);
  const scrollback = useTerminalSettings((state) => state.scrollback);
  const brightBold = useTerminalSettings((state) => state.brightBold);
  const optionAsMeta = useTerminalSettings((state) => state.optionAsMeta);
  const copyOnSelect = useTerminalSettings((state) => state.copyOnSelect);
  const rightClickPaste = useTerminalSettings((state) => state.rightClickPaste);
  return useMemo(
    () => ({
      schemeId,
      fontId,
      fontSize,
      lineHeight,
      letterSpacing,
      cursorStyle,
      cursorBlink,
      scrollback,
      brightBold,
      optionAsMeta,
      copyOnSelect,
      rightClickPaste,
    }),
    [
      schemeId,
      fontId,
      fontSize,
      lineHeight,
      letterSpacing,
      cursorStyle,
      cursorBlink,
      scrollback,
      brightBold,
      optionAsMeta,
      copyOnSelect,
      rightClickPaste,
    ],
  );
}

function safeFit(fit: FitAddon | null, container: HTMLElement | null) {
  if (!fit || !container || container.clientHeight === 0 || container.clientWidth === 0) return;
  try {
    fit.fit();
  } catch {
    return;
  }
}

// A typed line only counts as a command when the shell echoed it back, so
// input at password prompts never lands in the history.
function wasEchoed(terminal: Terminal, command: string) {
  const buffer = terminal.buffer.active;
  const row = buffer.baseY + buffer.cursorY;
  let text = "";
  for (let index = Math.max(0, row - 3); index <= row; index += 1) {
    text += buffer.getLine(index)?.translateToString(true) ?? "";
  }
  return text.includes(command);
}

function isSearchShortcut(event: KeyboardEvent) {
  const key = event.key.toLowerCase();
  return (
    event.type === "keydown" &&
    key === "f" &&
    ((event.metaKey && !event.ctrlKey) || (event.ctrlKey && event.shiftKey))
  );
}

export function TerminalView({ tab, active }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const searchRef = useRef<SearchAddon | null>(null);
  const sessionRef = useRef<Id | null>(null);
  const [terminal, setTerminal] = useState<Terminal | null>(null);
  const [session, setSession] = useState<SessionState>({ status: "connecting" });
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const bindSession = useTabs((state) => state.bindSession);
  const reconnect = useTabs((state) => state.reconnect);
  const closeTab = useTabs((state) => state.close);
  const settings = useSettingsSnapshot();
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const iconFont = useIconFont();
  const { data: hosts = [] } = useHosts();
  const { data: identities = [] } = useIdentities();
  const catalogRef = useRef({ hosts, identities });
  catalogRef.current = { hosts, identities };
  const terminalBackground = findScheme(settings.schemeId).background;

  useEffect(() => {
    let disposed = false;
    let instance: Terminal | null = null;
    let observer: ResizeObserver | null = null;
    const initial = settingsRef.current;
    Promise.all([
      import("@xterm/xterm"),
      import("@xterm/addon-fit"),
      import("@xterm/addon-web-links"),
      import("@xterm/addon-search"),
      import("@xterm/addon-unicode11"),
      loadFont(findFont(initial.fontId), initial.fontSize),
    ])
      .then(([xterm, fit, links, search, unicode11]) => {
        const container = containerRef.current;
        if (disposed || !container) return;
        instance = new xterm.Terminal({
          ...terminalOptions(settingsRef.current, null),
          allowProposedApi: true,
        });
        instance.loadAddon(new unicode11.Unicode11Addon());
        instance.unicode.activeVersion = "11";
        const fitAddon = new fit.FitAddon();
        const searchAddon = new search.SearchAddon();
        instance.loadAddon(fitAddon);
        instance.loadAddon(searchAddon);
        instance.loadAddon(
          new links.WebLinksAddon((_event, uri) => {
            openUrl(uri).catch(notifyError);
          }),
        );
        instance.attachCustomKeyEventHandler((event) => {
          if (isSearchShortcut(event)) {
            setSearchOpen(true);
            return false;
          }
          return true;
        });
        instance.open(container);
        fitRef.current = fitAddon;
        searchRef.current = searchAddon;
        safeFit(fitAddon, container);
        observer = new ResizeObserver(() => safeFit(fitAddon, container));
        observer.observe(container);
        setTerminal(instance);
      })
      .catch(notifyError);
    return () => {
      disposed = true;
      observer?.disconnect();
      instance?.dispose();
    };
  }, []);

  useEffect(() => {
    if (!terminal) return;
    let cancelled = false;
    loadFont(findFont(settings.fontId), settings.fontSize, iconFont).then(() => {
      if (cancelled) return;
      Object.assign(terminal.options, terminalOptions(settings, iconFont));
      requestAnimationFrame(() => safeFit(fitRef.current, containerRef.current));
    });
    return () => {
      cancelled = true;
    };
  }, [terminal, settings, iconFont]);

  useEffect(() => {
    if (!terminal || !settings.copyOnSelect) return;
    const subscription = terminal.onSelectionChange(() => {
      const selection = terminal.getSelection();
      if (selection) navigator.clipboard.writeText(selection).catch(() => undefined);
    });
    return () => subscription.dispose();
  }, [terminal, settings.copyOnSelect]);

  useEffect(() => {
    const container = containerRef.current;
    if (!terminal || !container || !settings.rightClickPaste) return;
    const onContextMenu = (event: MouseEvent) => {
      event.preventDefault();
      navigator.clipboard
        .readText()
        .then((text) => {
          if (text) terminal.paste(text);
        })
        .catch(() => undefined);
    };
    container.addEventListener("contextmenu", onContextMenu);
    return () => container.removeEventListener("contextmenu", onContextMenu);
  }, [terminal, settings.rightClickPaste]);

  useEffect(() => {
    if (!terminal) return;
    let disposed = false;
    const disposables: IDisposable[] = [];
    const closedEarly = new Map<Id, string | null>();
    let unlisten: UnlistenFn | null = null;
    let logId: string | null = null;

    const startLog = () => {
      const { hosts: knownHosts, identities: knownIdentities } = catalogRef.current;
      const host = knownHosts.find((item) => item.id === tab.hostId);
      if (tab.kind !== "ssh" || !host) return;
      logId = useConnectionLogs.getState().start({
        hostId: host.id,
        label: host.label,
        address: host.address,
        port: host.port,
        username: hostUsername(host, knownIdentities),
      });
    };

    const finishLog = () => {
      if (logId) useConnectionLogs.getState().finish(logId);
      logId = null;
    };

    const markClosed = (reason: string | null) => {
      finishLog();
      sessionRef.current = null;
      bindSession(tab.id, null);
      disposables.splice(0).forEach((item) => item.dispose());
      setSession({ status: "closed", reason });
      terminal.write(`\r\n\x1b[90m[${reason ?? "session closed"}]\x1b[0m\r\n`);
    };

    setSession({ status: "connecting" });
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (data) => {
      if (!disposed) terminal.write(toBytes(data));
    };

    events
      .onSessionClosed((closed) => {
        if (disposed) return;
        if (closed.sessionId === sessionRef.current) markClosed(closed.reason);
        else closedEarly.set(closed.sessionId, closed.reason);
      })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(notifyError);

    const opening =
      tab.kind === "ssh" && tab.hostId
        ? api.sessions.openSsh(tab.hostId, terminal.cols, terminal.rows, channel)
        : api.sessions.openLocal(terminal.cols, terminal.rows, channel);

    opening
      .then((sessionId) => {
        if (disposed) {
          api.sessions.close(sessionId).catch(() => undefined);
          return;
        }
        sessionRef.current = sessionId;
        bindSession(tab.id, sessionId);
        if (closedEarly.has(sessionId)) {
          markClosed(closedEarly.get(sessionId) ?? null);
          return;
        }
        setSession({ status: "open" });
        startLog();
        const tracker = createLineTracker();
        disposables.push(
          terminal.onData((data) => {
            sendTerminalInput(sessionId, data);
            tracker.feed(data).forEach((command) => {
              if (wasEchoed(terminal, command)) useShellHistory.getState().record(command);
            });
          }),
        );
        disposables.push(
          terminal.onResize(({ cols, rows }) => {
            api.sessions.resize(sessionId, cols, rows).catch(() => undefined);
          }),
        );
        api.sessions.resize(sessionId, terminal.cols, terminal.rows).catch(() => undefined);
        terminal.focus();
      })
      .catch((error) => {
        if (disposed) return;
        setSession({ status: "closed", reason: errorMessage(error) });
        terminal.write(`\r\n\x1b[31m${errorMessage(error)}\x1b[0m\r\n`);
      });

    return () => {
      disposed = true;
      finishLog();
      unlisten?.();
      disposables.forEach((item) => item.dispose());
      const sessionId = sessionRef.current;
      sessionRef.current = null;
      if (sessionId) api.sessions.close(sessionId).catch(() => undefined);
      bindSession(tab.id, null);
    };
  }, [terminal, tab.id, tab.kind, tab.hostId, tab.nonce, bindSession]);

  useEffect(() => {
    if (!active || !terminal) return;
    const frame = requestAnimationFrame(() => {
      safeFit(fitRef.current, containerRef.current);
      terminal.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [active, terminal]);

  const findNext = () => searchRef.current?.findNext(query);
  const findPrevious = () => searchRef.current?.findPrevious(query);
  const closeSearch = () => {
    setSearchOpen(false);
    searchRef.current?.clearDecorations();
    terminal?.focus();
  };

  return (
    <div className={classes.root} style={{ ["--terminal-bg" as string]: terminalBackground }}>
      <div ref={containerRef} className={classes.surface} />
      {searchOpen && (
        <Paper className={classes.search} shadow="md" p={4} withBorder>
          <Group gap={4}>
            <TextInput
              size="xs"
              autoFocus
              placeholder="Find"
              value={query}
              onChange={(event) => setQuery(event.currentTarget.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  if (event.shiftKey) findPrevious();
                  else findNext();
                }
                if (event.key === "Escape") closeSearch();
              }}
            />
            <ActionIcon size="sm" onClick={findPrevious} aria-label="Previous match">
              <IconArrowUp size={14} />
            </ActionIcon>
            <ActionIcon size="sm" onClick={findNext} aria-label="Next match">
              <IconArrowDown size={14} />
            </ActionIcon>
            <ActionIcon size="sm" onClick={closeSearch} aria-label="Close search">
              <IconX size={14} />
            </ActionIcon>
          </Group>
        </Paper>
      )}
      {session.status === "connecting" && (
        <Paper className={classes.overlay} shadow="md" px="md" py={8} withBorder>
          <Group gap="xs">
            <Loader size="xs" />
            <Text size="sm">Connecting…</Text>
          </Group>
        </Paper>
      )}
      {session.status === "closed" && (
        <Paper className={classes.overlay} shadow="md" px="md" py={8} withBorder>
          <Group gap="sm" wrap="nowrap">
            <Text size="sm" c="dimmed" maw={420} truncate>
              Disconnected{session.reason ? `: ${session.reason}` : ""}
            </Text>
            <Button leftSection={<IconPlugConnected size={14} />} onClick={() => reconnect(tab.id)}>
              Reconnect
            </Button>
            <Button variant="default" onClick={() => closeTab(tab.id)}>
              Close
            </Button>
          </Group>
        </Paper>
      )}
    </div>
  );
}
