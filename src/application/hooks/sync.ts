import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { notifyError } from "@/application/errors";
import { queryKeys } from "@/application/query-keys";
import type { SyncStatus } from "@/domain/types";
import { api } from "@/infrastructure/tauri/api";

export const useSyncStatus = () => useQuery({ queryKey: queryKeys.sync, queryFn: api.sync.status });

function useSyncAction<TVariables>(action: (variables: TVariables) => Promise<SyncStatus>) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: action,
    onSuccess: async (status) => {
      client.setQueryData(queryKeys.sync, status);
      await client.invalidateQueries({
        predicate: (query) => query.queryKey[0] !== "sync" && query.queryKey[0] !== "sftp",
      });
    },
    onError: (error) => notifyError(error, "Sync"),
  });
}

export const useSyncNow = () => useSyncAction(() => api.sync.now());
