import { Button, Menu, Select, TextInput } from "@mantine/core";
import { useForm } from "@mantine/form";
import { IconTrash } from "@tabler/icons-react";
import { useEffect } from "react";
import { useKeys, useSaveIdentity } from "@/application/hooks/entities";
import type { Identity, IdentityInput } from "@/domain/types";
import { SecretField, normalizeSecret } from "@/presentation/common/secret-field";
import { PanelSection, SidePanel } from "@/presentation/common/side-panel";

interface Props {
  opened: boolean;
  identity: Identity | null;
  onClose: () => void;
  onDelete: (identity: Identity) => void;
}

function initialValues(identity: Identity | null): IdentityInput {
  return {
    id: identity?.id ?? null,
    label: identity?.label ?? "",
    username: identity?.username ?? "",
    password: { action: "keep" },
    keyId: identity?.keyId ?? null,
  };
}

export function IdentityPanel({ opened, identity, onClose, onDelete }: Props) {
  const save = useSaveIdentity();
  const { data: keys = [] } = useKeys();
  const form = useForm<IdentityInput>({
    initialValues: initialValues(identity),
    validate: { username: (value) => (value.trim() ? null : "Username is required") },
  });
  const hasPassword = identity?.hasPassword ?? false;

  useEffect(() => {
    if (opened) {
      form.setValues(initialValues(identity));
      form.clearErrors();
    }
  }, [opened, identity]);

  return (
    <SidePanel
      opened={opened}
      title={identity ? "Edit Identity" : "New Identity"}
      onClose={onClose}
      menu={
        identity && (
          <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(identity)}>
            Delete
          </Menu.Item>
        )
      }
      footer={
        <Button type="submit" form="identity-panel-form" loading={save.isPending}>
          Save
        </Button>
      }
    >
      <form
        id="identity-panel-form"
        onSubmit={form.onSubmit((values) =>
          save.mutate(
            {
              ...values,
              label: values.label.trim() || values.username.trim(),
              username: values.username.trim(),
              password: normalizeSecret(values.password, hasPassword),
            },
            { onSuccess: onClose },
          ),
        )}
      >
        <PanelSection title="General">
          <TextInput label="Label" data-autofocus autoFocus {...form.getInputProps("label")} />
        </PanelSection>
        <PanelSection title="Credentials">
          <TextInput label="Username" {...form.getInputProps("username")} />
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
        </PanelSection>
      </form>
    </SidePanel>
  );
}
