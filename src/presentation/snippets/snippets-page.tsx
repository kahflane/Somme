import { Menu } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import {
  IconBraces,
  IconClockFilled,
  IconPencil,
  IconPlayerPlay,
  IconPlayerPlayFilled,
  IconStack2,
  IconTrash,
  IconTrashX,
} from "@tabler/icons-react";
import { useState } from "react";
import { useDeleteSnippet, useSaveSnippet, useSnippets } from "@/application/hooks/entities";
import { useShellHistory } from "@/application/stores/shell-history";
import { sortItems, useSortOrder, useViewMode } from "@/application/stores/view-modes";
import { runSnippet } from "@/application/terminal-io";
import type { Snippet } from "@/domain/types";
import { ContextMenu, useContextMenu } from "@/presentation/common/context-menu";
import { confirmDanger } from "@/presentation/common/dialogs";
import { EmptyBlock } from "@/presentation/common/empty-state";
import {
  CardGrid,
  EntityCard,
  IconButton,
  Muted,
  Page,
  PageSection,
  SearchToggle,
  SortMenu,
  SplitButton,
  ToolButton,
  ViewToggle,
} from "@/presentation/common/page";
import { SidePanel } from "@/presentation/common/side-panel";
import { SnippetPanel } from "@/presentation/snippets/snippet-panel";
import classes from "@/presentation/snippets/snippets.module.css";

type Panel = { kind: "history" } | { kind: "snippet"; snippet: Snippet | null } | null;

const LABEL_LENGTH = 40;

function run(label: string, script: string, target: "active" | "all") {
  const count = runSnippet(script, target);
  if (count === 0) {
    notifications.show({
      color: "orange",
      message: target === "active" ? "Open a terminal tab first" : "No open terminals",
    });
  } else {
    notifications.show({
      color: "blue",
      message: `Sent "${label}" to ${count} terminal${count > 1 ? "s" : ""}`,
    });
  }
}

function firstLine(script: string) {
  return (
    script
      .split("\n")
      .find((line) => line.trim())
      ?.trim() ?? ""
  );
}

function ShellHistoryPanel({ opened, onClose }: { opened: boolean; onClose: () => void }) {
  const commands = useShellHistory((state) => state.commands);
  const clear = useShellHistory((state) => state.clear);
  const save = useSaveSnippet();

  const saveAsSnippet = (command: string) =>
    save.mutate({
      id: null,
      label: command.length > LABEL_LENGTH ? `${command.slice(0, LABEL_LENGTH)}…` : command,
      script: command,
    });

  return (
    <SidePanel
      opened={opened}
      title="Shell History"
      onClose={onClose}
      flush
      menu={
        <Menu.Item leftSection={<IconTrashX size={16} />} disabled={commands.length === 0} onClick={clear}>
          Clear history
        </Menu.Item>
      }
    >
      {commands.length === 0 ? (
        <div className={classes.historyEmpty}>
          Commands you type in terminals show up here. They stay in memory until you quit or lock the vault.
        </div>
      ) : (
        <div className={classes.history}>
          {commands.map((command) => (
            <div key={command} className={classes.historyItem}>
              <span className={classes.historyText} title={command}>
                {command}
              </span>
              <span className={classes.historyActions}>
                <IconButton label="Save as snippet" onClick={() => saveAsSnippet(command)}>
                  <IconBraces size={16} stroke={2.4} />
                </IconButton>
                <IconButton label="Run in active terminal" onClick={() => run(command, command, "active")}>
                  <IconPlayerPlayFilled size={14} />
                </IconButton>
              </span>
            </div>
          ))}
        </div>
      )}
    </SidePanel>
  );
}

