# Join tickets

A phone joins a host only through the account both are signed in to. It asks
`POST /v1/devices/{key}/join` for a ticket naming the account, the host and itself, and hands
it to the host over ALPN `sikemux/join/1`. The host checks the ticket on its own, without
calling the API, then asks its owner to allow the phone. The ticket only says the phone and
the host share an account; it never lets a phone in by itself. A host listens only while it
is signed in: signing in turns remote access on and signing out turns it off.

A host links with another host the same way. The Mac app asks the same route, with its own
sign-in, for a ticket naming its host where the phone would be; the route signs one for the
Mac app's sign-in only when that key is one of the account's hosts, and for a phone's only
when it is one of its phones. The host hands it over in a `join` ask over ALPN
`sikemux/link/1`, and the other host's owner allows the link once (ADR 0013).

## What runs

| Piece               | Where                                                 |
| ------------------- | ----------------------------------------------------- |
| Route               | `server/api/src/join/routes.ts`                       |
| Key and signing     | `server/api/src/join/signer.ts`                       |
| Ticket and its text | `server/protocol/schema/join.json`                    |
| Shared test vector  | `server/protocol/vectors/join.json` (a throwaway key) |

The API signs the UTF-8 text
`sikemux-join|v1|<keyId>|<account>|<host>|<phone>|<issuedAt>|<expiresAt>` with Ed25519. A
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

The key file is a secret: anyone holding it can ask every signed-in host to let a phone in.
Keep it `root:sikemux`, mode `640`, beside `api.env`. It never goes in GitHub or CI.
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
