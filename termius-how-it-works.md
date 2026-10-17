# How Termius Works

Reverse-engineered from Termius 10.2.1 (macOS, arm64) installed at `/Applications/Termius.app`.
Tools: rea (JS + Electron analysis) and Ghidra 12.1.4 (native code). Date: 2026-10-16.

Tags used below:
- **[seen]** = read directly from the files or the running app
- **[guess]** = my reading of the evidence, not proven

---

## 1. Big picture

Termius is an **Electron app** (Chromium + Node.js), not a native Mac app. [seen]

It has 3 layers:

| Layer | File | Size | Job |
|---|---|---|---|
| Electron shell | `Contents/Frameworks/Electron Framework.framework` | 195 MB | browser + Node runtime (Electron 43.5.1) |
| App code (JS) | `Contents/Resources/app.asar` | 159 MB | UI, sync, app logic |
| Native engine (C++) | `app.asar.unpacked/node_modules/@termius/libtermius/mac-arm64/termius.node` | 3.6 MB | SSH, Telnet, SFTP, crypto |

---

## 2. Processes

Seen running on this Mac. [seen]

1. **Main process** — `main-process/main-process.js`. Opens windows, runs auto-update, talks to the keychain.
2. **Background process** — `background-process/src/background-process/main.html`. A hidden window. Syncs your data with Termius servers.
3. **UI process** — `ui-process/`. What you see: host list, terminal tabs.
4. Chromium helpers — GPU, network, crash handler.

Window settings found in the main bundle: [seen]

- One window: `nodeIntegration: false, contextIsolation: true` (locked down)
- Two windows: `nodeIntegration: true, contextIsolation: false` (full Node access)

---

## 3. Native addons

All in `app.asar.unpacked/node_modules/@termius/`. [seen]

| Addon | Job |
|---|---|
| `libtermius/termius.node` | the main engine (see section 4) |
| `node-pty/pty.node` + `spawn-helper` | local terminal on your Mac |
| `mosh/moshclient.node` | Mosh connections |
| `keytar/keytar.node` | macOS Keychain access |
| `libfido2/libfido2-nodejs.node` | FIDO2 hardware keys (YubiKey) |
| `serialport-bindings/bindings.node` | serial port connections |
| `mac-single-sign-in/...node` | macOS SSO |
| `restore-mas-purchase/...node` | Mac App Store purchase restore |
| `minidump-parser/...node` | crash report parsing |

Which JS bundle loads what: [seen]

- preload → `libtermius`, `mosh`, `libfido2`, `serialport-bindings`, `registry-js`
- background → `keytar`, `libtermius`, `restore-mas-purchase`
- main → `libtermius`, `mac-single-sign-in`, `minidump-parser`

---

## 4. The C++ engine: `termius.node`

Not stripped. 7,252 functions, 20,026 strings (Ghidra). [seen]

Links: `libssh2.1.dylib` (next to it), system zlib, Security.framework, Foundation. [seen]
Built in: **Botan** (935 functions), **libsodium**, **Argon2**, **libtelnet**. [seen]

### 4.1 Modules the JS can call

23 `InitNode*` functions. Each one adds a JS module. [seen]

| Group | Modules |
|---|---|
| Connections | `SshClient`, `TelnetClient`, `PortForwarding`, `RemoteTerminal`, `TerminalOutput` |
| SSH extras | `SshCertificate`, `SshAgentStorage`, `AgentMessagingClient`, `AgentMessagingServer` |
| Files (SFTP) | `FileSystem`, `File`, `FileAttrs`, `FileReader`, `FileWriter` |
| Keys | `Keygen`, `SEP` (Secure Enclave), `Webauthn`, `WindowsHello` |
| Vault crypto | `Crypto`, `CryptoModule`, `SRP` |
| Misc | `System`, `Logging` |

### 4.2 `SshClient` methods

From decompiling `InitNodeSshClient` @ `0x8bec`. [seen]

```
connect, exec, interactiveExec, shell, sftp, close,
localPortForwarding, remotePortForwarding, dynamicPortForwarding,
getClientSshMethods, getServerSshMethods, getAgreedSshMethod
```

### 4.3 Connect flow

`SshClient::Connect` @ `0xdf07c` just passes the options to `SshClientPimpl::Connect`. [seen]

```c
void SshClientPimpl::Connect(this, opts) {
    this->options = opts;
    if (debug) LOG("Starting a new connection to: " << options);
    if (CheckOptions(this, &options) != 0) {
        conn->vtable[0x28](conn, ...);   // hook up callbacks
        conn->vtable[0x30](conn, ...);   // start TCP + SSH handshake
    }
}
```

Steps:
1. Save options
2. Check options (`CheckOptions`)
3. Set up a connection object
4. Start TCP connect, then libssh2 handshake

Network uses libuv (`uv_connect`, `AllocBuffer`, `core::TCPConnectionHelper::ConnectCallback`). [seen]
Handshake is driven by `core::ssh::Connection` (`OnTransportRead`, `OnSendRequest`, `OnReceiveRequest`, `OnDisconnectMessage`). [seen]

