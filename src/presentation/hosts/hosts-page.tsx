import { Menu } from "@mantine/core";
import {
  IconChevronRight,
  IconCopy,
  IconFileImport,
  IconFolderFilled,
  IconFolderPlus,
  IconPencil,
  IconPencilFilled,
  IconPlayerPlay,
  IconStack2Filled,
  IconTagFilled,
  IconTrash,
} from "@tabler/icons-react";
import { useMemo, useState } from "react";
import { notifyError, notifySuccess } from "@/application/errors";
import { hostDetail } from "@/application/format";
import {
  useDeleteGroup,
  useDeleteHost,
  useGroups,
  useHosts,
  useIdentities,
  useImportSshConfig,
  useSaveHost,
} from "@/application/hooks/entities";
import { parseSshTarget } from "@/application/quick-connect";
import { useTabs } from "@/application/stores/tabs";
import { sortItems, useSortOrder, useViewMode } from "@/application/stores/view-modes";
import type { Group as HostGroup, Host } from "@/domain/types";
import { ContextMenu, useContextMenu } from "@/presentation/common/context-menu";
import { confirmDanger } from "@/presentation/common/dialogs";
import { EmptyBlock } from "@/presentation/common/empty-state";
import { ServerGlyph, TerminalGlyph } from "@/presentation/common/glyphs";
import {
  CardGrid,
  EntityCard,
  IconButton,
  Muted,
  Page,
  PageSection,
  SortMenu,
  SplitButton,
  ToolButton,
  ViewToggle,
} from "@/presentation/common/page";
import { GroupPanel } from "@/presentation/hosts/group-panel";
import { HostPanel } from "@/presentation/hosts/host-panel";
import classes from "@/presentation/hosts/hosts.module.css";

type Panel = { kind: "host"; host: Host | null } | { kind: "group"; group: HostGroup | null } | null;

type MenuTarget = { kind: "host"; host: Host } | { kind: "group"; group: HostGroup };

function matches(host: Host, query: string) {
  if (!query) return true;
  const needle = query.toLowerCase();
  return (
    host.label.toLowerCase().includes(needle) ||
    host.address.toLowerCase().includes(needle) ||
    host.tags.some((tag) => tag.toLowerCase().includes(needle))
  );
}

function groupTrail(groups: HostGroup[], groupId: string | null): HostGroup[] {
  const trail: HostGroup[] = [];
  let current = groups.find((group) => group.id === groupId);
  while (current && !trail.includes(current)) {
    trail.unshift(current);
    current = groups.find((group) => group.id === current?.parentId);
  }
  return trail;
}

function hostsBelow(groups: HostGroup[], hosts: Host[], groupId: string): number {
  const ids = new Set([groupId]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const group of groups) {
      if (group.parentId && ids.has(group.parentId) && !ids.has(group.id)) {
        ids.add(group.id);
        grew = true;
      }
    }
  }
  return hosts.filter((host) => host.groupId && ids.has(host.groupId)).length;
}

function plural(count: number, word: string) {
  return `${count} ${word}${count === 1 ? "" : "s"}`;
}

