# kraken-auth

The credentials of kraken-cli: browser sign-in, the stored API keys, which key
is active, and the precedence of flags, env vars and stored keys.

## Browser sign-in

The user signs in on Kraken's own page, and the CLI gets a restricted API key:
no withdrawal permissions, and always an expiry date. Everything that signs
requests keeps working, because the result is the same key and secret pair a
user would create by hand.

The CLI is open source, so it cannot keep a client secret. It is a _public_
OAuth client ([RFC 8252](https://www.rfc-editor.org/rfc/rfc8252)), and PKCE
([RFC 7636](https://www.rfc-editor.org/rfc/rfc7636)) protects every login.

```text
let client = AuthClient::new(Config::new(client_id), reqwest::Client::new());
let login = client.start_login(Permissions::all()).await?;  // listener is bound
open_browser(login.authorize_url());                        // the caller's job
let grant = login.finish().await?;                          // waits for the redirect
let saved = save_login_key(&grant, &store, KeyLifetime::default()).await?;
```

Each step consumes the one before it, so an out-of-order call does not compile.
The OAuth access token lives in the `Grant` only and is zeroized when it drops.

A `Permission` has no withdrawal or withdrawal-address variant. No caller can
request those rights through this crate. A grant must carry exactly the
requested scopes, and a created key must match the request (name, permissions,
an expiry at most 90 days away, which `KeyLifetime` enforces); otherwise it is
deleted and the sign-in fails. A code from the production sign-in page is only
ever redeemed at `https://api.kraken.com`.

## Stored keys and the active key

`CredentialStore` owns the `[auth]` table of `config.toml` and keeps every other
table. It stores two Spot keys side by side: the API key (`KeySlot::ApiKey`,
from `kraken auth set`) and the sign-in key (`KeySlot::SignIn`, from
`kraken auth login`). One of them is active.

```text
let store = CredentialStore::new(config_path);
let (stored, ()) = store.update(|stored| {   // read, change, write under one lock
    stored.set_own(KeyPair::new(key, secret)); // the active key does not change
    Ok(())
})?;
if stored.is_shadowed(KeySlot::ApiKey) {     // ask the user, then:
    store.update(|stored| stored.activate(KeySlot::ApiKey))?;
}

let credentials = resolve_supplied(Engine::Spot, flag, env)    // flag > env
    .or_else(|| resolve_stored(Engine::Spot, &stored));       // > the active key
```

Every write runs under a lock on `config.toml.lock`, so two commands at once
never lose a key, and replaces the file with a new 0600 file.

A new key never becomes active by itself, except over an active key that cannot
sign: expired, missing, or a choice this release cannot read. An expired active
sign-in key resolves to itself, never to the other key, and
`CredentialSource::is_expired` lets the caller refuse it at use. The CLI signs
only with a key that the user chose.