### 4.4 Login (auth) flow

Decompiled in Ghidra. All code below is **[seen]** unless tagged.

libssh2 return codes used: `-37` = EAGAIN (wait), `-18` = auth failed, `-16` = public key not verified.

#### Login order — `AddDefaultMethods` @ `0x10243c`

```
1. none                    // probe: get server's allowed methods + banner
2. one method per saved key  (AddPublicKeyMethod)
     key has no private part -> log "Not using one of the provided keys: Private Key is empty"
3. password                (only if a password is saved)
4. keyboard-interactive    (only if a password is saved)
log "Available client authentication methods: " << list
```

#### Picking the key method — `AddPublicKeyMethod` @ `0x102ae8`

```
if private key is empty:
    if signer is set and public key exists:
        CreatePublicKeyWithSignMethod(key)     // sign path
    else: skip
else:
    NormalizeKey(privateKey, passphrase)
    CreatePublicKeyMethod(key)                 // in-memory path
```

So there are **two kinds of key login**:

**A. Key stored in Termius** — `CreatePublicKeyMethod` lambda @ `0x103e74`

```c
if (CheckPrivateKeyEncrypted(privateKey))
    LOG("Decrypting the SSH Private key");
r = libssh2_userauth_publickey_frommemory(session, username, username_len,
        publicKey, publicKey_len, privateKey, privateKey_len, passphrase);
return (r == -16) ? -18 : r;
```

The private key and passphrase are inside the Termius process here.

**B. Key it can't read (agent / Secure Enclave)** — `CreatePublicKeyWithSignMethod` lambda @ `0x104420`

```c
ctx = libssh2_session_abstract(session);
ctx->self = this;
blob = GetPublicKeyBlob(publicKey);
return libssh2_userauth_publickey(session, username, blob, blob_len,
                                  &SshAuth::SignMethod, ctx);
```

Only the **public** key goes in. libssh2 calls `SignMethod` when it needs a signature.

**`SignMethod`** @ `0x104550`:

```c
self = (*abstract)->self;
copy data_to_sign;
return SignRequestHandler::HandleRequest(self->signer, keyId, data,
                                         &sig_out, &siglen_out);
```

It passes the data to `SignRequestHandler` and gets back only a signature. The private key never shows up on this path. Who signs is in section 4.6.

#### Password — `CreatePasswordMethod` lambda @ `0x104908`

```c
if (have_cached_answer) password = cached;
if (password empty && !have_cached_answer) {
    RunInteractiveCallback(this, ..., [ {text:"Password:", echo:false} ]);
    return -37;                          // wait for user to type it
}
r = libssh2_userauth_password_ex(session, user, user_len, password, pw_len, NULL);
if (r != -37) {
    clear cached answer;
    if (r == -18 && !libssh2_userauth_is_partial_success(session))
        zero out stored password;        // wipe on real failure
}
return r;
```

#### Main loop — `DispatchNextAuthMethod` @ `0x10640c`

```
loop:
    r = current_method()
    if method was "none" and libssh2_userauth_banner() ok:
        LOG("Server sent a message: " << banner) ; show banner in UI
    if r == -37: return               // paused, waiting for the UI
    if libssh2_userauth_authenticated(): success -> tell JS, done
    if failed: break
    if !NextAuthMethod(): break
LOG("No more authentication methods to try") ; tell JS "failed"
```

#### Choosing the next method — `NextAuthMethod` @ `0x106ef8`

```
info = libssh2_session_get_last_userauth_attempt_info(session)
partial = (info == 1)
allowed = methods the server says "can continue"
LOG("Partial success: yes/no")
LOG("Authentication that can continue: " << allowed)
if IsAuthInteractiveRequired(allowed, partial):
    ask the UI (which method / which key)
else:
    pick the next queued method the server allows
    or, if needed, ask the UI to choose a key
```

This is how **multi-step login** works (for example key first, then 2FA). The server says "partial success", and Termius moves to the next allowed method.

#### Summary table

| Method | libssh2 call (seen in code) |
|---|---|
| Saved key | `libssh2_userauth_publickey_frommemory` |
| Agent / Secure Enclave key | `libssh2_userauth_publickey` + `SignMethod` callback |
| Password | `libssh2_userauth_password_ex` |
| Loop | `libssh2_userauth_banner`, `libssh2_userauth_authenticated` |
| Next-method picker | `libssh2_session_get_last_userauth_attempt_info`, `libssh2_userauth_is_partial_success` |
| none / keyboard-interactive | not decompiled yet (lambdas at `0x23d228` / `0x23d558`) |

#### Security notes

- Password is asked for only when needed, and is **wiped on real failure**. It is kept on partial success, which is right for multi-step login. [seen]
- On the agent / Secure Enclave path the private key **never enters** Termius. [seen for this code; signer itself is a guess]
- On the saved-key path the private key + passphrase **are in memory**. No explicit wipe was seen before they are freed. [guess — based on the cleanup code only showing `operator delete`]

