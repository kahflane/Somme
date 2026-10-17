import { Button, Menu, NumberInput, Select, TagsInput, TextInput } from "@mantine/core";
import { useForm } from "@mantine/form";
import { IconCheck, IconCopy, IconFolderFilled, IconTrash } from "@tabler/icons-react";
import { useEffect } from "react";
import { tileColors } from "@/application/format";
import {
  useGroups,
  useHosts,
  useIdentities,
  useKeys,
  useSaveHost,
  useSnippets,
} from "@/application/hooks/entities";
import type { Host, HostInput } from "@/domain/types";
import { SecretField, normalizeSecret } from "@/presentation/common/secret-field";
import { PanelSection, SidePanel } from "@/presentation/common/side-panel";
import classes from "@/presentation/hosts/hosts.module.css";

interface Props {
  opened: boolean;
  host: Host | null;
  defaultGroupId: string | null;
  onClose: () => void;
  onDuplicate: (host: Host) => void;
  onSftp: (host: Host) => void;
  onDelete: (host: Host) => void;
}

function initialValues(host: Host | null, defaultGroupId: string | null): HostInput {
  return {
    id: host?.id ?? null,
    label: host?.label ?? "",
    address: host?.address ?? "",
    port: host?.port ?? 22,
    groupId: host?.groupId ?? defaultGroupId,
    identityId: host?.identityId ?? null,
    username: host?.username ?? null,
    password: { action: "keep" },
    keyId: host?.keyId ?? null,
    jumpHostId: host?.jumpHostId ?? null,
    startupSnippetId: host?.startupSnippetId ?? null,
    tags: host?.tags ?? [],
    color: host?.color ?? null,
  };
}

export function HostPanel({ opened, host, defaultGroupId, onClose, onDuplicate, onSftp, onDelete }: Props) {
  const save = useSaveHost();
  const { data: groups = [] } = useGroups();
  const { data: identities = [] } = useIdentities();
  const { data: keys = [] } = useKeys();
  const { data: hosts = [] } = useHosts();
  const { data: snippets = [] } = useSnippets();
  const form = useForm<HostInput>({
    initialValues: initialValues(host, defaultGroupId),
    validate: {
      address: (value) => (value.trim() ? null : "Address is required"),
      port: (value) => (value > 0 && value < 65536 ? null : "Port must be 1-65535"),
    },
  });

  useEffect(() => {
    if (opened) {
      form.setValues(initialValues(host, defaultGroupId));
      form.resetDirty();
      form.clearErrors();
    }
  }, [opened, host, defaultGroupId]);

  const usingIdentity = Boolean(form.values.identityId);
  const hasPassword = host?.hasPassword ?? false;
  const color = form.values.color;

  const submit = (values: HostInput) => {
    const input: HostInput = {
      ...values,
      label: values.label.trim() || values.address.trim(),
      address: values.address.trim(),
      username: values.username?.trim() ? values.username.trim() : null,
      password: normalizeSecret(values.password, hasPassword),
      color: values.color || null,
    };
    save.mutate(input, { onSuccess: onClose });
  };

  return (
    <SidePanel
      opened={opened}
      title={host ? "Edit Host" : "New Host"}
      onClose={onClose}
      menu={
        host && (
          <>
            <Menu.Item leftSection={<IconFolderFilled size={16} />} onClick={() => onSftp(host)}>
              Open SFTP
            </Menu.Item>
            <Menu.Item leftSection={<IconCopy size={16} />} onClick={() => onDuplicate(host)}>
              Duplicate
            </Menu.Item>
            <Menu.Divider />
            <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(host)}>
              Delete
            </Menu.Item>
          </>
        )
      }
      footer={
        <Button type="submit" form="host-panel-form" loading={save.isPending}>
          Save
        </Button>
      }
    >
      <form id="host-panel-form" onSubmit={form.onSubmit(submit)}>
        <PanelSection title="Address">
          <TextInput
            placeholder="IP or Hostname"
            data-autofocus
            autoFocus
            {...form.getInputProps("address")}
          />
        </PanelSection>
        <PanelSection title="General">
          <TextInput label="Label" placeholder="My server" {...form.getInputProps("label")} />
          <Select
            label="Parent group"
            placeholder="None"
            clearable
            data={groups.map((group) => ({ value: group.id, label: group.name }))}
            {...form.getInputProps("groupId")}
          />
          <TagsInput label="Tags" placeholder="Add a tag" {...form.getInputProps("tags")} />
          <div>
            <div className={classes.fieldLabel}>Color</div>
            <div className={classes.swatches}>
              {tileColors.map((swatch, index) => {
                const selected = (color ?? tileColors[0]) === swatch;
                return (
                  <button
                    key={swatch}
                    type="button"
                    className={classes.swatch}
                    style={{ background: swatch }}
                    data-selected={selected || undefined}
                    aria-label={`Color ${index + 1}`}
                    aria-pressed={selected}
                    onClick={() => form.setFieldValue("color", index === 0 ? null : swatch)}
                  >
                    {selected && <IconCheck size={14} stroke={3} />}
                  </button>
                );
              })}
            </div>
          </div>
        </PanelSection>
        <PanelSection title="SSH">
          <NumberInput label="Port" min={1} max={65535} {...form.getInputProps("port")} />
        </PanelSection>
        <PanelSection title="Credentials">
          <Select
            label="Identity"
            placeholder="None"
            clearable
            data={identities.map((identity) => ({
              value: identity.id,
              label: `${identity.label} (${identity.username})`,
            }))}
            {...form.getInputProps("identityId")}
          />
          {!usingIdentity && (
            <>
              <TextInput
                label="Username"
                value={form.values.username ?? ""}
                onChange={(event) => form.setFieldValue("username", event.currentTarget.value)}
              />
              <SecretField
                label="Password"
                hasExisting={hasPassword}
                value={form.values.password}
                onChange={(value) => form.setFieldValue("password", value)}
              />
              <Select
                label="SSH key"
                placeholder="None"
                clearable
                data={keys.map((key) => ({ value: key.id, label: `${key.label} · ${key.algorithm}` }))}
                {...form.getInputProps("keyId")}
              />
            </>
          )}
        </PanelSection>
        <PanelSection title="Advanced">
          <Select
            label="Jump host"
            placeholder="None"
            clearable
            data={hosts
              .filter((item) => item.id !== host?.id)
              .map((item) => ({ value: item.id, label: item.label }))}
            {...form.getInputProps("jumpHostId")}
          />
          <Select
            label="Startup snippet"
            placeholder="None"
            clearable
            data={snippets.map((snippet) => ({ value: snippet.id, label: snippet.label }))}
            {...form.getInputProps("startupSnippetId")}
          />
        </PanelSection>
      </form>
    </SidePanel>
  );
}
