import { CopyButton, Menu, SegmentedControl } from "@mantine/core";
import { IconCopy, IconFileImport, IconKey, IconTrash } from "@tabler/icons-react";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState, type DragEvent } from "react";
import { errorMessage } from "@/application/errors";
import { useGenerateKey, useImportKey, useRenameKey } from "@/application/hooks/entities";
import { looksLikePrivateKey, readKeyInfo } from "@/application/key-files";
import { baseName, joinPath, pathStyleOf } from "@/application/paths";
import type { KeyAlgorithm, SshKey } from "@/domain/types";
import { api } from "@/infrastructure/tauri/api";
import { FloatingField } from "@/presentation/common/floating-field";
import { PanelCard, PanelPrimaryButton, SidePanel, VaultPicker } from "@/presentation/common/side-panel";
import classes from "@/presentation/keychain/key-panel.module.css";

export type KeyPanelMode = { kind: "import" } | { kind: "generate" } | { kind: "edit"; key: SshKey };

export interface KeyDraft {
  label: string;
  type: string | null;
}

interface Props {
  mode: KeyPanelMode | null;
  onMode: (mode: KeyPanelMode) => void;
  onClose: () => void;
  onDelete: (key: SshKey) => void;
  onDraft: (draft: KeyDraft | null) => void;
}

const MAX_KEY_FILE_BYTES = 64 * 1024;

function DropIcon() {
  return (
    <svg
      width="34"
      height="46"
      viewBox="0 0 34 46"
      fill="none"
      className={classes.dropIcon}
      aria-hidden="true"
    >
      <rect x="15.5" y="0.75" width="17" height="17" rx="3" stroke="currentColor" strokeDasharray="2.2 2.2" />
      <path d="M24 5.5v7.5M20.25 9.25h7.5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
      <rect
        x="1"
        y="19"
        width="20"
        height="25"
        rx="4"
        fill="var(--x-field)"
        stroke="currentColor"
        strokeWidth="1.5"
      />
      <path
        d="M14 32l7 7M14 32v5.5M14 32h5.5"
        stroke="var(--x-focus)"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function ImportKeyForm({
  onCreated,
  onDraft,
}: {
  onCreated: (key: SshKey) => void;
  onDraft: Props["onDraft"];
}) {
  const importKey = useImportKey();
  const [label, setLabel] = useState("");
  const [fileName, setFileName] = useState<string | null>(null);
  const [privateKey, setPrivateKey] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [submitted, setSubmitted] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const attempted = useRef<string | null>(null);
  const info = useMemo(() => readKeyInfo(privateKey), [privateKey]);

  useEffect(() => {
    onDraft({ label, type: info.type });
  }, [label, info.type, onDraft]);

  useEffect(() => () => onDraft(null), [onDraft]);

  // Like Termius, the key is saved as soon as a whole private key is in place.
  useEffect(() => {
    const ready = info.complete && (!info.encrypted || submitted !== null);
    if (!ready || importKey.isPending) return;
    const signature = `${privateKey}\u0000${info.encrypted ? submitted : ""}`;
    if (attempted.current === signature) return;
    attempted.current = signature;
    setError(null);
    importKey.mutate(
      {
        label: label.trim() || fileName || `${info.type ?? "SSH"} key`,
        privateKey,
        passphrase: info.encrypted ? submitted : null,
      },
      { onSuccess: onCreated, onError: (failure) => setError(errorMessage(failure)) },
    );
  }, [info, submitted, privateKey, importKey, label, fileName, onCreated]);

  const loadKey = (text: string, name: string) => {
    if (!looksLikePrivateKey(text)) {
      setError("That file is not a private key.");
      return;
    }
    setError(null);
    setPrivateKey(text);
    setFileName(name);
    setLabel((current) => current || name);
  };

  const pickFile = async () => {
    const home = await api.localFs.home().catch(() => null);
    const selected = await open({
      title: "Import private key",
      multiple: false,
      directory: false,
      defaultPath: home ? joinPath(home, ".ssh", pathStyleOf(home)) : undefined,
    });
    if (typeof selected !== "string") return;
    try {
      loadKey(await api.localFs.readKey(selected), baseName(selected));
    } catch (failure) {
      setError(errorMessage(failure));
    }
  };

  const dropFile = async (event: DragEvent) => {
    event.preventDefault();
    setDragOver(false);
    const file = event.dataTransfer.files[0];
    if (!file) return;
    if (file.size > MAX_KEY_FILE_BYTES) {
      setError("That file is too large to be a private key.");
      return;
    }
    loadKey(await file.text(), file.name);
  };

  const keyError = error && !info.encrypted ? error : null;
  const passphraseError = error && info.encrypted ? error : null;
  const partial = privateKey.trim().length > 0 && !info.complete;

  return (
    <>
      <PanelCard>
        <FloatingField label="Label" value={label} onChange={setLabel} />
        <FloatingField
          label="Private key"
          required
          multiline
          mono={privateKey.length > 0}
          autoFocus
          value={privateKey}
          onChange={(value) => {
            setPrivateKey(value);
            setError(null);
          }}
          error={keyError}
        />
        {partial && (
          <div className={classes.hint}>Paste the whole key, including the BEGIN and END lines.</div>
        )}
        {info.encrypted && (
          <FloatingField
            label="Passphrase"
            required
            password
            value={passphrase}
            onChange={(value) => {
              setPassphrase(value);
              setError(null);
            }}
            onBlur={() => passphrase && setSubmitted(passphrase)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && passphrase) setSubmitted(passphrase);
            }}
            error={passphraseError}
          />
        )}
        <FloatingField label="Public key" value="" readOnly multiline hint="Filled in from the private key" />
      </PanelCard>
      <PanelCard>
        <div
          className={classes.dropzone}
          data-over={dragOver || undefined}
          onDragOver={(event) => {
            if (!event.dataTransfer.types.includes("Files")) return;
            event.preventDefault();
            event.dataTransfer.dropEffect = "copy";
            setDragOver(true);
          }}
          onDragLeave={() => setDragOver(false)}
          onDrop={dropFile}
        >
          <DropIcon />
          Drag and drop a private key file to import
        </div>
        <div className={classes.cardGap} />
        <PanelPrimaryButton onClick={pickFile} disabled={importKey.isPending}>
          Import from key file
        </PanelPrimaryButton>
      </PanelCard>
    </>
  );
}