### 4.5 Keyboard-interactive (password / 2FA prompts)

Decompiled `SshAuth::OnKeyboardInteractive` (called via `0x105d2c`). [seen]

```c
if (num_prompts == 0) return 0;
if (have_cached_answers) {
    fill responses from cache;          // answer right away
    return 0;
}
copy prompts (text + echo flag);
RunInteractiveCallback(...);            // send prompts to the JS UI
return -37;                             // LIBSSH2_ERROR_EAGAIN
```

How it works:
1. Server asks a question (password, 2FA).
2. No saved answer → C++ sends the question up to the UI.
3. C++ returns `EAGAIN` so libssh2 waits.
4. You type the answer → libssh2 retries → cached answer gets sent.

This is how a blocking C library works with the async JS UI.

### 4.6 Signing — `SignRequestHandler`

Decompiled in Ghidra. **[seen]** unless tagged.

**Main point: C++ never signs. It hands the job to JavaScript and waits.**

#### The wait-and-retry trick — `HandleRequest` @ `0x111068`

```c
if (ready) {                         // JS already answered
    if (sig empty) return {"", -32}; // denied
    ready = 0; return {sig, 0};      // give signature to libssh2
}
if (!asking) RunSignCallback(keyId, data);   // ask once
return {"", -37};                    // EAGAIN: try again later
```

The outer `HandleRequest` @ `0x110f30` copies the signature bytes into a `malloc` buffer for libssh2. No decoding in C++.

#### Sending to JS — `RunSignCallback` @ `0x1111cc`

```c
asking = 1;
if (Mode() == 1) {                   // "client" side
    SendAgentMessage({type 5, key, data});   // send to the other process
    return;
}
signCallback("", keyId, data, done); // call into JavaScript
```

#### Getting the answer — `OnSignResponse` @ `0x110d94`

```c
stored = sig; asking = 0; ready = 1;
dispatcher->wake();                  // libssh2 retries -> HandleRequest returns it
if (Mode() == 2) forward result back to the client;
```

#### Full signing flow

1. libssh2 needs a signature → `SignMethod` → `HandleRequest`
2. First call → `RunSignCallback` → JS gets `("", keyId, data, done)` → C++ returns `-37`
3. JS signs (Secure Enclave, FIDO2 key, ...) [guess: JS picks the signer]
4. JS calls `done(signature)` → `OnSignResponse` → wakes the loop
5. libssh2 retries → `HandleRequest` returns the signature

#### Between processes — `AgentMessagingDelegate`

- **Mode 1** (client): sends the sign request as an `AgentMessage` to another process. [seen]
- **Mode 2** (server): receives it, signs through its own JS callback, sends the answer back. [seen]
- The transport is a callback set from JS (`SetMessageCallback`). [guess] It is Electron IPC between the UI and background windows.
- Fully async. `SendAgentMessageSync` does not block. It only returns a reply that already arrived. [seen]

#### Secure Enclave — `SecureEnclave` / `SepKey` (Objective-C)

JS module `sep`: `isSupported`, `fetchAllKeys`, `generateNewKey`, class `Key` with `sign` and `remove`. [seen]

Make a key — `+[SecureEnclave generateNewKey:passwordProtected:error:]` @ `0xbdb2c`:

```objc
if (passwordProtected)
    ac = SecAccessControlCreateWithFlags(...,
           kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly,
           PrivateKeyUsage | UserPresence, ...);     // 0x40000001
key = SecKeyCreateRandomKey({
    kSecAttrKeyType:       kSecAttrKeyTypeECSECPrimeRandom,   // P-256
    kSecAttrKeySizeInBits: 256,
    kSecAttrTokenID:       kSecAttrTokenIDSecureEnclave,
    kSecPrivateKeyAttrs:   { IsPermanent: YES, AccessControl: ac }
});
```

Sign — `-[SepKey sign:error:]` @ `0xbe934`:

```objc
return SecKeyCreateSignature(priv, kSecKeyAlgorithmECDSASignatureMessageX962SHA256, data, &err);
```

- Key: ECDSA P-256, made inside the Secure Enclave. [seen]
- Private key can't be exported. The only `SecKeyCopyExternalRepresentation` call is on the **public** key (`getPublicKey` @ `0xbe424`). [seen]
- Touch ID / password is required **only if** the key was made with `passwordProtected`. [seen]

#### Agent forwarding (a remote server uses your keys)

- `RequestHandler::Sign` @ `0x113624` uses the **same** `SignRequestHandler` → JS path. [seen]
- `RequestHandler::Prompt` @ `0x11341c` shows the key's MD5 fingerprint and calls `AgentPromptRequestHandler::HandleRequest` @ `0x124ecc`. [seen]
- That asks you **only if** the host's agent "prompt" option is on. If it's off, it returns `0` straight away. [seen] [guess: 0 = allow]
- `RequestHandler::Run` @ `0x114034` pumps the forwarded channel using `libssh2_agent_forwarding_handle_message`. That function isn't in normal libssh2. [guess] Termius patched libssh2.

