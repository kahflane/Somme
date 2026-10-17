import { Button, Group, Loader, Menu, TextInput } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { IconFolderFilled, IconPlugConnectedX, IconSearch } from "@tabler/icons-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { errorMessage, notifyError } from "@/application/errors";
import { hostDetail } from "@/application/format";
import { useHosts, useIdentities } from "@/application/hooks/entities";
import { joinPath, pathStyleOf } from "@/application/paths";
import { queryKeys } from "@/application/query-keys";
import { useSftp } from "@/application/stores/sftp";
import type { Host, Id, SftpEntry } from "@/domain/types";
import { api } from "@/infrastructure/tauri/api";
import { confirmAction } from "@/presentation/common/dialogs";
import { EmptyBlock } from "@/presentation/common/empty-state";
import { ServerGlyph, TerminalGlyph } from "@/presentation/common/glyphs";
import { EntityCard } from "@/presentation/common/page";
import {
  FileBrowser,
  fileOps,
  type DragPayload,
  type FileSource,
  type PaneSide,
} from "@/presentation/sftp/file-browser";
import classes from "@/presentation/sftp/sftp.module.css";

type Connection =
  | { status: "idle" }
  | { status: "connecting" }
  | { status: "ready"; sftpId: Id; home: string }
  | { status: "failed"; reason: string };

function plural(count: number, word: string) {
  return `${count} ${word}${count === 1 ? "" : "s"}`;
}

