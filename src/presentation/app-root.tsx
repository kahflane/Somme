"use client";

import { Button, Center, Loader, Stack, Text } from "@mantine/core";
import { errorMessage } from "@/application/errors";
import { useGlobalEvents } from "@/application/hooks/global-events";
import { useVaultStatus } from "@/application/hooks/vault";
import { PromptHost } from "@/presentation/prompts/prompt-host";
import { MainShell } from "@/presentation/shell/main-shell";
import { SetupScreen } from "@/presentation/vault/setup-screen";
import { UnlockScreen } from "@/presentation/vault/unlock-screen";

export function AppRoot() {
  useGlobalEvents();
  const { data: status, error, refetch } = useVaultStatus();

  if (error) {
    return (
      <Center h="100vh">
        <Stack align="center">
          <Text c="red">{errorMessage(error)}</Text>
          <Button onClick={() => refetch()}>Retry</Button>
        </Stack>
      </Center>
    );
  }

  if (!status) {
    return (
      <Center h="100vh">
        <Loader />
      </Center>
    );
  }

  return (
    <>
      {!status.initialized && <SetupScreen />}
      {status.initialized && !status.unlocked && <UnlockScreen />}
      {status.initialized && status.unlocked && <MainShell />}
      <PromptHost />
    </>
  );
}
