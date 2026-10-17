import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { notifyError } from "@/application/errors";
import { queryKeys } from "@/application/query-keys";
import { useShellHistory } from "@/application/stores/shell-history";
import { useTabs } from "@/application/stores/tabs";
import type { VaultStatus } from "@/domain/types";
import { api } from "@/infrastructure/tauri/api";

export const useVaultStatus = () => useQuery({ queryKey: queryKeys.vault, queryFn: api.vault.status });

interface Credentials {
  password: string;
  remember: boolean;
}

function useVaultTransition(action: (credentials: Credentials) => Promise<VaultStatus>) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: action,
    onSuccess: (status) => {
      client.setQueryData(queryKeys.vault, status);
    },
    onError: (error) => notifyError(error, "Vault"),
  });
}

export const useVaultSetup = () =>
  useVaultTransition(({ password, remember }) => api.vault.setup(password, remember));

export const useVaultUnlock = () =>
  useVaultTransition(({ password, remember }) => api.vault.unlock(password, remember));

export function useVaultLock() {
  const client = useQueryClient();
  const reset = useTabs((state) => state.reset);
  return useMutation({
    mutationFn: api.vault.lock,
    onSuccess: (status) => {
      reset();
      useShellHistory.getState().clear();
      client.removeQueries({ predicate: (query) => query.queryKey[0] !== "vault" });
      client.setQueryData(queryKeys.vault, status);
    },
    onError: (error) => notifyError(error, "Vault"),
  });
}
