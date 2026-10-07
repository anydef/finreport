<script lang="ts">
	import { filterOptionGroups, type CategoryOptionGroup } from '$lib/categoryTree';

	interface Props {
		groups: CategoryOptionGroup[];
		value: string | null;
		onchange: (slug: string) => void;
		label: string;
		placeholder?: string;
		id?: string;
	}

	let { groups, value, onchange, label, placeholder = 'Select…', id }: Props = $props();

	const uid = $props.id();
	const listId = `${uid}-listbox`;

	let open = $state(false);
	let query = $state('');
	let active = $state(0);
	let root: HTMLElement | undefined = $state();
	let searchInput: HTMLInputElement | undefined = $state();

	const visibleGroups = $derived(filterOptionGroups(groups, query));
	const flat = $derived(visibleGroups.flatMap((g) => g.options));
	const selectedLabel = $derived(
		groups.flatMap((g) => g.options).find((o) => o.slug === value)?.label ?? null
	);

	function openMenu() {
		open = true;
		active = Math.max(
			0,
			flat.findIndex((o) => o.slug === value)
		);
	}

	function closeMenu() {
		open = false;
		query = '';
	}

	function toggle() {
		if (open) closeMenu();
		else openMenu();
	}

	function select(slug: string) {
		onchange(slug);
		closeMenu();
	}

	function onSearchKeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
			e.preventDefault();
			if (flat.length === 0) return;
			active = (active + (e.key === 'ArrowDown' ? 1 : -1) + flat.length) % flat.length;
		} else if (e.key === 'Enter') {
			e.preventDefault();
			const opt = flat[active];
			if (opt) select(opt.slug);
		}
	}

	function onRootKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && open) {
			e.stopPropagation();
			closeMenu();
		}
	}

	// Focus the search box on open; close on any pointer press outside.
	$effect(() => {
		if (!open) return;
		searchInput?.focus();
		function onPointerDown(e: PointerEvent) {
			if (root && !root.contains(e.target as Node)) closeMenu();
		}
		document.addEventListener('pointerdown', onPointerDown);
		return () => document.removeEventListener('pointerdown', onPointerDown);
	});

	// Keep the highlight in range and scrolled into view as the list changes.
	$effect(() => {
		if (active >= flat.length) active = Math.max(0, flat.length - 1);
	});
	$effect(() => {
		if (open) document.getElementById(`${uid}-opt-${active}`)?.scrollIntoView({ block: 'nearest' });
	});
</script>

<div class="relative inline-block" bind:this={root} onkeydown={onRootKeydown} role="presentation">
	<button
		{id}
		type="button"
		onclick={toggle}
		aria-haspopup="listbox"
		aria-expanded={open}
		aria-controls={listId}
		aria-label={label}
		class="focus-visible:outline-brand flex min-w-56 items-center justify-between gap-2 rounded-md border border-slate-300 bg-white px-2 py-1 text-left text-xs focus-visible:outline focus-visible:outline-2"
	>
		<span class={selectedLabel ? 'text-slate-900' : 'text-slate-500'}>
			{selectedLabel ?? placeholder}
		</span>
		<svg
			class="h-4 w-4 shrink-0 text-slate-500"
			viewBox="0 0 20 20"
			fill="currentColor"
			aria-hidden="true"
		>
			<path
				fill-rule="evenodd"
				d="M5.23 7.21a.75.75 0 011.06.02L10 11.17l3.71-3.94a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
				clip-rule="evenodd"
			/>
		</svg>
	</button>
	<div
		class:hidden={!open}
		class="absolute left-0 z-10 mt-1 w-72 max-w-[90vw] rounded-md border border-slate-200 bg-white shadow-lg"
	>
		<div class="border-b border-slate-200 p-2">
			<input
				type="search"
				bind:this={searchInput}
				bind:value={query}
				oninput={() => (active = 0)}
				onkeydown={onSearchKeydown}
				placeholder="Search categories"
				aria-label="Search categories"
				aria-controls={listId}
				aria-activedescendant={open && flat[active] ? `${uid}-opt-${active}` : undefined}
				class="w-full rounded-md border-slate-300 text-xs"
			/>
		</div>
		<ul id={listId} role="listbox" aria-label={label} class="max-h-60 overflow-y-auto py-1 text-xs">
			{#if flat.length === 0}
				<li class="px-3 py-2 text-slate-500" role="presentation">No categories match</li>
			{/if}
			{#each visibleGroups as group (group.groupLabel)}
				<li role="presentation">
					<div class="px-3 pt-2 pb-1 font-semibold text-slate-500">{group.groupLabel}</div>
					<ul role="group" aria-label={group.groupLabel}>
						{#each group.options as option (option.slug)}
							{@const idx = flat.findIndex((o) => o.slug === option.slug)}
							<li
								id={`${uid}-opt-${idx}`}
								role="option"
								aria-selected={option.slug === value}
								class="cursor-pointer px-3 py-1.5 text-slate-800 {idx === active
									? 'bg-slate-100'
									: ''} {option.slug === value ? 'font-semibold' : ''}"
								onmouseenter={() => (active = idx)}
								onclick={() => select(option.slug)}
								onkeydown={() => {}}
							>
								{option.label}
							</li>
						{/each}
					</ul>
				</li>
			{/each}
		</ul>
	</div>
</div>
