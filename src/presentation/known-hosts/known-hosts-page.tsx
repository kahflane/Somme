import { Button, Menu, TextInput, Textarea } from "@mantine/core";
import { IconFingerprint, IconTrash } from "@tabler/icons-react";
import dayjs from "dayjs";
import { useState } from "react";
import { useDeleteKnownHost, useKnownHosts } from "@/application/hooks/entities";
import { sortItems, useSortOrder, useViewMode } from "@/application/stores/view-modes";
import type { KnownHost } from "@/domain/types";
import { ContextMenu, useContextMenu } from "@/presentation/common/context-menu";
import { confirmDanger } from "@/presentation/common/dialogs";
import { EmptyBlock } from "@/presentation/common/empty-state";
import {
  CardGrid,
  EntityCard,
  Muted,
  Page,
  PageSection,
  SearchToggle,
  SortMenu,
  ViewToggle,
} from "@/presentation/common/page";
import { PanelSection, SidePanel } from "@/presentation/common/side-panel";

function hostName(item: KnownHost) {
  return item.port === 22 ? item.host : `[${item.host}]:${item.port}`;
}

const monospace = { input: { fontFamily: "var(--mantine-font-family-monospace)", fontSize: 12 } };

export function KnownHostsPage() {
  const { data: knownHosts = [] } = useKnownHosts();
  const remove = useDeleteKnownHost();
  const [view] = useViewMode("knownHosts");
  const [sort] = useSortOrder("knownHosts");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<KnownHost | null>(null);
  const menu = useContextMenu<KnownHost>();
  const visible = sortItems(knownHosts, sort, hostName).filter((item) =>
    hostName(item).toLowerCase().includes(query.toLowerCase()),
  );

  const forget = (item: KnownHost) =>
    confirmDanger(
      "Remove known host",
      `Forget the key for ${hostName(item)}? You will be asked to trust it again on the next connect.`,
      () => {
        remove.mutate(item.id);
        setSelected(null);
      },
    );

  return (
    <>
      <Page
        fill={knownHosts.length === 0}
        tools={
          <>
            <SearchToggle query={query} onQuery={setQuery} />
            <ViewToggle page="knownHosts" />
            <SortMenu page="knownHosts" />
          </>
        }
      >
        {knownHosts.length === 0 ? (
          <EmptyBlock
            icon={<IconFingerprint size={26} stroke={2} />}
            title="No known hosts"
            description="Server keys show up here after you trust them on the first connect."
          />
        ) : (
          <PageSection title="Known Hosts">
            {visible.length === 0 ? (
              <Muted>No known hosts match your search.</Muted>
            ) : (
              <CardGrid view={view}>
                {visible.map((item) => (
                  <EntityCard
                    key={item.id}
                    icon={<IconFingerprint size={26} stroke={2} />}
                    title={hostName(item)}
                    active={selected?.id === item.id}
                    onClick={() => setSelected(item)}
                    onContextMenu={(event) => menu.open(event, item)}
                  />
                ))}
              </CardGrid>
            )}
          </PageSection>
        )}
      </Page>
      <ContextMenu position={menu.position} onClose={menu.close}>
        {menu.target && <KnownHostMenuItems item={menu.target} onOpen={setSelected} onForget={forget} />}
      </ContextMenu>
      <SidePanel
        opened={selected !== null}
        title="Known Host"
        onClose={() => setSelected(null)}
        menu={
          selected && (
            <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => forget(selected)}>
              Remove
            </Menu.Item>
          )
        }
        footer={
          selected && (
            <Button variant="default" color="red" c="var(--x-danger)" onClick={() => forget(selected)}>
              Forget host key
            </Button>
          )
        }
      >
        {selected && (
          <>
            <PanelSection title="Host">
              <TextInput label="Address" value={selected.host} readOnly />
              <TextInput label="Port" value={String(selected.port)} readOnly />
            </PanelSection>
            <PanelSection title="Key">
              <TextInput label="Type" value={selected.keyType} readOnly />
              <Textarea
                label="Fingerprint"
                value={selected.fingerprint}
                readOnly
                autosize
                styles={monospace}
              />
              <TextInput
                label="Trusted on"
                value={dayjs(selected.createdAt).format("MMM D, YYYY HH:mm")}
                readOnly
              />
            </PanelSection>
          </>
        )}
      </SidePanel>
    </>
  );
}

interface KnownHostMenuProps {
  item: KnownHost;
  onOpen: (item: KnownHost) => void;
  onForget: (item: KnownHost) => void;
}

function KnownHostMenuItems({ item, onOpen, onForget }: KnownHostMenuProps) {
  return (
    <>
      <Menu.Item leftSection={<IconFingerprint size={16} />} onClick={() => onOpen(item)}>
        Show details
      </Menu.Item>
      <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onForget(item)}>
        Remove
      </Menu.Item>
    </>
  );
}