#### Security notes

- Secure Enclave key made **without** `passwordProtected` → no Touch ID / password check. [seen] [guess] The app can sign with it silently.
- Agent forwarding with the prompt option **off** → remote server gets signatures without asking you. [seen] [guess] Same risk as `ssh -A` with a normal agent.
- Prompt shows an MD5 fingerprint (old, but only used for display). [seen]
- No logging of the data or signature in the C++ code checked. JS side not checked.

---

## 5. Vault encryption (sync)

Your hosts, keys and passwords sync between devices. They are encrypted on your device first.

From decompiling `InitNodeCrypto` @ `0x5d20c`. [seen]

- `CryptoSystem`: `encrypt`, `decrypt`, `generateKeyPair`, `generateEncryptionKey`, `exchangeSecret`, `createPasswordHash`
- Setups: `FromPassword`, `FromKeyPair`, `FromEncryptionKey`, `ForOwner`, `ForMember`, `ForMemberWithOwnerKey` (team sharing)
- Ciphertext checks: `checkCiphertextFormat`, `getVersionFromCiphertext` (format has versions)

From decompiling `InitNodeSRP` @ `0x55fe8`. [seen]

- `ClientSession` / `ServerSession`: `generatePublicValue`, `generateProof`, `validateServerProof`, `generateVerifier`, `getSecretKey`

Primitives inside the binary: Argon2id, Curve25519, `curve25519xsalsa20poly1305`, AEGIS-128L / AEGIS-256, Botan. [seen]

What this adds up to:
1. Login uses SRP, so the server can check your password without seeing it. [seen, see 5.2]
2. Vault items are encrypted on your device, then uploaded. [seen, see 5.4]
3. Team sharing uses public-key crypto (`ForOwner`, `ForMember`, `exchangeSecret`). [seen, see 5.6]
4. Exception: old accounts not yet moved to SRP send an unsalted SHA-256 of the password. [seen, see 5.2]

The sync code is **JavaScript**, not C++. It lives in `background-process/assets/main-2598acda.js` (3.4 MB, minified). The C++ addon only does the crypto math. Sections 5.1–5.8 come from reading that file. **[seen]** unless tagged.

### 5.1 Endpoints

All paths are in one object `Pt={...}`. Base: `https://api.termius.com`.

| Purpose | Path | Method |
|---|---|---|
| **Sync (main)** | `/api/v4/terminal/sync/` | GET = pull, POST = push |
| Sync, personal only | `/api/v3/terminal/bulk/` | GET / POST |
| Sync over WebSocket | `/ws/v5/terminal/sync/` | WS |
| Account + team + plan | `/api/v4/bulk/account/` | GET |
| Connection history | `/api/v5/terminal/history-sync/` | POST |
| SRP salt | `/api/v3/user/srp/initial-data/` | POST |
| Legacy login | `/api/v3.3/auth/device/login/` | POST |
| Logout | `/api/v3/auth/device/logout/current/` | DELETE |
| My key pair | `/api/v3/keyserver/key/my/` | GET / POST |
| Team public keys | `/api/v3/keyserver/key/team/` | GET |
| Vaults | `/api/v4.1/team/vault/` | GET / POST / PATCH / DELETE |
| Vault keys | `/api/v4/team/vault/keys/` | GET / POST |
| Grant vault access | `/api/v4.1/team/vault/grant-access/` | POST |
| Push notifications | `/centrifugo/connection/websocket` | WS |
| Settings | `/api/v3/setting/mobile/` | GET / PUT |

Also: 2FA (`/api/v4/auth/otp/...`), SSO (`/api/v4.1/auth/sso/...`), session log upload (`/api/v4/terminal/history/log/upload/...`), `shell-assist`, `ssh-id`.

### 5.2 Login

Main login runs over a **WebSocket**, not REST: socket.io, path `/socket.io/grpc-proxy`, namespace `login_v2`.

1. Client sends `initialRequest {email, otp_token, device, ...}`
2. Server sends back salt + its SRP public value
3. Client makes its SRP proof → sends `finalRequest {public_data, proof}`
4. Server sends `finalResponse {proof, credentials, bulk_account, device, sessionSalt}`
5. Client checks the server's proof (`validateServerProof`). Both sides prove themselves.
6. The API token arrives **encrypted**. Client decrypts it with a key from the SRP session (`getSaltedSecretKey(sessionSalt)`).

**Legacy fallback** — if the server says "not migrated":

```js
password: NS.generateHash(n)
static generateHash(e){ return hex(sjcl.hash.sha256.hash(e)) }
```

Sends a plain **unsalted SHA-256** of your password to `/api/v3.3/auth/device/login/`. [guess] Anyone who gets that hash can log in with it, the same as with the password.

**Every REST request** carries:

```
Authorization: DeviceToken <token>
X-DEVICE-APP-VERSION: ...
X-DEVICE-PLATFORM: ...
```

401 / 403 / 490 → app logs you out / blocks access.

### 5.3 How sync works

