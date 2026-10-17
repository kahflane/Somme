import { Button, Menu, Popover, Tooltip } from "@mantine/core";
import { spotlight } from "@mantine/spotlight";
import {
  IconBell,
  IconBroadcast,
  IconChevronDown,
  IconCloud,
  IconCloudOff,
  IconFolderFilled,
  IconLock,
  IconPlus,
  IconRefresh,
  IconSettings,
  IconX,
} from "@tabler/icons-react";
import { useAccountStatus } from "@/application/hooks/account";
import { useHosts } from "@/application/hooks/entities";
import { useSyncNow, useSyncStatus } from "@/application/hooks/sync";
import { useVaultLock } from "@/application/hooks/vault";
import { useNavigation } from "@/application/stores/navigation";
import { SFTP_TAB_ID, VAULT_TAB_ID, useTabs, type WorkTab } from "@/application/stores/tabs";
import type { Host } from "@/domain/types";
import { ServerGlyph, TerminalGlyph, VaultGlyph } from "@/presentation/common/glyphs";
import classes from "@/presentation/shell/shell.module.css";
import { useTrafficLightInset } from "@/presentation/shell/use-window-inset";

function tabClass(active: boolean, extra = "") {
  return `${classes.tab} ${active ? classes.tabActive : ""} ${extra}`;
}

function openSettings() {
  useNavigation.getState().setSection("settings");
  useTabs.getState().activate(VAULT_TAB_ID);
}

function VaultTab({ active }: { active: boolean }) {
  const activate = useTabs((state) => state.activate);
  const lock = useVaultLock();
  const { data: account } = useAccountStatus();

  return (
    <div className={classes.vaultTab}>
      <button
        type="button"
        className={tabClass(active, classes.fixedTab)}
        onClick={() => activate(VAULT_TAB_ID)}
      >
        <VaultGlyph size={16} />
        <span className={classes.tabTitle}>Vaults</span>
      </button>
      {active && (
        <Menu position="bottom-start" width={220}>
          <Menu.Target>
            <button type="button" className={tabClass(true, classes.vaultChevron)} aria-label="Vault menu">
              <IconChevronDown size={16} stroke={2.4} />
            </button>
          </Menu.Target>
          <Menu.Dropdown>
            <Menu.Label>{account?.signedIn ? account.email : "Personal vault · this device"}</Menu.Label>
            <Menu.Item leftSection={<IconSettings size={16} />} onClick={openSettings}>
              Settings
            </Menu.Item>
            <Menu.Item leftSection={<IconLock size={16} />} onClick={() => lock.mutate()}>
              Lock vault
            </Menu.Item>
          </Menu.Dropdown>
        </Menu>
      )}
    </div>
  );
}

function WorkTabButton({ tab, host, active }: { tab: WorkTab; host: Host | undefined; active: boolean }) {
  const activate = useTabs((state) => state.activate);
  const close = useTabs((state) => state.close);
  return (
    <div
      role="tab"
      tabIndex={0}
      aria-selected={active}
      className={tabClass(active, classes.workTab)}
      onClick={() => activate(tab.id)}
      onKeyDown={(event) => {
        if (event.key === "Enter") activate(tab.id);
      }}
      onAuxClick={(event) => {
        if (event.button === 1) close(tab.id);
      }}
    >
      <span className={classes.tabTile} style={{ background: host?.color ?? undefined }}>
        {tab.kind === "local" ? <TerminalGlyph size={12} /> : <ServerGlyph size={12} />}
      </span>
      <span className={classes.tabTitle}>{tab.title}</span>
      <span
        role="button"
        tabIndex={-1}
        aria-label={`Close ${tab.title}`}
        className={classes.tabClose}
        onClick={(event) => {
          event.stopPropagation();
          close(tab.id);
        }}
      >
        <IconX size={14} stroke={2.2} />
      </span>
    </div>
  );
}

function SyncBell() {
  const { data: sync } = useSyncStatus();
  const { data: account } = useAccountStatus();
  const syncNow = useSyncNow();
  const failed = Boolean(sync?.enabled && sync.lastError);
  const title = !sync?.enabled ? "Local only" : sync.syncing ? "Syncing…" : failed ? "Sync paused" : "Synced";
  const detail = !sync?.enabled
    ? "Sign in to sync hosts, keys and snippets."
    : failed
      ? sync.lastError
      : (account?.email ?? "Somme Cloud");

  return (
    <Popover position="bottom-end" width={280}>
      <Popover.Target>
        <button type="button" className={classes.barIcon} aria-label="Notifications">
          <IconBell size={18} stroke={1.8} />
          {failed && <span className={classes.dot} />}
        </button>
      </Popover.Target>
      <Popover.Dropdown>
        <div className={classes.syncCard}>
          <div className={classes.syncRow}>
            {sync?.enabled ? (
              <IconCloud size={20} color={failed ? "var(--x-warning)" : "var(--x-success)"} />
            ) : (
              <IconCloudOff size={20} color="var(--x-text-secondary)" />
            )}
            <div>
              <div className={classes.syncTitle}>{title}</div>
              <div className={classes.syncText}>{detail}</div>
            </div>
          </div>
          {sync?.enabled ? (
            <Button
              variant="default"
              size="xs"
              leftSection={<IconRefresh size={14} />}
              loading={syncNow.isPending || sync.syncing}
              onClick={() => syncNow.mutate(undefined)}
            >
              Sync now
            </Button>
          ) : (
            <Button variant="default" size="xs" onClick={openSettings}>
              Open settings
            </Button>
          )}
        </div>
      </Popover.Dropdown>
    </Popover>
  );
}

export function TabBar() {
  const tabs = useTabs((state) => state.tabs);
  const activeId = useTabs((state) => state.activeId);
  const activate = useTabs((state) => state.activate);
  const broadcast = useTabs((state) => state.broadcast);
  const toggleBroadcast = useTabs((state) => state.toggleBroadcast);
  const { data: hosts = [] } = useHosts();
  const inset = useTrafficLightInset();

  return (
    <header className={classes.titleBar} data-inset={inset || undefined} data-tauri-drag-region>
      <div className={classes.tabs} role="tablist">
        <VaultTab active={activeId === VAULT_TAB_ID} />
        <button
          type="button"
          className={tabClass(activeId === SFTP_TAB_ID, classes.fixedTab)}
          onClick={() => activate(SFTP_TAB_ID)}
        >
          <IconFolderFilled size={16} />
          <span className={classes.tabTitle}>SFTP</span>
        </button>
        {tabs.map((tab) => (
          <WorkTabButton
            key={tab.id}
            tab={tab}
            host={hosts.find((host) => host.id === tab.hostId)}
            active={activeId === tab.id}
          />
        ))}
      </div>
      <Tooltip label="New tab">
        <button
          type="button"
          className={classes.barIcon}
          onClick={() => spotlight.open()}
          aria-label="New tab"
        >
          <IconPlus size={18} stroke={2} />
        </button>
      </Tooltip>
      <div className={classes.dragSpace} data-tauri-drag-region />
      {(broadcast || tabs.length > 1) && (
        <Tooltip label={broadcast ? "Broadcast input is on" : "Broadcast input to all terminals"}>
          <button
            type="button"
            className={classes.barIcon}
            data-on={broadcast || undefined}
            onClick={toggleBroadcast}
            aria-label="Broadcast input"
            aria-pressed={broadcast}
          >
            <IconBroadcast size={18} stroke={1.8} />
          </button>
        </Tooltip>
      )}
      <SyncBell />
    </header>
  );
}
