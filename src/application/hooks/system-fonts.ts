import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { notifyError } from "@/application/errors";
import { queryKeys } from "@/application/query-keys";
import { api } from "@/infrastructure/tauri/api";

export const useSystemFonts = () =>
  useQuery({ queryKey: queryKeys.systemFonts, queryFn: () => api.fonts.list(), staleTime: Infinity });

export function useRescanFonts() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () => api.fonts.list(true),
    onSuccess: (fonts) => client.setQueryData(queryKeys.systemFonts, fonts),
    onError: (error) => notifyError(error, "Fonts"),
  });
}
