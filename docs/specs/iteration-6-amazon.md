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

## 1. Access method (researched 2026-10-09; sample seen 2026-10-09)

**There is no consumer Amazon order API.** The Order History Reports CSV
download was removed (reported March 2023), and the only programmatic option,
the Amazon Business Reporting API, is restricted to managed *business*
accounts. Nothing exists to poll as a private customer.

So finreport **accepts a push**: a CSV upload. That is the only workable
direction, not a compromise.

### 1.1 The actual file

A real sample was supplied. Columns, in order:

```
Order ID, Order Date, Total Amount, Total Savings, Status,
Item ASIN, Item Quantity, Item Price, Item Discount, Promotions,
Item Title, Item URL, Details URL,
Recipient Name, Recipient Street, Recipient City,
Recipient State, Recipient Zip, Recipient Country
```

**One row per item**, with the order id, order date and *order* total repeated
on every row of that order. Grouping by `Order ID` reconstructs the order.

What this gives us, and it is enough to build on:

- `Order ID` — the dedup key.
- `Order Date` — ISO, e.g. `2026-10-08`.
- `Total Amount` — the order total, as `"5.98 EUR"`: amount and currency in one
  string, so it must be split, not parsed as a number.
- `Item ASIN`, `Item Title`, `Item Quantity`, `Item Price` — the split parts,
  and the title is what makes categorisation possible at all.
- `Item Discount`, `Promotions` (e.g. `Additional discount: €1`) — why items
  may not sum to the total.
- `Status` — per item, e.g. `Delivered 7 October`, `Return started`,
  `Arriving today`. Free text, and **localised**, so match loosely and never
  branch on an exact string.

### 1.2 What it does NOT give us

- **No shipment grouping and no charge information.** There is no charged
  amount, no payment instrument, no transaction reference. §2.3's problem
  therefore stands in full: Amazon charges per shipment, this file only knows
  orders, so the order total frequently matches no single bank line. Matching
  must stay confirmable.
- **No tax breakdown**, and no statement of whether `Item Price` is the unit
  price or the line total. Every row in the sample has quantity 1, so the file
  itself cannot settle it. **Resolve it per order at parse time:** compare
  `sum(price)` and `sum(price x quantity)` against `Total Amount` and take
  whichever reconciles. If neither does, treat the difference as the §2.4
  remainder rather than guessing.
- **No payment method**, so a gift-card-funded order is not identifiable from
  this file.

### 1.3 Is it enough to match bank transactions?

Available on the CSV side: order id, order date, order total. Available on the
bank side: booking date, amount, description.

**Mostly yes, decisively so if the order id appears in the bank description.**
German Amazon debits often carry the order number in the reference text. Where
they do, matching is an exact string lookup and every other heuristic is
unnecessary. This is the first thing to check against real data, because it
changes the matcher from a guess to a join.

**Where they do not, amount plus date is enough for the common case but
provably not for all.** The 15-row sample already contains a collision: two
different orders, both `7.99 EUR`, two days apart (`2026-10-04` and
`2026-10-02`). Two Amazon charges of equal value in the same week are not an
edge case for anyone who orders regularly, and an amount-and-date matcher
cannot tell them apart. Picking either one at random would attach the wrong
item titles - and therefore the wrong categories - to a transaction.

This is why §2.3 keeps matching confirmable. The resolution is not a cleverer
heuristic; it is that an ambiguous match is *shown* rather than guessed. Note
the mis-match is also mostly harmless when caught, since both candidates cost
the same - but the categories differ, which is the entire point of the feature.

**Multi-shipment orders remain the hard case** and cannot be solved from this
file at all: it carries no per-shipment amount, so when one order becomes three
charges, no subset sum is derivable from the data. Those go to review by
construction.

### 1.3 Two things to note about the file

**It is not Amazon's own export.** `Details URL` carries
`ref=ppx_yo2ov_dt_b_fed_order_details`, a web-UI tracking parameter, and the
recipient-address columns are not part of Amazon's Privacy Central dataset.
This file comes from a browser extension that reads the orders page. That is
the user's call to make and the parser does not care — but the format is a
third party's and can change without notice, so the parser must fail loudly on
an unrecognised header rather than silently mis-column.

**It contains the user's home address on every row.** `Recipient Name`,
`Street`, `City`, `State`, `Zip` and `Country` have no analytical value here.
**Drop them at the parser boundary**: never publish them to Kafka and never
store them. Kafka topics here are compacted and long-lived, so anything
published is effectively permanent; a postal address is exactly the kind of
data not to write into an event log by accident. The parser reads those columns
and discards them.

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