**Delta sync with a timestamp cursor.** Not full snapshots.

```js
push(e){ n = buildPushData(e); r = lastSync;
  if (n != null) return apiSync({...n, last_synced: r});   // push + get diff back
  else return pull({lastSynced: r}); }                    // nothing to send: just pull
pull(e){ return api.getDataSync(e) }                       // GET ?last_synced__gte=<cursor>
gotBulkData(e){ setLastSync(e.now) }                        // new cursor = server's "now"
```

- One POST does both: sends your changes, gets back everyone else's changes.
- Got a 400 → pull, repair local data, push again.
- First sync on a device → pull, then push.
- `last_sync_version` (`apiV2`, `apiV3`, `sharing`, `multiKey`, `sshCertificates`). If it changes → push, then full pull.

**WebSocket mode** (if the account has `websocket_sync`):

```
→ {action:"login",     payload:{device_token, device_version}}
→ {action:"sync_diff", payload:<your changes>}
← {action:"sync.chunks", set_name, payload:[...]}   (repeats)
← {action:"sync_diff", payload}
```

**What starts a sync:**
- Login / unlock
- Any edit (host, key, group, snippet, ...) — about 30 places in the code
- Server push: Centrifugo channel `notifications#<userId>`, message `notification_type:"sync"`. This is how other devices find out.

No timer for vault sync. There is a 3-minute timer, but it's for cloud imports (AWS / Azure / DigitalOcean).

### 5.4 What gets encrypted

**Always in plain text** (every record): `id`, `local_id`, `updated_at`, `is_shared`, `encrypted_with`, and links to other records (by id).

**Two formats:**

| Format | How |
|---|---|
| **v5** (new) | All data fields packed into one JSON → one encrypted `content` blob |
| **v3** (old) | Each secret field encrypted on its own; settings fields stay plain |

```js
if (schema === "v5") {
  content = encryptV5(JSON.stringify({version, ...cryptoFields, ...plainFields}), key);
  return {...commonFields, ...relations, content};
} else {
  for (field of data) if (isCryptoField(field)) data[field] = encryptV3(data[field], key);
}
```

**Secret fields per record type:**

| Type | Encrypted |
|---|---|
| Host | `address`, `label` |
| Identity | `username`, `password`, `label` |
| SSH key | `private_key`, `public_key`, `passphrase`, `label` |
| Known host | `hostnames`, `key` |
| Snippet | `script`, `label` |
| SSH config | `env_variables` |
| Port forward | `hostname` |
| Proxy | `hostname` |
| AWS config | `accessKeyId`, `secretAccessKey`, `region` |
| Azure config | `client_id`, `tenant_id`, `client_secret` |

**Plain in v3, encrypted in v5:** `port`, `agent_forwarding`, `use_mosh`, `charset`, `color_scheme`, `os_name`, port-forward ports.

**Which key:**
- Your own records → your personal key
- Shared records → the vault key named in `encrypted_with`

### 5.5 Record types that sync

`host_set`, `group_set`, `identity_set`, `sshkeycrypt_set`, `knownhost_set`, `snippet_set`, `package_set`, `sshconfig_set`, `telnetconfig_set`, `pfrule_set`, `portknocking_set`, `proxycommand_set`, `hostchain_set`, `hostsnippet_set`, `tag_set`, `taghost_set`, `multikey_set`, `sshcertificate_set`, plus personal and shared config-identity sets.

Keys, identities and passwords **only sync if "sync keys and passwords" is on**, or if they're shared with a team:

```js
shouldSyncRaw(e, n){ return n.syncKeysAndIdentities ? true : !!e.is_shared }
```

History syncs separately (`historyconnection_set`, `session_logs_set`).

### 5.6 Keys and teams

**Your keys** (made at signup):
1. Make a key pair (`generateKeyPair`)
2. Make a random personal key (`generateEncryptionKey`)
3. Lock the personal key with the key pair → `encrypted_personal_key`
4. Lock the private key with your password → `encrypted_private_key`
5. Upload `{public_key, encrypted_private_key, encrypted_personal_key}`

**Unlocking on a new device** is the reverse: password → private key → personal key.

**Teams:**
- Owner gives access: `exchangeSecret(member.public_key)` → member's `encrypted_key` → `grantAccessToVaults(...)`
- Member opens it: `ForMemberWithOwnerKey(ownerPublicKey, myPrivateKey, encryptedKey)`
- Vault keys come from `/api/v4/team/vault/keys/`
- The client refuses to edit a shared record if your role doesn't allow it: `"is_shared is true but cannot edit by role"`

### 5.7 Deletes and conflicts

Push body:

```
{ <set_name>: [changed records],
  delete_sets: { <set_name>: [ids] },
  last_synced: <cursor> }
```

- Only unconfirmed local changes get pushed (status `CREATE_FAILED`, `UPDATE_FAILED`, `DELETE_FAILED`).
- Server tells you about deletes in `delete_sets`. Deleting a host / group / identity also cleans up what's linked to it.
- On reply, the server's version overwrites the local one, matched by `id` then `local_id`.
- No client-side "newest wins" check found. [guess] The server decides conflicts.

