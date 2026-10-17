import { useState } from "react";
import { useNavigation } from "@/application/stores/navigation";
import { SFTP_TAB_ID, VAULT_TAB_ID, useTabs } from "@/application/stores/tabs";
import { SidePanelSlotContext, sidePanelSlotClass } from "@/presentation/common/side-panel";
import { ForwardsPage } from "@/presentation/forwards/forwards-page";
import { HostsPage } from "@/presentation/hosts/hosts-page";
import { KeychainPage } from "@/presentation/keychain/keychain-page";
import { KnownHostsPage } from "@/presentation/known-hosts/known-hosts-page";
import { LogsPage } from "@/presentation/logs/logs-page";
import { SettingsPage } from "@/presentation/settings/settings-page";
import { CommandPalette } from "@/presentation/shell/command-palette";
import { Nav } from "@/presentation/shell/nav";
import classes from "@/presentation/shell/shell.module.css";
import { TabBar } from "@/presentation/shell/tab-bar";
import { SftpView } from "@/presentation/sftp/sftp-view";
import { SnippetsPage } from "@/presentation/snippets/snippets-page";
import { TerminalView } from "@/presentation/terminal/terminal-view";

function SectionContent() {
  const section = useNavigation((state) => state.section);
  switch (section) {
    case "hosts":
      return <HostsPage />;
    case "keychain":
      return <KeychainPage />;
    case "forwards":
      return <ForwardsPage />;
    case "snippets":
      return <SnippetsPage />;
    case "knownHosts":
      return <KnownHostsPage />;
    case "logs":
      return <LogsPage />;
    case "settings":
      return <SettingsPage />;
  }
}

function paneClass(active: boolean) {
  return active ? classes.pane : `${classes.pane} ${classes.paneHidden}`;
}

export function MainShell() {
  const tabs = useTabs((state) => state.tabs);
  const activeId = useTabs((state) => state.activeId);
  const [panelSlot, setPanelSlot] = useState<HTMLDivElement | null>(null);

  return (
    <div className={classes.root}>
      <TabBar />
      <div className={classes.content}>
        <div className={paneClass(activeId === VAULT_TAB_ID)}>
          <Nav />
          <SidePanelSlotContext.Provider value={panelSlot}>
            <div className={classes.vaultPage}>
              <SectionContent />
            </div>
          </SidePanelSlotContext.Provider>
          <div ref={setPanelSlot} className={sidePanelSlotClass} />
        </div>
        <div className={paneClass(activeId === SFTP_TAB_ID)}>
          <SftpView />
        </div>
        {tabs.map((tab) => (
          <div key={tab.id} className={paneClass(activeId === tab.id)}>
            <TerminalView tab={tab} active={activeId === tab.id} />
          </div>
        ))}
      </div>
      <CommandPalette />
    </div>
  );
}
