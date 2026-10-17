import { Spotlight, type SpotlightActionGroupData } from "@mantine/spotlight";
import {
  IconBroadcast,
  IconFolder,
  IconLock,
  IconSearch,
  IconTerminal,
  IconTerminal2,
} from "@tabler/icons-react";
import { useMemo } from "react";
import { useHosts, useIdentities } from "@/application/hooks/entities";
import { useVaultLock } from "@/application/hooks/vault";
import { hostSubtitle } from "@/application/format";
import { useNavigation } from "@/application/stores/navigation";
import { VAULT_TAB_ID, useTabs } from "@/application/stores/tabs";
import { navItems, settingsItem } from "@/presentation/shell/nav";

export function CommandPalette() {
  const { data: hosts = [] } = useHosts();
  const { data: identities = [] } = useIdentities();
  const lock = useVaultLock();

  const actions = useMemo<SpotlightActionGroupData[]>(() => {
    const tabs = useTabs.getState();
    return [
      {
        group: "Connect",
        actions: hosts.map((host) => ({
          id: `connect-${host.id}`,
          label: host.label,
          description: hostSubtitle(host, identities),
          keywords: host.tags,
          leftSection: <IconTerminal size={18} />,
          onClick: () => tabs.openSsh(host),
        })),
      },
      {
        group: "SFTP",
        actions: hosts.map((host) => ({
          id: `sftp-${host.id}`,
          label: `SFTP · ${host.label}`,
          description: host.address,
          leftSection: <IconFolder size={18} />,
          onClick: () => tabs.openSftp(host),
        })),
      },
      {
        group: "Go to",
        actions: [...navItems, settingsItem].map((item) => ({
          id: `nav-${item.section}`,
          label: item.label,
          leftSection: <item.icon size={16} />,
          onClick: () => {
            useNavigation.getState().setSection(item.section);
            useTabs.getState().activate(VAULT_TAB_ID);
          },
        })),
      },
      {
        group: "Actions",
        actions: [
          {
            id: "local-terminal",
            label: "Open local terminal",
            leftSection: <IconTerminal2 size={18} />,
            onClick: () => useTabs.getState().openLocal(),
          },
          {
            id: "broadcast",
            label: "Toggle broadcast input",
            leftSection: <IconBroadcast size={18} />,
            onClick: () => useTabs.getState().toggleBroadcast(),
          },
          {
            id: "lock",
            label: "Lock vault",
            leftSection: <IconLock size={18} />,
            onClick: () => lock.mutate(),
          },
        ],
      },
    ];
  }, [hosts, identities, lock]);

  return (
    <Spotlight
      actions={actions}
      shortcut={["mod + K", "mod + P"]}
      nothingFound="Nothing found"
      highlightQuery
      limit={50}
      searchProps={{ leftSection: <IconSearch size={18} />, placeholder: "Search hosts and actions" }}
    />
  );
}
