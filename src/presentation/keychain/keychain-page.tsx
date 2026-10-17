import { CopyButton, Menu } from "@mantine/core";
import {
  IconCopy,
  IconKey,
  IconPencil,
  IconPencilFilled,
  IconTrash,
  IconUserFilled,
} from "@tabler/icons-react";
import { useState } from "react";
import { keyTypeLabel } from "@/application/format";
import { useDeleteIdentity, useDeleteKey, useIdentities, useKeys } from "@/application/hooks/entities";
import { sortItems, useSortOrder, useViewMode } from "@/application/stores/view-modes";
import type { Identity, SshKey } from "@/domain/types";
import { ContextMenu, useContextMenu } from "@/presentation/common/context-menu";
import { confirmDanger } from "@/presentation/common/dialogs";
import { EmptyBlock } from "@/presentation/common/empty-state";
import { KeyGlyph } from "@/presentation/common/glyphs";
import {
  CardGrid,
  EntityCard,
  Muted,
  Page,
  PageSection,
  SearchToggle,
  SortMenu,
  SplitButton,
  ToolButton,
  ViewToggle,
} from "@/presentation/common/page";
import pageClasses from "@/presentation/common/page.module.css";
import { IdentityPanel } from "@/presentation/keychain/identity-panel";
import { KeyPanel, type KeyDraft, type KeyPanelMode } from "@/presentation/keychain/key-panel";

type Panel = { kind: "key"; mode: KeyPanelMode } | { kind: "identity"; identity: Identity | null } | null;
type MenuTarget = { kind: "key"; key: SshKey } | { kind: "identity"; identity: Identity };

function includes(value: string, query: string) {
  return value.toLowerCase().includes(query.toLowerCase());
}

