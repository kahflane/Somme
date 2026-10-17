import { Loader, Menu, TextInput } from "@mantine/core";
import {
  IconArrowsLeftRight,
  IconChevronDown,
  IconChevronLeft,
  IconChevronRight,
  IconFileFilled,
  IconFolderFilled,
  IconFolderPlus,
  IconLink,
  IconPencil,
  IconRefresh,
  IconSearch,
  IconTrash,
} from "@tabler/icons-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import dayjs from "dayjs";
import { useEffect, useMemo, useRef, useState, type DragEvent, type MouseEvent, type ReactNode } from "react";
import { errorMessage, notifyError } from "@/application/errors";
import { formatBytes } from "@/application/format";
import { isRoot, joinPath, parentPath, pathCrumbs, type PathStyle } from "@/application/paths";
import { queryKeys } from "@/application/query-keys";
import type { Id, SftpEntry } from "@/domain/types";
import { api } from "@/infrastructure/tauri/api";
import { ContextMenu, useContextMenu } from "@/presentation/common/context-menu";
import { confirmDanger, promptText } from "@/presentation/common/dialogs";
import classes from "@/presentation/sftp/sftp.module.css";

export type FileSource = { kind: "local"; style: PathStyle } | { kind: "remote"; sftpId: Id };

export type PaneSide = "left" | "right";

export interface DragPayload {
  side: PaneSide;
  entries: SftpEntry[];
}

interface FileOps {
  style: PathStyle;
  queryKey: (path: string) => readonly unknown[];
  list: (path: string) => Promise<SftpEntry[]>;
  mkdir: (path: string) => Promise<void>;
  rename: (from: string, to: string) => Promise<void>;
  remove: (path: string, isDir: boolean) => Promise<void>;
}

export function fileOps(source: FileSource): FileOps {
  if (source.kind === "local") {
    return {
      style: source.style,
      queryKey: queryKeys.localFiles,
      list: api.localFs.list,
      mkdir: api.localFs.mkdir,
      rename: api.localFs.rename,
      remove: api.localFs.remove,
    };
  }
  const { sftpId } = source;
  return {
    style: "posix",
    queryKey: (path) => queryKeys.sftp(sftpId, path),
    list: (path) => api.sftp.list(sftpId, path),
    mkdir: (path) => api.sftp.mkdir(sftpId, path),
    rename: (from, to) => api.sftp.rename(sftpId, from, to),
    remove: (path, isDir) => api.sftp.remove(sftpId, path, isDir),
  };
}

function kindOf(entry: SftpEntry) {
  if (entry.isDir) return "folder";
  const dot = entry.name.lastIndexOf(".");
  if (dot > 0 && dot < entry.name.length - 1) return entry.name.slice(dot + 1).toLowerCase();
  return entry.isSymlink ? "alias" : "file";
}

interface Props {
  side: PaneSide;
  title: string;
  tile: ReactNode;
  tileColor?: string | null;
  source: FileSource;
  path: string;
  busy: boolean;
  copyLabel: string | null;
  dragging: DragPayload | null;
  onNavigate: (path: string) => void;
  onCopy: (entries: SftpEntry[]) => void;
  onDragStart: (payload: DragPayload | null) => void;
  onDrop: (payload: DragPayload) => void;
  actions?: ReactNode;
}

