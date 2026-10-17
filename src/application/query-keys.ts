export const queryKeys = {
  vault: ["vault"] as const,
  groups: ["groups"] as const,
  hosts: ["hosts"] as const,
  identities: ["identities"] as const,
  keys: ["keys"] as const,
  snippets: ["snippets"] as const,
  forwards: ["forwards"] as const,
  forwardsActive: ["forwards", "active"] as const,
  knownHosts: ["knownHosts"] as const,
  sync: ["sync"] as const,
  account: ["account"] as const,
  sftp: (sftpId: string | null, path: string) => ["sftp", sftpId, path] as const,
  localHome: ["local", "home"] as const,
  systemFonts: ["local", "fonts"] as const,
  localFiles: (path: string) => ["local", "files", path] as const,
};

export const entityKeys = [
  queryKeys.groups,
  queryKeys.hosts,
  queryKeys.identities,
  queryKeys.keys,
  queryKeys.snippets,
  queryKeys.forwards,
  queryKeys.knownHosts,
];
