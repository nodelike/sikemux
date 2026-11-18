# Work addresses

A person proves they hold an address at a domain an org verified by signing in to that address's
Google account from the Account page. The proof lets them join the org; the address never signs
in to Sikemux. We use a Google OAuth client of our own for it, not Clerk's, so the work Google
never becomes a way to sign in.

## What happens

"Verify with Google" calls `POST /v1/account/work-addresses/verify`, which keeps a sitting (the
state's hash, the nonce and the PKCE verifier) and sends the person to Google with the scopes
`openid email`. Google sends them back to `https://app.sikemux.com/work-address`, and the web app
finishes on their own session with `POST /v1/account/work-addresses/verify/finish`. The API trades
the code, checks the ID token, and keeps a keyed hash of the address (`ADDRESS_PROOF_KEY`) with
its domain and org. The address itself is never stored or logged.

A sitting finishes within ten minutes, and its proof counts toward joining the org once, within
half an hour, from the same account.

| Piece                 | Where                                          |
| --------------------- | ---------------------------------------------- |
| Google sign-in        | `server/api/src/account/work-google.ts`        |
| Routes                | `server/api/src/account/work-addresses.ts`     |
| Joining by a proof    | `server/api/src/orgs/domains.ts`               |
| Account page          | `server/app/src/WorkAddresses.tsx`             |
| Local stand-in Google | `server/api/dev/stand-in-clerk.ts` (`/google`) |

## Settings

In `/etc/sikemux/api.env`:

```sh
GOOGLE_WORK_ADDRESS_CLIENT_ID=225181228835-c594u3bojameqnp6clutft2qo122ov5o.apps.googleusercontent.com
GOOGLE_WORK_ADDRESS_CLIENT_SECRET=…
```

Set both or neither: one alone stops the API from starting. Without them the API starts and the
Account page hides "Verify a work address". The secret is kept like the rest of `api.env`,
`root:sikemux`, mode `640`, never in GitHub or CI.

## The client

The client is "Sikemux work address" in the Google Cloud project `sikemux`, beside Clerk's web
client, under Google Auth Platform → Clients. Its type is Web application, with no JavaScript
origins and one redirect URI, exactly `https://app.sikemux.com/work-address`. The project's
consent screen is published, so any Google account can sign in.

To rotate the secret, add a new one on the client, put it in `api.env`, restart `sikemux-api`,
then disable and delete the old one.
