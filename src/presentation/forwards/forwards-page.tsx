import { Menu } from "@mantine/core";
import { IconPencil, IconPencilFilled, IconPlayerPlay, IconPlayerStop, IconTrash } from "@tabler/icons-react";
import { useState } from "react";
import {
  useActiveForwards,
  useDeleteForward,
  useForwards,
  useHosts,
  useStartForward,
  useStopForward,
} from "@/application/hooks/entities";
import { sortItems, useSortOrder, useViewMode } from "@/application/stores/view-modes";
import type { ForwardKind, PortForward } from "@/domain/types";
import { ContextMenu, useContextMenu } from "@/presentation/common/context-menu";
import { confirmDanger } from "@/presentation/common/dialogs";
import { EmptyBlock } from "@/presentation/common/empty-state";
import { ForwardGlyph } from "@/presentation/common/glyphs";
import {
  CardGrid,
  EntityCard,
  Muted,
  Page,
  PageSection,
  SearchToggle,
  SortMenu,
  SplitButton,
  ViewToggle,
} from "@/presentation/common/page";
import { ForwardPanel } from "@/presentation/forwards/forward-panel";

type Panel = { rule: PortForward | null; kind: ForwardKind } | null;

const kindLabels: Record<ForwardKind, string> = { local: "Local", remote: "Remote", dynamic: "Dynamic" };

function describe(rule: PortForward) {
  const bind = `${rule.bindAddress}:${rule.bindPort}`;
  if (rule.kind === "dynamic") return `SOCKS on ${bind}`;
  return `${bind} → ${rule.targetHost}:${rule.targetPort}`;
}

export function ForwardsPage() {
  const { data: rules = [] } = useForwards();
  const { data: active = [] } = useActiveForwards();
  const { data: hosts = [] } = useHosts();
  const start = useStartForward();
  const stop = useStopForward();
  const remove = useDeleteForward();
  const [view] = useViewMode("forwards");
  const [sort] = useSortOrder("forwards");
  const [query, setQuery] = useState("");
  const [panel, setPanel] = useState<Panel>(null);
  const menu = useContextMenu<PortForward>();
  const pendingId = start.isPending ? start.variables : stop.isPending ? stop.variables : null;

  const hostLabel = (rule: PortForward) =>
    hosts.find((host) => host.id === rule.hostId)?.label ?? "Unknown host";
  const needle = query.toLowerCase();
  const visible = sortItems(rules, sort, (rule) => rule.label).filter(
    (rule) =>
      rule.label.toLowerCase().includes(needle) ||
      describe(rule).toLowerCase().includes(needle) ||
      hostLabel(rule).toLowerCase().includes(needle),
  );

  const toggle = (rule: PortForward) => {
    if (pendingId === rule.id) return;
    if (active.includes(rule.id)) stop.mutate(rule.id);
    else start.mutate(rule.id);
  };

  const deleteRule = (rule: PortForward) =>
    confirmDanger("Delete port forwarding", `Delete "${rule.label}"?`, () => {
      remove.mutate(rule.id);
      setPanel(null);
    });

  const newRule = (kind: ForwardKind) => setPanel({ rule: null, kind });

  return (
    <>
      <Page
        fill={rules.length === 0}
        toolbar={
          <SplitButton
            label="New forwarding"
            onClick={() => newRule("local")}
            menu={(["local", "remote", "dynamic"] as const).map((kind) => (
              <Menu.Item key={kind} onClick={() => newRule(kind)}>
                {kindLabels[kind]} forwarding
              </Menu.Item>
            ))}
          />
        }
        tools={
          <>
            <SearchToggle query={query} onQuery={setQuery} />
            <ViewToggle page="forwards" />
            <SortMenu page="forwards" />
          </>
        }
      >
        {rules.length === 0 ? (
          <EmptyBlock
            icon={<ForwardGlyph size={24} />}
            title="Set up port forwarding"
            description="Save port forwarding to access databases, web apps, and other services."
          />
        ) : (
          <PageSection title="Port Forwarding">
            {visible.length === 0 ? (
              <Muted>No rules match your search.</Muted>
            ) : (
              <CardGrid view={view}>
                {visible.map((rule) => {
                  const running = active.includes(rule.id);
                  return (
                    <EntityCard
                      key={rule.id}
                      icon={<ForwardGlyph size={20} />}
                      tileColor={running ? "var(--x-success)" : null}
                      title={rule.label}
                      subtitle={`${kindLabels[rule.kind]} · ${describe(rule)} · ${hostLabel(rule)}`}
                      badge={running}
                      active={panel?.rule?.id === rule.id}
                      onClick={() => toggle(rule)}
                      onContextMenu={(event) => menu.open(event, rule)}
                      action={{
                        label: "Edit",
                        icon: <IconPencilFilled size={16} />,
                        onClick: () => setPanel({ rule, kind: rule.kind }),
                      }}
                    />
                  );
                })}
              </CardGrid>
            )}
          </PageSection>
        )}
      </Page>
      <ContextMenu position={menu.position} onClose={menu.close}>
        {menu.target && (
          <RuleMenuItems
            rule={menu.target}
            running={active.includes(menu.target.id)}
            onToggle={toggle}
            onEdit={(rule) => setPanel({ rule, kind: rule.kind })}
            onDelete={deleteRule}
          />
        )}
      </ContextMenu>
      <ForwardPanel
        opened={panel !== null}
        rule={panel?.rule ?? null}
        defaultKind={panel?.kind ?? "local"}
        onClose={() => setPanel(null)}
        onDelete={deleteRule}
      />
    </>
  );
}

interface RuleMenuProps {
  rule: PortForward;
  running: boolean;
  onToggle: (rule: PortForward) => void;
  onEdit: (rule: PortForward) => void;
  onDelete: (rule: PortForward) => void;
}

function RuleMenuItems({ rule, running, onToggle, onEdit, onDelete }: RuleMenuProps) {
  return (
    <>
      <Menu.Item
        leftSection={running ? <IconPlayerStop size={16} /> : <IconPlayerPlay size={16} />}
        onClick={() => onToggle(rule)}
      >
        {running ? "Stop" : "Start"}
      </Menu.Item>
      <Menu.Item leftSection={<IconPencil size={16} />} onClick={() => onEdit(rule)}>
        Edit
      </Menu.Item>
      <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(rule)}>
        Delete
      </Menu.Item>
    </>
  );
}
