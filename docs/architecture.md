# finreport — how it works

Diagrams of the target design. Iteration 1 builds the import → Kafka → projector → GraphQL → UI path. The labeling machinery (rules, LLM, review) arrives in iteration 2. See `docs/specs/` for the details of each iteration and `docs/requirements.md` for the decisions behind them.

## 1. Data flow

```mermaid
flowchart LR
    subgraph Sources
        CD[Comdirect API]
        FUT[C24 · Scalable · PayPal<br/><i>later</i>]
    end

    subgraph Importers
        IMP[import-transactions<br/>one task per login]
        LB[legacy-backfill<br/>one-off]
    end

    subgraph Kafka["Kafka / Redpanda — source of truth"]
        T1[(finreport.account)]
        T2[(finreport.account-balance)]
        T3[(finreport.transaction)]
        WM[(finreport.import-watermark)]
    end

    subgraph Processors
        PRJ[projector<br/>raw → normalized read model]
        LBL[labeler<br/>rules → cache → LLM<br/><i>iteration 2</i>]
        TRF[transfer / recurring detector<br/><i>iteration 3</i>]
    end

    PG[(Postgres<br/>read model + labels + overrides)]
    API[GraphQL server<br/>actix + async-graphql]
    UI[SvelteKit UI<br/>LayerChart · Tailwind]

    CD --> IMP
    FUT -.-> IMP
    IMP -- raw bank JSON + headers --> T1 & T2 & T3
    IMP <--> WM
    LB -- "origin=legacy-backfill<br/>missing keys only" --> T1 & T2 & T3
    T1 & T2 & T3 --> PRJ --> PG
    PG --> LBL --> PG
    PG --> TRF --> PG
    PG --> API -- cookie session --> UI
```

**Notes and edge cases**
- The importer writes only to Kafka. Postgres is a read model and can always be rebuilt by replaying from offset 0.
- Raw bank records and replayed legacy records share the same topics and keys. Raw always wins, whichever arrives first.
- Processors work on the **normalized** transaction, never on a bank's raw JSON, so replayed legacy data gets the same enrichment as live data.
- Your labels and overrides live in separate tables, so a replay never overwrites them.

## 2. How a transaction gets its category

```mermaid
flowchart TD
    TX[New / changed transaction] --> SPLIT{Split by user?}
    SPLIT -- yes --> SPL[Use split parts<br/>each with own category]
    SPLIT -- no --> OVR{User override?}
    OVR -- yes --> USR[label source = user]
    OVR -- no --> RULE{Active rule matches?<br/>most specific wins}
    RULE -- yes --> R[label source = rule:id]
    RULE -- no --> CACHE{LLM cache hit?<br/>fingerprint + provider + model + prompt version}
    CACHE -- yes --> C[label source = llm-cache]
    CACHE -- no --> LLM[Ask LLM<br/>Anthropic · Ollama · OpenAI-compatible]
    LLM --> AMB{Ambiguous or<br/>suggests a new category?}
    AMB -- no --> L[label source = llm<br/>store in cache]
    AMB -- yes --> Q[[Review queue<br/>held for the user]]
    Q -- user decides --> USR
    L & C & USR --> LEARN[Feed rule learner]
```

**Notes and edge cases**
- The LLM is only called when no override, rule or cache entry applies. Repeated merchants like Lidl stop costing anything once they have a rule.
- A held transaction keeps no category until you decide. It isn't counted as uncategorized spending in the meantime: it shows up as "needs review".
- The UI highlights each label by its source, so you can see at a glance whether you, a rule, the cache or the LLM set it.

## 3. Learned rule lifecycle

```mermaid
stateDiagram-v2
    [*] --> Observing: label for counterparty X
    Observing --> Observing: same category, count < N (3)
    Observing --> Discarded: conflicting category seen
    Observing --> Candidate: same category N times

    Candidate --> AutoApproved: confidence ≥ threshold (0.9)
    Candidate --> InReview: confidence < threshold

    InReview --> Active: user approves / edits
    InReview --> Rejected: user rejects

    AutoApproved --> Active: visible with auto-approved badge
    Active --> Revoked: user revokes
    Revoked --> [*]: labels fall back to next source
    Rejected --> [*]
    Discarded --> [*]
```

**Notes and edge cases**
- A candidate's confidence is the **minimum** of its underlying LLM confidences, and a label you confirmed counts as 1.0. One shaky label is enough to stop automatic approval.
- A rule never overrides a user override or a split.
- Re-applying a changed rule to past transactions is idempotent and only touches labels the rule itself set.

## 4. Review flow (admin UI)

```mermaid
sequenceDiagram
    participant L as labeler
    participant DB as Postgres
    participant UI as Admin UI
    actor U as User

    L->>DB: hold transaction (reason: ambiguous / new category)
    L->>DB: propose rule (confidence < threshold)
    UI->>DB: list held transactions + pending rules
    U->>UI: pick category / split / approve rule
    UI->>DB: write user override (or activate rule)
    DB-->>L: rule active → re-apply to non-overridden txns
    Note over DB,UI: overridden labels are highlighted in all views
```
