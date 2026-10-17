import { useQuery } from "@tanstack/react-query";
import { useApiMutation } from "@/application/hooks/use-api-mutation";
import { queryKeys } from "@/application/query-keys";
import type { Id } from "@/domain/types";
import { api } from "@/infrastructure/tauri/api";

export const useGroups = () => useQuery({ queryKey: queryKeys.groups, queryFn: api.groups.list });
export const useSaveGroup = () =>
  useApiMutation(api.groups.save, { invalidate: [queryKeys.groups], success: "Group saved" });
export const useDeleteGroup = () =>
  useApiMutation(api.groups.remove, { invalidate: [queryKeys.groups, queryKeys.hosts] });

export const useHosts = () => useQuery({ queryKey: queryKeys.hosts, queryFn: api.hosts.list });
export const useSaveHost = () =>
  useApiMutation(api.hosts.save, { invalidate: [queryKeys.hosts], success: "Host saved" });
export const useDeleteHost = () =>
  useApiMutation(api.hosts.remove, { invalidate: [queryKeys.hosts, queryKeys.forwards] });
export const useImportSshConfig = () =>
  useApiMutation(() => api.hosts.importSshConfig(), { invalidate: [queryKeys.hosts, queryKeys.keys] });

export const useIdentities = () => useQuery({ queryKey: queryKeys.identities, queryFn: api.identities.list });
export const useSaveIdentity = () =>
  useApiMutation(api.identities.save, {
    invalidate: [queryKeys.identities],
    success: "Identity saved",
  });
export const useDeleteIdentity = () =>
  useApiMutation(api.identities.remove, {
    invalidate: [queryKeys.identities, queryKeys.hosts],
  });

export const useKeys = () => useQuery({ queryKey: queryKeys.keys, queryFn: api.keys.list });
export const useImportKey = () =>
  useApiMutation(api.keys.importKey, {
    invalidate: [queryKeys.keys],
    success: "Key imported",
    inlineErrors: true,
  });
export const useGenerateKey = () =>
  useApiMutation(api.keys.generate, { invalidate: [queryKeys.keys], success: "Key generated" });
export const useRenameKey = () =>
  useApiMutation(({ id, label }: { id: Id; label: string }) => api.keys.rename(id, label), {
    invalidate: [queryKeys.keys],
  });
export const useDeleteKey = () =>
  useApiMutation(api.keys.remove, {
    invalidate: [queryKeys.keys, queryKeys.identities, queryKeys.hosts],
  });

export const useSnippets = () => useQuery({ queryKey: queryKeys.snippets, queryFn: api.snippets.list });
export const useSaveSnippet = () =>
  useApiMutation(api.snippets.save, { invalidate: [queryKeys.snippets], success: "Snippet saved" });
export const useDeleteSnippet = () =>
  useApiMutation(api.snippets.remove, { invalidate: [queryKeys.snippets, queryKeys.hosts] });

export const useForwards = () => useQuery({ queryKey: queryKeys.forwards, queryFn: api.forwards.list });
export const useActiveForwards = () =>
  useQuery({ queryKey: queryKeys.forwardsActive, queryFn: api.forwards.active });
export const useSaveForward = () =>
  useApiMutation(api.forwards.save, { invalidate: [queryKeys.forwards], success: "Rule saved" });
export const useDeleteForward = () =>
  useApiMutation(api.forwards.remove, { invalidate: [queryKeys.forwards] });
export const useStartForward = () =>
  useApiMutation(api.forwards.start, { invalidate: [queryKeys.forwardsActive] });
export const useStopForward = () =>
  useApiMutation(api.forwards.stop, { invalidate: [queryKeys.forwardsActive] });

export const useKnownHosts = () => useQuery({ queryKey: queryKeys.knownHosts, queryFn: api.knownHosts.list });
export const useDeleteKnownHost = () =>
  useApiMutation(api.knownHosts.remove, { invalidate: [queryKeys.knownHosts] });
