export type Id = string;

export type SecretUpdate =
  | { action: "keep" }
  | { action: "clear" }
  | { action: "set"; value: string };

export interface VaultStatus {
  initialized: boolean;
  unlocked: boolean;
}

export interface Group {
  id: Id;
  name: string;
  parentId: Id | null;
  createdAt: number;
  updatedAt: number;
}

export interface GroupInput {
  id: Id | null;
  name: string;
  parentId: Id | null;
}

export interface Host {
  id: Id;
  label: string;
  address: string;
  port: number;
  groupId: Id | null;
  identityId: Id | null;
  username: string | null;
  hasPassword: boolean;
  keyId: Id | null;
  jumpHostId: Id | null;
  startupSnippetId: Id | null;
  tags: string[];
  color: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface HostInput {
  id: Id | null;
  label: string;
  address: string;
  port: number;
  groupId: Id | null;
  identityId: Id | null;
  username: string | null;
  password: SecretUpdate;
  keyId: Id | null;
  jumpHostId: Id | null;
  startupSnippetId: Id | null;
  tags: string[];
  color: string | null;
}

export interface Identity {
  id: Id;
  label: string;
  username: string;
  hasPassword: boolean;
  keyId: Id | null;
  createdAt: number;
  updatedAt: number;
}

export interface IdentityInput {
  id: Id | null;
  label: string;
  username: string;
  password: SecretUpdate;
  keyId: Id | null;
}

export type KeyAlgorithm = "ed25519" | "ecdsa" | "rsa";

export interface SshKey {
  id: Id;
  label: string;
  algorithm: string;
  publicKey: string;
  fingerprint: string;
  hasPassphrase: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface KeyImportInput {
  label: string;
  privateKey: string;
  passphrase: string | null;
}

export interface KeyGenerateInput {
  label: string;
  algorithm: KeyAlgorithm;
}

export interface Snippet {
  id: Id;
  label: string;
  script: string;
  createdAt: number;
  updatedAt: number;
}

export interface SnippetInput {
  id: Id | null;
  label: string;
  script: string;
}

export type ForwardKind = "local" | "remote" | "dynamic";

export interface PortForward {
  id: Id;
  label: string;
  hostId: Id;
  kind: ForwardKind;
  bindAddress: string;
  bindPort: number;
  targetHost: string | null;
  targetPort: number | null;
  createdAt: number;
  updatedAt: number;
}

export interface PortForwardInput {
  id: Id | null;
  label: string;
  hostId: Id;
  kind: ForwardKind;
  bindAddress: string;
  bindPort: number;
  targetHost: string | null;
  targetPort: number | null;
}

export interface KnownHost {
  id: Id;
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
  createdAt: number;
}

export interface SftpEntry {
  name: string;
  path: string;
  isDir: boolean;
  isSymlink: boolean;
  size: number;
  modifiedAt: number | null;
  permissions: string | null;
}

export interface SystemFont {
  family: string;
  monospace: boolean;
}

export interface SftpOpened {
  sftpId: Id;
  home: string;
}

export interface SyncStatus {
  enabled: boolean;
  url: string | null;
  syncing: boolean;
  lastSyncAt: number | null;
  lastError: string | null;
}

export interface AccountStatus {
  signedIn: boolean;
  email: string | null;
}

export interface KeyboardPrompt {
  text: string;
  echo: boolean;
}

export type PromptKind =
  | { type: "password"; username: string; host: string }
  | { type: "passphrase"; keyLabel: string }
  | {
      type: "keyboardInteractive";
      name: string;
      instructions: string;
      prompts: KeyboardPrompt[];
    }
  | {
      type: "hostKey";
      host: string;
      port: number;
      keyType: string;
      fingerprint: string;
      previousFingerprint: string | null;
    };

export interface PromptRequest {
  id: Id;
  title: string;
  kind: PromptKind;
}

export type PromptResponse =
  | { type: "answers"; answers: string[] }
  | { type: "accept" }
  | { type: "cancel" };

export interface SessionClosed {
  sessionId: Id;
  reason: string | null;
}

export interface ForwardStopped {
  ruleId: Id;
  reason: string | null;
}

export interface ApiError {
  kind: string;
  message: string;
}
