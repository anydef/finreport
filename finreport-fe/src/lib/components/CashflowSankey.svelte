<script lang="ts">
	import { Chart, Group, Link, Rect, Layer, Text } from 'layerchart';
	import { Sankey } from 'layerchart/graph';
	import { colorForKind, type ShapedSankeyGraph, type ShapedSankeyNode } from '$lib/chartShaping';

	interface Props {
		graph: ShapedSankeyGraph;
		periodLabel: string;
		onNodeClick?: (node: ShapedSankeyNode) => void;
		onLinkClick?: (sourceId: string, targetId: string) => void;
	}

	let { graph, periodLabel, onNodeClick, onLinkClick }: Props = $props();

	// d3-sankey mutates nodes/links in place during layout, so each render
	// needs its own fresh objects — reusing `graph` directly across renders
	// would accumulate stale x0/x1/y0/y1 from a previous period's layout.
	const sankeyData = $derived({
		nodes: graph.nodes.map((n) => ({ ...n })),
		links: graph.links.map((l) => ({ ...l }))
	});

	// Last-column nodes get their label to the left (so it isn't clipped at
	// the chart's right edge); every other column gets it to the right.
	const maxDepth = $derived(graph.nodes.reduce((max, n) => Math.max(max, n.depth), 0));
</script>

<div
	role="img"
	aria-label="Cash flow for {periodLabel}: income sources into accounts, accounts into spending categories{graph.truncated
		? ' (top categories shown, remainder folded into Other)'
		: ''}. See the transaction list below for the underlying data."
	class="h-96 w-full"
>
	{#if graph.nodes.length === 0}
		<p class="flex h-full items-center justify-center text-sm text-slate-500">
			No transactions in this period.
		</p>
	{:else}
		<Chart data={sankeyData} flatData={[]} height={360}>
			<Layer>
				<Sankey nodeId={(d: ShapedSankeyNode) => d.id} nodeWidth={14} nodePadding={12}>
					{#snippet children({ links, nodes })}
						{#each links as link, i (i)}
							<Link
								sankey
								data={link}
								strokeWidth={Math.max(link.width ?? 1, 1)}
								class="cursor-pointer stroke-slate-300 transition-colors hover:stroke-slate-400"
								role="button"
								tabindex={0}
								onclick={() => onLinkClick?.(link.source.id, link.target.id)}
							>
								<title
									>{link.source.label} → {link.target.label}: {link.value.toLocaleString()}</title
								>
							</Link>
						{/each}
						{#each nodes as node (node.id)}
							{@const nodeWidth = (node.x1 ?? 0) - (node.x0 ?? 0)}
							{@const nodeHeight = (node.y1 ?? 0) - (node.y0 ?? 0)}
							<Group x={node.x0} y={node.y0}>
								<Rect
									width={nodeWidth}
									height={nodeHeight}
									fill={colorForKind(node.kind)}
									class="cursor-pointer"
									role="button"
									tabindex={0}
									onclick={() => onNodeClick?.(node)}
								>
									<title>{node.label}: {node.value.toLocaleString()}</title>
								</Rect>
								<Text
									value={node.label}
									x={node.depth === maxDepth ? -6 : nodeWidth + 6}
									y={nodeHeight / 2}
									textAnchor={node.depth === maxDepth ? 'end' : 'start'}
									verticalAnchor="middle"
									class="fill-slate-700 text-xs"
								/>
							</Group>
						{/each}
					{/snippet}
				</Sankey>
			</Layer>
		</Chart>
	{/if}
</div>