export function SnippetsPage() {
  const { data: snippets = [] } = useSnippets();
  const remove = useDeleteSnippet();
  const [view] = useViewMode("snippets");
  const [sort] = useSortOrder("snippets");
  const [query, setQuery] = useState("");
  const [panel, setPanel] = useState<Panel>(null);
  const menu = useContextMenu<Snippet>();
  const needle = query.toLowerCase();
  const visible = sortItems(snippets, sort, (snippet) => snippet.label).filter(
    (snippet) =>
      snippet.label.toLowerCase().includes(needle) || snippet.script.toLowerCase().includes(needle),
  );

  const deleteSnippet = (snippet: Snippet) =>
    confirmDanger("Delete snippet", `Delete "${snippet.label}"?`, () => {
      remove.mutate(snippet.id);
      setPanel(null);
    });

  const historyOpen = panel?.kind === "history";

  return (
    <>
      <Page
        fill={snippets.length === 0}
        toolbar={
          <>
            <SplitButton label="New snippet" onClick={() => setPanel({ kind: "snippet", snippet: null })} />
            <ToolButton
              filled
              active={historyOpen}
              icon={<IconClockFilled size={18} />}
              onClick={() => setPanel(historyOpen ? null : { kind: "history" })}
            >
              Shell History
            </ToolButton>
          </>
        }
        tools={
          <>
            <SearchToggle query={query} onQuery={setQuery} />
            <ViewToggle page="snippets" />
            <SortMenu page="snippets" />
          </>
        }
      >
        {snippets.length === 0 ? (
          <EmptyBlock
            icon={<IconBraces size={24} stroke={2.6} />}
            title="Create snippet"
            description="Save your most used commands as snippets to reuse them in one click."
          />
        ) : (
          <PageSection title="Snippets">
            {visible.length === 0 ? (
              <Muted>No snippets match your search.</Muted>
            ) : (
              <CardGrid view={view}>
                {visible.map((snippet) => (
                  <EntityCard
                    key={snippet.id}
                    icon={<IconBraces size={20} stroke={2.6} />}
                    title={snippet.label}
                    subtitle={firstLine(snippet.script)}
                    active={panel?.kind === "snippet" && panel.snippet?.id === snippet.id}
                    onClick={() => setPanel({ kind: "snippet", snippet })}
                    onContextMenu={(event) => menu.open(event, snippet)}
                    action={{
                      label: "Run in active terminal",
                      icon: <IconPlayerPlayFilled size={14} />,
                      onClick: () => run(snippet.label, snippet.script, "active"),
                    }}
                  />
                ))}
              </CardGrid>
            )}
          </PageSection>
        )}
      </Page>
      <ContextMenu position={menu.position} onClose={menu.close}>
        {menu.target && (
          <SnippetMenuItems
            snippet={menu.target}
            onEdit={(snippet) => setPanel({ kind: "snippet", snippet })}
            onDelete={deleteSnippet}
          />
        )}
      </ContextMenu>
      <ShellHistoryPanel opened={historyOpen} onClose={() => setPanel(null)} />
      <SnippetPanel
        opened={panel?.kind === "snippet"}
        snippet={panel?.kind === "snippet" ? panel.snippet : null}
        onClose={() => setPanel(null)}
        onRun={(snippet) => run(snippet.label, snippet.script, "active")}
        onDelete={deleteSnippet}
      />
    </>
  );
}

interface SnippetMenuProps {
  snippet: Snippet;
  onEdit: (snippet: Snippet) => void;
  onDelete: (snippet: Snippet) => void;
}

function SnippetMenuItems({ snippet, onEdit, onDelete }: SnippetMenuProps) {
  return (
    <>
      <Menu.Item
        leftSection={<IconPlayerPlay size={16} />}
        onClick={() => run(snippet.label, snippet.script, "active")}
      >
        Run in active terminal
      </Menu.Item>
      <Menu.Item
        leftSection={<IconStack2 size={16} />}
        onClick={() => run(snippet.label, snippet.script, "all")}
      >
        Run in all terminals
      </Menu.Item>
      <Menu.Divider />
      <Menu.Item leftSection={<IconPencil size={16} />} onClick={() => onEdit(snippet)}>
        Edit
      </Menu.Item>
      <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(snippet)}>
        Delete
      </Menu.Item>
    </>
  );
}
