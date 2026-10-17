import { Button, Group, Menu, NumberInput, SegmentedControl, Select, Text, TextInput } from "@mantine/core";
import { useForm } from "@mantine/form";
import { IconTrash } from "@tabler/icons-react";
import { useEffect } from "react";
import { useHosts, useSaveForward } from "@/application/hooks/entities";
import type { ForwardKind, PortForward, PortForwardInput } from "@/domain/types";
import { PanelSection, SidePanel } from "@/presentation/common/side-panel";

interface Props {
  opened: boolean;
  rule: PortForward | null;
  defaultKind: ForwardKind;
  onClose: () => void;
  onDelete: (rule: PortForward) => void;
}

interface FormValues {
  label: string;
  hostId: string | null;
  kind: ForwardKind;
  bindAddress: string;
  bindPort: number;
  targetHost: string;
  targetPort: number | string;
}

function initialValues(rule: PortForward | null, defaultKind: ForwardKind): FormValues {
  return {
    label: rule?.label ?? "",
    hostId: rule?.hostId ?? null,
    kind: rule?.kind ?? defaultKind,
    bindAddress: rule?.bindAddress ?? "127.0.0.1",
    bindPort: rule?.bindPort ?? 8080,
    targetHost: rule?.targetHost ?? "127.0.0.1",
    targetPort: rule?.targetPort ?? 80,
  };
}

const help: Record<ForwardKind, string> = {
  local: "Listen on this computer and send traffic through the server to the target.",
  remote: "Listen on the server and send traffic back to a target reachable from this computer.",
  dynamic: "Run a SOCKS5 proxy on this computer that sends traffic through the server.",
};

export function ForwardPanel({ opened, rule, defaultKind, onClose, onDelete }: Props) {
  const save = useSaveForward();
  const { data: hosts = [] } = useHosts();
  const form = useForm<FormValues>({
    initialValues: initialValues(rule, defaultKind),
    validate: {
      hostId: (value) => (value ? null : "Pick a host"),
      bindPort: (value) => (value > 0 && value < 65536 ? null : "Port must be 1-65535"),
      targetHost: (value, values) =>
        values.kind === "dynamic" || value.trim() ? null : "Target host is required",
      targetPort: (value, values) => {
        if (values.kind === "dynamic") return null;
        const port = Number(value);
        return port > 0 && port < 65536 ? null : "Port must be 1-65535";
      },
    },
  });

  useEffect(() => {
    if (opened) {
      form.setValues(initialValues(rule, defaultKind));
      form.clearErrors();
    }
  }, [opened, rule, defaultKind]);

  const submit = (values: FormValues) => {
    if (!values.hostId) return;
    const dynamic = values.kind === "dynamic";
    const input: PortForwardInput = {
      id: rule?.id ?? null,
      label: values.label.trim() || `${values.kind} ${values.bindPort}`,
      hostId: values.hostId,
      kind: values.kind,
      bindAddress: values.bindAddress.trim() || "127.0.0.1",
      bindPort: values.bindPort,
      targetHost: dynamic ? null : values.targetHost.trim(),
      targetPort: dynamic ? null : Number(values.targetPort),
    };
    save.mutate(input, { onSuccess: onClose });
  };

  const kind = form.values.kind;

  return (
    <SidePanel
      opened={opened}
      title={rule ? "Edit Port Forwarding" : "New Port Forwarding"}
      onClose={onClose}
      menu={
        rule && (
          <Menu.Item color="red" leftSection={<IconTrash size={16} />} onClick={() => onDelete(rule)}>
            Delete
          </Menu.Item>
        )
      }
      footer={
        <Button type="submit" form="forward-panel-form" loading={save.isPending}>
          Save
        </Button>
      }
    >
      <form id="forward-panel-form" onSubmit={form.onSubmit(submit)}>
        <PanelSection title="Type">
          <SegmentedControl
            fullWidth
            data={[
              { value: "local", label: "Local" },
              { value: "remote", label: "Remote" },
              { value: "dynamic", label: "Dynamic" },
            ]}
            {...form.getInputProps("kind")}
          />
          <Text size="sm" c="dimmed">
            {help[kind]}
          </Text>
        </PanelSection>
        <PanelSection title="General">
          <TextInput label="Label" data-autofocus autoFocus {...form.getInputProps("label")} />
          <Select
            label="Through host"
            placeholder="Pick a host"
            searchable
            data={hosts.map((host) => ({ value: host.id, label: host.label }))}
            {...form.getInputProps("hostId")}
          />
        </PanelSection>
        <PanelSection title={kind === "remote" ? "Listen on server" : "Listen on this computer"}>
          <Group grow align="flex-start" wrap="nowrap">
            <TextInput label="Bind address" {...form.getInputProps("bindAddress")} />
            <NumberInput label="Port" min={1} max={65535} maw={110} {...form.getInputProps("bindPort")} />
          </Group>
        </PanelSection>
        {kind !== "dynamic" && (
          <PanelSection title="Destination">
            <Group grow align="flex-start" wrap="nowrap">
              <TextInput label="Host" {...form.getInputProps("targetHost")} />
              <NumberInput label="Port" min={1} max={65535} maw={110} {...form.getInputProps("targetPort")} />
            </Group>
          </PanelSection>
        )}
      </form>
    </SidePanel>
  );
}