function GenerateKeyForm({
  onCreated,
  onDraft,
}: {
  onCreated: (key: SshKey) => void;
  onDraft: Props["onDraft"];
}) {
  const generate = useGenerateKey();
  const [label, setLabel] = useState("");
  const [algorithm, setAlgorithm] = useState<KeyAlgorithm>("ed25519");
  const type = algorithm === "rsa" ? "RSA" : algorithm.toUpperCase();

  useEffect(() => {
    onDraft({ label, type });
  }, [label, type, onDraft]);

  useEffect(() => () => onDraft(null), [onDraft]);

  return (
    <>
      <PanelCard>
        <FloatingField label="Label" value={label} onChange={setLabel} autoFocus />
        <SegmentedControl
          fullWidth
          value={algorithm}
          onChange={(value) => setAlgorithm(value === "rsa" || value === "ecdsa" ? value : "ed25519")}
          data={[
            { value: "ed25519", label: "ED25519" },
            { value: "ecdsa", label: "ECDSA" },
            { value: "rsa", label: "RSA 4096" },
          ]}
        />
      </PanelCard>
      <PanelCard>
        <PanelPrimaryButton
          disabled={generate.isPending}
          onClick={() =>
            generate.mutate({ label: label.trim() || `${type} key`, algorithm }, { onSuccess: onCreated })
          }
        >
          {generate.isPending ? "Generating…" : "Generate key"}
        </PanelPrimaryButton>
      </PanelCard>
    </>
  );
}

function EditKeyForm({ sshKey }: { sshKey: SshKey }) {
  const rename = useRenameKey();
  const [label, setLabel] = useState(sshKey.label);

  useEffect(() => {
    setLabel(sshKey.label);
  }, [sshKey.id, sshKey.label]);

  const commit = () => {
    const next = label.trim();
    if (!next) setLabel(sshKey.label);
    else if (next !== sshKey.label) rename.mutate({ id: sshKey.id, label: next });
  };

  return (
    <>
      <PanelCard>
        <FloatingField
          label="Label"
          value={label}
          onChange={setLabel}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === "Enter") event.currentTarget.blur();
          }}
        />
        <FloatingField
          label="Private key"
          value=""
          readOnly
          multiline
          placeholder="Encrypted in your vault"
        />
        <FloatingField label="Public key" value={sshKey.publicKey} readOnly multiline mono />
      </PanelCard>
      <PanelCard>
        <CopyButton value={sshKey.publicKey}>
          {({ copied, copy }) => (
            <PanelPrimaryButton onClick={copy}>{copied ? "Copied" : "Copy public key"}</PanelPrimaryButton>
          )}
        </CopyButton>
      </PanelCard>
    </>
  );
}

export function KeyPanel({ mode, onMode, onClose, onDelete, onDraft }: Props) {
  const created = (key: SshKey) => onMode({ kind: "edit", key });

  return (
    <SidePanel
      opened={mode !== null}
      title={mode?.kind === "edit" ? "Edit Key" : "New Key"}
      onClose={onClose}
      menu={
        <>
          {mode?.kind === "import" && (
            <Menu.Item leftSection={<IconKey size={16} />} onClick={() => onMode({ kind: "generate" })}>
              Generate a new key instead
            </Menu.Item>
          )}
          {mode?.kind === "generate" && (
            <Menu.Item leftSection={<IconFileImport size={16} />} onClick={() => onMode({ kind: "import" })}>
              Import an existing key instead
            </Menu.Item>
          )}
          {mode?.kind === "edit" && (
            <>
              <CopyButton value={mode.key.publicKey}>
                {({ copy }) => (
                  <Menu.Item leftSection={<IconCopy size={16} />} onClick={copy}>
                    Copy public key
                  </Menu.Item>
                )}
              </CopyButton>
              <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(mode.key)}>
                Delete
              </Menu.Item>
            </>
          )}
        </>
      }
    >
      <VaultPicker />
      {mode?.kind === "import" && <ImportKeyForm onCreated={created} onDraft={onDraft} />}
      {mode?.kind === "generate" && <GenerateKeyForm onCreated={created} onDraft={onDraft} />}
      {mode?.kind === "edit" && <EditKeyForm key={mode.key.id} sshKey={mode.key} />}
    </SidePanel>
  );
}
