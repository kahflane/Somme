import { notifications } from "@mantine/notifications";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { entityKeys, queryKeys } from "@/application/query-keys";
import { usePrompts } from "@/application/stores/prompts";
import { events } from "@/infrastructure/tauri/api";

export function useGlobalEvents() {
  const client = useQueryClient();
  const push = usePrompts((state) => state.push);

  useEffect(() => {
    const subscriptions = [
      events.onPrompt(push),
      events.onDataChanged(() => {
        entityKeys.forEach((queryKey) => client.invalidateQueries({ queryKey }));
      }),
      events.onSyncStatus((status) => {
        client.setQueryData(queryKeys.sync, status);
        client.invalidateQueries({ queryKey: queryKeys.account });
      }),
      events.onForwardStopped((stopped) => {
        notifications.show({
          color: "orange",
          title: "Port forwarding stopped",
          message: stopped.reason ?? "The tunnel was closed",
        });
        client.invalidateQueries({ queryKey: queryKeys.forwardsActive });
      }),
    ];
    return () => {
      subscriptions.forEach((subscription) => {
        subscription.then((unlisten) => unlisten()).catch(() => undefined);
      });
    };
  }, [client, push]);
}