### 5.8 Local storage

- **IndexedDB** (Dexie), one database per type: `hosts`, `groups`, `keys`, `ssh_identities`, `known_hosts`, `snippets`, `pf_rules`, `proxies`, `ssh_configs`, `tags`, ...
- Each table: `"++local_id,&local_id,&id,status"`.
- Data in IndexedDB is encrypted again with a **local key** (`LocalCryptoSystem`).
- The local key lives in the **macOS Keychain** (keytar). If there's no keychain, it goes in `localStorage` instead.
- localStorage keys: `localKey`, `local_key_vault`, `last_sync_version`, `encryptionSchema`.

### 5.9 One full sync, start to end

1. **Login** — SRP over WebSocket → get encrypted token → decrypt it
2. **Unlock** — password → private key → personal key. Vault keys from the server.
3. **Pull** — `GET /api/v4/terminal/sync/?last_synced__gte=<cursor>`
4. **Decrypt** — `content` blob (v5) or field by field (v3)
5. **Save** — re-encrypt with local key → IndexedDB, mark `SYNCHRONIZED`, cursor = server `now`
6. **You edit** something → marked as "not confirmed" → sync starts
7. **Encrypt** — with personal key or vault key
8. **Push** — `POST /api/v4/terminal/sync/` with changes + deletes + cursor → get back the diff
9. **Other devices** — get a Centrifugo `"sync"` message → do the same

### 5.10 Security notes

- **Legacy login** sends unsalted SHA-256 of your password. Only for accounts not moved to SRP. [seen]
- **Metadata is not hidden:** ids, `updated_at`, sharing flags, and links between records are always plain. [seen] [guess] The server can see how many hosts / keys you have, when you change them, and which host uses which key.
- **v3 data leaks more:** ports, agent forwarding, mosh, OS name are plain. v5 fixes this. [seen]
- **Keys sync is opt-in:** private keys and passwords stay on the device unless you turn on "sync keys and passwords" or share them. [seen]
- **No keychain → local key in localStorage**, which is weaker. On a normal Mac the keychain is used. [seen]

---

## 6. Servers it talks to

Found as strings in the bundles. [seen]

| Host | Job |
|---|---|
| `api.termius.com` | sync API — paths `/api/v2` to `/api/v5`, many "bulk" calls |
| `account.termius.com` | account and team pages |
| `autoupdate.termius.com` | updates |
| S3 bucket `termius.desktop.autoupdate` (us-east-1) | update files (`app-update.yml`) |

Sync happens in the background process (`cryptoSystem` and `bulk` used many times there). [seen]

---

## 7. Local data

- Secrets → macOS Keychain via `keytar`. [seen]
- App data folder → `~/Library/Application Support/Termius`. [seen]

---

## 8. Security settings (Electron fuses)

Read from the Electron framework binary. [seen]

| Fuse | State | Meaning |
|---|---|---|
| RunAsNode | on | `ELECTRON_RUN_AS_NODE` works |
| EnableCookieEncryption | on | |
| EnableNodeOptionsEnvironmentVariable | off | `NODE_OPTIONS` ignored |
| EnableNodeCliInspectArguments | off | `--inspect` debugger ignored |
| EnableEmbeddedAsarIntegrityValidation | on | app.asar is checked |
| OnlyLoadAppFromAsar | on | only loads code from app.asar |

Note: the unpacked `.node` files don't match the hashes in app.asar. This is normal — code signing changes them after packing. [seen]

---

## 9. One-line summary

Electron UI → JS calls a C++ addon (libssh2 + Botan + libsodium) → it does SSH, Telnet, SFTP and port forwarding, and encrypts your vault (Argon2id + SRP + Curve25519) before syncing it to `api.termius.com`.

---

## 10. Not done yet

- `none` and keyboard-interactive login lambdas (`0x23d228`, `0x23d558`)
- JS side of signing: which signer JS picks per key type
- Termius's patched `libssh2_agent_forwarding_handle_message` (does Sign wait for the prompt answer?)
- Inside `encryptV5` / `encryptV3` (exact cipher per version, in native code)
- How the server settles conflicts
- Where the API token and personal key are stored on disk
- Whether uploaded session logs are encrypted
- Secure Enclave code (`InitNodeSEP` @ `0x6c104`)
- Full JS analysis of the UI and background bundles

---

## 11. All features

Found with rea (JS analysis of the UI + preload) and by searching the shipped bundles. **[seen]** unless tagged.

Termius has its own feature list in code (used for upgrade screens):

```js
{INTEGRATIONS, SECURE_SYNC, CROSS_PLATFORM, VAULT, SERIAL_CONNECTIONS, FIDO2,
 BIOMETRIC_LOCK, AUTOMATION, TUNNELING, CERTIFICATES, AGENT_FORWARDING,
 LOCAL_TERMINAL, BROADCAST_INPUT, TEXT_TO_COMMAND_WITH_AI, SNIPPETS_WITH_AI,
 TERMINAL_SHARING, AI_AGENT, AUTOCOMPLETE, WORKSPACES}
```

