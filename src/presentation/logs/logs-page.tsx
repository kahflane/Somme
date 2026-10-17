import { Menu, Tooltip } from "@mantine/core";
import {
  IconBookmark,
  IconBookmarkFilled,
  IconClockFilled,
  IconPlayerPlay,
  IconSortAscending,
  IconSortDescending,
  IconTrash,
  IconTrashX,
} from "@tabler/icons-react";
import dayjs from "dayjs";
import { useState, type MouseEvent } from "react";
import { useAccountStatus } from "@/application/hooks/account";
import { useHosts, useSaveHost } from "@/application/hooks/entities";
import { useConnectionLogs, type ConnectionLog } from "@/application/stores/connection-logs";
import { useTabs } from "@/application/stores/tabs";
import type { Host } from "@/domain/types";
import { ContextMenu, useContextMenu } from "@/presentation/common/context-menu";
import { confirmDanger } from "@/presentation/common/dialogs";
import { EmptyBlock } from "@/presentation/common/empty-state";
import { ServerGlyph } from "@/presentation/common/glyphs";
import classes from "@/presentation/logs/logs.module.css";

function timeRange(entry: ConnectionLog) {
  const start = dayjs(entry.startedAt);
  if (entry.endedAt === null) return start.format("HH:mm");
  const end = dayjs(entry.endedAt);
  const days = end.startOf("day").diff(start.startOf("day"), "day");
  return `${start.format("HH:mm")} - ${end.format("HH:mm")}${days > 0 ? ` (+${days}d)` : ""}`;
}

interface RowProps {
  entry: ConnectionLog;
  host: Host | undefined;
  user: string;
  onConnect: (entry: ConnectionLog) => void;
  onSave: (entry: ConnectionLog) => void;
  onMenu: (event: MouseEvent, entry: ConnectionLog) => void;
}

function LogRow({ entry, host, user, onConnect, onSave, onMenu }: RowProps) {
  const username = host?.username ?? entry.username;
  return (
    <div
      role="button"
      tabIndex={0}
      className={`${classes.columns} ${classes.row}`}
      onClick={() => onConnect(entry)}
      onKeyDown={(event) => {
        if (event.key === "Enter") onConnect(entry);
      }}
      onContextMenu={(event) => onMenu(event, entry)}
    >
      <div className={classes.cell}>
        <div className={classes.lines}>
          <span className={classes.primary}>{dayjs(entry.startedAt).format("MMM DD, YYYY")}</span>
          <span className={classes.secondary}>{timeRange(entry)}</span>
        </div>
      </div>
      <div className={classes.cell}>
        <span className={classes.avatar}>{user.slice(0, 1)}</span>
        <div className={classes.lines}>
          <span className={classes.primary}>{user}</span>
          <span className={classes.secondary}>This device</span>
        </div>
      </div>
      <div className={classes.cell}>
        <span className={classes.hostTile} style={{ background: host?.color ?? undefined }}>
          <ServerGlyph size={14} />
        </span>
        <div className={classes.lines}>
          <span className={classes.primary}>{host?.label ?? entry.label}</span>
          <span className={classes.secondary}>{username ? `ssh, ${username}` : "ssh"}</span>
        </div>
      </div>
      <div className={classes.cell}>
        <Tooltip label={host ? "Saved in your vault" : "Save as host"}>
          <button
            type="button"
            className={classes.saved}
            aria-label={host ? "Saved host" : "Save as host"}
            onClick={(event) => {
              event.stopPropagation();
              if (!host) onSave(entry);
            }}
          >
            {host ? <IconBookmarkFilled size={14} /> : <IconBookmark size={14} stroke={2.2} />}
          </button>
        </Tooltip>
      </div>
    </div>
  );
}

export function LogsPage() {
  const entries = useConnectionLogs((state) => state.entries);
  const removeEntry = useConnectionLogs((state) => state.remove);
  const clear = useConnectionLogs((state) => state.clear);
  const { data: hosts = [] } = useHosts();
  const { data: account } = useAccountStatus();
  const saveHost = useSaveHost();
  const openSsh = useTabs((state) => state.openSsh);
  const [newestFirst, setNewestFirst] = useState(true);
  const menu = useContextMenu<ConnectionLog>();
  const user = account?.signedIn && account.email ? account.email : "You";
  const sorted = newestFirst ? entries : [...entries].reverse();
  const hostFor = (entry: ConnectionLog) => hosts.find((host) => host.id === entry.hostId);

  const connect = (entry: ConnectionLog) => {
    const host = hostFor(entry);
    if (host) openSsh(host);
  };

  const save = (entry: ConnectionLog) =>
    saveHost.mutate({
      id: null,
      label: entry.label,
      address: entry.address,
      port: entry.port,
      groupId: null,
      identityId: null,
      username: entry.username,
      password: { action: "keep" },
      keyId: null,
      jumpHostId: null,
      startupSnippetId: null,
      tags: [],
      color: null,
    });

  if (entries.length === 0) {
    return (
      <div className={classes.page}>
        <EmptyBlock
          icon={<IconClockFilled size={24} />}
          title="No connection logs"
          description="Every SSH session you open shows up here, with when it started and ended."
        />
      </div>
    );
  }

  return (
    <div className={classes.page}>
      <div className={`${classes.columns} ${classes.header}`}>
        <div className={classes.headerCell}>
          <button
            type="button"
            className={classes.sortButton}
            onClick={() => setNewestFirst((value) => !value)}
          >
            Date
            {newestFirst ? <IconSortDescending size={16} /> : <IconSortAscending size={16} />}
          </button>
        </div>
        <div className={classes.headerCell}>User</div>
        <div className={classes.headerCell}>Host</div>
        <div className={classes.headerCell}>
          Saved
          <IconBookmark size={15} color="var(--x-icon-muted)" stroke={2} />
        </div>
      </div>
      <div className={classes.body}>
        <div className={classes.list}>
          {sorted.map((entry) => (
            <LogRow
              key={entry.id}
              entry={entry}
              host={hostFor(entry)}
              user={user}
              onConnect={connect}
              onSave={save}
              onMenu={menu.open}
            />
          ))}
        </div>
      </div>
      <ContextMenu position={menu.position} onClose={menu.close}>
        {menu.target && (
          <LogMenuItems
            entry={menu.target}
            canConnect={Boolean(hostFor(menu.target))}
            onConnect={connect}
            onRemove={(entry) => removeEntry(entry.id)}
            onClear={() => confirmDanger("Clear logs", "Remove every connection log on this device?", clear)}
          />
        )}
      </ContextMenu>
    </div>
  );
}

interface LogMenuProps {
  entry: ConnectionLog;
  canConnect: boolean;
  onConnect: (entry: ConnectionLog) => void;
  onRemove: (entry: ConnectionLog) => void;
  onClear: () => void;
}

function LogMenuItems({ entry, canConnect, onConnect, onRemove, onClear }: LogMenuProps) {
  return (
    <>
      <Menu.Item
        leftSection={<IconPlayerPlay size={16} />}
        disabled={!canConnect}
        onClick={() => onConnect(entry)}
      >
        Connect
      </Menu.Item>
      <Menu.Item leftSection={<IconTrash size={16} />} onClick={() => onRemove(entry)}>
        Remove from logs
      </Menu.Item>
      <Menu.Divider />
      <Menu.Item color="red" leftSection={<IconTrashX size={16} />} onClick={onClear}>
        Clear all logs
      </Menu.Item>
    </>
  );
}
