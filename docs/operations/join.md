# Join tickets

A phone joins a host only through the account both are signed in to. It asks
`POST /v1/devices/{key}/join` for a ticket naming the account, the host and itself, and hands
it to the host over ALPN `sikemux/join/1`. The host checks the ticket on its own, without
calling the API, and lets the phone straight in with full access (ADR 0018): the ticket says
the phone is signed in to the host's own account. A host whose person turned on **Require
approval for new devices** asks them first instead, and the request waits ten minutes whether
or not the phone stays connected. A host listens only while it is signed in: signing in turns
remote access on and signing out turns it off.

Every host links with every other host on its account the same way, on its own: it asks the
same route, signing the request with its key, for a ticket naming itself where the phone would
be, and hands it over in a `join` ask over ALPN `sikemux/link/1`. The route signs one for a host
only when the key is one of the account's hosts, and for a phone's sign-in only when it is one
of its phones. A host looks for hosts to link with when it starts, when its live connection to
the account comes up, and when the account adds a host (ADR 0013, 0018).

The desktop app joins the account's other hosts as a device of its own, with a key of its own
(ADR 0017). Its host adds that key to the account as a client, signing the request itself, and
the route then signs tickets for it only when the host asking is the one that added it. Such a
ticket also names that host, as `via` and `viaName`, signed after the other fields, and the host
it joins asks its owner to allow "The app on <viaName>" rather than whatever the app calls
itself. Removing the host from the account removes the apps it added.

"Install Sikemux here" signs a new host in from the app on another: the app reads the code
`sikemux login` prints over the person's SSH, and its host approves it with
`POST /v1/host-sign-ins/{code}/approve-from-host`, signed as the host. A host signs requests only
for the person in its own app, so the route takes the signature as the person's approval.

## What runs

| Piece               | Where                                                 |
| ------------------- | ----------------------------------------------------- |
| Route               | `server/api/src/join/routes.ts`                       |
| Key and signing     | `server/api/src/join/signer.ts`                       |
| Ticket and its text | `server/protocol/schema/join.json`                    |
| Shared test vector  | `server/protocol/vectors/join.json` (a throwaway key) |

The API signs the UTF-8 text
`sikemux-join|v1|<keyId>|<account>|<host>|<phone>|<issuedAt>|<expiresAt>` with Ed25519,
followed by `|<via>|<viaName>` for an app a host added. A
ticket lasts ten minutes. Each phone gets 10 a minute, and each one writes `join.issued` to
the audit table.

## Settings

In `/etc/sikemux/api.env`:

```sh
# The signing key, PKCS8 Ed25519 in PEM. The API does not start without it.
JOIN_SIGNING_KEY_FILE=/etc/sikemux/join-signing-key.pem
# Which key it is, as hosts know it. prod-1 when unset.
# JOIN_SIGNING_KEY_ID=prod-1
```

The key file is the most sensitive secret the API has: since ADR 0018, anyone holding it can
sign a ticket that every signed-in host lets straight in, which is a shell on each of them. Keep
it in its own file, never in `api.env`, `root:sikemux`, mode `640`. It never goes in GitHub or
CI. On any suspicion, change it as below.
Production builds of the core trust `prod-1` as the public key
`7f72791233bdb262c930822cce460eb36483842ae8053f44193a4b3e3dc9ff6c`. On start the API logs
`signing join tickets` with its key id and public key; they must match.

A local API without `JOIN_SIGNING_KEY_FILE` makes `~/.config/sikemux/dev/join-signing-key.pem`
(mode `600`) on its first start and signs as `dev-1`. Dev builds of the core read the public
key from that same file.

## Changing the key

Hosts trust keys by id, so a new key gets a new id. Add `prod-2` and its public key to the
core's trusted keys and ship it. Once hosts run that build, put the new key on citadel,
set `JOIN_SIGNING_KEY_ID=prod-2`, and restart the API. Drop `prod-1` from the core in a later
release. Tickets last ten minutes, so none are left to honour.