### 11.1 Connections

| Feature | Evidence |
|---|---|
| SSH | `SshClient` (C++), connection type `ssh` |
| Mosh | `moshclient.node`, type `mosh` |
| Telnet | `TelnetClient` (C++), type `telnet` |
| Serial port | `serialport-bindings`, menu "New Serial Connection", type `serial` |
| Local terminal | `node-pty`, menu "New Local Terminal" |
| Session log replay | type `session-log` |
| Host chaining (jump hosts) | "Host Chaining Screen", `hostchain_set` |
| Proxies | "Proxies Screen", `proxycommand_set` |
| Port knocking | `portknocking_set` |
| Startup snippet (run a command on connect) | "Startup Snippet Connection Flow" |
| Environment variables | "Environment Variables Screen" |
| Agent forwarding | "Agent Forwarding Screen", C++ `RequestHandler` |

Connection types map: `ssh, local, mosh, telnet, stream, session-log, serial`.

### 11.2 Port forwarding (tunneling)

Local, remote and dynamic (SOCKS). C++: `localPortForwarding`, `remotePortForwarding`, `dynamicPortForwarding`. UI: "Port Forwarding Screen".

### 11.3 Terminal

| Feature | Evidence |
|---|---|
| Split view | `SPLIT_VIEW`, "Split View From Tab Context Menu" |
| Broadcast input (type into many tabs) | "Broadcast Input", `toggleSplitViewBroadcastMode` |
| Command palette | "Open Command Palette" |
| Autocomplete | `AUTOCOMPLETE`, bundled command specs (docker, kubectl, helm, git, ...) |
| Command history | "Terminal Commands History Screen", "Global Commands History Screen" |
| Shell integration | `out/shell-integration/` for bash, zsh, fish, tcsh |
| Workspaces | `WORKSPACES`, "WorkspaceLayoutType" |
| Themes + fonts | "Toggle app theme", Nerd Fonts bundled (Fira Mono, Droid Sans Mono) |
| Font size | menu "Increase / Decrease / Reset terminal font size" |
| Find | menu "Find" |
| Side panel | menu "Show terminal side-panel" |

### 11.4 Files

SFTP: "SFTP Screen", menu "SFTP", C++ `file_system`, `FileReader`, `FileWriter`.

### 11.5 Snippets and automation

- Snippets and snippet packages (`snippet_set`, `package_set`)
- Run a snippet in all tabs ("SNIPPET_RUN_IN_ALL_TABS")
- Startup snippets (run on connect)

### 11.6 AI

| Feature | Evidence |
|---|---|
| Text to command | "Text To Command With AI", "AI is converting text to command" |
| Snippets with AI | "Create snippets with AI" |
| AI agent | `AI_AGENT:"AI Agent"` |
| Shell assist | API `/api/v4/terminal/shell-assist/` |

### 11.7 Keys and login security

| Feature | Evidence |
|---|---|
| SSH key generation | C++ `Keygen` |
| SSH certificates | `SshCertificate`, `sshcertificate_set` |
| FIDO2 / security keys (YubiKey) | `libfido2`, "AddFIDO2KeyButton", PIN prompt |
| Passkeys | "ProcessingFIDO2Passkey", `sk_passkeys_enabled` |
| Secure Enclave keys | C++ `SEP`, `SEPKey` |
| SSH.id (Termius identity tied to a device) | "SSHidSetupDialog", `/api/v4/ssh-id/` |
| MultiKey (one key per team member) | "MultiKey", `generate_multi_key_pair` |
| Known hosts | "Known Host", `knownhost_set` |
| Biometric lock (Touch ID) | `BIOMETRIC_LOCK`, "Fingerprint Unlock Screen" |
| 2FA for the account | "OTPTokenDialog", `/api/v4/auth/otp/...` |
| SSO | `mac-single-sign-in`, `/api/v4.1/auth/sso/...` |

### 11.8 Import

From: **CSV, MobaXterm, PuTTY, SecureCRT, SSH config, key file**. ("Import from ...")

### 11.9 Cloud integrations

AWS, Azure, DigitalOcean. Hosts are pulled in automatically (3-minute refresh, see 5.3). Azure SDK packages ship in `package.json`.

### 11.10 Sync, vaults and teams

| Feature | Evidence |
|---|---|
| Encrypted cloud vault + sync | section 5 |
| Team vault, sharing hosts | "ShareEntities", `SHARE_HOSTS` |
| Multiple vaults + access control | `multiple_vaults`, "GrantVaultsAccess" |
| Terminal multiplayer (shared live session) | "Start multiplayer", "Terminal Sharing Screen" |
| Session logs (recorded sessions) | "Logs Screen", `session_logs_set` |
| Invite members | "InviteMembersTrialOnboarding" |

### 11.11 Other

