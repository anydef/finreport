# Iteration 5 — PayPal import

Bring PayPal in as a second source, because the bank record cannot identify a
PayPal merchant: every PayPal line on the Comdirect account reads
`01INSTANT TRANSFER 02End-to-End-Ref.: 03<digits>`. There is no merchant name
anywhere in it, so no amount of inference on the bank side can recover one.
`docs/requirements.md` ("More bank/broker integrations") already anticipated
this: *"the PayPal side carries the real merchant and is the one categorized"*.

Inputs: `docs/requirements.md` (More bank/broker integrations; Bank connections
via UI), `docs/specs/iteration-1.md` §2.2 (envelope contract) and §2.3
(projector), `docs/architecture.md`.

## 1. Access method (researched 2026-10-09)

**API:** `GET /v1/reporting/transactions` (Transaction Search), hosts
`api-m.paypal.com` and `api-m.sandbox.paypal.com`.

**Auth:** OAuth2 **client credentials** — a client id and secret from a PayPal
app, exchanged for a bearer token. Not an end-user password.

**Scope:** `https://uri.paypal.com/services/reporting/search/read`, which
requires the app's "Transaction Search" permission to be enabled. A token
without it fails with `REQUIRED_SCOPE_MISSING` / "No permission for the
requested operation". The enabled permission has been reported not to appear on
the token immediately, so a first failure may resolve by retrying later.

**Constraints that shape the design:**

| Constraint | Consequence |
|---|---|
| Max **31 days** per request | Fetch in windows, not one call |
| Max **10,000 records** per request | Shorten the window if a window overflows |
| `page` / `page_size`, page size max **500** | Paginate within each window |
| Only the **previous 3 years** | A first import cannot go deeper; record that rather than appear to have complete history |
| A transaction can take **up to 3 hours** to appear | **The watermark must lag.** See §2.3 |

**This is materially easier than Comdirect.** The Comdirect endpoint has no date
filter, so iteration 1 resorted to client-side early-stop pagination that
assumes newest-first ordering — the guard `comdirect-rs` carries and that
CLAUDE.md calls the riskiest part of the design. PayPal takes `start_date` and
`end_date`, so resume is an honest server-side date filter with no ordering
assumption.

## 2. Design

### 2.1 Same topics, new source

