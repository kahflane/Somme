import { Button, Menu, Select, TextInput } from "@mantine/core";
import { useForm } from "@mantine/form";
import { IconTrash } from "@tabler/icons-react";
import { useEffect } from "react";
import { useGroups, useSaveGroup } from "@/application/hooks/entities";
import type { Group as HostGroup, GroupInput } from "@/domain/types";
import { PanelSection, SidePanel } from "@/presentation/common/side-panel";

interface Props {
  opened: boolean;
  group: HostGroup | null;
  defaultParentId: string | null;
  onClose: () => void;
  onDelete: (group: HostGroup) => void;
}

function initialValues(group: HostGroup | null, defaultParentId: string | null): GroupInput {
  return { id: group?.id ?? null, name: group?.name ?? "", parentId: group?.parentId ?? defaultParentId };
}

export function GroupPanel({ opened, group, defaultParentId, onClose, onDelete }: Props) {
  const save = useSaveGroup();
  const { data: groups = [] } = useGroups();
  const form = useForm<GroupInput>({
    initialValues: initialValues(group, defaultParentId),
    validate: { name: (value) => (value.trim() ? null : "Name is required") },
  });

  useEffect(() => {
    if (opened) {
      form.setValues(initialValues(group, defaultParentId));
      form.clearErrors();
    }
  }, [opened, group, defaultParentId]);

  return (
    <SidePanel
      opened={opened}
      title={group ? "Edit Group" : "New Group"}
      onClose={onClose}
      menu={
        group && (
          <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(group)}>
            Delete
          </Menu.Item>
        )
      }
      footer={
        <Button type="submit" form="group-panel-form" loading={save.isPending}>
          Save
        </Button>
      }
    >
      <form
        id="group-panel-form"
        onSubmit={form.onSubmit((values) =>
          save.mutate({ ...values, name: values.name.trim() }, { onSuccess: onClose }),
        )}
      >
        <PanelSection title="General">
          <TextInput label="Name" data-autofocus autoFocus {...form.getInputProps("name")} />
          <Select
            label="Parent group"
            placeholder="None"
            clearable
            data={groups
              .filter((item) => item.id !== group?.id)
              .map((item) => ({ value: item.id, label: item.name }))}
            {...form.getInputProps("parentId")}
          />
        </PanelSection>
      </form>
    </SidePanel>
  );
}
