//! `cashflowGraph` (§5): Sankey-shaped node/link building. Grouping and
//! conservation rules live here as pure functions over pre-aggregated rows
//! — no DB, no async-graphql `Context` — so they're unit-testable without a
//! database (§8).

use async_graphql::{ErrorExtensions, ID};
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::graphql::scalars::Decimal as GqlDecimal;
use crate::graphql::types::{CashflowDimension, CashflowLink, CashflowNode, CashflowNodeKind};

/// The only `dimensions` iteration 1 accepts.
pub const SUPPORTED_DIMENSIONS: [CashflowDimension; 3] = [
    CashflowDimension::IncomeSource,
    CashflowDimension::Account,
    CashflowDimension::Outcome,
];

/// Iteration 2 (§5) addition: the outcome side grouped by resolved category
/// instead of counterparty. `TAG` stays rejected either way.
pub const SUPPORTED_DIMENSIONS_CATEGORY: [CashflowDimension; 3] = [
    CashflowDimension::IncomeSource,
    CashflowDimension::Account,
    CashflowDimension::Category,
];

pub fn validate_dimensions(dimensions: &[CashflowDimension]) -> async_graphql::Result<()> {
    if dimensions == SUPPORTED_DIMENSIONS.as_slice()
        || dimensions == SUPPORTED_DIMENSIONS_CATEGORY.as_slice()
    {
        Ok(())
    } else {
        Err(
            async_graphql::Error::new("cashflowGraph: only [INCOME_SOURCE, ACCOUNT, OUTCOME] or [INCOME_SOURCE, ACCOUNT, CATEGORY] is supported in this iteration")
                .extend_with(|_, e| e.set("code", "UNSUPPORTED_DIMENSIONS")),
        )
    }
}