export function KeychainPage() {
  const { data: keys = [] } = useKeys();
  const { data: identities = [] } = useIdentities();
  const removeKey = useDeleteKey();
  const removeIdentity = useDeleteIdentity();
  const [view] = useViewMode("keychain");
  const [sort] = useSortOrder("keychain");
  const [query, setQuery] = useState("");
  const [panel, setPanel] = useState<Panel>(null);
  const [draft, setDraft] = useState<KeyDraft | null>(null);
  const menu = useContextMenu<MenuTarget>();

  const visibleKeys = sortItems(keys, sort, (key) => key.label).filter(
    (key) => includes(key.label, query) || includes(key.algorithm, query),
  );
  const visibleIdentities = sortItems(identities, sort, (identity) => identity.label).filter(
    (identity) => includes(identity.label, query) || includes(identity.username, query),
  );
  const keyName = (id: string | null) => keys.find((key) => key.id === id)?.label;

  const deleteKey = (key: SshKey) =>
    confirmDanger(
      "Delete key",
      `Delete "${key.label}"? Hosts and identities using it will lose this key.`,
      () => {
        removeKey.mutate(key.id);
        setPanel(null);
      },
    );

  const deleteIdentity = (identity: Identity) =>
    confirmDanger("Delete identity", `Delete "${identity.label}"?`, () => {
      removeIdentity.mutate(identity.id);
      setPanel(null);
    });

  const empty = keys.length === 0 && identities.length === 0 && !draft;
  const editingKey = panel?.kind === "key" && panel.mode.kind === "edit" ? panel.mode.key.id : null;
  // The panel keeps the key it opened with; show the latest copy after a rename.
  const keyMode: KeyPanelMode | null =
    panel?.kind !== "key"
      ? null
      : panel.mode.kind === "edit"
        ? { kind: "edit", key: keys.find((key) => key.id === editingKey) ?? panel.mode.key }
        : panel.mode;
  const editingIdentity = panel?.kind === "identity" ? panel.identity?.id : null;

  return (
    <>
      <Page
        fill={empty}
        toolbar={
          <>
            <SplitButton
              label="New key"
              onClick={() => setPanel({ kind: "key", mode: { kind: "import" } })}
              menu={
                <Menu.Item
                  leftSection={<IconKey size={16} />}
                  onClick={() => setPanel({ kind: "key", mode: { kind: "generate" } })}
                >
                  Generate key
                </Menu.Item>
              }
            />
            <ToolButton
              icon={<IconUserFilled size={18} />}
              onClick={() => setPanel({ kind: "identity", identity: null })}
            >
              Identity
            </ToolButton>
          </>
        }
        tools={
          <>
            <SearchToggle query={query} onQuery={setQuery} />
            <ViewToggle page="keychain" />
            <SortMenu page="keychain" />
          </>
        }
      >
        {empty ? (
          <EmptyBlock
            icon={<KeyGlyph size={24} />}
            title="Set up your keychain"
            description="Generate or import SSH keys, and save identities to reuse them across hosts."
          />
        ) : (
          <>
            <PageSection title="Keys">
              {visibleKeys.length === 0 && !draft ? (
                <Muted>{query ? "No keys match your search." : "No keys yet."}</Muted>
              ) : (
                <CardGrid view={view}>
                  {draft && (
                    <EntityCard
                      active
                      icon={<KeyGlyph size={22} />}
                      title={
                        draft.label || <span className={pageClasses.cardPlaceholder}>Add a label...</span>
                      }
                      subtitle={`Type ${draft.type ?? "unknown"}`}
                    />
                  )}
                  {visibleKeys.map((key) => (
                    <EntityCard
                      key={key.id}
                      icon={<KeyGlyph size={22} />}
                      title={key.label}
                      subtitle={`Type ${keyTypeLabel(key.algorithm)}`}
                      active={editingKey === key.id}
                      onClick={() => setPanel({ kind: "key", mode: { kind: "edit", key } })}
                      onContextMenu={(event) => menu.open(event, { kind: "key", key })}
                    />
                  ))}
                </CardGrid>
              )}
            </PageSection>
            {(identities.length > 0 || !query) && (
              <PageSection title="Identities">
                {visibleIdentities.length === 0 ? (
                  <Muted>
                    {query
                      ? "No identities match your search."
                      : "An identity is a username with a password or key that many hosts can share."}
                  </Muted>
                ) : (
                  <CardGrid view={view}>
                    {visibleIdentities.map((identity) => {
                      const key = keyName(identity.keyId);
                      return (
                        <EntityCard
                          key={identity.id}
                          icon={<IconUserFilled size={20} />}
                          title={identity.label}
                          subtitle={key ? `${identity.username}, ${key}` : identity.username}
                          active={editingIdentity === identity.id}
                          onClick={() => setPanel({ kind: "identity", identity })}
                          onContextMenu={(event) => menu.open(event, { kind: "identity", identity })}
                          action={{
                            label: "Edit identity",
                            icon: <IconPencilFilled size={16} />,
                            onClick: () => setPanel({ kind: "identity", identity }),
                          }}
                        />
                      );
                    })}
                  </CardGrid>
                )}
              </PageSection>
            )}
          </>
        )}
      </Page>
      <ContextMenu position={menu.position} onClose={menu.close}>
        {menu.target?.kind === "key" && (
          <KeyMenuItems
            sshKey={menu.target.key}
            onEdit={(key) => setPanel({ kind: "key", mode: { kind: "edit", key } })}
            onDelete={deleteKey}
          />
        )}
        {menu.target?.kind === "identity" && (
          <IdentityMenuItems
            identity={menu.target.identity}
            onEdit={(identity) => setPanel({ kind: "identity", identity })}
            onDelete={deleteIdentity}
          />
        )}
      </ContextMenu>
      <KeyPanel
        onMode={(mode) => setPanel({ kind: "key", mode })}
        onDraft={setDraft}
        mode={keyMode}
        onClose={() => setPanel(null)}
        onDelete={deleteKey}
      />
      <IdentityPanel
        opened={panel?.kind === "identity"}
        identity={panel?.kind === "identity" ? panel.identity : null}
        onClose={() => setPanel(null)}
        onDelete={deleteIdentity}
      />
    </>
  );
}

interface KeyMenuProps {
  sshKey: SshKey;
  onEdit: (key: SshKey) => void;
  onDelete: (key: SshKey) => void;
}

function KeyMenuItems({ sshKey, onEdit, onDelete }: KeyMenuProps) {
  return (
    <>
      <CopyButton value={sshKey.publicKey}>
        {({ copy }) => (
          <Menu.Item leftSection={<IconCopy size={16} />} onClick={copy}>
            Copy public key
          </Menu.Item>
        )}
      </CopyButton>
      <Menu.Item leftSection={<IconPencil size={16} />} onClick={() => onEdit(sshKey)}>
        Edit
      </Menu.Item>
      <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(sshKey)}>
        Delete
      </Menu.Item>
    </>
  );
}

interface IdentityMenuProps {
  identity: Identity;
  onEdit: (identity: Identity) => void;
  onDelete: (identity: Identity) => void;
}

function IdentityMenuItems({ identity, onEdit, onDelete }: IdentityMenuProps) {
  return (
    <>
      <Menu.Item leftSection={<IconPencil size={16} />} onClick={() => onEdit(identity)}>
        Edit
      </Menu.Item>
      <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(identity)}>
        Delete
      </Menu.Item>
    </>
  );
}
