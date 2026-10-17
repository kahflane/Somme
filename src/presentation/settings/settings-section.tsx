import { Paper, Stack, Text } from "@mantine/core";
import type { ReactNode } from "react";

interface Props {
  title: string;
  description: string;
  children: ReactNode;
}

export function SettingsSection({ title, description, children }: Props) {
  return (
    <Paper p="lg" radius="lg" bg="var(--x-card)">
      <Stack gap="md">
        <div>
          <Text fw={700} size="md">
            {title}
          </Text>
          <Text size="sm" c="dimmed">
            {description}
          </Text>
        </div>
        {children}
      </Stack>
    </Paper>
  );
}
