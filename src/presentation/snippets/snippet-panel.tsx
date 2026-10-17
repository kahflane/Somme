import { Button, Menu, TextInput, Textarea } from "@mantine/core";
import { useForm } from "@mantine/form";
import { IconPlayerPlay, IconTrash } from "@tabler/icons-react";
import { useEffect } from "react";
import { useSaveSnippet } from "@/application/hooks/entities";
import type { Snippet, SnippetInput } from "@/domain/types";
import { PanelSection, SidePanel } from "@/presentation/common/side-panel";

interface Props {
  opened: boolean;
  snippet: Snippet | null;
  onClose: () => void;
  onRun: (snippet: Snippet) => void;
  onDelete: (snippet: Snippet) => void;
}

function initialValues(snippet: Snippet | null): SnippetInput {
  return { id: snippet?.id ?? null, label: snippet?.label ?? "", script: snippet?.script ?? "" };
}

export function SnippetPanel({ opened, snippet, onClose, onRun, onDelete }: Props) {
  const save = useSaveSnippet();
  const form = useForm<SnippetInput>({
    initialValues: initialValues(snippet),
    validate: {
      label: (value) => (value.trim() ? null : "Label is required"),
      script: (value) => (value.trim() ? null : "Script is required"),
    },
  });

  useEffect(() => {
    if (opened) {
      form.setValues(initialValues(snippet));
      form.clearErrors();
    }
  }, [opened, snippet]);

  return (
    <SidePanel
      opened={opened}
      title={snippet ? "Edit Snippet" : "New Snippet"}
      onClose={onClose}
      menu={
        snippet && (
          <>
            <Menu.Item leftSection={<IconPlayerPlay size={16} />} onClick={() => onRun(snippet)}>
              Run in active terminal
            </Menu.Item>
            <Menu.Divider />
            <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(snippet)}>
              Delete
            </Menu.Item>
          </>
        )
      }
      footer={
        <Button type="submit" form="snippet-panel-form" loading={save.isPending}>
          Save
        </Button>
      }
    >
      <form
        id="snippet-panel-form"
        onSubmit={form.onSubmit((values) =>
          save.mutate({ ...values, label: values.label.trim() }, { onSuccess: onClose }),
        )}
      >
        <PanelSection title="General">
          <TextInput label="Label" data-autofocus autoFocus {...form.getInputProps("label")} />
        </PanelSection>
        <PanelSection title="Script">
          <Textarea
            autosize
            minRows={6}
            maxRows={18}
            placeholder="sudo apt update"
            styles={{ input: { fontFamily: "var(--mantine-font-family-monospace)", fontSize: 12 } }}
            {...form.getInputProps("script")}
          />
        </PanelSection>
      </form>
    </SidePanel>
  );
}