function HostPicker({ onCancel }: { onCancel: () => void }) {
  const { data: hosts = [] } = useHosts();
  const { data: identities = [] } = useIdentities();
  const connect = useSftp((state) => state.connect);
  const [query, setQuery] = useState("");
  const needle = query.toLowerCase();
  const visible = hosts
    .filter(
      (host) => host.label.toLowerCase().includes(needle) || host.address.toLowerCase().includes(needle),
    )
    .sort((left, right) => left.label.localeCompare(right.label));

  return (
    <div className={classes.pane}>
      <div className={classes.paneHeader}>
        <span className={classes.paneTitle}>Select host</span>
        <button type="button" className={classes.headerButton} onClick={onCancel}>
          Cancel
        </button>
      </div>
      <div className={classes.picker}>
        <TextInput
          autoFocus
          placeholder="Search hosts"
          leftSection={<IconSearch size={16} />}
          value={query}
          onChange={(event) => setQuery(event.currentTarget.value)}
        />
        <div className={classes.pickerList}>
          {visible.map((host) => (
            <EntityCard
              key={host.id}
              icon={<ServerGlyph size={22} />}
              tileColor={host.color}
              title={host.label}
              subtitle={hostDetail(host, identities)}
              onClick={() => connect(host.id)}
            />
          ))}
          {visible.length === 0 && (
            <div className={classes.status}>
              {hosts.length === 0 ? "No saved hosts yet." : "No hosts match."}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function useRemoteConnection(hostId: Id | null, nonce: number): [Connection, () => void] {
  const [connection, setConnection] = useState<Connection>({ status: "idle" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!hostId) {
      setConnection({ status: "idle" });
      return;
    }
    let disposed = false;
    let opened: Id | null = null;
    setConnection({ status: "connecting" });
    api.sftp
      .open(hostId)
      .then((result) => {
        if (disposed) {
          api.sftp.close(result.sftpId).catch(() => undefined);
          return;
        }
        opened = result.sftpId;
        setConnection({ status: "ready", sftpId: result.sftpId, home: result.home });
      })
      .catch((error) => {
        if (!disposed) setConnection({ status: "failed", reason: errorMessage(error) });
      });
    return () => {
      disposed = true;
      if (opened) api.sftp.close(opened).catch(() => undefined);
    };
  }, [hostId, nonce, attempt]);

  return [connection, () => setAttempt((value) => value + 1)];
}

export function SftpView() {
  const client = useQueryClient();
  const { data: hosts = [] } = useHosts();
  const hostId = useSftp((state) => state.remoteHostId);
  const nonce = useSftp((state) => state.nonce);
  const disconnect = useSftp((state) => state.disconnect);
  const [connection, retry] = useRemoteConnection(hostId, nonce);
  const [picking, setPicking] = useState(false);
  const [localPath, setLocalPath] = useState("");
  const [remotePath, setRemotePath] = useState("");
  const [busy, setBusy] = useState<PaneSide | null>(null);
  const [dragging, setDragging] = useState<DragPayload | null>(null);
  const host: Host | undefined = hosts.find((item) => item.id === hostId);

  const home = useQuery({ queryKey: queryKeys.localHome, queryFn: api.localFs.home, staleTime: Infinity });

  useEffect(() => {
    if (home.data && !localPath) setLocalPath(home.data);
  }, [home.data, localPath]);

  useEffect(() => {
    if (connection.status === "ready") setRemotePath(connection.home);
  }, [connection]);

  useEffect(() => {
    if (hostId) setPicking(false);
  }, [hostId, nonce]);

  const localSource = useMemo<FileSource>(
    () => ({ kind: "local", style: pathStyleOf(home.data ?? "/") }),
    [home.data],
  );
  const sftpId = connection.status === "ready" ? connection.sftpId : null;
  const remoteSource = useMemo<FileSource | null>(
    () => (sftpId ? { kind: "remote", sftpId } : null),
    [sftpId],
  );

  const transfer = (from: PaneSide, entries: SftpEntry[]) => {
    setDragging(null);
    if (!sftpId || !remoteSource || !localPath || !remotePath) return;
    const files = entries.filter((entry) => !entry.isDir);
    if (files.length === 0) {
      notifications.show({
        color: "orange",
        message: "Copying folders is not supported yet. Pick files instead.",
      });
      return;
    }
    const toRemote = from === "left";
    const target = toRemote ? fileOps(remoteSource) : fileOps(localSource);
    const targetPath = toRemote ? remotePath : localPath;
    const existing = client.getQueryData<SftpEntry[]>(target.queryKey(targetPath)) ?? [];
    const clashes = files.filter((file) => existing.some((item) => item.name === file.name));
    const skipped = entries.length - files.length;

    const copy = async () => {
      setBusy(toRemote ? "right" : "left");
      try {
        for (const file of files) {
          if (toRemote) await api.sftp.upload(sftpId, file.path, remotePath);
          else await api.sftp.download(sftpId, file.path, joinPath(localPath, file.name, target.style));
        }
        const note = skipped > 0 ? ` (${plural(skipped, "folder")} skipped)` : "";
        notifications.show({
          color: "blue",
          message: `Copied ${plural(files.length, "file")} to ${toRemote ? (host?.label ?? "host") : "Local"}${note}`,
        });
      } catch (error) {
        notifyError(error, "Transfer failed");
      } finally {
        setBusy(null);
        client.invalidateQueries({ queryKey: target.queryKey(targetPath) });
      }
    };

    if (clashes.length > 0) {
      confirmAction(
        "Replace files?",
        clashes.length === 1
          ? `"${clashes[0].name}" already exists in the destination. Replace it?`
          : `${clashes.length} files already exist in the destination. Replace them?`,
        "Replace",
        () => {
          copy();
        },
      );
    } else {
      copy();
    }
  };

  const remoteLabel = host?.label ?? "host";

  const rightPane = () => {
    if (picking) return <HostPicker onCancel={() => setPicking(false)} />;
    if (connection.status === "idle") {
      return (
        <div className={`${classes.pane} ${classes.paneEmpty}`}>
          <EmptyBlock
            narrow
            icon={<IconFolderFilled size={24} />}
            title="Connect to host"
            description="Start by connecting to a saved host to manage your files with SFTP."
            action={
              <button type="button" className={classes.selectHost} onClick={() => setPicking(true)}>
                Select host
              </button>
            }
          />
        </div>
      );
    }
    if (connection.status === "connecting") {
      return (
        <div className={classes.pane}>
          <div className={classes.failed}>
            <Loader size="sm" />
            <div className={classes.failedText}>Connecting to {remoteLabel}…</div>
          </div>
        </div>
      );
    }
    if (connection.status === "failed") {
      return (
        <div className={classes.pane}>
          <div className={classes.failed}>
            <div className={classes.failedText}>
              Could not open SFTP on {remoteLabel}: {connection.reason}
            </div>
            <Group gap="xs">
              <Button onClick={retry}>Try again</Button>
              <Button variant="default" onClick={() => setPicking(true)}>
                Choose another host
              </Button>
            </Group>
          </div>
        </div>
      );
    }
    if (!remoteSource || !remotePath) return null;
    return (
      <FileBrowser
        key={connection.sftpId}
        side="right"
        title={remoteLabel}
        tile={<ServerGlyph size={14} />}
        tileColor={host?.color}
        source={remoteSource}
        path={remotePath}
        busy={busy === "right"}
        copyLabel="Local"
        dragging={dragging}
        onNavigate={setRemotePath}
        onCopy={(entries) => transfer("right", entries)}
        onDragStart={setDragging}
        onDrop={(payload) => transfer(payload.side, payload.entries)}
        actions={
          <>
            <Menu.Item leftSection={<ServerGlyph size={16} />} onClick={() => setPicking(true)}>
              Switch host
            </Menu.Item>
            <Menu.Item leftSection={<IconPlugConnectedX size={16} />} onClick={disconnect}>
              Disconnect
            </Menu.Item>
          </>
        }
      />
    );
  };

  return (
    <div className={classes.root}>
      {localPath ? (
        <FileBrowser
          side="left"
          title="Local"
          tile={<TerminalGlyph size={14} />}
          source={localSource}
          path={localPath}
          busy={busy === "left"}
          copyLabel={sftpId ? remoteLabel : null}
          dragging={dragging}
          onNavigate={setLocalPath}
          onCopy={(entries) => transfer("left", entries)}
          onDragStart={setDragging}
          onDrop={(payload) => transfer(payload.side, payload.entries)}
        />
      ) : (
        <div className={classes.pane}>
          <div className={classes.status}>{home.error ? errorMessage(home.error) : <Loader size="sm" />}</div>
        </div>
      )}
      <div className={classes.divider} />
      {rightPane()}
    </div>
  );
}
