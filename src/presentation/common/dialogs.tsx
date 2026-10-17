import { Button, Group, Stack, Text, TextInput } from "@mantine/core";
import { modals } from "@mantine/modals";
import { useState } from "react";

interface TextPromptOptions {
  title: string;
  label: string;
  initial?: string;
  confirmLabel?: string;
}

function TextPromptForm({
  label,
  initial,
  confirmLabel,
  onSubmit,
  onCancel,
}: {
  label: string;
  initial: string;
  confirmLabel: string;
  onSubmit: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        if (value.trim()) onSubmit(value.trim());
      }}
    >
      <Stack>
        <TextInput
          label={label}
          value={value}
          autoFocus
          data-autofocus
          onChange={(event) => setValue(event.currentTarget.value)}
        />
        <Group justify="flex-end">
          <Button variant="default" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="submit" disabled={!value.trim()}>
            {confirmLabel}
          </Button>
        </Group>
      </Stack>
    </form>
  );
}

export function promptText({
  title,
  label,
  initial = "",
  confirmLabel = "Save",
}: TextPromptOptions): Promise<string | null> {
  return new Promise((resolve) => {
    let settled = false;
    const settle = (value: string | null) => {
      if (settled) return;
      settled = true;
      resolve(value);
    };
    const id = modals.open({
      title,
      onClose: () => settle(null),
      children: (
        <TextPromptForm
          label={label}
          initial={initial}
          confirmLabel={confirmLabel}
          onSubmit={(value) => {
            settle(value);
            modals.close(id);
          }}
          onCancel={() => modals.close(id)}
        />
      ),
    });
  });
}

export function confirmAction(
  title: string,
  message: string,
  confirmLabel: string,
  onConfirm: () => void,
  danger = false,
) {
  modals.openConfirmModal({
    title,
    children: <Text size="sm">{message}</Text>,
    labels: { confirm: confirmLabel, cancel: "Cancel" },
    confirmProps: danger ? { color: "red" } : undefined,
    onConfirm,
  });
}

export function confirmDanger(title: string, message: string, onConfirm: () => void) {
  confirmAction(title, message, "Delete", onConfirm, true);
}
