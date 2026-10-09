# Iteration 6 — Amazon purchase history → split transactions

An Amazon bank line is a single amount with no idea what was bought. The order
itself knows: item titles, per-item prices, quantities. Feeding that in turns
one opaque `AMAZON.DE` charge into parts with real categories —
`docs/requirements.md` already names this case verbatim: *"one transaction can
be split into parts, each with its own category and amount (e.g. an Amazon
order covering groceries and electronics)"*.

Inputs: `docs/requirements.md` (Split transactions; More bank/broker
integrations), `docs/specs/iteration-2.md` §2.2–§2.9 (label precedence),
`docs/specs/iteration-5-paypal.md` §2.1 (source extension), `docs/architecture.md`.

## 1. Access method (researched 2026-10-09)

**There is no consumer Amazon order API.** Confirmed: the Order History Reports
CSV download was removed (reported March 2023), and the only programmatic
option, the Amazon Business Reporting API, is restricted to managed *business*
accounts. Nothing exists to poll as a private customer.

The available route is the account data export: **Your Account → Privacy
Central → Request your data → Your Orders**, which emails a ZIP within roughly
6–72 hours containing a `Retail.OrderHistory` dataset. Browser extensions that
scrape the orders page exist; they require handing a third party an
authenticated Amazon session, so they are out.

**Consequence — and why the user's instinct is right.** finreport cannot pull
from Amazon, so it must *accept* a push. The user: *"I will go the other way
around. You need to build an api to allow import the purchase history from
amazon."* That is the only workable direction, not a compromise.

## 2. Design

### 2.1 Orders are a new kind of record, not transactions

An Amazon order is **not** a bank transaction and must not be published to
`finreport.transaction`. It is evidence *about* a transaction. Publishing it as
a transaction would double-count every purchase: the bank line already exists.

New compacted topic **`finreport.purchase-order`**, keyed by
`<source>:<order_id>`, carrying the order and its items as supplied. Projected
to `purchase_order` / `purchase_order_item`. `prevent_destroy`, mirrored in
**both** `terraform/kafka/main.tf` and `docker-compose.local.yml`'s
`finreport-redpanda-init`.

Keeping orders as their own records means a re-import, a replay or a later
better matcher all work from the same evidence, instead of from splits already
baked into the read model.

### 2.2 The ingest API

Events are the contract, as everywhere else. Two producers:

1. **`amazon-import`** — a one-shot binary that parses an export file and
   publishes. Deployed as a run-once service like `finreport-be-category-seed`,
   matching the stated preference for *"a deployed container that runs once and
   terminates"* over manual local commands.
2. **A GraphQL mutation** taking orders and items as structured input, for a UI
   upload or the planned Kotlin app to push without a file on the server.

Both land on the same topic, so neither is privileged. The parser is a library
function both call, tested against recorded export fixtures.

### 2.3 Matching an order to a bank transaction — the risky part

**Amazon charges per shipment, not per order.** One order of three items
shipped separately produces three bank lines, none of which equals the order
total. Any matcher that assumes order total = transaction amount will be wrong
most of the time.

So matching is **a separate, confirmable step, not a side effect of import**:

- Signals: amount against *shipment* totals where the export provides them,
  date proximity, and the order id when the bank description carries it.
- A high-confidence match may apply automatically. Anything else goes to the
  **existing review queue** for confirmation rather than guessing.
- **Never overwrite a user's own split.** A split the user made by hand is a
  user decision and outranks any derived one; the import may only propose.
  `split > user override > rule > llm-cache > llm` already encodes this
  precedence — derived splits enter *below* the user's, and the records must
  carry their origin so the distinction survives a replay.

A wrong automatic split silently misattributes spending across categories,
which is worse than leaving the transaction whole. When in doubt, do nothing.

### 2.4 Items → split parts

Per matched shipment, one part per item: `amount` from the item's price × qty,
and a category.

- **Categorise from the item title**, which is the thing of actual value here:
  "Anker USB-C 100W" is categorisable in a way `AMAZON.DE` never is. Route it
  through the existing labeling chain rather than a second categoriser — the
  item title as the text, the existing catalog, the existing cache. A rule
  learned from item titles then generalises across orders.
- **Parts must sum to the transaction.** Shipping, tax, gift-card balance,
  promotions and rounding mean the items alone will not. Emit the difference as
  an explicit remainder part rather than silently distributing it — a visible
  "shipping & adjustments" part is auditable; a fudge spread across categories
  is not. Verify what `splitTransaction` enforces today and keep that invariant.
- **A returned or refunded item** arrives later as its own bank credit. Do not
  retro-edit the original split; the refund nets in the breakdown already.

### 2.5 Multi-currency, digital and gift-card orders

Record the currency as supplied; do not convert. An order paid wholly from gift
card balance has no bank line to match and must not become a phantom
transaction — import the order, match nothing, and say so.

## 3. Scope

**In scope.** The export parser with fixtures; `finreport.purchase-order` plus
its projection; the one-shot binary and its deployed service; the ingest
mutation; order and item display on the transaction; the matcher behind
confirmation; item-title categorisation feeding the split.

**Non-goals.** Scraping Amazon. Any stored Amazon credential — there is nothing
to authenticate against. Automatic high-volume matching without review until
real export data has been seen. Other retailers, though the topic is named
`purchase-order` rather than `amazon-order` so one can be added without a
second pipeline.

**Decide while implementing.**
- The export's real column set and whether item prices are pre- or post-tax —
  drive this off a real export, not a guess.
- Whether an order with one shipment and an exact amount match is safe to apply
  without confirmation. Measure against real data before deciding.
- How an order matched to the wrong transaction is undone.

## 4. Testing

| Level | What |
|---|---|
| unit | parsing a recorded export: multi-item orders, multi-shipment orders, a gift-card order, a refund row, a non-ASCII item title |
| unit | item prices × quantity summing to a shipment total; the remainder part when they do not; a zero-amount order rejected rather than split |
| unit | matcher confidence: exact single-shipment match, ambiguous equal-amount candidates, no candidate |
| integration | an order projects; a re-import is idempotent; a replay from offset 0 reproduces orders and items |
| integration | a derived split never replaces a user's manual split, and a replay preserves that precedence |
| integration | split parts sum to the parent transaction, enforced |
| manual | one real export end to end, recorded as a fixture so the suite never needs Amazon |

The parser is where a provider's surprises surface, so fixtures come from a
real export rather than a hand-written sample.