export function FileBrowser({
  side,
  title,
  tile,
  tileColor,
  source,
  path,
  busy,
  copyLabel,
  dragging,
  onNavigate,
  onCopy,
  onDragStart,
  onDrop,
  actions,
}: Props) {
  const client = useQueryClient();
  const ops = useMemo(() => fileOps(source), [source]);
  const [back, setBack] = useState<string[]>([]);
  const [forward, setForward] = useState<string[]>([]);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [filterOpen, setFilterOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const [showHidden, setShowHidden] = useState(false);
  const [dropActive, setDropActive] = useState(false);
  const menu = useContextMenu<SftpEntry>();
  const crumbsRef = useRef<HTMLDivElement>(null);

  const listing = useQuery({
    queryKey: ops.queryKey(path),
    queryFn: () => ops.list(path),
    enabled: Boolean(path),
    staleTime: 0,
  });

  useEffect(() => {
    setSelected(new Set());
    const crumbBar = crumbsRef.current;
    if (crumbBar) crumbBar.scrollLeft = crumbBar.scrollWidth;
  }, [path]);

  const entries = useMemo(() => {
    const needle = filter.toLowerCase();
    return (listing.data ?? []).filter(
      (entry) => (showHidden || !entry.name.startsWith(".")) && entry.name.toLowerCase().includes(needle),
    );
  }, [listing.data, filter, showHidden]);

  const selectedEntries = entries.filter((entry) => selected.has(entry.path));
  const refresh = () => client.invalidateQueries({ queryKey: ops.queryKey(path) });

  const open = (next: string) => {
    if (next === path) return;
    setBack((items) => [...items, path]);
    setForward([]);
    onNavigate(next);
  };

  const goBack = () => {
    const previous = back[back.length - 1];
    if (previous === undefined) return;
    setBack((items) => items.slice(0, -1));
    setForward((items) => [path, ...items]);
    onNavigate(previous);
  };

  const goForward = () => {
    const next = forward[0];
    if (next === undefined) return;
    setForward((items) => items.slice(1));
    setBack((items) => [...items, path]);
    onNavigate(next);
  };

  const run = async (action: () => Promise<void>) => {
    try {
      await action();
    } catch (error) {
      notifyError(error);
    } finally {
      refresh();
    }
  };

  const mkdir = async () => {
    const name = await promptText({ title: "New folder", label: "Folder name", confirmLabel: "Create" });
    if (name) await run(() => ops.mkdir(joinPath(path, name, ops.style)));
  };

  const rename = async (entry: SftpEntry) => {
    const name = await promptText({
      title: "Rename",
      label: "New name",
      initial: entry.name,
      confirmLabel: "Rename",
    });
    if (name && name !== entry.name) await run(() => ops.rename(entry.path, joinPath(path, name, ops.style)));
  };

  const remove = (targets: SftpEntry[]) => {
    const [first] = targets;
    if (!first) return;
    const message =
      targets.length === 1
        ? `Delete "${first.name}"${first.isDir ? " and everything inside it" : ""}?`
        : `Delete ${targets.length} items? Folders are deleted with everything inside them.`;
    confirmDanger(targets.length === 1 && first.isDir ? "Delete folder" : "Delete", message, () => {
      run(async () => {
        for (const entry of targets) await ops.remove(entry.path, entry.isDir);
      });
    });
  };

  const targetsFor = (entry: SftpEntry) => (selected.has(entry.path) ? selectedEntries : [entry]);

  const select = (event: MouseEvent, entry: SftpEntry) => {
    if (event.metaKey || event.ctrlKey) {
      setSelected((current) => {
        const next = new Set(current);
        if (next.has(entry.path)) next.delete(entry.path);
        else next.add(entry.path);
        return next;
      });
    } else {
      setSelected(new Set([entry.path]));
    }
  };

  const dragStart = (event: DragEvent, entry: SftpEntry) => {
    const targets = targetsFor(entry);
    if (!selected.has(entry.path)) setSelected(new Set([entry.path]));
    event.dataTransfer.effectAllowed = "copy";
    event.dataTransfer.setData("text/plain", targets.map((item) => item.path).join("\n"));
    onDragStart({ side, entries: targets });
  };

  const acceptsDrop = Boolean(dragging && dragging.side !== side && copyLabel !== null);

  const crumbs = pathCrumbs(path, ops.style);

  return (
    <div className={classes.pane}>
      <div className={classes.paneHeader}>
        <span className={classes.paneTile} style={{ background: tileColor ?? undefined }}>
          {tile}
        </span>
        <span className={classes.paneTitle}>{title}</span>
        {busy && <Loader size={14} />}
        {filterOpen ? (
          <TextInput
            className={classes.filter}
            autoFocus
            placeholder="Filter"
            leftSection={<IconSearch size={14} />}
            value={filter}
            onChange={(event) => setFilter(event.currentTarget.value)}
            onBlur={() => {
              if (!filter) setFilterOpen(false);
            }}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                setFilter("");
                setFilterOpen(false);
              }
            }}
          />
        ) : (
          <button type="button" className={classes.headerButton} onClick={() => setFilterOpen(true)}>
            <IconSearch size={18} stroke={2.2} />
            Filter
          </button>
        )}
        <Menu position="bottom-end" width={230}>
          <Menu.Target>
            <button type="button" className={classes.headerButton}>
              Actions
              <IconChevronDown size={16} stroke={2.4} />
            </button>
          </Menu.Target>
          <Menu.Dropdown>
            <Menu.Item leftSection={<IconFolderPlus size={16} />} onClick={mkdir}>
              New folder
            </Menu.Item>
            <Menu.Item leftSection={<IconRefresh size={16} />} onClick={refresh}>
              Refresh
            </Menu.Item>
            <Menu.Item
              leftSection={<IconArrowsLeftRight size={16} />}
              disabled={!copyLabel || selectedEntries.length === 0}
              onClick={() => onCopy(selectedEntries)}
            >
              {copyLabel ? `Copy selected to ${copyLabel}` : "Copy selected"}
            </Menu.Item>
            <Menu.Item
              rightSection={showHidden ? "✓" : null}
              onClick={() => setShowHidden((value) => !value)}
            >
              Show hidden files
            </Menu.Item>
            {actions && (
              <>
                <Menu.Divider />
                {actions}
              </>
            )}
          </Menu.Dropdown>
        </Menu>
      </div>
      <div className={classes.crumbBar}>
        <button
          type="button"
          className={classes.navButton}
          disabled={back.length === 0}
          onClick={goBack}
          aria-label="Back"
        >
          <IconChevronLeft size={18} stroke={2.4} />
        </button>
        <button
          type="button"
          className={classes.navButton}
          disabled={forward.length === 0}
          onClick={goForward}
          aria-label="Forward"
        >
          <IconChevronRight size={18} stroke={2.4} />
        </button>
        <div ref={crumbsRef} className={classes.crumbs}>
          <div className={classes.crumbsInner}>
            {crumbs.length === 0 && (
              <button type="button" className={classes.crumb} onClick={() => open(path)}>
                <IconFolderFilled size={18} className={classes.folder} />
                {path}
              </button>
            )}
            {crumbs.map((crumb, index) => (
              <span key={crumb.path} className={classes.crumbsInner}>
                {index > 0 && <IconChevronRight size={16} stroke={2.4} className={classes.crumbSeparator} />}
                <button type="button" className={classes.crumb} onClick={() => open(crumb.path)}>
                  <IconFolderFilled size={18} className={classes.folder} />
                  {crumb.label}
                </button>
              </span>
            ))}
          </div>
        </div>
      </div>
      <div className={`${classes.columns} ${classes.tableHeader}`}>
        <div className={classes.headerCell}>Name</div>
        <div className={classes.headerCell}>Date Modified</div>
        <div className={classes.headerCell}>Size</div>
        <div className={classes.headerCell}>Kind</div>
      </div>
      <div
        className={classes.list}
        data-drop={(acceptsDrop && dropActive) || undefined}
        onClick={(event) => {
          if (event.target === event.currentTarget) setSelected(new Set());
        }}
        onDragOver={(event) => {
          if (!acceptsDrop) return;
          event.preventDefault();
          event.dataTransfer.dropEffect = "copy";
          setDropActive(true);
        }}
        onDragLeave={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDropActive(false);
        }}
        onDrop={(event) => {
          setDropActive(false);
          if (!acceptsDrop || !dragging) return;
          event.preventDefault();
          onDrop(dragging);
        }}
      >
        {listing.isLoading ? (
          <div className={classes.status}>
            <Loader size="sm" />
          </div>
        ) : listing.error ? (
          <div className={classes.status}>{errorMessage(listing.error)}</div>
        ) : (
          <>
            {!isRoot(path, ops.style) && (
              <div
                className={`${classes.columns} ${classes.row}`}
                onDoubleClick={() => open(parentPath(path, ops.style))}
              >
                <div className={classes.nameCell}>
                  <IconFolderFilled size={22} className={classes.folder} />
                  <span className={classes.name}>..</span>
                </div>
              </div>
            )}
            {entries.map((entry) => (
              <div
                key={entry.path}
                className={`${classes.columns} ${classes.row}`}
                data-selected={selected.has(entry.path) || undefined}
                draggable
                onDragStart={(event) => dragStart(event, entry)}
                onDragEnd={() => onDragStart(null)}
                onClick={(event) => select(event, entry)}
                onDoubleClick={() => {
                  if (entry.isDir) open(entry.path);
                  else if (copyLabel) onCopy([entry]);
                }}
                onContextMenu={(event) => {
                  if (!selected.has(entry.path)) setSelected(new Set([entry.path]));
                  menu.open(event, entry);
                }}
              >
                <div className={classes.nameCell}>
                  {entry.isDir ? (
                    <IconFolderFilled size={22} className={classes.folder} />
                  ) : entry.isSymlink ? (
                    <IconLink size={20} className={classes.fileIcon} />
                  ) : (
                    <IconFileFilled size={20} className={classes.fileIcon} />
                  )}
                  <div className={classes.nameLines}>
                    <span className={classes.name}>{entry.name}</span>
                    {entry.permissions && <span className={classes.meta}>{entry.permissions}</span>}
                  </div>
                </div>
                <div className={classes.cell}>
                  {entry.modifiedAt ? dayjs(entry.modifiedAt).format("M/D/YYYY, h:mm A") : "- -"}
                </div>
                <div className={classes.cell}>{entry.isDir ? "- -" : formatBytes(entry.size)}</div>
                <div className={classes.cell}>{kindOf(entry)}</div>
              </div>
            ))}
            {entries.length === 0 && (
              <div className={classes.status}>
                {filter ? "Nothing matches the filter." : "This folder is empty."}
              </div>
            )}
          </>
        )}
      </div>
      <ContextMenu position={menu.position} onClose={menu.close}>
        {menu.target && (
          <EntryMenuItems
            entry={menu.target}
            count={targetsFor(menu.target).length}
            copyLabel={copyLabel}
            onOpen={(entry) => open(entry.path)}
            onCopy={(entry) => onCopy(targetsFor(entry))}
            onRename={rename}
            onDelete={(entry) => remove(targetsFor(entry))}
          />
        )}
      </ContextMenu>
    </div>
  );
}

