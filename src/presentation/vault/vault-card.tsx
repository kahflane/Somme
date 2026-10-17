import { Center, Paper, Stack, Text, ThemeIcon, Title } from "@mantine/core";
import { IconTerminal2 } from "@tabler/icons-react";
import type { ReactNode } from "react";

interface Props {
  title: string;
  subtitle: string;
  children: ReactNode;
}

export function VaultCard({ title, subtitle, children }: Props) {
  return (
    <Center h="100vh" bg="var(--x-bg)">
      <Paper w={420} p="xl" radius="lg" withBorder bg="var(--x-bg)">
        <Stack gap="lg">
          <Stack gap="sm">
            <ThemeIcon size={48} radius="xl" color="blue">
              <IconTerminal2 size={26} stroke={2.2} />
            </ThemeIcon>
            <Title order={2}>{title}</Title>
            <Text size="md" c="dimmed">
              {subtitle}
            </Text>
          </Stack>
          {children}
        </Stack>
      </Paper>
    </Center>
  );
}
