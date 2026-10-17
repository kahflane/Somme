import {
  Button,
  CopyButton,
  Menu,
  PasswordInput,
  SegmentedControl,
  TextInput,
  Textarea,
} from "@mantine/core";
import { useForm } from "@mantine/form";
import { IconCheck, IconCopy, IconTrash } from "@tabler/icons-react";
import { useEffect } from "react";
import { keyTypeLabel } from "@/application/format";
import { useGenerateKey, useImportKey, useRenameKey } from "@/application/hooks/entities";
import type { KeyAlgorithm, SshKey } from "@/domain/types";
import { PanelSection, SidePanel } from "@/presentation/common/side-panel";

export type KeyPanelMode = { kind: "generate" } | { kind: "import" } | { kind: "edit"; key: SshKey };

interface Props {
  mode: KeyPanelMode | null;
  onClose: () => void;
  onDelete: (key: SshKey) => void;
}

const monospace = { input: { fontFamily: "var(--mantine-font-family-monospace)", fontSize: 12 } };

function GenerateKeyForm({
  generate,
  onDone,
}: {
  generate: ReturnType<typeof useGenerateKey>;
  onDone: () => void;
}) {
  const form = useForm<{ label: string; algorithm: KeyAlgorithm }>({
    initialValues: { label: "", algorithm: "ed25519" },
    validate: { label: (value) => (value.trim() ? null : "Label is required") },
  });
  return (
    <form
      id="key-panel-form"
      onSubmit={form.onSubmit((values) =>
        generate.mutate({ ...values, label: values.label.trim() }, { onSuccess: onDone }),
      )}
    >
      <PanelSection title="General">
        <TextInput label="Label" data-autofocus autoFocus {...form.getInputProps("label")} />
      </PanelSection>
      <PanelSection title="Key type">
        <SegmentedControl
          fullWidth
          data={[
            { value: "ed25519", label: "ED25519" },
            { value: "ecdsa", label: "ECDSA" },
            { value: "rsa", label: "RSA 4096" },
          ]}
          {...form.getInputProps("algorithm")}
        />
      </PanelSection>
    </form>
  );
}

function ImportKeyForm({
  importKey,
  onDone,
}: {
  importKey: ReturnType<typeof useImportKey>;
  onDone: () => void;
}) {
  const form = useForm({
    initialValues: { label: "", privateKey: "", passphrase: "" },
    validate: {
      label: (value) => (value.trim() ? null : "Label is required"),
      privateKey: (value) =>
        value.includes("PRIVATE KEY") || value.startsWith("PuTTY") ? null : "Paste a private key",
    },
  });
  return (
    <form
      id="key-panel-form"
      onSubmit={form.onSubmit((values) =>
        importKey.mutate(
          {
            label: values.label.trim(),
            privateKey: values.privateKey,
            passphrase: values.passphrase ? values.passphrase : null,
          },
          { onSuccess: onDone },
        ),
      )}
    >
      <PanelSection title="General">
        <TextInput label="Label" data-autofocus autoFocus {...form.getInputProps("label")} />
      </PanelSection>
      <PanelSection title="Private key">
        <Textarea
          description="OpenSSH, PEM or PuTTY format. It is encrypted with your master password."
          autosize
          minRows={8}
          maxRows={16}
          styles={monospace}
          {...form.getInputProps("privateKey")}
        />
        <PasswordInput
          label="Passphrase"
          description="Leave empty if the key has none"
          {...form.getInputProps("passphrase")}
        />
      </PanelSection>
    </form>
  );
}

interface EditKeyFormProps {
  sshKey: SshKey;
  rename: ReturnType<typeof useRenameKey>;
  onDone: () => void;
}

function EditKeyForm({ sshKey, rename, onDone }: EditKeyFormProps) {
  const form = useForm({
    initialValues: { label: sshKey.label },
    validate: { label: (value) => (value.trim() ? null : "Label is required") },
  });

  useEffect(() => {
    form.setValues({ label: sshKey.label });
  }, [sshKey.id, sshKey.label]);

  return (
    <form
      id="key-panel-form"
      onSubmit={form.onSubmit((values) =>
        rename.mutate({ id: sshKey.id, label: values.label.trim() }, { onSuccess: onDone }),
      )}
    >
      <PanelSection title="General">
        <TextInput label="Label" {...form.getInputProps("label")} />
        <TextInput label="Type" value={keyTypeLabel(sshKey.algorithm)} readOnly />
        <TextInput label="Fingerprint" value={sshKey.fingerprint} readOnly styles={monospace} />
      </PanelSection>
      <PanelSection title="Public key">
        <Textarea value={sshKey.publicKey} readOnly autosize minRows={4} maxRows={10} styles={monospace} />
        <CopyButton value={sshKey.publicKey}>
          {({ copied, copy }) => (
            <Button
              variant="default"
              leftSection={copied ? <IconCheck size={16} /> : <IconCopy size={16} />}
              onClick={copy}
            >
              {copied ? "Copied" : "Copy public key"}
            </Button>
          )}
        </CopyButton>
      </PanelSection>
    </form>
  );
}

const titles = { generate: "New Key", import: "Import Key", edit: "Edit Key" } as const;
const actions = { generate: "Generate", import: "Import", edit: "Save" } as const;

export function KeyPanel({ mode, onClose, onDelete }: Props) {
  const generate = useGenerateKey();
  const importKey = useImportKey();
  const rename = useRenameKey();
  const pending = generate.isPending || importKey.isPending || rename.isPending;
  const kind = mode?.kind ?? "generate";

  return (
    <SidePanel
      opened={mode !== null}
      title={titles[kind]}
      onClose={onClose}
      menu={
        mode?.kind === "edit" && (
          <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(mode.key)}>
            Delete
          </Menu.Item>
        )
      }
      footer={
        <Button type="submit" form="key-panel-form" loading={pending}>
          {actions[kind]}
        </Button>
      }
    >
      {mode?.kind === "generate" && <GenerateKeyForm generate={generate} onDone={onClose} />}
      {mode?.kind === "import" && <ImportKeyForm importKey={importKey} onDone={onClose} />}
      {mode?.kind === "edit" && (
        <EditKeyForm key={mode.key.id} sshKey={mode.key} rename={rename} onDone={onClose} />
      )}
    </SidePanel>
  );
}