interface EntryMenuProps {
  entry: SftpEntry;
  count: number;
  copyLabel: string | null;
  onOpen: (entry: SftpEntry) => void;
  onCopy: (entry: SftpEntry) => void;
  onRename: (entry: SftpEntry) => void;
  onDelete: (entry: SftpEntry) => void;
}

function EntryMenuItems({ entry, count, copyLabel, onOpen, onCopy, onRename, onDelete }: EntryMenuProps) {
  return (
    <>
      {entry.isDir && count === 1 && (
        <Menu.Item leftSection={<IconFolderFilled size={16} />} onClick={() => onOpen(entry)}>
          Open
        </Menu.Item>
      )}
      <Menu.Item
        leftSection={<IconArrowsLeftRight size={16} />}
        disabled={!copyLabel}
        onClick={() => onCopy(entry)}
      >
        {copyLabel ? `Copy to ${copyLabel}` : "Copy to other side"}
      </Menu.Item>
      {count === 1 && (
        <Menu.Item leftSection={<IconPencil size={16} />} onClick={() => onRename(entry)}>
          Rename
        </Menu.Item>
      )}
      <Menu.Divider />
      <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(entry)}>
        {count > 1 ? `Delete ${count} items` : "Delete"}
      </Menu.Item>
    </>
  );
}
