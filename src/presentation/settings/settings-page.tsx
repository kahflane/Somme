import {
  Alert,
  Button,
  Group,
  PasswordInput,
  SegmentedControl,
  Stack,
  Text,
  TextInput,
  ThemeIcon,
  useMantineColorScheme,
} from "@mantine/core";
import { useForm } from "@mantine/form";
import { IconCloud, IconLock, IconLogout, IconRefresh } from "@tabler/icons-react";
import dayjs from "dayjs";
import relativeTime from "dayjs/plugin/relativeTime";
import { useState } from "react";
import {
  useAccountLogin,
  useAccountLogout,
  useAccountRegister,
  useAccountStatus,
} from "@/application/hooks/account";
import { useSyncNow, useSyncStatus } from "@/application/hooks/sync";
import { useVaultLock } from "@/application/hooks/vault";
import { Page, ToolbarTitle } from "@/presentation/common/page";
import { SettingsSection } from "@/presentation/settings/settings-section";
import { TerminalSettingsSection } from "@/presentation/settings/terminal-settings-section";

dayjs.extend(relativeTime);

type AccountMode = "login" | "register";

const MIN_PASSWORD = 10;

function AccountForm() {
  const [mode, setMode] = useState<AccountMode>("login");
  const login = useAccountLogin();
  const register = useAccountRegister();
  const form = useForm({
    initialValues: { email: "", password: "", confirm: "" },
    validate: {
      email: (value) => (/^\S+@\S+\.\S+$/.test(value.trim()) ? null : "Enter a valid email"),
      password: (value) =>
        mode === "register" && value.length < MIN_PASSWORD
          ? `Use at least ${MIN_PASSWORD} characters`
          : value.length === 0
            ? "Enter your password"
            : null,
      confirm: (value, values) =>
        mode === "register" && value !== values.password ? "Passwords do not match" : null,
    },
  });
  const pending = login.isPending || register.isPending;

  return (
    <form
      onSubmit={form.onSubmit((values) => {
        const credentials = { email: values.email.trim(), password: values.password };
        const action = mode === "login" ? login : register;
        action.mutate(credentials, { onSuccess: () => form.reset() });
      })}
    >
      <Stack gap="md">
        <SegmentedControl
          w={280}
          value={mode}
          onChange={(value) => setMode(value === "register" ? "register" : "login")}
          data={[
            { value: "login", label: "Sign in" },
            { value: "register", label: "Create account" },
          ]}
        />
        <TextInput
          label="Email"
          placeholder="you@example.com"
          autoComplete="email"
          {...form.getInputProps("email")}
        />
        <PasswordInput
          label="Password"
          description={mode === "register" ? `At least ${MIN_PASSWORD} characters` : undefined}
          autoComplete={mode === "login" ? "current-password" : "new-password"}
          {...form.getInputProps("password")}
        />
        {mode === "register" && (
          <PasswordInput
            label="Confirm password"
            autoComplete="new-password"
            {...form.getInputProps("confirm")}
          />
        )}
        <Group>
          <Button type="submit" loading={pending}>
            {mode === "login" ? "Sign in" : "Create account"}
          </Button>
        </Group>
      </Stack>
    </form>
  );
}

function SyncDetails({ email }: { email: string | null }) {
  const { data: status } = useSyncStatus();
  const syncNow = useSyncNow();
  const logout = useAccountLogout();

  const lastSync = status?.syncing
    ? "Syncing now…"
    : status?.lastSyncAt
      ? `Last synced ${dayjs(status.lastSyncAt).fromNow()}`
      : "Not synced yet";

  return (
    <Stack gap="md">
      <Group gap="sm" wrap="nowrap">
        <ThemeIcon size={40} radius="xl" variant="light">
          <IconCloud size={20} />
        </ThemeIcon>
        <Stack gap={0}>
          <Text fw={700}>{email ?? "Signed in"}</Text>
          <Text size="sm" c="dimmed">
            {status?.enabled ? lastSync : "Sync is starting…"}
          </Text>
        </Stack>
      </Group>
      {status?.lastError && (
        <Alert color="red" variant="light" title="Last sync failed" radius="md">
          {status.lastError}
        </Alert>
      )}
      <Group gap="sm">
        <Button
          leftSection={<IconRefresh size={16} />}
          loading={syncNow.isPending || Boolean(status?.syncing)}
          disabled={!status?.enabled}
          onClick={() => syncNow.mutate(undefined)}
        >
          Sync now
        </Button>
        <Button
          variant="default"
          leftSection={<IconLogout size={16} />}
          loading={logout.isPending}
          onClick={() => logout.mutate(undefined)}
        >
          Sign out
        </Button>
      </Group>
    </Stack>
  );
}

function AccountSection() {
  const { data: account } = useAccountStatus();
  return (
    <SettingsSection
      title="Somme account & sync"
      description="Sign in to sync hosts, keys and snippets across your devices."
    >
      {account?.signedIn ? <SyncDetails email={account.email} /> : <AccountForm />}
      <Text size="xs" c="dimmed">
        Your vault secrets stay encrypted with your master password.
      </Text>
    </SettingsSection>
  );
}

export function SettingsPage() {
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  const lock = useVaultLock();

  return (
    <Page toolbar={<ToolbarTitle>Settings</ToolbarTitle>}>
      <Stack gap="md" maw={760} pb="xl">
        <AccountSection />
        <TerminalSettingsSection />
        <SettingsSection title="Appearance" description="App theme.">
          <SegmentedControl
            w={280}
            value={colorScheme === "light" ? "light" : "dark"}
            onChange={(value) => setColorScheme(value === "light" ? "light" : "dark")}
            data={[
              { value: "dark", label: "Dark" },
              { value: "light", label: "Light" },
            ]}
          />
        </SettingsSection>
        <SettingsSection
          title="Security"
          description="Lock the vault. Open terminals are closed and the master password is needed again."
        >
          <Group>
            <Button color="red" leftSection={<IconLock size={16} />} onClick={() => lock.mutate()}>
              Lock vault
            </Button>
          </Group>
        </SettingsSection>
      </Stack>
    </Page>
  );
}