export function HostsPage() {
  const { data: hosts = [] } = useHosts();
  const { data: groups = [] } = useGroups();
  const { data: identities = [] } = useIdentities();
  const deleteHost = useDeleteHost();
  const deleteGroup = useDeleteGroup();
  const saveHost = useSaveHost();
  const importConfig = useImportSshConfig();
  const openSsh = useTabs((state) => state.openSsh);
  const openSftp = useTabs((state) => state.openSftp);
  const openLocal = useTabs((state) => state.openLocal);
  const [view] = useViewMode("hosts");
  const [sort] = useSortOrder("hosts");
  const [query, setQuery] = useState("");
  const [groupId, setGroupId] = useState<string | null>(null);
  const [tagFilter, setTagFilter] = useState<string[]>([]);
  const [panel, setPanel] = useState<Panel>(null);
  const menu = useContextMenu<MenuTarget>();

  const known = useMemo(() => new Set(groups.map((group) => group.id)), [groups]);
  const currentGroup = groupId && known.has(groupId) ? groupId : null;
  const trail = groupTrail(groups, currentGroup);
  const allTags = useMemo(
    () => [...new Set(hosts.flatMap((host) => host.tags))].sort((a, b) => a.localeCompare(b)),
    [hosts],
  );

  const searching = query.trim().length > 0 || tagFilter.length > 0;
  const visibleHosts = sortItems(hosts, sort, (host) => host.label)
    .filter((host) => matches(host, query.trim()))
    .filter((host) => tagFilter.every((tag) => host.tags.includes(tag)))
    .filter((host) => {
      if (searching) return true;
      const parent = host.groupId && known.has(host.groupId) ? host.groupId : null;
      return parent === currentGroup;
    });
  const sortedGroups = sortItems(groups, sort, (group) => group.name);
  const visibleGroups = searching
    ? sortedGroups.filter(
        (group) => query.trim() && group.name.toLowerCase().includes(query.trim().toLowerCase()),
      )
    : sortedGroups.filter(
        (group) => (group.parentId && known.has(group.parentId) ? group.parentId : null) === currentGroup,
      );

  const duplicate = (host: Host) =>
    saveHost.mutate({
      id: null,
      label: `${host.label} copy`,
      address: host.address,
      port: host.port,
      groupId: host.groupId,
      identityId: host.identityId,
      username: host.username,
      password: { action: "keep" },
      keyId: host.keyId,
      jumpHostId: host.jumpHostId,
      startupSnippetId: host.startupSnippetId,
      tags: host.tags,
      color: host.color,
    });

  const removeHost = (host: Host) =>
    confirmDanger("Delete host", `Delete "${host.label}"? Its port forwarding rules are removed too.`, () => {
      deleteHost.mutate(host.id);
      setPanel(null);
    });

  const removeGroup = (group: HostGroup) =>
    confirmDanger(
      "Delete group",
      `Delete "${group.name}"? Hosts inside it are kept and moved out of the group.`,
      () => {
        deleteGroup.mutate(group.id);
        setPanel(null);
        if (currentGroup === group.id) setGroupId(group.parentId);
      },
    );

  const runImport = () =>
    importConfig.mutate(undefined, {
      onSuccess: (count) =>
        notifySuccess(count === 0 ? "No new hosts found in ~/.ssh/config" : `Imported ${count} hosts`),
    });

  const quickConnect = () => {
    const text = query.trim();
    if (!text) return;
    if (visibleHosts.length === 1) {
      openSsh(visibleHosts[0]);
      return;
    }
    const target = parseSshTarget(text);
    if (!target) {
      notifyError("Type a saved host name or user@hostname", "Can't connect");
      return;
    }
    const existing = hosts.find(
      (host) =>
        host.address === target.address &&
        host.port === target.port &&
        (target.username === null || host.username === target.username),
    );
    if (existing) {
      openSsh(existing);
      return;
    }
    saveHost.mutate(
      {
        id: null,
        label: target.address,
        address: target.address,
        port: target.port,
        groupId: currentGroup,
        identityId: null,
        username: target.username,
        password: { action: "keep" },
        keyId: null,
        jumpHostId: null,
        startupSnippetId: null,
        tags: [],
        color: null,
      },
      {
        onSuccess: (host) => {
          setQuery("");
          openSsh(host);
        },
      },
    );
  };

  const search = (
    <div className={classes.search}>
      <input
        className={classes.searchInput}
        placeholder="Find a host or ssh user@hostname..."
        value={query}
        spellCheck={false}
        onChange={(event) => setQuery(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") quickConnect();
          if (event.key === "Escape") setQuery("");
        }}
      />
      <button type="button" className={classes.connect} disabled={!query.trim()} onClick={quickConnect}>
        Connect
      </button>
    </div>
  );

  const toolbar = (
    <>
      <SplitButton
        label="New host"
        onClick={() => setPanel({ kind: "host", host: null })}
        menu={
          <>
            <Menu.Item
              leftSection={<IconFolderPlus size={16} />}
              onClick={() => setPanel({ kind: "group", group: null })}
            >
              New group
            </Menu.Item>
            <Menu.Item leftSection={<IconFileImport size={16} />} onClick={runImport}>
              Import from ~/.ssh/config
            </Menu.Item>
          </>
        }
      />
      <ToolButton icon={<TerminalGlyph size={16} />} onClick={openLocal}>
        Terminal
      </ToolButton>
    </>
  );

  const tools = (
    <>
      <ViewToggle page="hosts" />
      <Menu position="bottom-end" closeOnItemClick={false} width={200}>
        <Menu.Target>
          <span>
            <IconButton label="Filter by tag" active={tagFilter.length > 0}>
              <IconTagFilled size={18} />
            </IconButton>
          </span>
        </Menu.Target>
        <Menu.Dropdown>
          {allTags.length === 0 ? (
            <Menu.Label>No tags yet</Menu.Label>
          ) : (
            allTags.map((tag) => {
              const on = tagFilter.includes(tag);
              return (
                <Menu.Item
                  key={tag}
                  rightSection={on ? "✓" : null}
                  onClick={() =>
                    setTagFilter((current) =>
                      on ? current.filter((item) => item !== tag) : [...current, tag],
                    )
                  }
                >
                  {tag}
                </Menu.Item>
              );
            })
          )}
          {tagFilter.length > 0 && (
            <>
              <Menu.Divider />
              <Menu.Item onClick={() => setTagFilter([])}>Clear filter</Menu.Item>
            </>
          )}
        </Menu.Dropdown>
      </Menu>
      <SortMenu page="hosts" />
    </>
  );

  const empty = hosts.length === 0 && groups.length === 0;
  const hostsTitle =
    trail.length > 0 && !searching ? (
      <span className={classes.crumbs}>
        <button type="button" className={classes.crumb} onClick={() => setGroupId(null)}>
          Hosts
        </button>
        {trail.map((group, index) => (
          <span key={group.id} className={classes.crumbs}>
            <IconChevronRight size={14} color="var(--x-text-secondary)" />
            {index === trail.length - 1 ? (
              <span>{group.name}</span>
            ) : (
              <button type="button" className={classes.crumb} onClick={() => setGroupId(group.id)}>
                {group.name}
              </button>
            )}
          </span>
        ))}
      </span>
    ) : (
      "Hosts"
    );

  return (
    <>
      <Page search={search} toolbar={toolbar} tools={tools} fill={empty}>
        {empty ? (
          <EmptyBlock
            icon={<ServerGlyph size={24} />}
            title="Create host"
            description="Save your servers as hosts to connect to them in one click."
          />
        ) : (
          <>
            {visibleGroups.length > 0 && (
              <PageSection title="Groups">
                <CardGrid view={view}>
                  {visibleGroups.map((group) => (
                    <EntityCard
                      key={group.id}
                      icon={<IconStack2Filled size={20} />}
                      title={group.name}
                      subtitle={plural(hostsBelow(groups, hosts, group.id), "host")}
                      active={panel?.kind === "group" && panel.group?.id === group.id}
                      onClick={() => {
                        setQuery("");
                        setTagFilter([]);
                        setGroupId(group.id);
                      }}
                      onContextMenu={(event) => menu.open(event, { kind: "group", group })}
                      action={{
                        label: "Edit group",
                        icon: <IconPencilFilled size={16} />,
                        onClick: () => setPanel({ kind: "group", group }),
                      }}
                    />
                  ))}
                </CardGrid>
              </PageSection>
            )}
            <PageSection title={hostsTitle}>
              {visibleHosts.length === 0 ? (
                <Muted>{searching ? "No hosts match your search." : "No hosts in this group yet."}</Muted>
              ) : (
                <CardGrid view={view}>
                  {visibleHosts.map((host) => (
                    <EntityCard
                      key={host.id}
                      icon={<ServerGlyph size={22} />}
                      tileColor={host.color}
                      title={host.label}
                      subtitle={hostDetail(host, identities)}
                      active={panel?.kind === "host" && panel.host?.id === host.id}
                      onClick={() => openSsh(host)}
                      onContextMenu={(event) => menu.open(event, { kind: "host", host })}
                      action={{
                        label: "Edit host",
                        icon: <IconPencilFilled size={16} />,
                        onClick: () => setPanel({ kind: "host", host }),
                      }}
                    />
                  ))}
                </CardGrid>
              )}
            </PageSection>
          </>
        )}
      </Page>
      <ContextMenu position={menu.position} onClose={menu.close}>
        {menu.target?.kind === "host" && (
          <HostMenuItems
            host={menu.target.host}
            onConnect={openSsh}
            onSftp={openSftp}
            onEdit={(host) => setPanel({ kind: "host", host })}
            onDuplicate={duplicate}
            onDelete={removeHost}
          />
        )}
        {menu.target?.kind === "group" && (
          <GroupMenuItems
            group={menu.target.group}
            onOpen={(group) => setGroupId(group.id)}
            onEdit={(group) => setPanel({ kind: "group", group })}
            onDelete={removeGroup}
          />
        )}
      </ContextMenu>
      <HostPanel
        opened={panel?.kind === "host"}
        host={panel?.kind === "host" ? panel.host : null}
        defaultGroupId={currentGroup}
        onClose={() => setPanel(null)}
        onDuplicate={duplicate}
        onSftp={openSftp}
        onDelete={removeHost}
      />
      <GroupPanel
        opened={panel?.kind === "group"}
        group={panel?.kind === "group" ? panel.group : null}
        defaultParentId={currentGroup}
        onClose={() => setPanel(null)}
        onDelete={removeGroup}
      />
    </>
  );
}

