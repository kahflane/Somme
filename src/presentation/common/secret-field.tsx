import { Button, Group, PasswordInput, Text } from "@mantine/core";
import type { SecretUpdate } from "@/domain/types";

interface Props {
  label: string;
  hasExisting: boolean;
  value: SecretUpdate;
  onChange: (value: SecretUpdate) => void;
}

export function SecretField({ label, hasExisting, value, onChange }: Props) {
  if (hasExisting && value.action === "keep") {
    return (
      <div>
        <Text size="sm" fw={500} mb={4}>
          {label}
        </Text>
        <Group gap="xs">
          <Text size="sm" c="dimmed">
            Saved in vault
          </Text>
          <Button variant="light" onClick={() => onChange({ action: "set", value: "" })}>
            Change
          </Button>
          <Button variant="subtle" color="red" onClick={() => onChange({ action: "clear" })}>
            Remove
          </Button>
        </Group>
      </div>
    );
  }

  if (value.action === "clear") {
    return (
      <div>
        <Text size="sm" fw={500} mb={4}>
          {label}
        </Text>
        <Group gap="xs">
          <Text size="sm" c="red">
            Will be removed
          </Text>
          <Button variant="subtle" onClick={() => onChange({ action: "keep" })}>
            Undo
          </Button>
        </Group>
      </div>
    );
  }

  return (
    <PasswordInput
      label={label}
      value={value.action === "set" ? value.value : ""}
      onChange={(event) => {
        const next = event.currentTarget.value;
        onChange(next || hasExisting ? { action: "set", value: next } : { action: "keep" });
      }}
    />
  );
}

export function normalizeSecret(value: SecretUpdate, hasExisting: boolean): SecretUpdate {
  if (value.action === "set" && value.value === "") return { action: "keep" };
  if (value.action === "clear" && !hasExisting) return { action: "keep" };
  return value;
}