PayPal publishes to the **existing** ingest topics
(`finreport.transaction`, `.account`, `.account-balance`) with the envelope's
`source` header set to `paypal`. The projector already resolves a mapper by
`(source, origin)` (`projection/mod.rs`'s registry), so a new source is a new
mapper registration, not a new pipeline. Payloads stay the provider's raw JSON,
byte for byte, per §2.2 — our metadata rides in headers.

*Rejected: PayPal-specific topics.* Everything downstream — the projector, the
labeler, detection, goals — works on the normalized `transaction` row, not on a
bank's JSON. A second set of topics would duplicate the projection for no gain.

### 2.2 Dedup key

PayPal's `transaction_info.transaction_id`, mapped to `transaction.external_id`
with `source = 'paypal'`. The deterministic row id is then
`transaction_uuid("paypal", transaction_id)` exactly as for Comdirect, so a
re-import is idempotent and a replay converges.

### 2.3 The watermark must lag behind now

A PayPal transaction can appear up to **three hours** late. Advancing the
watermark to `now` would skip every transaction that settles in that gap, and
`finreport.import-watermark` is compacted — the skip would be permanent.

So: each cycle fetches from the stored watermark up to `now - safety_margin`,
with `APP_paypal_settle_margin_hours` defaulting to **6** (double the observed
latency), and advances the watermark only to the end of the window it actually
fetched. Windows additionally overlap by one day, because the dedup key makes
re-fetching free and a missed transaction is not recoverable.

This mirrors iteration 1's rule that a publish failure must not advance the
watermark: *the watermark is a promise that everything before it was seen.*

### 2.4 A `Source` trait, without refactoring Comdirect yet

`docs/requirements.md` asks for one crate per source behind a common trait, with
Comdirect refactored behind it. This iteration defines the trait and implements
it for PayPal only. Comdirect keeps working untouched.

Deliberate: the Comdirect importer is deployed, carries the newest-first guard,
and handles multi-login push-TAN state machines. Refactoring it *while* adding a
second source risks the working one for the benefit of symmetry. The trait's
shape is proven against PayPal first; Comdirect moves behind it as follow-up,
when the trait has a real second implementation to answer to.

### 2.5 Credentials: multiple accounts, configured in the admin UI

The user's decision, 2026-10-09: *"I'd like the support for multiple paypal
accounts. Also I want you to stop using 1password as the medium, instead, these
accounts have to be configurable via the admin UI."* This supersedes the
env-var approach and settles the credential-storage question that
`docs/requirements.md` ("Bank connections via UI") had left open.

So: PayPal accounts are **rows, not environment variables**, created and edited
by an admin in the UI, with any number of them.

#### 2.5.1 One secret still has to live outside the database

Credentials stored in Postgres must be encrypted at rest - a database dump,
a backup, a replica or a `SELECT` by anyone with read access must not yield
usable PayPal credentials. Encryption needs a key, and **that key cannot live
in the database it protects**, or it is not encryption, only obfuscation.

The honest consequence: this does not remove the external secret, it reduces
it from N per-account secrets to **one master key**, delivered the way the
stack already delivers `POSTGRES_PASSWORD` and the admin password - generated
by Terraform, stored in 1Password, injected as `APP_credential_key` through
`module "portainer_stack"`'s `extra_env`. Everything the user adds afterwards
goes through the UI and never touches 1Password or a deploy.

This is a real improvement - adding a PayPal account stops being a deploy - but
it is not "no 1Password at all", and pretending otherwise would mean storing
the key next to the ciphertext.

#### 2.5.1b Envelope encryption: a per-user key, wrapped by the master key

The user's refinement, 2026-10-09: *"but server can create a master key per
user, no? user doesn't even have to see it."* Yes - and it is the better
design. The server generates a **per-user data key**, stores it encrypted under
the master key from 2.5.1, and the user never sees or handles it.

What this buys, stated honestly:

- **Crypto-shredding.** Deleting a user destroys their data key, which makes
  their stored credentials unrecoverable immediately - including in backups
  and replicas already taken. Without it, "delete the user" means rewriting
  rows and hoping no copy survives.
- **Blast radius.** One leaked data key exposes one user, not the estate.
- **Independent rotation** per user, without touching anyone else's rows.

What it does **not** buy: protection from an attacker holding both the database
and the master key, since the master unwraps every data key. No scheme can,
while an unattended importer still has to decrypt at 3am with nobody logged in.
Do not let the extra layer suggest otherwise.

Cost is small - one wrapped key per user plus unwrap-on-use - so the extra
indirection is worth it for the shredding property alone.

#### 2.5.2 Rules the implementation must hold

- **Encrypt with an AEAD** (XChaCha20-Poly1305 or AES-256-GCM) from a vetted
  crate. Fresh random nonce per write. Bind the account id as associated data,
  so a ciphertext cannot be moved between rows.
- **Secrets are write-only across the API.** No query, type or error returns a
  client secret, ever. The UI shows "configured" or "not configured", when it
  was last changed, and at most a non-reversible hint - never the value, not
  even to an admin. There is no legitimate read path: the importer decrypts
  server-side and uses it directly.
- **Never logged.** `SecretString` with `ExposeSecret` at the single point of
  use, per the repo convention. No `Debug` derive that could print it; assert
  this in a test.
- **Admin-only mutations**, enforced server-side on every one - the existing
  role check, not a hidden menu.
- **Key rotation must be possible** without re-entering every credential.
  Rotating the master re-wraps the per-user data keys only, not every
  credential row - the cheap rotation being the point of the envelope. Record
  which key version wrapped each data key so a rotation is resumable and
  auditable.
- **Deleting a user destroys their data key**, and that destruction is the
  deletion; do not rely on row deletion alone.
- **Audit the fact, never the value.** A credential being created, changed or
  deleted is worth a log line naming the account and the actor; its contents
  are not.
- **Validate before saving.** A credential that does not authenticate is worse
  than none, because it fails later and silently. Exchange it for a token once,
  report the result, and surface `REQUIRED_SCOPE_MISSING` as "the app is
  missing the Transaction Search permission" rather than a raw 401.

#### 2.5.3 Shape

A generic credential store, not a PayPal-specific one. The user confirmed the
intent, 2026-10-09: *"in the next iterations the bank connectors will be
configured like that as well."* So PayPal is the **first** consumer of this
store, not its owner, and the bank connections `docs/requirements.md` lists as
blocked are the next. A second implementation of credential encryption is how
one of them ends up weaker than the other; design the store against both from
the start, even though only PayPal uses it this iteration.

Concretely, that means the credential row is keyed by *provider plus account*,
not by "paypal account", and that nothing provider-specific leaks into the
encryption, rotation or admin surface. A Comdirect login needs a different set
of fields than a PayPal app (zugangsnummer and PIN against client id and
secret), so the stored payload is a provider-tagged structure rather than two
fixed columns.

Each PayPal account row carries its own client id, secret, environment
(live or sandbox) and display name, and its own watermark key, so accounts
resume independently - mirroring how Comdirect logins already each keep their
own session file and watermark. One failing account must not stall the others.

**Comdirect stays on env vars for now.** Migrating it is a follow-up, not part
of this iteration: it is deployed and working, and moving its credentials while
also adding a new source risks the one that currently feeds all the data.

### 2.6 Linking the two sides

A PayPal purchase usually also appears on the funding account, so counting both
would double-count spending. Requirements: detect and link the pair, and
categorise the PayPal side, which carries the merchant.

**Reuse the reimbursement-link model** (built 2026-10-09: a user-declared,
kind-tagged link between transactions with roles, as an event plus a
projection). A PayPal pairing is the same relationship with a different kind and
an automatic origin rather than a user-declared one. Add the kind; do not build
a second linking mechanism, and do not reuse `transaction_insight.is_transfer`,
which means "between the user's own accounts" and is the detector's to own.

Matching signal: amount and date proximity, plus the end-to-end reference where
both sides carry it. **Out of scope for this iteration** — land the import
first, look at real paired data, then spec the matcher. A wrong automatic link
hides real spending, which is worse than two visible rows.

## 3. Scope

**In scope.** The OAuth client-credentials flow; the windowed, paginated
Transaction Search fetch; a `paypal` mapper in the projector; the watermark with
its settle margin; a `paypal-import` binary and its deployed service;
the credential store and its admin UI; the `Source` trait.

**Non-goals.** Linking PayPal to bank lines (§2.6). Refactoring Comdirect behind
the trait (§2.4). Multi-currency conversion — record the currency the provider
reports and show it; do not convert. The UI credential flow. C24 and Scalable
Capital.

**Edge cases to decide while implementing.**
- **Pending vs booked.** `transaction_status` is S (success), P (pending), V
  (refunded), D (denied). Decide which are imported and how a status change is
  represented; a pending transaction that later succeeds must not become two
  rows — the dedup key makes that automatic, but the status must update.
- **Refunds and chargebacks** arrive as their own transactions with their own
  ids. They net correctly in the breakdown already (signed accumulation), so no
  special handling — verify rather than assume.
- **Balance.** PayPal balances are per currency. Decide whether to publish one
  `account-balance` per currency or only the account's primary.
- **The 3-year limit** means a first import is incomplete by nature. Say so in
  the runbook rather than leaving the gap to be discovered.

## 4. Testing

| Level | What |
|---|---|
| unit | window splitting across 31-day boundaries, including a window that must be shortened for the 10,000-record ceiling; watermark advance never passing `now - margin`; the one-day overlap |
| unit | mapping a recorded PayPal payload to a `TransactionRecord`: id, amount sign, currency, counterparty, description, status |
| unit | OAuth token reuse and refresh on expiry; `REQUIRED_SCOPE_MISSING` surfaced as a clear operator error naming the missing permission, not a generic 401 |
| integration | the mapper through the projector: a payload projects, a re-import is idempotent, a replay from offset 0 reproduces the rows |
| integration | a publish failure does not advance the watermark |
| manual | one sandbox run against `api-m.sandbox.paypal.com`, recorded as a fixture so the suite never needs the network |

Record real payloads as fixtures. Every other source in this repo is tested
against recorded JSON, and the mapper is where a provider's surprises surface.
