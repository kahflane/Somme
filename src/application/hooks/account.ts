import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage, notifyError } from "@/application/errors";
import { entityKeys, queryKeys } from "@/application/query-keys";
import type { AccountStatus } from "@/domain/types";
import { api } from "@/infrastructure/tauri/api";

export const useAccountStatus = () => useQuery({ queryKey: queryKeys.account, queryFn: api.account.status });

function errorKind(error: unknown): string | null {
  if (typeof error === "object" && error !== null && "kind" in error) {
    const kind = (error as { kind: unknown }).kind;
    return typeof kind === "string" ? kind : null;
  }
  return null;
}

function accountError(error: unknown): string {
  switch (errorKind(error)) {
    case "offline":
      return "Somme Cloud is unreachable. Check your connection and try again.";
    case "signed_out":
      return "Your session expired. Please sign in again.";
    default:
      return errorMessage(error);
  }
}

interface Credentials {
  email: string;
  password: string;
}

function useAccountAction<TVariables>(action: (variables: TVariables) => Promise<AccountStatus>) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: action,
    onSuccess: async (status) => {
      client.setQueryData(queryKeys.account, status);
      await Promise.all([
        client.invalidateQueries({ queryKey: queryKeys.sync }),
        client.invalidateQueries({ queryKey: queryKeys.vault }),
        ...entityKeys.map((queryKey) => client.invalidateQueries({ queryKey })),
      ]);
    },
    onError: (error) => notifyError(accountError(error), "Somme account"),
  });
}

export const useAccountLogin = () =>
  useAccountAction(({ email, password }: Credentials) => api.account.login(email, password));

export const useAccountRegister = () =>
  useAccountAction(({ email, password }: Credentials) => api.account.register(email, password));

export const useAccountLogout = () => useAccountAction(() => api.account.logout());