/// Whether the outcome side should group by category (iteration 2, §5)
/// rather than counterparty — decided once by `fetch_graph` from the
/// validated `dimensions` and threaded through to `build_graph`.
pub fn is_category_dimension(dimensions: &[CashflowDimension]) -> bool {
    dimensions == SUPPORTED_DIMENSIONS_CATEGORY.as_slice()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowDirection {
    Income,
    Spending,
}

/// One pre-aggregated row: the total flow between one account and one named
/// (or absent) counterparty, in one direction, over the filtered period —
/// what the SQL side groups by `(account_id, counterparty_name, sign)`.
#[derive(Debug, Clone)]
pub struct AggregatedFlow {
    pub account_id: Uuid,
    pub account_label: String,
    /// `None` buckets into the always-present `Unknown` node.
    pub counterparty_name: Option<String>,
    pub direction: FlowDirection,
    /// Always positive.
    pub amount: Decimal,
}

fn account_node_id(account_id: Uuid) -> ID {
    ID(format!("account:{account_id}"))
}

fn counterparty_node_id(prefix: &str, name: &str) -> ID {
    ID(format!("{prefix}:{name}"))
}

fn other_node_id(prefix: &str) -> ID {
    ID(format!("{prefix}:other"))
}

fn unknown_node_id(prefix: &str) -> ID {
    ID(format!("{prefix}:unknown"))
}

fn net_node_id(account_id: Uuid) -> ID {
    ID(format!("net:{account_id}"))
}

fn deficit_node_id(account_id: Uuid) -> ID {
    ID(format!("deficit:{account_id}"))
}

/// Folds one side (income or outcome) of the flows into top-N named nodes +
/// `Other` + `Unknown`, returning `(nodes, links, this_side_truncated)`.
/// `link_from_counterparty` / `link_from_account` pick the link direction:
/// income flows counterparty -> account, outcome flows account ->
/// counterparty. `node_ref_type` is `Some("category")` for the iteration-2
/// `CATEGORY` dimension: kept nodes get `refType`/`refId` set to the slug
/// for drill-down (§5); `unknown_label` names the "no name" bucket
/// (`"Unknown"` for counterparty mode, `"Uncategorized"` for category mode).
#[allow(clippy::too_many_arguments)]
fn fold_side(
    rows: &[&AggregatedFlow],
    prefix: &str,
    kind: CashflowNodeKind,
    depth: i32,
    max_nodes: usize,
    income_side: bool,
    node_ref_type: Option<&'static str>,
    unknown_label: &'static str,
) -> (Vec<CashflowNode>, Vec<CashflowLink>, bool) {
    // Total per named counterparty, across all accounts, to rank top-N.
    let mut totals: BTreeMap<String, Decimal> = BTreeMap::new();
    let mut unknown_total = Decimal::ZERO;
    for row in rows {
        match &row.counterparty_name {
            Some(name) => *totals.entry(name.clone()).or_insert(Decimal::ZERO) += row.amount,
            None => unknown_total += row.amount,
        }
    }

    let mut ranked: Vec<(&String, &Decimal)> = totals.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let truncated = ranked.len() > max_nodes;

    let kept: std::collections::HashSet<&str> = ranked
        .iter()
        .take(max_nodes)
        .map(|(name, _)| name.as_str())
        .collect();

    let mut nodes = Vec::new();
    let mut links = Vec::new();

    // Per-(node, account) link values, keyed by node id so the `Other`
    // bucket's folded counterparties still land on the right account links.
    let mut per_account: BTreeMap<ID, BTreeMap<Uuid, (Decimal, String)>> = BTreeMap::new();
    let mut node_totals: BTreeMap<ID, Decimal> = BTreeMap::new();
    let mut node_labels: BTreeMap<ID, String> = BTreeMap::new();
    // Tracks whether a node id is a "kept, named" node (eligible for
    // `refType`/`refId`) as opposed to the synthetic `Other`/unknown
    // buckets, which never drill down anywhere.
    let mut node_ref_ids: BTreeMap<ID, String> = BTreeMap::new();

    for row in rows {
        let (id, label) = match &row.counterparty_name {
            Some(name) if kept.contains(name.as_str()) => {
                (counterparty_node_id(prefix, name), name.clone())
            }
            Some(_) => (other_node_id(prefix), "Other".to_string()),
            None => (unknown_node_id(prefix), unknown_label.to_string()),
        };
        *node_totals.entry(id.clone()).or_insert(Decimal::ZERO) += row.amount;
        node_labels.entry(id.clone()).or_insert_with(|| label.clone());
        if let Some(name) = &row.counterparty_name
            && kept.contains(name.as_str())
        {
            node_ref_ids.entry(id.clone()).or_insert_with(|| name.clone());
        }
        let entry = per_account
            .entry(id)
            .or_default()
            .entry(row.account_id)
            .or_insert((Decimal::ZERO, row.account_label.clone()));
        entry.0 += row.amount;
    }

    for (node_id, total) in &node_totals {
        let node_kind = if node_id.0.ends_with(":other") {
            CashflowNodeKind::Other
        } else {
            kind
        };
        let ref_id = node_ref_ids.get(node_id).cloned();
        nodes.push(CashflowNode {
            id: node_id.clone(),
            label: node_labels.get(node_id).cloned().unwrap_or_default(),
            kind: node_kind,
            depth,
            value: GqlDecimal(*total),
            ref_type: ref_id.as_ref().and(node_ref_type).map(str::to_string),
            ref_id,
        });
        for (account_id, (amount, _label)) in &per_account[node_id] {
            if amount.is_zero() {
                continue;
            }
            let (source, target) = if income_side {
                (node_id.clone(), account_node_id(*account_id))
            } else {
                (account_node_id(*account_id), node_id.clone())
            };
            links.push(CashflowLink {
                source_id: source,
                target_id: target,
                value: GqlDecimal(*amount),
            });
        }
    }

    (nodes, links, truncated)
}

pub struct GraphResult {
    pub nodes: Vec<CashflowNode>,
    pub links: Vec<CashflowLink>,
    pub truncated: bool,
}

/// Builds the full Sankey graph from pre-aggregated per-account/counterparty
/// flows. Conservation is per account (§5): each account's total inflow
/// (income + any `DEFICIT`) equals its total outflow (spending + any `NET`).
/// `category_mode` selects the iteration-2 `CATEGORY` dimension for the
/// outcome side (§5) — `rows`' `counterparty_name` then carries the
/// resolved category slug instead, built by `build_category_graph_sql`.
pub fn build_graph(rows: &[AggregatedFlow], max_nodes_per_dimension: i32, category_mode: bool) -> GraphResult {
    let max_nodes = max_nodes_per_dimension.max(0) as usize;

    let income_rows: Vec<&AggregatedFlow> = rows
        .iter()
        .filter(|r| r.direction == FlowDirection::Income)
        .collect();
    let outcome_rows: Vec<&AggregatedFlow> = rows
        .iter()
        .filter(|r| r.direction == FlowDirection::Spending)
        .collect();

    let (income_nodes, income_links, income_truncated) = fold_side(
        &income_rows,
        "income",
        CashflowNodeKind::IncomeSource,
        0,
        max_nodes,
        true,
        None,
        "Unknown",
    );
    let (outcome_kind, outcome_prefix, outcome_ref_type, outcome_unknown_label) = if category_mode
    {
        (CashflowNodeKind::Category, "category", Some("category"), "Uncategorized")
    } else {
        (CashflowNodeKind::Spending, "outcome", None, "Unknown")
    };
    let (outcome_nodes, outcome_links, outcome_truncated) = fold_side(
        &outcome_rows,
        outcome_prefix,
        outcome_kind,
        2,
        max_nodes,
        false,
        outcome_ref_type,
        outcome_unknown_label,
    );

    // Per-account income/spending totals, to size the NET/DEFICIT node (§5).
    let mut account_labels: BTreeMap<Uuid, String> = BTreeMap::new();
    let mut income_by_account: BTreeMap<Uuid, Decimal> = BTreeMap::new();
    let mut spending_by_account: BTreeMap<Uuid, Decimal> = BTreeMap::new();
    for row in rows {
        account_labels
            .entry(row.account_id)
            .or_insert_with(|| row.account_label.clone());
        let bucket = match row.direction {
            FlowDirection::Income => &mut income_by_account,
            FlowDirection::Spending => &mut spending_by_account,
        };
        *bucket.entry(row.account_id).or_insert(Decimal::ZERO) += row.amount;
    }

    let mut account_nodes = Vec::new();
    let mut net_deficit_nodes = Vec::new();
    let mut net_deficit_links = Vec::new();

    for (account_id, label) in &account_labels {
        let income = income_by_account.get(account_id).copied().unwrap_or(Decimal::ZERO);
        let spending = spending_by_account.get(account_id).copied().unwrap_or(Decimal::ZERO);
        let inflow = if spending > income { spending } else { income };
        account_nodes.push(CashflowNode {
            id: account_node_id(*account_id),
            label: label.clone(),
            kind: CashflowNodeKind::Account,
            depth: 1,
            value: GqlDecimal(inflow),
            ref_type: Some("account".to_string()),
            ref_id: Some(account_id.to_string()),
        });

        if income > spending {
            let net = income - spending;
            net_deficit_nodes.push(CashflowNode {
                id: net_node_id(*account_id),
                label: "Net".to_string(),
                kind: CashflowNodeKind::Net,
                depth: 2,
                value: GqlDecimal(net),
                ref_type: None,
                ref_id: None,
            });
            net_deficit_links.push(CashflowLink {
                source_id: account_node_id(*account_id),
                target_id: net_node_id(*account_id),
                value: GqlDecimal(net),
            });
        } else if spending > income {
            let deficit = spending - income;
            net_deficit_nodes.push(CashflowNode {
                id: deficit_node_id(*account_id),
                label: "Deficit".to_string(),
                kind: CashflowNodeKind::Deficit,
                depth: 0,
                value: GqlDecimal(deficit),
                ref_type: None,
                ref_id: None,
            });
            net_deficit_links.push(CashflowLink {
                source_id: deficit_node_id(*account_id),
                target_id: account_node_id(*account_id),
                value: GqlDecimal(deficit),
            });
        }
    }

    let mut nodes = Vec::new();
    nodes.extend(income_nodes);
    nodes.extend(account_nodes);
    nodes.extend(outcome_nodes);
    nodes.extend(net_deficit_nodes);

    let mut links = Vec::new();
    links.extend(income_links);
    links.extend(outcome_links);
    links.extend(net_deficit_links);

    GraphResult {
        nodes,
        links,
        truncated: income_truncated || outcome_truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn flow(account: Uuid, label: &str, counterparty: Option<&str>, direction: FlowDirection, amount: &str) -> AggregatedFlow {
        AggregatedFlow {
            account_id: account,
            account_label: label.to_string(),
            counterparty_name: counterparty.map(str::to_string),
            direction,
            amount: Decimal::from_str_exact(amount).unwrap(),
        }
    }

    #[test]
    fn empty_period_yields_no_nodes_or_links() {
        let result = build_graph(&[], 8, false);
        assert!(result.nodes.is_empty());
        assert!(result.links.is_empty());
        assert!(!result.truncated);
    }

    #[test]
    fn single_transaction_yields_a_valid_two_link_graph() {
        let rows = vec![flow(acc(1), "Checking", Some("Employer"), FlowDirection::Income, "1000.00")];
        let result = build_graph(&rows, 8, false);
        // income -> account, and since there is no spending, account -> NET.
        assert_eq!(result.links.len(), 2);
        assert!(result
            .nodes
            .iter()
            .any(|n| n.kind == CashflowNodeKind::Net));
    }

    #[test]
    fn account_with_income_exceeding_spending_gets_a_net_node() {
        let rows = vec![
            flow(acc(1), "Checking", Some("Employer"), FlowDirection::Income, "2000.00"),
            flow(acc(1), "Checking", Some("Landlord"), FlowDirection::Spending, "800.00"),
        ];
        let result = build_graph(&rows, 8, false);
        let net_link = result
            .links
            .iter()
            .find(|l| l.target_id.0.starts_with("net:"))
            .expect("expected a NET link");
        assert_eq!(net_link.value.0, Decimal::from_str_exact("1200.00").unwrap());
    }

    #[test]
    fn account_with_spending_exceeding_income_gets_a_deficit_node() {
        let rows = vec![
            flow(acc(1), "Checking", Some("Employer"), FlowDirection::Income, "500.00"),
            flow(acc(1), "Checking", Some("Landlord"), FlowDirection::Spending, "800.00"),
        ];
        let result = build_graph(&rows, 8, false);
        let deficit_link = result
            .links
            .iter()
            .find(|l| l.source_id.0.starts_with("deficit:"))
            .expect("expected a DEFICIT link");
        assert_eq!(deficit_link.value.0, Decimal::from_str_exact("300.00").unwrap());
    }

    #[test]
    fn one_account_net_and_another_deficit_can_coexist() {
        let rows = vec![
            flow(acc(1), "Savings", Some("Employer"), FlowDirection::Income, "2000.00"),
            flow(acc(1), "Savings", Some("Landlord"), FlowDirection::Spending, "500.00"),
            flow(acc(2), "Checking", Some("Employer"), FlowDirection::Income, "500.00"),
            flow(acc(2), "Checking", Some("Landlord"), FlowDirection::Spending, "900.00"),
        ];
        let result = build_graph(&rows, 8, false);
        assert!(result.nodes.iter().any(|n| n.kind == CashflowNodeKind::Net));
        assert!(result.nodes.iter().any(|n| n.kind == CashflowNodeKind::Deficit));
    }

    #[test]
    fn top_n_truncation_folds_the_rest_into_other() {
        let rows: Vec<AggregatedFlow> = (0..12)
            .map(|i| {
                flow(
                    acc(1),
                    "Checking",
                    Some(&format!("Source{i:02}")),
                    FlowDirection::Income,
                    "10.00",
                )
            })
            .collect();
        let result = build_graph(&rows, 8, false);
        assert!(result.truncated);
        assert!(result
            .nodes
            .iter()
            .any(|n| n.kind == CashflowNodeKind::Other));
        // 8 kept + 1 Other = 9 income-side nodes.
        let income_nodes = result
            .nodes
            .iter()
            .filter(|n| n.depth == 0 && n.kind != CashflowNodeKind::Deficit)
            .count();
        assert_eq!(income_nodes, 9);
    }

    #[test]
    fn missing_counterparty_lands_in_an_unknown_node_not_dropped() {
        let rows = vec![flow(acc(1), "Checking", None, FlowDirection::Income, "42.00")];
        let result = build_graph(&rows, 8, false);
        let unknown = result
            .nodes
            .iter()
            .find(|n| n.id.0 == "income:unknown")
            .expect("Unknown node must be present");
        assert_eq!(unknown.value.0, Decimal::from_str_exact("42.00").unwrap());
    }

    #[test]
    fn every_node_conserves_its_flow_sum_in_equals_sum_out() {
        let rows = vec![
            flow(acc(1), "Checking", Some("Employer"), FlowDirection::Income, "3000.00"),
            flow(acc(1), "Checking", Some("Freelance"), FlowDirection::Income, "500.00"),
            flow(acc(1), "Checking", Some("Landlord"), FlowDirection::Spending, "1200.00"),
            flow(acc(1), "Checking", Some("Groceries"), FlowDirection::Spending, "400.00"),
        ];
        let result = build_graph(&rows, 8, false);

        let mut inflow: BTreeMap<String, Decimal> = BTreeMap::new();
        let mut outflow: BTreeMap<String, Decimal> = BTreeMap::new();
        for link in &result.links {
            *inflow.entry(link.target_id.0.clone()).or_insert(Decimal::ZERO) += link.value.0;
            *outflow.entry(link.source_id.0.clone()).or_insert(Decimal::ZERO) += link.value.0;
        }
        for node in &result.nodes {
            let into = inflow.get(&node.id.0).copied().unwrap_or(Decimal::ZERO);
            let out_of = outflow.get(&node.id.0).copied().unwrap_or(Decimal::ZERO);
            // A node is either a pure source, a pure sink, or (ACCOUNT) both
            // — and when both, conservation requires them equal.
            if !into.is_zero() && !out_of.is_zero() {
                assert_eq!(into, out_of, "node {} does not conserve flow", node.id.0);
            }
        }
    }

    #[test]
    fn totals_reconcile_with_summary_style_sums() {
        let rows = vec![
            flow(acc(1), "Checking", Some("Employer"), FlowDirection::Income, "3000.00"),
            flow(acc(1), "Checking", Some("Landlord"), FlowDirection::Spending, "1200.00"),
            flow(acc(2), "Savings", Some("Interest"), FlowDirection::Income, "50.00"),
        ];
        let total_income: Decimal = rows
            .iter()
            .filter(|r| r.direction == FlowDirection::Income)
            .map(|r| r.amount)
            .sum();
        let total_spending: Decimal = rows
            .iter()
            .filter(|r| r.direction == FlowDirection::Spending)
            .map(|r| r.amount)
            .sum();

        let result = build_graph(&rows, 8, false);
        let income_source_total: Decimal = result
            .links
            .iter()
            .filter(|l| l.source_id.0.starts_with("income:"))
            .map(|l| l.value.0)
            .sum();
        let outcome_total: Decimal = result
            .links
            .iter()
            .filter(|l| l.target_id.0.starts_with("outcome:"))
            .map(|l| l.value.0)
            .sum();
        assert_eq!(income_source_total, total_income);
        assert_eq!(outcome_total, total_spending);
    }
}