interface HostMenuProps {
  host: Host;
  onConnect: (host: Host) => void;
  onSftp: (host: Host) => void;
  onEdit: (host: Host) => void;
  onDuplicate: (host: Host) => void;
  onDelete: (host: Host) => void;
}

function HostMenuItems({ host, onConnect, onSftp, onEdit, onDuplicate, onDelete }: HostMenuProps) {
  return (
    <>
      <Menu.Item leftSection={<IconPlayerPlay size={16} />} onClick={() => onConnect(host)}>
        Connect
      </Menu.Item>
      <Menu.Item leftSection={<IconFolderFilled size={16} />} onClick={() => onSftp(host)}>
        Open SFTP
      </Menu.Item>
      <Menu.Divider />
      <Menu.Item leftSection={<IconPencil size={16} />} onClick={() => onEdit(host)}>
        Edit
      </Menu.Item>
      <Menu.Item leftSection={<IconCopy size={16} />} onClick={() => onDuplicate(host)}>
        Duplicate
      </Menu.Item>
      <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(host)}>
        Delete
      </Menu.Item>
    </>
  );
}

interface GroupMenuProps {
  group: HostGroup;
  onOpen: (group: HostGroup) => void;
  onEdit: (group: HostGroup) => void;
  onDelete: (group: HostGroup) => void;
}

function GroupMenuItems({ group, onOpen, onEdit, onDelete }: GroupMenuProps) {
  return (
    <>
      <Menu.Item leftSection={<IconFolderFilled size={16} />} onClick={() => onOpen(group)}>
        Open
      </Menu.Item>
      <Menu.Item leftSection={<IconPencil size={16} />} onClick={() => onEdit(group)}>
        Edit
      </Menu.Item>
      <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(group)}>
        Delete
      </Menu.Item>
    </>
  );
}
