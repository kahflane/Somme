import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ForwardStopped,
  Group,
  GroupInput,
  Host,
  HostInput,
  Id,
  Identity,
  IdentityInput,
  KeyGenerateInput,
  KeyImportInput,
  KnownHost,
  PortForward,
  PortForwardInput,
  PromptRequest,
  PromptResponse,
  SessionClosed,
  SftpEntry,
  SftpOpened,
  Snippet,
  SnippetInput,
  SshKey,
  SyncStatus,
  SystemFont,
  VaultStatus,
  AccountStatus,
} from "@/domain/types";

export const api = {
  vault: {
    status: () => invoke<VaultStatus>("vault_status"),
    setup: (password: string, remember: boolean) =>
      invoke<VaultStatus>("vault_setup", { password, remember }),
    unlock: (password: string, remember: boolean) =>
      invoke<VaultStatus>("vault_unlock", { password, remember }),
    lock: () => invoke<VaultStatus>("vault_lock"),
  },
  groups: {
    list: () => invoke<Group[]>("groups_list"),
    save: (input: GroupInput) => invoke<Group>("group_save", { input }),
    remove: (id: Id) => invoke<void>("group_delete", { id }),
  },
  hosts: {
    list: () => invoke<Host[]>("hosts_list"),
    save: (input: HostInput) => invoke<Host>("host_save", { input }),
    remove: (id: Id) => invoke<void>("host_delete", { id }),
    importSshConfig: () => invoke<number>("import_ssh_config"),
  },
  identities: {
    list: () => invoke<Identity[]>("identities_list"),
    save: (input: IdentityInput) => invoke<Identity>("identity_save", { input }),
    remove: (id: Id) => invoke<void>("identity_delete", { id }),
  },
  keys: {
    list: () => invoke<SshKey[]>("keys_list"),
    importKey: (input: KeyImportInput) => invoke<SshKey>("key_import", { input }),
    generate: (input: KeyGenerateInput) => invoke<SshKey>("key_generate", { input }),
    rename: (id: Id, label: string) => invoke<SshKey>("key_rename", { id, label }),
    remove: (id: Id) => invoke<void>("key_delete", { id }),
  },
  snippets: {
    list: () => invoke<Snippet[]>("snippets_list"),
    save: (input: SnippetInput) => invoke<Snippet>("snippet_save", { input }),
    remove: (id: Id) => invoke<void>("snippet_delete", { id }),
  },
  forwards: {
    list: () => invoke<PortForward[]>("forwards_list"),
    save: (input: PortForwardInput) => invoke<PortForward>("forward_save", { input }),
    remove: (id: Id) => invoke<void>("forward_delete", { id }),
    start: (id: Id) => invoke<void>("forward_start", { id }),
    stop: (id: Id) => invoke<void>("forward_stop", { id }),
    active: () => invoke<Id[]>("forwards_active"),
  },
  knownHosts: {
    list: () => invoke<KnownHost[]>("known_hosts_list"),
    remove: (id: Id) => invoke<void>("known_host_delete", { id }),
  },
  sessions: {
    openSsh: (hostId: Id, cols: number, rows: number, onData: Channel<ArrayBuffer>) =>
      invoke<Id>("ssh_open", { hostId, cols, rows, onData }),
    openLocal: (cols: number, rows: number, onData: Channel<ArrayBuffer>) =>
      invoke<Id>("local_open", { cols, rows, onData }),
    write: (sessionId: Id, data: string) => invoke<void>("session_write", { sessionId, data }),
    resize: (sessionId: Id, cols: number, rows: number) =>
      invoke<void>("session_resize", { sessionId, cols, rows }),
    close: (sessionId: Id) => invoke<void>("session_close", { sessionId }),
  },
  prompts: {
    respond: (id: Id, response: PromptResponse) => invoke<void>("prompt_respond", { id, response }),
  },
  sftp: {
    open: (hostId: Id) => invoke<SftpOpened>("sftp_open", { hostId }),
    list: (sftpId: Id, path: string) => invoke<SftpEntry[]>("sftp_list", { sftpId, path }),
    mkdir: (sftpId: Id, path: string) => invoke<void>("sftp_mkdir", { sftpId, path }),
    rename: (sftpId: Id, from: string, to: string) => invoke<void>("sftp_rename", { sftpId, from, to }),
    remove: (sftpId: Id, path: string, isDir: boolean) =>
      invoke<void>("sftp_remove", { sftpId, path, isDir }),
    download: (sftpId: Id, remotePath: string, localPath: string) =>
      invoke<void>("sftp_download", { sftpId, remotePath, localPath }),
    upload: (sftpId: Id, localPath: string, remoteDir: string) =>
      invoke<string>("sftp_upload", { sftpId, localPath, remoteDir }),
    close: (sftpId: Id) => invoke<void>("sftp_close", { sftpId }),
  },
  fonts: {
    list: (refresh = false) => invoke<SystemFont[]>("system_fonts", { refresh }),
    data: (family: string, weight: number, italic: boolean) =>
      invoke<ArrayBuffer>("system_font_data", { family, weight, italic }),
  },
  localFs: {
    home: () => invoke<string>("local_home"),
    list: (path: string) => invoke<SftpEntry[]>("local_list", { path }),
    mkdir: (path: string) => invoke<void>("local_mkdir", { path }),
    rename: (from: string, to: string) => invoke<void>("local_rename", { from, to }),
    remove: (path: string, isDir: boolean) => invoke<void>("local_remove", { path, isDir }),
    readKey: (path: string) => invoke<string>("local_read_key", { path }),
  },
  account: {
    status: () => invoke<AccountStatus>("account_status"),
    register: (email: string, password: string) =>
      invoke<AccountStatus>("account_register", { email, password }),
    login: (email: string, password: string) => invoke<AccountStatus>("account_login", { email, password }),
    logout: () => invoke<AccountStatus>("account_logout"),
  },
  sync: {
    status: () => invoke<SyncStatus>("sync_status"),
    now: () => invoke<SyncStatus>("sync_now"),
  },
};

export const events = {
  onPrompt: (handler: (request: PromptRequest) => void): Promise<UnlistenFn> =>
    listen<PromptRequest>("prompt", (event) => handler(event.payload)),
  onSessionClosed: (handler: (closed: SessionClosed) => void): Promise<UnlistenFn> =>
    listen<SessionClosed>("session-closed", (event) => handler(event.payload)),
  onSyncStatus: (handler: (status: SyncStatus) => void): Promise<UnlistenFn> =>
    listen<SyncStatus>("sync-status", (event) => handler(event.payload)),
  onForwardStopped: (handler: (stopped: ForwardStopped) => void): Promise<UnlistenFn> =>
    listen<ForwardStopped>("forward-stopped", (event) => handler(event.payload)),
  onDataChanged: (handler: () => void): Promise<UnlistenFn> => listen("data-changed", () => handler()),
};

export { Channel };
