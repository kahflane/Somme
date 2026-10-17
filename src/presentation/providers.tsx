"use client";

import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { Notifications } from "@mantine/notifications";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useState, type ReactNode } from "react";
import { cssVariablesResolver, theme } from "@/presentation/theme";

export function Providers({ children }: { children: ReactNode }) {
  const [client] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: { retry: false, refetchOnWindowFocus: false, staleTime: 30_000 },
        },
      }),
  );
  return (
    <MantineProvider theme={theme} defaultColorScheme="dark" cssVariablesResolver={cssVariablesResolver}>
      <QueryClientProvider client={client}>
        <ModalsProvider>
          <Notifications position="bottom-center" limit={4} />
          {children}
        </ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>
  );
}
