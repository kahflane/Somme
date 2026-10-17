import { Button, Checkbox, PasswordInput, Stack } from "@mantine/core";
import { useForm } from "@mantine/form";
import { useVaultSetup } from "@/application/hooks/vault";
import { VaultCard } from "@/presentation/vault/vault-card";

export function SetupScreen() {
  const setup = useVaultSetup();
  const form = useForm({
    initialValues: { password: "", confirm: "", remember: true },
    validate: {
      password: (value) => (value.length < 8 ? "Use at least 8 characters" : null),
      confirm: (value, values) => (value !== values.password ? "Passwords do not match" : null),
    },
  });

  return (
    <VaultCard
      title="Create your vault"
      subtitle="Pick a master password. It encrypts your passwords and keys on this device and every synced device."
    >
      <form onSubmit={form.onSubmit((values) => setup.mutate(values))}>
        <Stack>
          <PasswordInput
            label="Master password"
            autoFocus
            {...form.getInputProps("password")}
          />
          <PasswordInput label="Confirm password" {...form.getInputProps("confirm")} />
          <Checkbox
            label="Remember on this device"
            {...form.getInputProps("remember", { type: "checkbox" })}
          />
          <Button type="submit" size="md" fullWidth loading={setup.isPending}>
            Create vault
          </Button>
        </Stack>
      </form>
    </VaultCard>
  );
}
