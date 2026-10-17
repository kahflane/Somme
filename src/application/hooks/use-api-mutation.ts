import { useMutation, useQueryClient, type QueryKey } from "@tanstack/react-query";
import { notifyError, notifySuccess } from "@/application/errors";

interface Options {
  invalidate?: readonly QueryKey[];
  success?: string;
  // The caller shows errors next to the field instead of as a notification.
  inlineErrors?: boolean;
}

export function useApiMutation<TVariables, TResult>(
  mutationFn: (variables: TVariables) => Promise<TResult>,
  options: Options = {},
) {
  const client = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: async () => {
      await Promise.all((options.invalidate ?? []).map((queryKey) => client.invalidateQueries({ queryKey })));
      if (options.success) notifySuccess(options.success);
    },
    onError: (error) => {
      if (!options.inlineErrors) notifyError(error);
    },
  });
}