- Deep links: `termius://`
- Auto update (S3 bucket)
- Crash reports (`minidump-parser`, Sentry)
- Mac App Store purchase restore
- Dev tools in the menu ("Termius Dev Tools", "Background Dev Tools")

### 11.12 Plans

From the "Choose plan" dialog code:

| Plan | Adds |
|---|---|
| **Pro** | Personal cloud vault, sync across mobile and desktop, session logs, snippets automation |
| **Team** | Everything in Pro + one team vault, real-time collaboration, shared session logs, consolidated billing |
| **Business** | Everything in Team + multiple team vaults, granular vault access control. Add-on: SAML SSO |
| **Enterprise** | name seen in code, details not checked |

The "ProFeatures" upgrade screen also lists: encrypted vault, secure sync, integrations (AWS, DigitalOcean, Azure), automation / startup snippet. [guess] These are locked on the free plan.

The "FreeFeatures" screen lists: Backup & Sync, AI (text to command, snippets with AI), terminal multiplayer. [guess] These are offered on the free plan, possibly with limits.

### 11.13 More features (second pass)

Found by a second rea pass over 7 core files (2.1M syntax nodes, 2,493 findings). **[seen]** unless tagged.

**Connections**
- Auto-reconnect (`reconnectSaga`, `DISABLE_AUTORECONNECT`)
- Open many hosts at once (`MULTIPLE_CONNECTIONS_INITIATED`)
- Proxy types: `socks5`, `http`, `telnet`, plus ProxyCommand
- Mosh default server command: `mosh-server new -s -l LANG=en_US.UTF-8`

**Terminal**
- Keyword highlighting with custom colors (`KEYWORD_HIGHLIGHTING_*`)
- Settings: bell, bright bold text, Option-as-Meta, right-click paste, cursor style
- Saved workspace templates (`SAVE_WORKSPACE_TEMPLATE`)
- Autocomplete uses 603 command specs (Fig-style, `@termius/autocomplete/specs/*.json`)
- Shell integration events: `SHELL_PROMPT_BEGINS`, `COMMAND_STARTED`, `COMMAND_EXITED`
- Password prompt detection (`PASSWORD_PROMPT_DETECTED`). [guess] Offers to fill in a saved password.
- Add snippets from command history

**SFTP**
- Open a remote file in a local app, then upload changes. Remembers which app per file type (`SAVE_FILE_TYPE_APP_ASSOCIATION`).

**Keys**
- Windows Hello (Windows only)
- SSH.id creates a CA key and signs SSH certificates, backed by a passkey (`SSH_ID_GENERATE_CA_KEY`, `SSH_ID_GENERATE_SSH_KEY_CERTIFICATE`)
- MultiKey exports keys as a script (`CREATE_MULTIKEY_EXPORT_SCRIPT_BUTTON_PRESSED`)

**Organizing**
- Groups pass their settings down to hosts
- Universal undo (`UNIVERSAL_UNDO_PROCESSED`)
- More import sources: **KiTTY, mRemoteNG**, plus auto-import (`TRY_AUTO_IMPORT`)
- AWS GovCloud supported

**Teams**
- Turn off credential sharing ("no_credentials_sharing")
- Share a host by link ("Copy host details link")
- Session logs: bookmarks, comments, download

**Multiplayer (shared terminal)**
- Built on **WebRTC** (`RTCPeerConnection`, signaling token)
- Guests ask for permission to type. The host grants or takes it away (`TERMINAL_SHARING_INPUT_PERMISSIONS_*`).

**Account**
- Password breach check with **HaveIBeenPwned** (`PASSWORD_CHECKED_WITH_HIBP`)
- 2FA with TOTP or Authy
- SSO: Firebase, WorkOS, SAML add-on
- Device list + log out a device remotely (`LOGOUT_DEVICE`)
- Delete account, account recovery

**Plans**
- Starter (free), Pro (14-day trial), Team, Business, Enterprise
- Offers for GitHub students and teachers
- Buy through Mac App Store, Windows Store, or the web

**Tracking**
- **Mixpanel**: usage events (for example "Port Forwarding Screen Opened")
- **Sentry**: errors and session replay (`sentry-ipc` scheme). [guess] Not checked whether replay is on in real builds.
- Crash dumps: Crashpad + `minidump-parser`

**Main-process IPC channels**
`check-for-update`, `dialog.showOpenDialog`, `dialog.showSaveDialog`, `open-connections-in-new-window`, `open-external`, `open-new-electron-window`, `open-popup-menu`, `redux-action`, `update-restart`, `update-system-menu`, `wasAppOpenedWithDeeplink`

**Open questions**
- What gets sent to `shell-assist` with AI prompts (terminal output too?)
- What Mixpanel and Sentry actually send
- The settings window bundle (`ui-process/src/settings-process`) was not analyzed

### 11.14 Limits of this list

- Built from names and text in the code, not by clicking through the app.
- The bundled autocomplete command library (docker, kubectl, ansible, ...) mentions many tools. Those are not Termius features.
- Exact free vs paid gating comes from the server (`authorized_features`), so it's not fully visible in the app.
