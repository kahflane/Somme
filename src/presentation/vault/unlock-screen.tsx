import { Button, Checkbox, PasswordInput, Stack } from "@mantine/core";
import { useForm } from "@mantine/form";
import { useVaultUnlock } from "@/application/hooks/vault";
import { VaultCard } from "@/presentation/vault/vault-card";

export function UnlockScreen() {
  const unlock = useVaultUnlock();
  const form = useForm({
    initialValues: { password: "", remember: false },
    validate: { password: (value) => (value ? null : "Enter your master password") },
  });

  return (
    <VaultCard title="Unlock Somme" subtitle="Enter your master password to open the vault.">
      <form onSubmit={form.onSubmit((values) => unlock.mutate(values))}>
        <Stack>
          <PasswordInput
            label="Master password"
            autoFocus
            {...form.getInputProps("password")}
          />
          <Checkbox
            label="Remember on this device"
            {...form.getInputProps("remember", { type: "checkbox" })}
          />
          <Button type="submit" size="md" fullWidth loading={unlock.isPending}>
            Unlock
          </Button>
        </Stack>
      </form>
    </VaultCard>
  );
}
